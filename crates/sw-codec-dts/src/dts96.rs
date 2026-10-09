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
}

/// Half-band decimator taps (linear phase, Kaiser window), `2 * HALF + 1` of them.
const HALF: usize = 191;

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

fn decimator_taps(frac: f64) -> Vec<f64> {
    let fc = 22_300.0 / 96_000.0;
    let beta = 9.0;
    let mut h = Vec::with_capacity(2 * HALF + 1);
    for i in 0..=2 * HALF {
        let m = i as f64 - HALF as f64 - frac;
        let sinc = if m == 0.0 { 2.0 * fc } else { (2.0 * std::f64::consts::PI * fc * m).sin() / (std::f64::consts::PI * m) };
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
    nprim: usize,
    n: usize,
    x96_bytes: usize,
    l48: usize,
    phase: usize,
    lookahead: usize,
    ext_delay: usize,
    ext_shift: isize,
    ext_adj: isize,
    timing: [std::time::Duration; 4],
    hra: Option<crate::hra::HraStage>,
}

impl Dts96Encoder {
    pub fn new(cfg: Dts96Config) -> Result<Dts96Encoder, Error> {
        let nprim = cfg.map.order.len();
        let mut core = CoreEncoder::new(CoreConfig {
            sample_rate: 48_000,
            amode: cfg.map.amode,
            channels: nprim,
            lfe: cfg.map.lfe.is_some(),
            bitrate: cfg.bitrate,
            dialnorm: cfg.dialnorm,
            pcm_bits: cfg.pcm_bits,
            es: false,
            xch: false,
            perfect: false,
            blocks: 16,
        })?;
        core.keep_residual = true;
        core.x96_reserve = cfg.x96_bytes;
        let n = core.frame_samples() * 2;
        let l48: usize = std::env::var("SW_L48").ok().and_then(|v| v.parse().ok()).unwrap_or(480);
        let phase: f64 = std::env::var("SW_PHASE").ok().and_then(|v| v.parse().ok()).unwrap_or(0.5);
        let frac = phase - phase.floor();
        let phase = phase.floor() as usize;
        let ext: Vec<Analysis64> = (0..nprim).map(|_| Analysis64::new()).collect();
        let ext_delay = ext[0].delay();
        let ext_shift: isize = std::env::var("SW_X96_SHIFT").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
        let lookahead = (2 * (l48 + 1) + phase + HALF + 2).max(ext_delay + 64 * 16 + 64);
        Ok(Dts96Encoder {
            core,
            ext,
            x96: X96Encoder::new(16),
            taps: decimator_taps(frac),
            buf: vec![Vec::new(); nprim + usize::from(cfg.map.lfe.is_some())],
            dropped: 0,
            emitted: 0,
            nprim,
            n,
            x96_bytes: cfg.x96_bytes,
            l48,
            phase,
            lookahead,
            ext_delay,
            ext_shift,
            ext_adj: std::env::var("SW_X96_ADJ").ok().and_then(|v| v.parse().ok()).unwrap_or(0),
            timing: [std::time::Duration::ZERO; 4],
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
        let mut out = Vec::new();
        while self.buf[0].len() + self.dropped >= (self.emitted + 1) * self.n + self.lookahead {
            out.push(self.next_frame());
        }
        out
    }

    pub fn finish(&mut self) -> Vec<Vec<u8>> {
        let pushed = self.buf[0].len() + self.dropped;
        let frames = pushed.div_ceil(self.n) + 1;
        let want = frames * self.n + self.lookahead;
        for b in self.buf.iter_mut() {
            b.resize((want - self.dropped).max(b.len()), 0.0);
        }
        let mut out = Vec::new();
        while self.emitted < frames {
            out.push(self.next_frame());
        }
        if std::env::var_os("SW_TIMING").is_some() {
            eprintln!("dts96 timing: decimate {:?}, core {:?}, ext analysis {:?}, x96 coding {:?}", self.timing[0], self.timing[1], self.timing[2], self.timing[3]);
        }
        out
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

    fn next_frame(&mut self) -> Vec<u8> {
        let f = self.emitted;
        let nch = self.buf.len();
        let t0 = std::time::Instant::now();
        let core_in: Vec<Vec<f64>> = {
            let me: &Dts96Encoder = &*self;
            use rayon::prelude::*;
            (0..nch).into_par_iter().map(|c| me.core_input(c, f)).collect()
        };
        self.timing[0] += t0.elapsed();
        let t0 = std::time::Instant::now();
        let mut frame = self.core.encode_frame(&core_in);
        self.timing[1] += t0.elapsed();
        let t0 = std::time::Instant::now();
        // extension analysis of the programme, aligned with what the decoder's synthesis outputs (one
        // thread per channel: the analysis banks are independent)
        let blocks = 16;
        let (f_off, n, delay, shift, adj, dropped) = (f * self.n, self.n, self.ext_delay, self.ext_shift, self.ext_adj, self.dropped);
        let _ = n;
        let mut resid: Vec<Vec<Vec<f64>>> = vec![Vec::new(); self.nprim];
        let deq = &self.core.last_deq;
        {
            use rayon::prelude::*;
            let bufs = &self.buf;
            self.ext.par_iter_mut().zip(resid.par_iter_mut()).enumerate().for_each(|(ch, (ana, out))| {
                let src = &bufs[ch];
                let mut bands = vec![vec![0f64; blocks]; BANDS64];
                for b in 0..blocks {
                    let start = (f_off + b * 64) as isize + delay as isize + 64 * shift + adj;
                    let mut input = [0f64; 64];
                    for (i, v) in input.iter_mut().enumerate() {
                        let idx = start + i as isize - dropped as isize;
                        if idx >= 0 && (idx as usize) < src.len() {
                            *v = src[idx as usize];
                        }
                    }
                    let mut o = [0f64; BANDS64];
                    ana.step(&input, &mut o);
                    for band in 0..BANDS64 {
                        bands[band][b] = o[band];
                    }
                }
                // lower half: what the core did not capture
                for band in 0..32 {
                    for b in 0..blocks {
                        bands[band][b] -= deq[ch][band][b] as f64;
                    }
                }
                *out = bands;
            });
        }
        self.timing[2] += t0.elapsed();
        let t0 = std::time::Instant::now();
        let start = frame.len() - self.x96_bytes;
        let block = self.x96.encode(&resid, self.x96_bytes);
        self.timing[3] += t0.elapsed();
        frame[start..].copy_from_slice(&block);
        if let Some(h) = self.hra.as_mut() {
            frame = h.wrap(frame, &self.core.last_residual);
        }
        self.emitted += 1;
        // drop samples no later frame needs
        let keep_from = (self.emitted * self.n).saturating_sub(HALF + 4 + 64);
        if keep_from > self.dropped + (1 << 16) {
            let cut = keep_from - self.dropped;
            for b in self.buf.iter_mut() {
                b.drain(..cut);
            }
            self.dropped = keep_from;
        }
        frame
    }
}
