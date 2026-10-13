//! The frame encoder: a 48 kHz core, the X96 extension and the refinement, per frame.

use crate::container::exss::{exss_frame_x96, exss_frame_xxch_x96, ExssParams};
use crate::dca::dts96::{decimator_taps, CORE_ADVANCE, DECIMATION_PHASE, HALF};
use crate::dca::encoder::Config as CoreConfig;
use crate::dca::encoder::Encoder as CoreEncoder;
use crate::dca::encoder::XxchParams;
use crate::dca::qmf64::Analysis64;
use crate::dca::qmf64::BANDS as BANDS64;
use crate::dca::x96::X96Encoder;
use crate::hra::hra96::map::Hra96Kind;
use crate::hra::hra96::{MAX_X96_BYTES, XXCH_FIXED_BYTES};
use crate::Error;

pub struct Hra96Config {
    /// Core channel layout: DTS channel arrangement code and the number of full-range channels (5 for 5.x and above).
    pub amode: u8,
    pub nprim: usize,
    pub has_lfe: bool,
    pub kind: Hra96Kind,
    pub total_kbps: u32,
    pub dialnorm: i32,
    pub pcm_bits: u32,
    pub exss: ExssParams,
    /// Flag the core as ES also for a layout that is not a 6.x one (side-rear pair carried by the surrounds).
    pub core_es: bool,
    /// The XXCH header of a 7.x stream.
    pub xxch: Option<XxchParams>,
    /// Stereo downmix levels for the core's auxiliary data.
    pub aux: Option<crate::dca::encoder::AuxDownmix>,
}

pub struct Hra96Encoder {
    pub(crate) kind: Hra96Kind,
    pub(crate) core: CoreEncoder,
    /// The encoder of the rear pair (7.x only).
    pub(crate) rear: Option<CoreEncoder>,
    /// 64-band analysis of the five core channels, then of the extra channels that get an X96 set.
    pub(crate) core_ana: Vec<Analysis64>,
    pub(crate) extra_ana: Vec<Analysis64>,
    pub(crate) x96: X96Encoder,
    pub(crate) params: XxchParams,
    pub(crate) exss: ExssParams,
    pub(crate) taps: Vec<f64>,
    /// Source samples (96 kHz) per channel: the five core channels, the LFE, then the extra channels; not yet consumed.
    pub(crate) buf: Vec<Vec<f64>>,
    /// Which buffers feed the core encoder, in the order it takes them (the five, the XCH channel, the LFE).
    pub(crate) core_inputs: Vec<usize>,
    /// First buffer of the extra channels.
    pub(crate) extra_start: usize,
    pub(crate) dropped: usize,
    pub(crate) emitted: usize,
    pub(crate) n: usize,
    pub(crate) l48: usize,
    pub(crate) phase: usize,
    pub(crate) lookahead: usize,
    pub(crate) ana_delay: usize,
    pub(crate) bits_num: u64,
    pub(crate) bits_den: u64,
    pub(crate) overhead: usize,
}

pub(crate) fn core_config(
    amode: u8,
    nprim: usize,
    es: bool,
    xch: bool,
    has_lfe: bool,
    dialnorm: i32,
    pcm_bits: u32,
) -> CoreConfig {
    CoreConfig {
        sample_rate: 48_000,
        amode,
        channels: nprim,
        lfe: has_lfe,
        bitrate: 1_509_000,
        dialnorm,
        pcm_bits,
        es,
        xch,
        perfect: false,
        blocks: 16,
    }
}

