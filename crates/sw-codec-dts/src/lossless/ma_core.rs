//! DTS-HD Master Audio with a core: a CBR core in the core substream plus the lossless residual of the
//! core in an extension substream.
//!
//! For 96 kHz sources the core is a 48 kHz stream of the (decimated) programme; the decoder synthesises
//! it at 96 kHz with its 64-band filter bank and the lossless extension carries the remaining difference.

use crate::container::exss::{exss_frame, ExssParams};
use crate::dca::dts96::{CORE_ADVANCE, DECIMATION_PHASE};
use crate::dca::encoder::{Config as CoreConfig, Encoder as CoreEncoder};
use crate::dca::native::{map_channels as core_map_channels, LayoutMap};
use crate::dca::qmf64::{Analysis64, BANDS as BANDS64};
use crate::dca::x96::X96Encoder;
use crate::lossless::ma::{map_layout_with, Hier, MaMap};
use crate::lossless::xll::{XllConfig, XllEncoder};
use sw_core::{ChannelLayout, Downmix};

pub struct MaCoreConfig {
    pub layout: &'static ChannelLayout,
    pub sample_rate: u32,
    pub bit_res: u32,
    pub core_bitrate: u32,
    pub dialnorm: i32,
    /// 96 kHz only: give the core an X96 extension (a 96/24 core), so decoders that play just the core
    /// substream still get the full band. A lossless decoder ignores it.
    pub x96: bool,
    /// Add the speaker remapping sets of a wide 7.1 stream (Bitstream tab).
    pub wide_remap: bool,
    /// The Downmix tab (embedded stereo set, levels of the folds into 5.1).
    pub downmix: Downmix,
    /// How hard the lossless stage searches.
    pub effort: sw_core::Effort,
}

/// Half-length of the decimator (linear phase, Kaiser window, `2 * half + 1` taps) for a rate ratio of 2 or 4.
fn half_len(ratio: usize) -> usize {
    if ratio >= 4 {
        383
    } else {
        191
    }
}

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

fn decimator_taps(frac: f64, ratio: usize) -> Vec<f64> {
    let half = half_len(ratio);
    let fc = 22_300.0 / (48_000.0 * ratio as f64); // cycles per sample
    let beta = 9.0;
    let mut h = Vec::with_capacity(2 * half + 1);
    for i in 0..=2 * half {
        let m = i as f64 - half as f64 - frac;
        let sinc = if m == 0.0 {
            2.0 * fc
        } else {
            (2.0 * std::f64::consts::PI * fc * m).sin() / (std::f64::consts::PI * m)
        };
        let r = m / half as f64;
        let w = bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / bessel_i0(beta);
        h.push(sinc * w);
    }
    let sum: f64 = h.iter().sum();
    h.iter().map(|v| v / sum).collect()
}

/// One frame ready for the lossless stage: the core frame, the samples to code (a residual of the core, or the
/// plain frame when `core0` is given) and, at 176.4 / 192 kHz, the core's output for band 0.
type Job = (Vec<u8>, Vec<Vec<i32>>, Option<Vec<Vec<i32>>>);

