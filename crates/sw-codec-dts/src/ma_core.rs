//! DTS-HD Master Audio with a core: a CBR core in the core substream plus the lossless residual of the
//! core in an extension substream.
//!
//! For 96 kHz sources the core is a 48 kHz stream of the (decimated) programme; the decoder synthesises
//! it at 96 kHz with its 64-band filter bank and the lossless extension carries the remaining difference.

use crate::encoder::{Config as CoreConfig, Encoder as CoreEncoder};
use crate::exss::{exss_frame, ExssParams};
use crate::ma::map_layout;
use crate::native::map_channels as core_map_channels;
use crate::ma::Hier;
use crate::xll::{XllConfig, XllEncoder};
use sw_core::ChannelLayout;

pub struct MaCoreConfig {
    pub layout: &'static ChannelLayout,
    pub sample_rate: u32,
    pub bit_res: u32,
    pub core_bitrate: u32,
    pub dialnorm: i32,
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
    let fc = 22_300.0 / 96_000.0; // cycles per sample
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

/// Core (CBR, lossy) plus lossless residual, emitted frame by frame.
pub struct MaCoreEncoder {
    core: CoreEncoder,
    exss: ExssParams,
    /// Source samples per XLL channel (coded order), not yet fully consumed.
    buf: Vec<Vec<i32>>,
    /// Core slot (primary channels in stream order, then LFE) for every XLL channel (none for channels
    /// that only the lossless extension carries).
    xll_to_core: Vec<Option<usize>>,
    hier: Option<Hier>,
    /// Source samples per frame (XLL frame size).
    n: usize,
    /// Source samples the core input runs ahead of the output.
    lookahead: usize,
    /// 2 when the source is 96 kHz and the core runs at 48 kHz.
    ratio: usize,
    taps: Vec<f64>,
    /// Core-rate samples of advance and decimation phase.
    l48: usize,
    /// Whole samples (at the source rate) of decimation phase; the fraction lives in the filter taps.
    phase: usize,
    emitted: usize,
    t_core: std::time::Duration,
    t_xll: std::time::Duration,
    /// Lossless stage of the previous batch of frames, running while the next batch is core-encoded.
    pending: Option<std::thread::JoinHandle<Vec<Vec<u8>>>>,
    /// Source samples already discarded from the front of `buf`.
    dropped: usize,
    xll_cfg: XllConfig,
    core_pad: usize,
    shift: u32,
}

impl MaCoreEncoder {
    pub fn new(cfg: MaCoreConfig) -> Result<MaCoreEncoder, crate::Error> {
        let xmap = map_layout(cfg.layout);
        // the core carries the channels of the first set
        let base = xmap.set_sizes[0];
        let base_chs: Vec<sw_core::Channel> = xmap.order[..base].iter().map(|&i| cfg.layout.channels[i]).collect();
        let cmap = core_map_channels(&base_chs, cfg.layout.id)?;
        if base != cfg.layout.channels.len() && xmap.hier.is_none() {
            return Err(crate::Error::Unsupported(format!("layout '{}' needs the DTS extension channels (not implemented yet)", cfg.layout.id)));
        }
        let nprim = cmap.order.len();
        let ratio = match cfg.sample_rate {
            44_100 | 48_000 => 1,
            88_200 | 96_000 => 2,
            r => return Err(crate::Error::Unsupported(format!("{r} Hz is not supported by DTS-HD MA yet"))),
        };
        let mut core = CoreEncoder::new(CoreConfig {
            sample_rate: cfg.sample_rate / ratio as u32,
            amode: cmap.amode,
            channels: nprim,
            lfe: cmap.lfe.is_some(),
            bitrate: cfg.core_bitrate,
            dialnorm: cfg.dialnorm,
            pcm_bits: cfg.bit_res,
            es: false,
            xch: false,
            perfect: false,
            blocks: 16,
        })?;
        core.enable_exact_decode(ratio == 2);
        let n = core.frame_samples() * ratio;
        let nch = xmap.order.len();
        let xll_to_core: Vec<Option<usize>> = (0..nch)
            .map(|x| {
                if x >= base {
                    return None;
                }
                Some(cmap.order.iter().position(|&o| o == x).or_else(|| (cmap.lfe == Some(x)).then_some(nprim)).expect("layout channel missing from the core"))
            })
            .collect();
        let xll_cfg = XllConfig { sample_rate: cfg.sample_rate, bit_res: cfg.bit_res, frame_samples: n, sets: xmap.xll_sets(true) };
        let exss = ExssParams { sample_rate: cfg.sample_rate, channels: nch, bit_res: cfg.bit_res, spk_mask: xmap.exss_mask, with_core: true, dialnorm: 0, frame_duration_code: 0 };
        let core_pad = (4 - core.frame_bytes() % 4) % 4;
        let mut frac = 0.0;
        let (l48, phase) = if ratio == 1 {
            (core.delay(), 0)
        } else {
            let l48 = std::env::var("SW_L48").ok().and_then(|v| v.parse().ok()).unwrap_or(480);
            let phase: f64 = std::env::var("SW_PHASE").ok().and_then(|v| v.parse().ok()).unwrap_or(0.5);
            frac = phase - phase.floor();
            (l48, phase.floor() as usize)
        };
        let lookahead = if ratio == 1 { l48 } else { 2 * (l48 + 1) + phase + HALF + 2 };
        Ok(MaCoreEncoder {
            core,
            exss,
            buf: vec![Vec::new(); nch],
            xll_to_core,
            hier: xmap.hier.clone(),
            n,
            lookahead,
            ratio,
            taps: if ratio == 2 { decimator_taps(frac) } else { Vec::new() },
            l48,
            phase,
            emitted: 0,
            t_core: std::time::Duration::ZERO,
            t_xll: std::time::Duration::ZERO,
            pending: None,
            dropped: 0,
            xll_cfg,
            core_pad,
            shift: 24 - cfg.bit_res,
        })
    }

