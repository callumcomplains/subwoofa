//! DTS-HD Master Audio with a core: a CBR core in the core substream plus the lossless residual of the
//! core in an extension substream.
//!
//! For 96 kHz sources the core is a 48 kHz stream of the (decimated) programme; the decoder synthesises
//! it at 96 kHz with its 64-band filter bank and the lossless extension carries the remaining difference.

use crate::encoder::{Config as CoreConfig, Encoder as CoreEncoder};
use crate::exss::{exss_frame, ExssParams};
use crate::ma::map_layout;
use crate::native::map_layout as core_map_layout;
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

fn decimator_taps() -> Vec<f64> {
    let fc = 22_300.0 / 96_000.0; // cycles per sample
    let beta = 9.0;
    let mut h = Vec::with_capacity(2 * HALF + 1);
    for i in 0..=2 * HALF {
        let m = i as f64 - HALF as f64;
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
    /// Core slot (primary channels in stream order, then LFE) for every XLL channel.
    xll_to_core: Vec<usize>,
    /// Source samples per frame (XLL frame size).
    n: usize,
    /// Source samples the core input runs ahead of the output.
    lookahead: usize,
    /// 2 when the source is 96 kHz and the core runs at 48 kHz.
    ratio: usize,
    taps: Vec<f64>,
    /// Core-rate samples of advance and decimation phase.
    l48: usize,
    phase: usize,
    emitted: usize,
    t_core: std::time::Duration,
    t_xll: std::time::Duration,
    /// Source samples already discarded from the front of `buf`.
    dropped: usize,
    xll_cfg: XllConfig,
    core_pad: usize,
    shift: u32,
}

impl MaCoreEncoder {
    pub fn new(cfg: MaCoreConfig) -> Result<MaCoreEncoder, crate::Error> {
        let cmap = core_map_layout(cfg.layout)?;
        let xmap = map_layout(cfg.layout);
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
            blocks: 16,
        })?;
        core.enable_exact_decode(ratio == 2);
        let n = core.frame_samples() * ratio;
        let nch = xmap.order.len();
        let xll_to_core: Vec<usize> = xmap
            .order
            .iter()
            .map(|&lc| cmap.order.iter().position(|&o| o == lc).or_else(|| (cmap.lfe == Some(lc)).then_some(nprim)).expect("layout channel missing from the core"))
            .collect();
        let xll_cfg = XllConfig { sample_rate: cfg.sample_rate, channels: nch, ch_mask: xmap.xll_mask, bit_res: cfg.bit_res, residual_of_core: true, frame_samples: n };
        let exss = ExssParams { sample_rate: cfg.sample_rate, channels: nch, bit_res: cfg.bit_res, spk_mask: xmap.exss_mask, with_core: true, dialnorm: 0, frame_duration_code: 0 };
        let core_pad = (4 - core.frame_bytes() % 4) % 4;
        let (l48, phase) = if ratio == 1 {
            (core.delay(), 0)
        } else {
            let l48 = std::env::var("SW_L48").ok().and_then(|v| v.parse().ok()).unwrap_or(236);
            let phase = std::env::var("SW_PHASE").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
            (l48, phase)
        };
        let lookahead = if ratio == 1 { l48 } else { 2 * (l48 + 1) + phase + HALF + 2 };
        Ok(MaCoreEncoder {
            core,
            exss,
            buf: vec![Vec::new(); nch],
            xll_to_core,
            n,
            lookahead,
            ratio,
            taps: if ratio == 2 { decimator_taps() } else { Vec::new() },
            l48,
            phase,
            emitted: 0,
            t_core: std::time::Duration::ZERO,
            t_xll: std::time::Duration::ZERO,
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
        for (b, f) in self.buf.iter_mut().zip(frame) {
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
        self.finish_jobs(jobs)
    }

    /// Lossless stage of several frames in parallel (the frames are independent of each other).
    fn finish_jobs(&mut self, jobs: Vec<(Vec<u8>, Vec<Vec<i32>>)>) -> Vec<Vec<u8>> {
        if jobs.is_empty() {
            return Vec::new();
        }
        let t0 = std::time::Instant::now();
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(jobs.len());
        let exss = &self.exss;
        let xll_cfg = self.xll_cfg.clone();
        let mut results: Vec<Option<Vec<u8>>> = vec![None; jobs.len()];
        let per = jobs.len().div_ceil(threads);
        std::thread::scope(|sc| {
            for (chunk_jobs, chunk_out) in jobs.chunks(per).zip(results.chunks_mut(per)) {
                let cfg = xll_cfg.clone();
                sc.spawn(move || {
                    let mut xll = XllEncoder::new(cfg);
                    for ((core_bytes, resid), out) in chunk_jobs.iter().zip(chunk_out.iter_mut()) {
                        let data = xll.encode_frame(resid);
                        let mut frame = core_bytes.clone();
                        frame.extend_from_slice(&exss_frame(exss, &data));
                        *out = Some(frame);
                    }
                });
            }
        });
        self.t_xll += t0.elapsed();
        if std::env::var_os("SW_TIMING").is_some() {
            eprintln!("ma timing: core {:?}, xll {:?}", self.t_core, self.t_xll);
        }
        results.into_iter().map(|r| r.unwrap()).collect()
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
        let ncore = self.xll_to_core.iter().copied().max().map_or(0, |m| m + 1);
        let mut core_in: Vec<Vec<f64>> = vec![Vec::new(); ncore];
        for (x, &slot) in self.xll_to_core.iter().enumerate() {
            core_in[slot] = self.core_input(x, self.emitted);
        }
        let t0 = std::time::Instant::now();
        let mut core_bytes = self.core.encode_frame(&core_in);
        self.t_core += t0.elapsed();
        let round = if self.shift > 0 { 1i32 << (self.shift - 1) } else { 0 };
        let mut resid: Vec<Vec<i32>> = Vec::with_capacity(nch);
        for (x, &slot) in self.xll_to_core.iter().enumerate() {
            let c = &self.core.last_core_pcm[slot];
            debug_assert_eq!(c.len(), n);
            resid.push((0..n).map(|i| self.buf[x][start + i] - ((c[i] + round) >> self.shift)).collect());
        }
        core_bytes.resize(core_bytes.len() + self.core_pad, 0);
        self.emitted += 1;
        // drop samples no later frame needs (keep the decimator's look-back)
        let keep_from = (self.emitted * n).saturating_sub(HALF + 4);
        if keep_from > self.dropped + 1 << 16 {
            let cut = keep_from - self.dropped;
            for b in self.buf.iter_mut() {
                b.drain(..cut);
            }
            self.dropped = keep_from;
        }
        (core_bytes, resid)
    }
}
