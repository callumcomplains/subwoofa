//! DTS 96/24: a 48 kHz core of the decimated programme plus the X96 extension that carries the part of
//! the 96 kHz signal the core cannot (the upper half of the spectrum and a refinement of the lower half).

use crate::encoder::{Config as CoreConfig, Encoder as CoreEncoder};
use crate::native::LayoutMap;
use crate::qmf64::{Analysis64, BANDS as BANDS64};
use crate::x96::X96Encoder;
use crate::Error;

pub struct Dts96Config {
    pub map: LayoutMap,
    pub bitrate: u32,
    pub dialnorm: i32,
    pub pcm_bits: u32,
    /// Bytes of every core frame given to the X96 extension (a multiple of 4).
    pub x96_bytes: usize,
    /// Sample rate of the core (48 or 44.1 kHz: half the stream's) and its frame length in 32-sample blocks (16, or
    /// 32 for a DTS CD).
    pub core_rate: u32,
    pub blocks: usize,
    /// Stereo downmix levels for the core's auxiliary data.
    pub aux: Option<crate::encoder::AuxDownmix>,
    /// Flag the core as ES (6.x: the centre surround is mixed into the surrounds before the core).
    pub es: bool,
}

/// Half-band decimator taps (linear phase, Kaiser window), `2 * HALF + 1` of them.
pub(crate) const HALF: usize = 191;

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..60 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < 1e-14 * sum {
            break;
        }
    }
    sum
}

pub(crate) fn decimator_taps(frac: f64) -> Vec<f64> {
    let fc = 22_300.0 / 96_000.0;
    let beta = 9.0;
    let mut h = Vec::with_capacity(2 * HALF + 1);
    for i in 0..=2 * HALF {
        let m = i as f64 - HALF as f64 - frac;
        let sinc = if m == 0.0 {
            2.0 * fc
        } else {
            (2.0 * std::f64::consts::PI * fc * m).sin() / (std::f64::consts::PI * m)
        };
        let r = m / HALF as f64;
        let w = bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / bessel_i0(beta);
        h.push(sinc * w);
    }
    let sum: f64 = h.iter().sum();
    h.iter().map(|v| v / sum).collect()
}

pub struct Dts96Encoder {
    core: CoreEncoder,
    ext: Vec<Analysis64>,
    x96: X96Encoder,
    taps: Vec<f64>,
    /// Source samples per channel (primary channels in stream order, then the LFE), not yet consumed.
    buf: Vec<Vec<f64>>,
    dropped: usize,
    emitted: usize,
    n: usize,
    x96_bytes: usize,
    l48: usize,
    phase: usize,
    lookahead: usize,
    ext_delay: usize,
    ext_shift: isize,
    ext_adj: isize,
    hra: Option<crate::hra::HraStage>,
}