    pub fn frame_samples(&self) -> usize {
        self.n
    }

    /// Feed source samples per XLL channel (any number); returns the encoded frames that became ready.
    pub fn push(&mut self, frame: &[Vec<i32>]) -> Vec<Vec<u8>> {
        let mut sent = frame.to_vec();
        if let Some(h) = &self.hier {
            h.apply(&mut sent);
        }
        for (b, f) in self.buf.iter_mut().zip(&sent) {
            b.extend_from_slice(f);
        }
        let mut jobs = Vec::new();
        while self.buf[0].len() >= (self.emitted + 1) * self.n + self.lookahead - self.dropped {
            jobs.push(self.core_stage());
        }
        self.finish_jobs(jobs)
    }

    /// Flush: pad with silence so every pushed sample is covered by an emitted frame.
    pub fn finish(&mut self) -> Vec<Vec<u8>> {
        let pushed = self.buf[0].len() + self.dropped;
        let frames = pushed.div_ceil(self.n);
        let want = frames * self.n + self.lookahead - self.dropped;
        for b in self.buf.iter_mut() {
            b.resize(want.max(b.len()), 0);
        }
        let mut jobs = Vec::new();
        while self.emitted < frames {
            jobs.push(self.core_stage());
        }
        let mut out = self.finish_jobs(jobs);
        out.extend(self.flush_pending());
        out
    }

    /// Hand the lossless stage of a batch of frames to the worker pool and return the frames of the previous
    /// batch (which ran meanwhile); `finish` collects the last batch.
    fn finish_jobs(&mut self, jobs: Vec<(Vec<u8>, Vec<Vec<i32>>)>) -> Vec<Vec<u8>> {
        if jobs.is_empty() {
            return Vec::new();
        }
        let cfg = self.xll_cfg.clone();
        let exss = self.exss.clone();
        let handle = std::thread::spawn(move || xll_stage(jobs, cfg, exss));
        match self.pending.replace(handle) {
            Some(prev) => prev.join().expect("lossless worker panicked"),
            None => Vec::new(),
        }
    }

    fn flush_pending(&mut self) -> Vec<Vec<u8>> {
        self.pending.take().map(|h| h.join().expect("lossless worker panicked")).unwrap_or_default()
    }

    /// Core-rate input samples for one channel and frame, in 24-bit units.
    fn core_input(&self, x: usize, frame: usize) -> Vec<f64> {
        let n48 = self.n / self.ratio;
        let src = &self.buf[x];
        let gain = (1u64 << self.shift) as f64;
        if self.ratio == 1 {
            let start = frame * self.n + self.l48 - self.dropped;
            return src[start..start + n48].iter().map(|v| *v as f64 * gain).collect();
        }
        let mut out = Vec::with_capacity(n48);
        for i in 0..n48 {
            let m = frame * n48 + i + self.l48;
            let centre = (2 * m + self.phase) as isize - self.dropped as isize;
            let mut acc = 0.0;
            for (j, h) in self.taps.iter().enumerate() {
                let idx = centre + j as isize - HALF as isize;
                if idx >= 0 && (idx as usize) < src.len() {
                    acc += h * src[idx as usize] as f64;
                }
            }
            out.push(acc * gain);
        }
        out
    }

