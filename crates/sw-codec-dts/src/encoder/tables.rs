//! Small helpers on the core's quantizer tables: step sizes, level counts and the bit costs of plain coding.

use crate::tables::{QUANT_LEVELS, STEP_LOSSY};

pub(crate) const SEL_BITS: [u32; 10] = [1, 2, 2, 2, 2, 3, 3, 3, 3, 3];

pub(crate) const GROUP_SIZE: [usize; 10] = [1, 3, 3, 3, 3, 7, 7, 7, 7, 7];

/// Bits of one block code word (4 samples) for abits 1..=7.
pub(crate) const BLOCK_BITS: [u32; 7] = [7, 10, 12, 13, 15, 17, 19];

pub(crate) fn step_of(abits: usize) -> f64 {
    STEP_LOSSY[abits] as f64 / 4_194_304.0
}

pub(crate) fn max_q(abits: usize) -> i32 {
    (QUANT_LEVELS[abits] as i32 - 1) / 2
}

/// Bits of a plain (no further coding) index.
pub(crate) fn raw_bits(abits: usize) -> u32 {
    abits as u32 - 3
}

/// Decoder output scale relative to the subband values produced by the analysis bank.
pub const SUBBAND_GAIN: f64 = 1.0 / 181.019_335_983_756_17;