impl Dts96Encoder {
    pub fn new(cfg: Dts96Config) -> Result<Dts96Encoder, Error> {
        let nprim = cfg.map.order.len();
        let mut core = CoreEncoder::new(CoreConfig {
            sample_rate: cfg.core_rate,
            amode: cfg.map.amode,
            channels: nprim,
            lfe: cfg.map.lfe.is_some(),
            bitrate: cfg.bitrate,
            dialnorm: cfg.dialnorm,
            pcm_bits: cfg.pcm_bits,
            es: cfg.es,
            xch: false,
            perfect: false,
            blocks: cfg.blocks,
        })?;
        core.keep_residual = true;
        core.x96_reserve = cfg.x96_bytes;
        core.set_aux_downmix(cfg.aux.clone());
        let n = core.frame_samples() * 2;
        let l48: usize = std::env::var("SW_L48").ok().and_then(|v| v.parse().ok()).unwrap_or(480);
        let phase: f64 = std::env::var("SW_PHASE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.5);
        let frac = phase - phase.floor();
        let phase = phase.floor() as usize;
        let ext: Vec<Analysis64> = (0..nprim).map(|_| Analysis64::new()).collect();
        let ext_delay = ext[0].delay();
        let ext_shift: isize = std::env::var("SW_X96_SHIFT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let lookahead = (2 * (l48 + 1) + phase + HALF + 2).max(ext_delay + 64 * cfg.blocks + 64);
        Ok(Dts96Encoder {
            core,
            ext,
            x96: X96Encoder::new(cfg.blocks),
            taps: decimator_taps(frac),
            buf: vec![Vec::new(); nprim + usize::from(cfg.map.lfe.is_some())],
            dropped: 0,
            emitted: 0,
            n,
            x96_bytes: cfg.x96_bytes,
            l48,
            phase,
            lookahead,
            ext_delay,
            ext_shift,
            ext_adj: std::env::var("SW_X96_ADJ")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            hra: None,
        })
    }

    /// Add an XBR extension substream to every frame (DTS-HD High Resolution Audio at 96 kHz).
    pub fn set_hra(&mut self, hra: crate::hra::HraStage) {
        self.hra = Some(hra);
    }

    /// Source samples (96 kHz) per frame.
    pub fn frame_samples(&self) -> usize {
        self.n
    }

    pub fn push(&mut self, frame: &[Vec<f64>]) -> Vec<Vec<u8>> {
        for (b, f) in self.buf.iter_mut().zip(frame) {
            b.extend_from_slice(f);
        }
        // every frame whose samples (and the look-ahead behind them) have arrived
        let arrived = self.buf[0].len() + self.dropped;
        let ready = (arrived.saturating_sub(self.lookahead) / self.n).saturating_sub(self.emitted);
        self.next_frames(ready)
    }

    pub fn finish(&mut self) -> Vec<Vec<u8>> {
        let pushed = self.buf[0].len() + self.dropped;
        let frames = pushed.div_ceil(self.n) + 1;
        let want = frames * self.n + self.lookahead;
        for b in self.buf.iter_mut() {
            b.resize((want - self.dropped).max(b.len()), 0.0);
        }
        self.next_frames(frames.saturating_sub(self.emitted))
    }

    fn core_input(&self, ch: usize, frame: usize) -> Vec<f64> {
        let n48 = self.n / 2;
        let src = &self.buf[ch];
        let mut out = Vec::with_capacity(n48);
        for i in 0..n48 {
            let m = frame * n48 + i + self.l48;
            let centre = (2 * m + self.phase) as isize - self.dropped as isize;
            let mut acc = 0.0;
            for (j, h) in self.taps.iter().enumerate() {
                let idx = centre + j as isize - HALF as isize;
                if idx >= 0 && (idx as usize) < src.len() {
                    acc += h * src[idx as usize];
                }
            }
            out.push(acc);
        }
        out
    }

    /// Encodes the next `count` frames. The core codes them together; the extension filter banks follow in order
    /// (each channel on its own thread), and the X96 and XBR coding of the frames runs on all cores.
    fn next_frames(&mut self, count: usize) -> Vec<Vec<u8>> {
        use rayon::prelude::*;
        if count == 0 {
            return Vec::new();
        }
        let first = self.emitted;
        let nch = self.buf.len();
        let core_in: Vec<Vec<Vec<f64>>> = {
            let me: &Dts96Encoder = &*self;
            (first..first + count)
                .into_par_iter()
                .map(|f| (0..nch).map(|c| me.core_input(c, f)).collect())
                .collect()
        };
        let refs: Vec<&[Vec<f64>]> = core_in.iter().map(|f| f.as_slice()).collect();
        let coded = self.core.encode_batch(&refs);

        // extension analysis of the programme, aligned with what the decoder's synthesis outputs: the banks of the
        // channels are independent, and each follows its own channel through the frames
        let blocks = self.core.frame_samples() / 32;
        let (n, delay, shift, adj, dropped) = (self.n, self.ext_delay, self.ext_shift, self.ext_adj, self.dropped);
        let bufs = &self.buf;
        let per_channel: Vec<Vec<Vec<Vec<f64>>>> = self
            .ext
            .par_iter_mut()
            .enumerate()
            .map(|(ch, ana)| {
                let src = &bufs[ch];
                (0..count)
                    .map(|k| {
                        let start = ((first + k) * n) as isize + delay as isize + 64 * shift + adj - dropped as isize;
                        let input: Vec<f64> = (0..blocks * 64)
                            .map(|i| {
                                let idx = start + i as isize;
                                if idx >= 0 && (idx as usize) < src.len() {
                                    src[idx as usize]
                                } else {
                                    0.0
                                }
                            })
                            .collect();
                        let mut bands = ana.analyse_blocks(&input);
                        debug_assert_eq!(bands.len(), BANDS64);
                        // lower half: what the core did not capture
                        for band in 0..32 {
                            for b in 0..blocks {
                                bands[band][b] -= coded[k].1.deq[ch][band][b] as f64;
                            }
                        }
                        bands
                    })
                    .collect()
            })
            .collect();
        let mut columns: Vec<_> = per_channel.into_iter().map(|c| c.into_iter()).collect();
        let resid: Vec<Vec<Vec<Vec<f64>>>> = (0..count)
            .map(|_| columns.iter_mut().map(|c| c.next().unwrap()).collect())
            .collect();

        let x96 = &self.x96;
        let x96_bytes = self.x96_bytes;
        let mut frames: Vec<Vec<u8>> = coded
            .par_iter()
            .zip(resid.par_iter())
            .map(|((core, _), resid)| {
                let mut frame = core.clone();
                let start = frame.len() - x96_bytes;
                frame[start..].copy_from_slice(&x96.encode(resid, x96_bytes));
                frame
            })
            .collect();
        if let Some(h) = self.hra.as_mut() {
            let wrapped: Vec<(Vec<u8>, &[Vec<Vec<f64>>])> = frames
                .drain(..)
                .zip(&coded)
                .map(|(frame, (_, left))| (frame, &left.residual[..]))
                .collect();
            frames = h.wrap_batch(wrapped);
        }
        self.emitted += count;
        // drop samples no later frame needs
        let keep_from = (self.emitted * self.n).saturating_sub(HALF + 4 + 64);
        if keep_from > self.dropped + (1 << 16) {
            let cut = keep_from - self.dropped;
            for b in self.buf.iter_mut() {
                b.drain(..cut);
            }
            self.dropped = keep_from;
        }
        frames
    }
}