/// Core (CBR, lossy) plus lossless residual, emitted frame by frame.
pub struct MaCoreEncoder {
    core: CoreEncoder,
    exss: ExssParams,
    /// Source samples per XLL channel (coded order), not yet fully consumed.
    buf: Vec<Vec<i32>>,
    /// With an embedded stereo set the core carries the 5.x programme, not the stereo downmix of `buf`: the same
    /// channels with only the folds into 5.1 applied (empty otherwise).
    cbuf: Vec<Vec<i32>>,
    /// Coded channels coded as the residual of the core.
    resid: std::ops::Range<usize>,
    /// Core slot (primary channels in stream order, then LFE) for every XLL channel (none for channels
    /// that only the lossless extension carries).
    xll_to_core: Vec<Option<usize>>,
    hier: Option<Hier>,
    /// Source samples per frame (XLL frame size).
    n: usize,
    /// Source samples the core input runs ahead of the output.
    lookahead: usize,
    /// 2 when the source is 96 kHz and the core runs at 48 kHz, 4 at 192 kHz (the core then sits under band 0).
    ratio: usize,
    half: usize,
    /// 192 kHz: feed the core from band 0 of the lifting split instead of from the raw source.
    from_band0: bool,
    /// Taps (ratio 2) that decimate band 0 (96 kHz) to the core's 48 kHz.
    taps_b0: Vec<f64>,
    /// Gain of the core's input relative to the source (band 0 of a 192 kHz stream is not at unity).
    cgain: f64,
    taps: Vec<f64>,
    /// Core-rate samples of advance and decimation phase.
    l48: usize,
    /// Whole samples (at the source rate) of decimation phase; the fraction lives in the filter taps.
    phase: usize,
    emitted: usize,
    /// The previous batch of frames, waiting for its lossless stage, which runs while the next batch is
    /// core-encoded.
    pending: Option<Vec<Job>>,
    /// Source samples already discarded from the front of `buf`.
    dropped: usize,
    xll_cfg: XllConfig,
    core_pad: usize,
    shift: u32,
    x96: Option<X96State>,
    /// Primary channels of the core (the LFE slot comes after them).
    nprim: usize,
    /// Lossless channel that also goes into the core's XCH extension (the centre surround of a 6.x discrete stream).
    xch_x: Option<usize>,
}

/// The X96 part of a 96/24 core: 64-band analysis of the programme, aligned with the core's subbands.
struct X96State {
    ext: Vec<Analysis64>,
    enc: X96Encoder,
    bytes: usize,
    delay: usize,
}

impl X96State {
    /// Sets the core up to reserve room for the X96 extension and keep its residual.
    fn new(core: &mut CoreEncoder, nprim: usize) -> X96State {
        let bytes = (core.frame_bytes() / 8).max(96) & !3;
        core.keep_residual = true;
        core.x96_reserve = bytes;
        let ext: Vec<Analysis64> = (0..nprim).map(|_| Analysis64::new()).collect();
        let delay = ext[0].delay();
        X96State {
            ext,
            enc: X96Encoder::new(16),
            bytes,
            delay,
        }
    }
}

/// How many times the source rate is above the core's: 1 up to 48 kHz, 2 at 88.2 / 96, 4 at 176.4 / 192.
fn rate_ratio(rate: u32) -> Result<usize, crate::Error> {
    match rate {
        44_100 | 48_000 => Ok(1),
        88_200 | 96_000 => Ok(2),
        176_400 | 192_000 => Ok(4),
        r => Err(crate::Error::Unsupported(format!(
            "{r} Hz is not supported by DTS-HD MA yet"
        ))),
    }
}

fn core_encoder(
    cfg: &MaCoreConfig,
    cmap: &LayoutMap,
    nprim: usize,
    xch: bool,
    ratio: usize,
) -> Result<CoreEncoder, crate::Error> {
    let mut core = CoreEncoder::new(CoreConfig {
        sample_rate: cfg.sample_rate / ratio as u32,
        amode: cmap.amode,
        channels: nprim,
        lfe: cmap.lfe.is_some(),
        bitrate: cfg.core_bitrate,
        dialnorm: cfg.dialnorm,
        pcm_bits: cfg.bit_res,
        // the commercial encoder flags the core of a 6.x or Ls/Rs + Lsr/Rsr stream as ES (its surrounds carry more)
        es: cfg.layout.id.starts_with("6.") || cfg.layout.id == "7.1-ls-rs-lsr-rsr",
        xch,
        perfect: false,
        blocks: 16,
    })?;
    core.set_aux_downmix(crate::lossless::ma::core_aux(
        &cfg.downmix,
        cfg.layout,
        cmap.amode,
        cmap.lfe.is_some(),
        xch,
    ));
    Ok(core)
}