    /// Core encoding of the next frame and its residual (sequential: the decoder state carries over).
    fn core_stage(&mut self) -> (Vec<u8>, Vec<Vec<i32>>) {
        let n = self.n;
        let start = self.emitted * n - self.dropped;
        let nch = self.buf.len();
        let ncore = self.xll_to_core.iter().flatten().copied().max().map_or(0, |m| m + 1);
        let mut core_in: Vec<Vec<f64>> = vec![Vec::new(); ncore];
        let inputs: Vec<(usize, usize, Vec<f64>)> = {
            use rayon::prelude::*;
            let me: &MaCoreEncoder = &*self;
            let work: Vec<(usize, usize)> = me.xll_to_core.iter().enumerate().filter_map(|(x, slot)| slot.map(|s| (x, s))).collect();
            work.into_par_iter().map(|(x, slot)| (x, slot, me.core_input(x, me.emitted))).collect()
        };
        for (x, slot, mut v) in inputs {
            if let Some(h) = &self.hier {
                let g = h.core_gain(x);
                v.iter_mut().for_each(|s| *s *= g);
            }
            core_in[slot] = v;
        }
        let t0 = std::time::Instant::now();
        let mut core_bytes = self.core.encode_frame(&core_in);
        self.t_core += t0.elapsed();
        let round = if self.shift > 0 { 1i32 << (self.shift - 1) } else { 0 };
        let mut resid: Vec<Vec<i32>> = Vec::with_capacity(nch);
        for (x, slot) in self.xll_to_core.iter().enumerate() {
            let Some(slot) = *slot else {
                resid.push(self.buf[x][start..start + n].to_vec());
                continue;
            };
            let c = &self.core.last_core_pcm[slot];
            debug_assert_eq!(c.len(), n);
            match &self.hier {
                // the decoder undoes the downmix scale on the core before adding the residual
                Some(h) => {
                    let inv = h.inv[x] as i64;
                    resid.push(
                        (0..n)
                            .map(|i| {
                                let up = ((c[i] as i64 * inv + (1 << 15)) >> 16) as i32;
                                let core = (((up as i64 + round as i64) >> self.shift) as i32).clamp(-(1 << 23), (1 << 23) - 1);
                                self.buf[x][start + i] - core
                            })
                            .collect(),
                    )
                }
                None => resid.push((0..n).map(|i| self.buf[x][start + i] - ((c[i] + round) >> self.shift)).collect()),
            }
        }
        if let Some(path) = std::env::var_os("SW_DUMP_RESID") {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                for r in &resid {
                    for v in r {
                        let _ = f.write_all(&v.to_le_bytes());
                    }
                }
            }
        }
        core_bytes.resize(core_bytes.len() + self.core_pad, 0);
        self.emitted += 1;
        // drop samples no later frame needs (keep the decimator's look-back)
        let keep_from = (self.emitted * n).saturating_sub(HALF + 4);
        if keep_from > self.dropped + (1 << 16) {
            let cut = keep_from - self.dropped;
            for b in self.buf.iter_mut() {
                b.drain(..cut);
            }
            self.dropped = keep_from;
        }
        (core_bytes, resid)
    }
}


/// Lossless coding of a batch of independent frames on the rayon pool; returns the finished stream frames.
fn xll_stage(jobs: Vec<(Vec<u8>, Vec<Vec<i32>>)>, cfg: XllConfig, exss: ExssParams) -> Vec<Vec<u8>> {
    use rayon::prelude::*;
    let threads = rayon::current_num_threads().max(1);
    let per = jobs.len().div_ceil(threads).max(1);
    jobs.par_chunks(per)
        .flat_map_iter(|chunk| {
            let mut xll = XllEncoder::new(cfg.clone());
            chunk
                .iter()
                .map(|(core_bytes, resid)| {
                    let data = xll.encode_frame(resid);
                    let mut frame = core_bytes.clone();
                    frame.extend_from_slice(&exss_frame(&exss, &data));
                    frame
                })
                .collect::<Vec<_>>()
        })
        .collect()
}
