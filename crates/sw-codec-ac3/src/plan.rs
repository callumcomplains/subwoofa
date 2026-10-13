//! What the encoder works out for a frame before writing it: per channel and set of blocks the exponents, power
//! spectral density and masking curve.

use crate::exponents::Strat;
use crate::mdct::M;
use crate::{BANDS, BLOCKS};

/// Everything derived from the coefficients of one channel for one set of blocks.
pub(crate) struct SetPlan {
    pub(crate) strat: Strat,
    /// The coupling channel's exponents as sent (reference first, then one per bin from its start).
    pub(crate) virt: Vec<i32>,
    pub(crate) exp: Vec<i32>,
    pub(crate) psd: Vec<i32>,
    pub(crate) mask: [i32; BANDS],
}

pub(crate) struct ChannelPlan {
    pub(crate) start: usize,
    pub(crate) is_cpl: bool,
    pub(crate) end: usize,
    pub(crate) strat: Strat,
    pub(crate) is_lfe: bool,
    pub(crate) coef: Vec<[f32; M]>,
    pub(crate) sets: Vec<SetPlan>,
}

/// A frame after the transform: what the parallel part of the encoder needs.
pub(crate) struct Analysed {
    pub(crate) coefs: Vec<Vec<[f32; M]>>,
    pub(crate) blksw: Vec<[bool; BLOCKS]>,
    pub(crate) words: usize,
    pub(crate) frmsizecod: u32,
}