/// The extension substream's description of the stream.
fn exss_params(cfg: &MaCoreConfig, xmap: &MaMap, x96: bool, xch: bool) -> ExssParams {
    ExssParams {
        sample_rate: cfg.sample_rate,
        channels: xmap.order.len(),
        bit_res: cfg.bit_res,
        spk_mask: xmap.exss_mask,
        with_core: true,
        dialnorm: (-cfg.dialnorm) as u32,
        frame_duration_code: 0,
        x96_in_core: x96,
        xch_in_core: xch,
        secondary: false,
        extras: crate::container::exss::ExssExtras {
            min_header: 32,
            emb_5_1: xmap.hier.as_ref().is_some_and(|h| h.extras),
            emb_stereo: xmap.hier.as_ref().is_some_and(|h| h.stereo),
            wide_remap: cfg.wide_remap,
            repr_type: (cfg.layout.kind == sw_core::layout::LayoutKind::LtRt).then_some(2),
            ..Default::default()
        },
    }
}

/// Where the core's samples line up with the source's: the advance and decimation phase of the resampler, and how
/// far ahead of a frame the source must have been read.
struct Timing {
    /// Core-rate samples of advance.
    l48: usize,
    /// Whole samples (at the source rate) of decimation phase.
    phase: usize,
    /// The fraction of a sample of phase, which lives in the filter taps.
    frac: f64,
    half: usize,
    lookahead: usize,
}

impl Timing {
    fn new(core: &CoreEncoder, ratio: usize, x96: Option<&X96State>) -> Timing {
        let mut frac = 0.0;
        let (l48, phase) = if ratio == 1 {
            (core.delay(), 0)
        } else {
            // at 192 kHz the core is fed from band 0 of the split, which has no half-sample offset
            let phase = if ratio == 4 { 0.0 } else { DECIMATION_PHASE };
            frac = phase - phase.floor();
            (CORE_ADVANCE, phase.floor() as usize)
        };
        let half = half_len(ratio);
        let mut lookahead = if ratio == 1 {
            l48
        } else {
            ratio * (l48 + 1) + phase + half + 2
        };
        if let Some(x) = x96 {
            lookahead = lookahead.max(x.delay + 64 * 16 + 64);
        }
        Timing {
            l48,
            phase,
            frac,
            half,
            lookahead,
        }
    }
}

