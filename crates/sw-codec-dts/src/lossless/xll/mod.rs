//! DTS-HD lossless extension (XLL): frame encoder.
//!
//! Per frame and channel set: optional pairwise channel decorrelation, adaptive prediction through
//! quantized reflection coefficients (or a fixed polynomial predictor), and Rice / linear coding of the
//! residual in segments (ETSI TS 102 114 clause 8).

mod arith;
pub mod bands;
mod plan;
mod predict;
mod residual;
#[rustfmt::skip]
pub mod table;
mod write;

pub const SYNC_XLL: u32 = 0x41A2_9547;

/// One row of an embedded downmix matrix: how a channel of a lower set was scaled and what the channels
/// of this set were added to it with (9-bit codes: bit 8 set = positive, low 8 bits index `DMIX`).
#[derive(Debug, Clone)]
pub struct DmixRow {
    pub scale_code: u16,
    pub coeff_codes: Vec<u16>,
}

#[derive(Debug, Clone)]
pub struct DmixHeader {
    /// One row per channel of the lower-indexed sets.
    pub rows: Vec<DmixRow>,
}

#[derive(Debug, Clone)]
pub struct SetCfg {
    pub channels: usize,
    /// Speaker position bit mask of the set (bit 0 = C, 1 = L, 2 = R, 3 = Ls, 4 = Rs, 5 = LFE1, ...).
    pub ch_mask: u32,
    /// Residual of a core (true) or the original audio (false).
    pub residual_of_core: bool,
    /// Present for the non-primary sets of a hierarchy.
    pub dmix: Option<DmixHeader>,
    /// The primary set can describe how a decoder mixes its channels down to stereo: the matrix type (1 = Lo/Ro,
    /// 2 = Lt/Rt) and, for each of the two outputs, a 9-bit code for every channel of the set.
    pub matrix: Option<(u8, Vec<Vec<u16>>)>,
}

#[derive(Debug, Clone)]
pub struct XllConfig {
    pub sample_rate: u32,
    /// Resolution of the coded samples (16, 20 or 24 bits).
    pub bit_res: u32,
    /// Samples per channel in one frame.
    pub frame_samples: usize,
    pub sets: Vec<SetCfg>,
    /// How many predictors are tried for every channel.
    pub effort: sw_core::Effort,
}

pub fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

pub(crate) struct BandPlan {
    pub(crate) plans: Vec<ChannelPlan>,
    pub(crate) orig_order: Vec<usize>,
    pub(crate) coeffs: Vec<i32>,
    pub(crate) decor: bool,
}

/// A channel set: one band below 192 kHz, two above (plus the band-0 history the decoder needs).
pub(crate) struct SetPlan {
    pub(crate) bands: Vec<BandPlan>,
    pub(crate) history: Vec<[i32; crate::lossless::xll::bands::HISTORY]>,
}

pub(crate) struct ChannelPlan {
    pub(crate) adapt_order: usize,
    pub(crate) fixed_order: usize,
    pub(crate) rc_idx: Vec<i32>,
    pub(crate) residual: Vec<i32>,
}

pub struct XllEncoder {
    pub(crate) cfg: XllConfig,
    pub max_order: usize,
}

/// Segment count assumed while choosing predictors.
const NSEG: usize = 4;

impl XllEncoder {
    pub fn new(cfg: XllConfig) -> XllEncoder {
        let max_order = if cfg.sample_rate > 48_000 { 15 } else { 12 };
        XllEncoder { cfg, max_order }
    }

    fn nabits(&self) -> u32 {
        let n = if self.cfg.bit_res > 16 {
            5
        } else if self.cfg.bit_res > 8 {
            4
        } else {
            3
        };
        // room for the embedded downmix and the decimator saturation
        if (self.cfg.sets.len() > 1 || self.nbands() > 1) && n < 5 {
            n + 1
        } else {
            n
        }
    }

    /// Frequency bands: above 96 kHz the signal is split in two.
    fn nbands(&self) -> usize {
        if self.cfg.sample_rate > 96_000 {
            2
        } else {
            1
        }
    }

    /// Samples per channel and band in one frame.
    fn band_len(&self) -> usize {
        self.cfg.frame_samples / self.nbands()
    }

    /// Encode one frame of samples (`bit_res`-bit signed integers, one vector per channel: the channels of
    /// the first set in speaker order, then those of the next set, ...).
    pub fn encode_frame(&mut self, pcm: &[Vec<i32>]) -> Vec<u8> {
        self.encode_frame_core(pcm, None)
    }

    /// Like `encode_frame`, for a stream with a core under the lossless data at 176.4 / 192 kHz: `core0[ch]` is the
    /// core's decoded output at the band rate (half the sample rate) for each channel of the first set, which the
    /// decoder adds to band 0; the band-0 samples are coded as the difference.
    pub fn encode_frame_core(&mut self, pcm: &[Vec<i32>], core0: Option<&[Vec<i32>]>) -> Vec<u8> {
        let cfg = self.cfg.clone();
        let len = cfg.frame_samples;
        assert!(
            self.nbands() == 1 || cfg.sets.len() == 1,
            "frequency bands: a single channel set only"
        );
        assert_eq!(pcm.len(), cfg.sets.iter().map(|s| s.channels).sum::<usize>());
        assert!(pcm.iter().all(|c| c.len() == len));
        let mut set_plans = Vec::new();
        let mut at = 0;
        for (si, set) in cfg.sets.iter().enumerate() {
            set_plans.push(self.plan_set(&pcm[at..at + set.channels], if si == 0 { core0 } else { None }));
            at += set.channels;
        }
        // band data: one block per segment; try several segment counts and keep the smallest frame
        let mut best_frame: Option<Vec<u8>> = None;
        for &nseg in &[1usize, 2, 4, 8] {
            let seg_len = self.band_len() / nseg;
            if seg_len < 16 || seg_len > if cfg.sample_rate > 48_000 { 512 } else { 256 } {
                continue;
            }
            let segs: Vec<Vec<Vec<Vec<u8>>>> = set_plans
                .iter()
                .map(|sp| (0..sp.bands.len()).map(|b| self.set_segments(sp, b, nseg)).collect())
                .collect();
            let frame = self.assemble(&set_plans, &segs, nseg);
            if best_frame.as_ref().is_none_or(|f| frame.len() < f.len()) {
                best_frame = Some(frame);
            }
        }
        best_frame.expect("no valid segmentation")
    }
}

pub fn fs_index(rate: u32) -> u32 {
    match rate {
        8000 => 0,
        16000 => 1,
        32000 => 2,
        64000 => 3,
        128000 => 4,
        22050 => 5,
        44100 => 6,
        88200 => 7,
        176400 => 8,
        352800 => 9,
        12000 => 10,
        24000 => 11,
        48000 => 12,
        96000 => 13,
        192000 => 14,
        384000 => 15,
        _ => 12,
    }
}
