//! DTS-HD Master Audio with a core: a CBR core in the core substream plus the lossless residual of the
//! core in an extension substream.
//!
//! For 96 kHz sources the core is a 48 kHz stream of the (decimated) programme; the decoder synthesises
//! it at 96 kHz with its 64-band filter bank and the lossless extension carries the remaining difference.

mod input;
mod resample;
mod setup;
mod stages;
mod x96;

use resample::{decimator_taps, rate_ratio, Timing};
use setup::{core_encoder, exss_params};
use x96::X96State;

use crate::container::exss::ExssParams;
use crate::dca::encoder::Encoder as CoreEncoder;
use crate::dca::native::map_channels as core_map_channels;
use crate::lossless::ma::{map_layout_with, Hier};
use crate::lossless::xll::XllConfig;
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
}