impl MaCoreEncoder {
    pub fn new(cfg: MaCoreConfig) -> Result<MaCoreEncoder, crate::Error> {
        let xmap = map_layout_with(cfg.layout, &cfg.downmix);
        // the core carries the channels of the first set
        let base = xmap.core_channels();
        let base_chs: Vec<sw_core::Channel> = xmap.order[..base].iter().map(|&i| cfg.layout.channels[i]).collect();
        let cmap = core_map_channels(&base_chs, cfg.layout.id)?;
        if base != cfg.layout.channels.len() && xmap.hier.is_none() {
            return Err(crate::Error::Unsupported(format!(
                "layout '{}' needs the DTS extension channels (not implemented yet)",
                cfg.layout.id
            )));
        }
        let nprim = cmap.order.len();
        // 6.x discrete: the centre surround also rides in the core as an XCH extension, so ES decoders can play it
        // from the core alone (the lossless data still carries it as a set of its own)
        let xch_x =
            (cfg.layout.id.starts_with("6.") && cfg.layout.kind == sw_core::layout::LayoutKind::Discrete && !cfg.x96)
                .then(|| {
                    xmap.order
                        .iter()
                        .position(|&i| cfg.layout.channels[i] == sw_core::Channel::Cs)
                })
                .flatten()
                .filter(|&x| x >= base);
        let ratio = rate_ratio(cfg.sample_rate)?;
        let mut core = core_encoder(&cfg, &cmap, nprim, xch_x.is_some(), ratio)?;
        let x96 = (cfg.x96 && ratio == 2).then(|| X96State::new(&mut core, nprim));
        core.enable_exact_decode(ratio >= 2);
        let n = core.frame_samples() * ratio;
        let nch = xmap.order.len();
        // for each lossless channel, the core channel (LFE after the primaries) it also feeds
        let xll_to_core: Vec<Option<usize>> = (0..nch)
            .map(|x| {
                if x >= base {
                    return None;
                }
                Some(
                    cmap.order
                        .iter()
                        .position(|&o| o == x)
                        .or_else(|| (cmap.lfe == Some(x)).then_some(nprim))
                        .expect("layout channel missing from the core"),
                )
            })
            .collect();
        let xll_cfg = XllConfig {
            sample_rate: cfg.sample_rate,
            bit_res: cfg.bit_res,
            frame_samples: n,
            sets: xmap.xll_sets(true),
            effort: cfg.effort,
        };
        let exss = exss_params(&cfg, &xmap, x96.is_some(), xch_x.is_some());
        let core_pad = (4 - core.frame_bytes() % 4) % 4;
        let timing = Timing::new(&core, ratio, x96.as_ref());
        Ok(MaCoreEncoder {
            core,
            exss,
            buf: vec![Vec::new(); nch],
            cbuf: if xmap.hier.as_ref().is_some_and(|h| h.stereo) {
                vec![Vec::new(); nch]
            } else {
                Vec::new()
            },
            resid: match &xmap.hier {
                Some(h) => h.sets[xmap.resid_set].clone(),
                None => 0..base,
            },
            xll_to_core,
            hier: xmap.hier.clone(),
            n,
            lookahead: timing.lookahead,
            ratio,
            half: timing.half,
            from_band0: ratio == 4,
            taps_b0: if ratio == 4 {
                decimator_taps(timing.frac, 2)
            } else {
                Vec::new()
            },
            cgain: 1.0,
            taps: if ratio >= 2 {
                decimator_taps(timing.frac, ratio)
            } else {
                Vec::new()
            },
            l48: timing.l48,
            phase: timing.phase,
            emitted: 0,
            pending: None,
            dropped: 0,
            xll_cfg,
            core_pad,
            shift: 24 - cfg.bit_res,
            x96,
            nprim,
            xch_x,
        })
    }

    pub fn frame_samples(&self) -> usize {
        self.n
    }

    /// Feed source samples per XLL channel (any number); returns the encoded frames that became ready.
    pub fn push(&mut self, frame: &[Vec<i32>]) -> Vec<Vec<u8>> {
        let mut sent = frame.to_vec();
        if let Some(h) = &self.hier {
            h.apply(&mut sent, false);
            if h.stereo {
                let mut core_sent = frame.to_vec();
                h.apply(&mut core_sent, true);
                for (b, f) in self.cbuf.iter_mut().zip(&core_sent) {
                    b.extend_from_slice(f);
                }
            }
        }
        for (b, f) in self.buf.iter_mut().zip(&sent) {
            b.extend_from_slice(f);
        }
        // every frame whose samples (and the look-ahead behind them) have arrived
        let arrived = self.buf[0].len() + self.dropped;
        let ready = (arrived.saturating_sub(self.lookahead) / self.n).saturating_sub(self.emitted);
        self.next_batch(ready)
    }

    /// Flush: pad with silence so every pushed sample is covered by an emitted frame.
    pub fn finish(&mut self) -> Vec<Vec<u8>> {
        let pushed = self.buf[0].len() + self.dropped;
        let frames = pushed.div_ceil(self.n);
        let want = frames * self.n + self.lookahead - self.dropped;
        for b in self.buf.iter_mut().chain(self.cbuf.iter_mut()) {
            b.resize(want.max(b.len()), 0);
        }
        let mut out = self.next_batch(frames.saturating_sub(self.emitted));
        out.extend(self.flush_pending());
        out
    }

    /// Core-encodes the next `count` frames while the lossless stage runs on the batch before them, and returns
    /// the finished frames of that earlier batch (`finish` collects the last one).
    fn next_batch(&mut self, count: usize) -> Vec<Vec<u8>> {
        let (cfg, exss) = (self.xll_cfg.clone(), self.exss.clone());
        let earlier = self.pending.take();
        let (done, jobs) = rayon::join(
            move || earlier.map(|jobs| xll_stage(jobs, cfg, exss)).unwrap_or_default(),
            || self.core_stages(count),
        );
        self.pending = (!jobs.is_empty()).then_some(jobs);
        done
    }