impl Hra96Encoder {
    pub fn new(cfg: Hra96Config) -> Result<Hra96Encoder, Error> {
        let has_lfe = cfg.has_lfe;
        let es = matches!(cfg.kind, Hra96Kind::Matrix | Hra96Kind::Discrete) || cfg.core_es;
        let mut core = CoreEncoder::new(core_config(
            cfg.amode,
            cfg.nprim,
            es,
            cfg.kind == Hra96Kind::Discrete,
            has_lfe,
            cfg.dialnorm,
            cfg.pcm_bits,
        ))?;
        core.keep_residual = true;
        core.set_aux_downmix(cfg.aux.clone());
        let rear = if cfg.kind == Hra96Kind::Rear {
            let mut r = CoreEncoder::new(CoreConfig {
                amode: 2,
                channels: 2,
                lfe: false,
                bitrate: 1_536_000,
                es: false,
                xch: false,
                ..core_config(2, 2, false, false, false, cfg.dialnorm, cfg.pcm_bits)
            })?;
            r.keep_residual = true;
            Some(r)
        } else {
            None
        };
        let n = core.frame_samples() * 2;
        let l48 = CORE_ADVANCE;
        let frac = DECIMATION_PHASE - DECIMATION_PHASE.floor();
        let phase = DECIMATION_PHASE.floor() as usize;
        let nprim = cfg.nprim;
        let core_ana: Vec<Analysis64> = (0..nprim).map(|_| Analysis64::new()).collect();
        // the rears and the XCH channel each get an X96 channel; the LFE does not
        let extra_ana: Vec<Analysis64> = (0..cfg.kind.extra_channels()).map(|_| Analysis64::new()).collect();
        let ana_delay = core_ana[0].delay();
        let lookahead = (2 * (l48 + 1) + phase + HALF + 2).max(ana_delay + 64 * 16 + 64);
        let params = cfg.xxch.clone().unwrap_or(XxchParams {
            core_mask: 0,
            spk_mask: 0,
            mask_bits: 9,
            dmix: None,
        });
        // size of the extension substream header (what the components do not account for)
        let overhead = match cfg.kind {
            Hra96Kind::Rear => exss_frame_xxch_x96(&cfg.exss, &[0u8; 4], &[0u8; 4]).len() - 8,
            _ => exss_frame_x96(&cfg.exss, &[0u8; 4]).len() - 4,
        };
        let extra_start = nprim + usize::from(has_lfe);
        // the core takes its five channels, then the XCH channel (if any), then the LFE
        let mut core_inputs: Vec<usize> = (0..nprim).collect();
        if cfg.kind == Hra96Kind::Discrete {
            core_inputs.push(extra_start);
        }
        if has_lfe {
            core_inputs.push(nprim);
        }
        Ok(Hra96Encoder {
            kind: cfg.kind,
            core,
            rear,
            core_ana,
            extra_ana,
            x96: X96Encoder::new(16),
            params,
            exss: cfg.exss,
            taps: decimator_taps(frac),
            buf: vec![Vec::new(); extra_start + cfg.kind.extra_channels()],
            core_inputs,
            extra_start,
            dropped: 0,
            emitted: 0,
            n,
            l48,
            phase,
            lookahead,
            ana_delay,
            bits_num: cfg.total_kbps as u64 * 1000 * n as u64,
            bits_den: 96_000,
            overhead,
        })
    }

    /// Source samples (96 kHz) per frame.
    pub fn frame_samples(&self) -> usize {
        self.n
    }

    /// Delay in 96 kHz samples between the source and what the decoder outputs: the filter bank's, less the
    /// lookahead the decimated core input is taken ahead by.
    pub fn delay(&self) -> usize {
        (2 * self.core.delay()).saturating_sub(2 * self.l48)
    }

