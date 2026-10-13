//! Decimating a 96 or 192 kHz source to the core's 48 kHz: the filter, the rate ratio and the timing of the
//! first frame.

use crate::dca::dts96::{CORE_ADVANCE, DECIMATION_PHASE};
use crate::dca::encoder::Encoder as CoreEncoder;
use crate::lossless::ma_core::x96::X96State;

/// Half-length of the decimator (linear phase, Kaiser window, `2 * half + 1` taps) for a rate ratio of 2 or 4.
pub(crate) fn half_len(ratio: usize) -> usize {
    if ratio >= 4 {
        383
    } else {
        191
    }
}

pub(crate) fn bessel_i0(x: f64) -> f64 {
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

pub(crate) fn decimator_taps(frac: f64, ratio: usize) -> Vec<f64> {
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

/// How many times the source rate is above the core's: 1 up to 48 kHz, 2 at 88.2 / 96, 4 at 176.4 / 192.
pub(crate) fn rate_ratio(rate: u32) -> Result<usize, crate::Error> {
    match rate {
        44_100 | 48_000 => Ok(1),
        88_200 | 96_000 => Ok(2),
        176_400 | 192_000 => Ok(4),
        r => Err(crate::Error::Unsupported(format!(
            "{r} Hz is not supported by DTS-HD MA yet"
        ))),
    }
}

/// Where the core's samples line up with the source's: the advance and decimation phase of the resampler, and how
/// far ahead of a frame the source must have been read.
pub(crate) struct Timing {
    /// Core-rate samples of advance.
    pub(crate) l48: usize,
    /// Whole samples (at the source rate) of decimation phase.
    pub(crate) phase: usize,
    /// The fraction of a sample of phase, which lives in the filter taps.
    pub(crate) frac: f64,
    pub(crate) half: usize,
    pub(crate) lookahead: usize,
}

impl Timing {
    pub(crate) fn new(core: &CoreEncoder, ratio: usize, x96: Option<&X96State>) -> Timing {
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