    fn flush_pending(&mut self) -> Vec<Vec<u8>> {
        self.pending
            .take()
            .map(|jobs| xll_stage(jobs, self.xll_cfg.clone(), self.exss.clone()))
            .unwrap_or_default()
    }

    /// Analyse the (core-domain) programme with the 64-band bank, subtract what the core coded in the lower
    /// half and code the rest into the X96 block at the end of the core frame.
    fn add_x96(&mut self, frame: &mut [u8]) {
        use rayon::prelude::*;
        let blocks = 16usize;
        let (n, f_off, dropped, nprim) = (self.n, self.emitted * self.n, self.dropped, self.nprim);
        let gain0 = (1u64 << self.shift) as f64;
        let hier = &self.hier;
        let bufs = if self.cbuf.is_empty() { &self.buf } else { &self.cbuf };
        let deq = &self.core.last_deq;
        let work: Vec<(usize, usize)> = self
            .xll_to_core
            .iter()
            .enumerate()
            .filter_map(|(x, slot)| slot.filter(|s| *s < nprim).map(|s| (x, s)))
            .collect();
        let x96 = self.x96.as_mut().unwrap();
        let delay = x96.delay;
        let _ = n;
        let mut resid: Vec<Vec<Vec<f64>>> = vec![Vec::new(); nprim];
        let jobs: Vec<(usize, usize, &mut Analysis64, &mut Vec<Vec<f64>>)> = {
            let mut anas: Vec<Option<&mut Analysis64>> = x96.ext.iter_mut().map(Some).collect();
            let mut outs: Vec<Option<&mut Vec<Vec<f64>>>> = resid.iter_mut().map(Some).collect();
            work.iter()
                .map(|&(x, slot)| (x, slot, anas[slot].take().unwrap(), outs[slot].take().unwrap()))
                .collect()
        };
        jobs.into_par_iter().for_each(|(x, slot, ana, out)| {
            let g = gain0 * hier.as_ref().map_or(1.0, |h| h.core_gain(x));
            let src = &bufs[x];
            let start = f_off as isize + delay as isize - dropped as isize;
            let input: Vec<f64> = (0..blocks * 64)
                .map(|i| {
                    let idx = start + i as isize;
                    if idx >= 0 && (idx as usize) < src.len() {
                        src[idx as usize] as f64 * g
                    } else {
                        0.0
                    }
                })
                .collect();
            let mut bands = ana.analyse_blocks(&input);
            debug_assert_eq!(bands.len(), BANDS64);
            for band in 0..32 {
                for b in 0..blocks {
                    bands[band][b] -= deq[slot][band][b] as f64;
                }
            }
            *out = bands;
        });
        let bytes = x96.bytes;
        let block = x96.enc.encode(&resid, bytes);
        let start = frame.len() - bytes;
        frame[start..].copy_from_slice(&block);
    }

    /// The programme the core is made from, per coded channel.
    fn core_src(&self) -> &Vec<Vec<i32>> {
        if self.cbuf.is_empty() {
            &self.buf
        } else {
            &self.cbuf
        }
    }