    /// Feed source samples, one vector per buffer: the five core channels (C L R Ls Rs, already mixed as the core
    /// carries them), the LFE if there is one, then the extra channels.
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
        out
    }

    fn decimated(&self, ch: usize, frame: usize) -> Vec<f64> {
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

    /// 64-band analysis of one frame of a channel, aligned with what the decoder's synthesis outputs, minus the
    /// 48 kHz coding's reconstruction in the lower 32 bands.
    fn analyse(
        ana: &mut Analysis64,
        src: &[f64],
        f_off: usize,
        dropped: usize,
        delay: usize,
        deq: &[Vec<i32>],
    ) -> Vec<Vec<f64>> {
        let blocks = 16;
        let start = f_off as isize + delay as isize - dropped as isize;
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
        for band in 0..32 {
            for b in 0..blocks {
                bands[band][b] -= deq[band][b] as f64;
            }
        }
        bands
    }

    fn next_frame(&mut self) -> Vec<u8> {
        let f = self.emitted;
        let dec: Vec<Vec<f64>> = {
            let me: &Hra96Encoder = &*self;
            use rayon::prelude::*;
            (0..me.buf.len()).into_par_iter().map(|c| me.decimated(c, f)).collect()
        };
        let core_in: Vec<Vec<f64>> = self.core_inputs.iter().map(|&i| dec[i].clone()).collect();
        let mut data = self.core.encode_frame(&core_in);
        data.resize(data.len() + (4 - data.len() % 4) % 4, 0);
        // sizes: the rest of the frame after the core and the extension substream header
        let upto = |k: u64| (k * self.bits_num) / (self.bits_den * 8);
        let total = (upto(f as u64 + 1) - upto(f as u64)) as usize;
        let rest = total.saturating_sub(data.len() + self.overhead);
        let (xxch_bytes, x96_bytes) = if self.kind == Hra96Kind::Rear {
            // the rear pair gets a fixed 512 bytes; X96 takes the rest up to its limit and XXCH what is beyond it
            let x96 = ((rest.saturating_sub(XXCH_FIXED_BYTES)) & !3).clamp(64, MAX_X96_BYTES);
            ((rest.saturating_sub(x96) & !3).clamp(64, 16_000), x96)
        } else {
            (0, (rest & !3).clamp(64, MAX_X96_BYTES))
        };
        let xxch = match self.rear.as_mut() {
            Some(r) => r.encode_xxch(&dec[self.extra_start..], xxch_bytes, &self.params),
            None => Vec::new(),
        };
        // X96: the five core channels, then the extra channels that have a set of their own
        let (f_off, dropped, delay) = (f * self.n, self.dropped, self.ana_delay);
        let bufs = &self.buf;
        let core_deq = &self.core.last_deq;
        let extra_start = self.extra_start;
        let rear_deq = self.rear.as_ref().map(|r| &r.last_deq);
        let kind = self.kind;
        let (r_core, r_extra): (Vec<Vec<Vec<f64>>>, Vec<Vec<Vec<f64>>>) = {
            use rayon::prelude::*;
            let a: Vec<_> = self
                .core_ana
                .par_iter_mut()
                .enumerate()
                .map(|(ch, ana)| Self::analyse(ana, &bufs[ch], f_off, dropped, delay, &core_deq[ch]))
                .collect();
            let b: Vec<_> = self
                .extra_ana
                .par_iter_mut()
                .enumerate()
                .map(|(j, ana)| {
                    // the XCH channel's reconstruction is the core encoder's sixth plan; the rears' is the rear encoder's
                    let deq = if kind == Hra96Kind::Discrete {
                        &core_deq[5]
                    } else {
                        &rear_deq.unwrap()[j]
                    };
                    Self::analyse(ana, &bufs[extra_start + j], f_off, dropped, delay, deq)
                })
                .collect();
            (a, b)
        };
        let x96 = if r_extra.is_empty() {
            self.x96.encode_exss(&[&r_core], x96_bytes)
        } else {
            self.x96.encode_exss(&[&r_core, &r_extra], x96_bytes)
        };
        if self.kind == Hra96Kind::Rear {
            data.extend_from_slice(&exss_frame_xxch_x96(&self.exss, &xxch, &x96));
        } else {
            data.extend_from_slice(&exss_frame_x96(&self.exss, &x96));
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
        data
    }
}
