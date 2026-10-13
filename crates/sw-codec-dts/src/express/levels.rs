//! The shape of a stream: how many bands are coded and how the regions of the spectrum fall (`Geo`), the quantizer
//! alphabets and the levels a quantizer profile assigns to each band.

use crate::express::filterbank::BANDS;
use crate::express::tables::{
    QUANT_AMP, RSD_LEVEL_16, RSD_LEVEL_2A, RSD_LEVEL_3, RSD_LEVEL_5, RSD_LEVEL_8, SB_REORDER, SCF_TO_GRID_1,
    SCF_TO_GRID_2,
};

/// Amplitude the decoder applies to a sample pair with scale factor `s` and `step` attenuation steps: the step
/// is subtracted from the scale factor, and a negative result silences the pair.
pub(crate) fn step_gain(s: usize, step: u8) -> f64 {
    if (step as usize) > s {
        0.0
    } else {
        QUANT_AMP[s - step as usize] as f64
    }
}

/// The values the decoder reconstructs, before the scale factor, for quantizer level 1 to 5.
pub(crate) fn alphabet(level: usize) -> Vec<f32> {
    match level {
        1 => RSD_LEVEL_2A.to_vec(),
        2 => RSD_LEVEL_3.to_vec(),
        3 => RSD_LEVEL_5.to_vec(),
        4 => RSD_LEVEL_8.to_vec(),
        _ => RSD_LEVEL_16.to_vec(),
    }
}

pub(crate) fn nearest(alpha: &[f32], x: f32) -> usize {
    let mut best = 0;
    let mut bd = f32::MAX;
    for (i, a) in alpha.iter().enumerate() {
        let d = (a - x).abs();
        if d < bd {
            bd = d;
            best = i;
        }
    }
    best
}

/// Quantizer level (1..=5) of every position from the quantizer profile byte (clause 9.6.3.3.2).
pub(crate) fn levels_from_profile(profile: u32, nsubbands: usize, limited_rate: u32) -> Vec<usize> {
    let ol = ((profile >> 3) & 7) as i32;
    let st = (profile >> 6) as i32;
    let max_sb = (profile & 7) as usize;
    let q: Vec<usize> = (0..nsubbands)
        .map(|sb| {
            let f = (sb as i32) * limited_rate as i32 / nsubbands as i32;
            let a = 18000 / (12 * f / 1000 + 100 + 40 * st) + 20 * ol;
            match a {
                i32::MIN..=95 => 1,
                96..=140 => 2,
                141..=180 => 3,
                181..=230 => 4,
                _ => 5,
            }
        })
        .collect();
    let mut out = q.clone();
    for sb in 0..8.min(nsubbands) {
        out[sb] = q[SB_REORDER[max_sb][sb] as usize];
    }
    out
}

pub(crate) const SUBBANDS: usize = BANDS;

/// Band layout of a stream: the number of coded subbands and the regions derived from it (clause 9.6.2).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Geo {
    /// 0, 1 or 2: the stream is limited to 1/1, 1/2 or 1/4 of the sample rate's bandwidth.
    pub(crate) band_limit: u32,
    pub(crate) nsb: usize,
    pub(crate) limited_rate: u32,
    pub(crate) min_mono: usize,
    pub(crate) max_mono: usize,
    /// Groups of the first grid (bands 2 up to this) and of the second grid.
    pub(crate) ng1: usize,
    pub(crate) ng2: usize,
    /// Bits of a band index in the time-sample chunks.
    pub(crate) idx_bits: u32,
}

impl Geo {
    pub(crate) fn new(sample_rate: u32, band_limit: u32) -> Geo {
        let nsb = SUBBANDS >> band_limit;
        let limited_rate = sample_rate >> band_limit;
        let half = (limited_rate / 2) as usize;
        Geo {
            band_limit,
            nsb,
            limited_rate,
            min_mono: (nsb * 2000 / half).min(nsb),
            max_mono: (nsb * 14000 / half).min(nsb),
            ng1: SCF_TO_GRID_1[nsb - 1] as usize + 1,
            ng2: SCF_TO_GRID_2[nsb - 1] as usize + 1,
            idx_bits: 3 + (2 - band_limit),
        }
    }
}

pub(crate) const AMP_MAX: usize = 56;

/// Narrowest coded bandwidth (in bands) the rate control may fall back to.
pub(crate) const MIN_K: usize = 6;