    /// Core-rate input samples for one channel and frame, in 24-bit units.
    fn core_input(&self, x: usize, frame: usize) -> Vec<f64> {
        let n48 = self.n / self.ratio;
        let src = &self.core_src()[x];
        let gain = (1u64 << self.shift) as f64;
        if self.ratio == 1 {
            let start = frame * self.n + self.l48 - self.dropped;
            return src[start..start + n48].iter().map(|v| *v as f64 * gain).collect();
        }
        if self.from_band0 {
            // split the stretch of source this frame's core samples draw on into the two bands (as the lossless
            // stage will), and decimate band 0 by two with the 96 kHz filter
            let half2 = half_len(2);
            let first_m = frame * n48 + self.l48;
            let lo_b0 = (2 * first_m) as isize - half2 as isize - 8;
            let hi_b0 = (2 * (first_m + n48)) as isize + half2 as isize + 8 + self.phase as isize / 2;
            let src_lo = (2 * lo_b0 - self.dropped as isize).max(0) as usize & !1;
            let src_hi = ((2 * hi_b0 - self.dropped as isize) as usize).min(src.len()) & !1;
            let mut out = vec![0f64; n48];
            if src_hi > src_lo + 64 {
                let (b0, _, _) = crate::lossless::xll::bands::split(&src[src_lo..src_hi]);
                let b0_origin = ((src_lo + self.dropped) / 2) as isize; // band-0 index of b0[0]
                for (i, o) in out.iter_mut().enumerate() {
                    let centre = (2 * (first_m + i) + self.phase) as isize - b0_origin;
                    let mut acc = 0.0;
                    for (j, h) in self.taps_b0.iter().enumerate() {
                        let idx = centre + j as isize - half2 as isize;
                        if idx >= 0 && (idx as usize) < b0.len() {
                            acc += h * b0[idx as usize] as f64;
                        }
                    }
                    *o = acc * gain * self.cgain;
                }
            }
            return out;
        }
        let mut out = Vec::with_capacity(n48);
        for i in 0..n48 {
            let m = frame * n48 + i + self.l48;
            let centre = (self.ratio * m + self.phase) as isize - self.dropped as isize;
            let mut acc = 0.0;
            for (j, h) in self.taps.iter().enumerate() {
                let idx = centre + j as isize - self.half as isize;
                if idx >= 0 && (idx as usize) < src.len() {
                    acc += h * src[idx as usize] as f64;
                }
            }
            out.push(acc * gain * self.cgain);
        }
        out
    }

    /// Core encoding of the next `count` frames and their residuals. The core encodes the frames together; what
    /// follows for each frame (the X96 block, the residual, dropping old samples) is in order, because it
    /// reads the core's leftovers of that frame.
    fn core_stages(&mut self, count: usize) -> Vec<Job> {
        use rayon::prelude::*;
        if count == 0 {
            return Vec::new();
        }
        let first = self.emitted;
        let inputs: Vec<Vec<Vec<f64>>> = {
            let me: &MaCoreEncoder = &*self;
            (first..first + count)
                .into_par_iter()
                .map(|frame| me.core_inputs(frame))
                .collect()
        };
        let refs: Vec<&[Vec<f64>]> = inputs.iter().map(|f| f.as_slice()).collect();
        let coded = self.core.encode_batch(&refs);
        coded
            .into_iter()
            .map(|(bytes, left)| {
                self.core.adopt(left);
                self.after_core(bytes)
            })
            .collect()
    }

    /// What the core encodes for `frame`: its input per core slot (primaries in stream order, the XCH plane, then
    /// the LFE).
    fn core_inputs(&self, frame: usize) -> Vec<Vec<f64>> {
        let ncore = self.xll_to_core.iter().flatten().copied().max().map_or(0, |m| m + 1);
        let mut core_in: Vec<Vec<f64>> = vec![Vec::new(); ncore];
        for (x, slot) in self.xll_to_core.iter().enumerate() {
            let Some(slot) = *slot else { continue };
            let mut v = self.core_input(x, frame);
            if let Some(h) = &self.hier {
                let g = h.core_gain(x);
                v.iter_mut().for_each(|s| *s *= g);
            }
            core_in[slot] = v;
        }
        if let Some(xs) = self.xch_x {
            // the XCH plane sits between the primaries and the LFE
            let v = self.core_input(xs, frame);
            core_in.insert(self.nprim, v);
        }
        core_in
    }

    /// The rest of the next frame after the core has coded it: the X96 block, and the residual the lossless
    /// stage codes (sequential: the decoder state carries over).
    fn after_core(&mut self, mut core_bytes: Vec<u8>) -> Job {
        let n = self.n;
        let start = self.emitted * n - self.dropped;
        let nch = self.buf.len();
        if self.x96.is_some() {
            self.add_x96(&mut core_bytes);
        }
        let round = if self.shift > 0 { 1i32 << (self.shift - 1) } else { 0 };
        if self.ratio == 4 {
            // 176.4 / 192 kHz: the core (48 kHz, synthesised to 96 kHz by the decoder) sits under band 0; the
            // lossless stage splits the plain frame into its two bands and codes band 0 minus the core
            let half = n / 2;
            let core0: Vec<Vec<i32>> = self
                .xll_to_core
                .iter()
                .map(|slot| match *slot {
                    None => vec![0; half],
                    Some(slot) => {
                        let c = &self.core.last_core_pcm[slot];
                        debug_assert_eq!(c.len(), half);
                        (0..half)
                            .map(|i| {
                                (((c[i] as i64 + round as i64) >> self.shift) as i32).clamp(-(1 << 23), (1 << 23) - 1)
                            })
                            .collect::<Vec<i32>>()
                    }
                })
                .collect();
            let frame: Vec<Vec<i32>> = (0..nch).map(|x| self.buf[x][start..start + n].to_vec()).collect();
            core_bytes.resize(core_bytes.len() + self.core_pad, 0);
            self.emitted += 1;
            let keep_from = (self.emitted * n).saturating_sub(self.half + 4);
            if keep_from > self.dropped + (1 << 16) {
                let cut = keep_from - self.dropped;
                for b in self.buf.iter_mut().chain(self.cbuf.iter_mut()) {
                    b.drain(..cut);
                }
                self.dropped = keep_from;
            }
            return (core_bytes, frame, Some(core0));
        }
        let mut resid: Vec<Vec<i32>> = Vec::with_capacity(nch);
        for (x, slot) in self.xll_to_core.iter().enumerate() {
            let Some(slot) = *slot else {
                resid.push(self.buf[x][start..start + n].to_vec());
                continue;
            };
            if !self.resid.contains(&x) {
                // carried by the core but coded on its own (the stereo downmix of an embedded stereo set)
                resid.push(self.buf[x][start..start + n].to_vec());
                continue;
            }
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
                                let core = (((up as i64 + round as i64) >> self.shift) as i32)
                                    .clamp(-(1 << 23), (1 << 23) - 1);
                                self.buf[x][start + i] - core
                            })
                            .collect(),
                    )
                }
                None => resid.push(
                    (0..n)
                        .map(|i| self.buf[x][start + i] - ((c[i] + round) >> self.shift))
                        .collect(),
                ),
            }
        }
        core_bytes.resize(core_bytes.len() + self.core_pad, 0);
        self.emitted += 1;
        // drop samples no later frame needs (keep the decimator's look-back)
        let keep_from = (self.emitted * n).saturating_sub(self.half + 4);
        if keep_from > self.dropped + (1 << 16) {
            let cut = keep_from - self.dropped;
            for b in self.buf.iter_mut().chain(self.cbuf.iter_mut()) {
                b.drain(..cut);
            }
            self.dropped = keep_from;
        }
        (core_bytes, resid, None)
    }
}

/// Lossless coding of a batch of independent frames on the rayon pool; returns the finished stream frames.
fn xll_stage(jobs: Vec<Job>, cfg: XllConfig, exss: ExssParams) -> Vec<Vec<u8>> {
    use rayon::prelude::*;
    let threads = rayon::current_num_threads().max(1);
    let per = jobs.len().div_ceil(threads).max(1);
    jobs.par_chunks(per)
        .flat_map_iter(|chunk| {
            let mut xll = XllEncoder::new(cfg.clone());
            chunk
                .iter()
                .map(|(core_bytes, resid, core0)| {
                    let data = xll.encode_frame_core(resid, core0.as_deref());
                    let mut frame = core_bytes.clone();
                    frame.extend_from_slice(&exss_frame(&exss, &data));
                    frame
                })
                .collect::<Vec<_>>()
        })
        .collect()
}
