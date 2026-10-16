//! One channel of a block: the cheapest of constant, raw, fixed polynomial and fitted linear predictors.

use crate::bitwriter::BitWriter;
use crate::flac::rice::{rice_code, write_rice, RiceCoding};
use crate::flac::search::{Search, LPC_PRECISION};

pub(crate) enum Predictor {
    Constant(i32),
    Verbatim,
    Fixed {
        order: usize,
    },
    Linear {
        order: usize,
        shift: u32,
        coefficients: Vec<i32>,
    },
}

/// A channel of a block, coded.
pub(crate) struct Subframe {
    /// Low bits that were zero in every sample and are sent once instead.
    pub(crate) wasted: u32,
    /// Bits per sample after removing the wasted ones.
    pub(crate) bits_per_sample: u32,
    pub(crate) predictor: Predictor,
    pub(crate) samples: Vec<i32>,
    pub(crate) residual: Vec<i32>,
    pub(crate) rice: Option<RiceCoding>,
    /// Size in bits, header included.
    pub(crate) bits: usize,
}

/// Codes one channel with the smallest predictor. `sample_bits` is the sample width of this channel (one more for the
/// side channel of a stereo pair).
pub(crate) fn code_channel(samples: &[i32], sample_bits: u32, search: Search) -> Subframe {
    let n = samples.len();
    let or_all = samples.iter().fold(0i32, |a, &s| a | s);
    if or_all == 0 {
        return Subframe {
            wasted: 0,
            bits_per_sample: sample_bits,
            predictor: Predictor::Constant(0),
            samples: Vec::new(),
            residual: Vec::new(),
            rice: None,
            bits: 8 + sample_bits as usize,
        };
    }
    let wasted = or_all.trailing_zeros();
    let x: Vec<i32> = samples.iter().map(|&s| s >> wasted).collect();
    let bps = sample_bits - wasted;
    let header = 8 + wasted as usize; // type byte (with the wasted flag) and the unary wasted-bits count
    let mut best = Subframe {
        wasted,
        bits_per_sample: bps,
        predictor: Predictor::Verbatim,
        samples: x.clone(),
        residual: Vec::new(),
        rice: None,
        bits: header + n * bps as usize,
    };
    if x.iter().all(|&v| v == x[0]) {
        best.predictor = Predictor::Constant(x[0]);
        best.bits = header + bps as usize;
        return best;
    }
    // fixed polynomial predictors
    for order in 0..=search.fixed_max.min(n.saturating_sub(1)) {
        let residual = fixed_residual(&x, order);
        let rice = rice_code(&residual, order, n, search.max_partition);
        let bits = header + order * bps as usize + rice.bits;
        if bits < best.bits {
            best = Subframe {
                wasted,
                bits_per_sample: bps,
                predictor: Predictor::Fixed { order },
                samples: x.clone(),
                residual,
                rice: Some(rice),
                bits,
            };
        }
    }
    // linear predictors fitted to this block
    if n >= 64 {
        for (order, coefficients, shift) in fit_linear(&x, search) {
            let residual = linear_residual(&x, &coefficients, shift);
            let rice = rice_code(&residual, order, n, search.max_partition);
            let bits = header + 4 + 5 + order * LPC_PRECISION as usize + order * bps as usize + rice.bits;
            if bits < best.bits {
                best = Subframe {
                    wasted,
                    bits_per_sample: bps,
                    predictor: Predictor::Linear {
                        order,
                        shift,
                        coefficients,
                    },
                    samples: x.clone(),
                    residual,
                    rice: Some(rice),
                    bits,
                };
            }
        }
    }
    best
}

pub(crate) fn fixed_residual(x: &[i32], order: usize) -> Vec<i32> {
    // coefficients of the fixed predictors, newest sample first
    const COEFFICIENTS: [&[i64]; 5] = [&[], &[1], &[2, -1], &[3, -3, 1], &[4, -6, 4, -1]];
    let c = COEFFICIENTS[order];
    (order..x.len())
        .map(|i| {
            let prediction: i64 = c.iter().enumerate().map(|(j, &cj)| cj * x[i - 1 - j] as i64).sum();
            (x[i] as i64 - prediction) as i32
        })
        .collect()
}

pub(crate) fn linear_residual(x: &[i32], coefficients: &[i32], shift: u32) -> Vec<i32> {
    let order = coefficients.len();
    (order..x.len())
        .map(|i| {
            let prediction: i64 = coefficients
                .iter()
                .enumerate()
                .map(|(j, &c)| c as i64 * x[i - 1 - j] as i64)
                .sum();
            (x[i] as i64 - (prediction >> shift)) as i32
        })
        .collect()
}

/// Linear predictors of the orders in `search.lpc_orders`: the block is windowed, its autocorrelation is turned into
/// predictor coefficients with the Levinson-Durbin recursion, and the coefficients are rounded to integers with a
/// common shift. Returns (order, coefficients, shift) for each.
pub(crate) fn fit_linear(x: &[i32], search: Search) -> Vec<(usize, Vec<i32>, u32)> {
    let top = search.top_order();
    let n = x.len();
    // Tukey window (flat in the middle, cosine ramps over the outer quarter at each end)
    let ramp = (n / 4).max(1);
    let windowed: Vec<f64> = x
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let edge = i.min(n - 1 - i);
            let w = if edge < ramp {
                0.5 - 0.5 * (std::f64::consts::PI * edge as f64 / ramp as f64).cos()
            } else {
                1.0
            };
            s as f64 * w
        })
        .collect();
    // every lag in one pass (each lag still adds its products in time order); the reversed copy puts the earlier
    // samples of one position side by side, so the inner loop is a plain vector multiply-add
    let reversed: Vec<f64> = windowed.iter().rev().copied().collect();
    let mut autocorrelation = vec![0f64; top + 1];
    for (j, w) in windowed.iter().enumerate() {
        let lags = j.min(top);
        for (slot, earlier) in autocorrelation[..=lags].iter_mut().zip(&reversed[n - 1 - j..]) {
            *slot += w * earlier;
        }
    }
    if autocorrelation[0] <= 0.0 {
        return Vec::new();
    }
    autocorrelation[0] *= 1.0 + 1e-9; // keeps the recursion stable on very tonal blocks

    // Levinson-Durbin: coefficient sets for every order up to the maximum
    let mut sets: Vec<Vec<f64>> = vec![Vec::new()];
    let mut current: Vec<f64> = Vec::new();
    let mut error = autocorrelation[0];
    for order in 1..=top {
        let acc: f64 = autocorrelation[order]
            - current
                .iter()
                .enumerate()
                .map(|(j, c)| c * autocorrelation[order - 1 - j])
                .sum::<f64>();
        let reflection = acc / error;
        let mut next = vec![0f64; order];
        for j in 0..order - 1 {
            next[j] = current[j] - reflection * current[order - 2 - j];
        }
        next[order - 1] = reflection;
        error *= 1.0 - reflection * reflection;
        current = next;
        sets.push(current.clone());
        if error <= 0.0 {
            break;
        }
    }
    search
        .lpc_orders
        .iter()
        .filter(|&&order| order < sets.len() && order < n)
        .map(|&order| {
            let (coefficients, shift) = quantise(&sets[order]);
            (order, coefficients, shift)
        })
        .collect()
}

/// Rounds predictor coefficients to `LPC_PRECISION`-bit integers sharing one right shift.
pub(crate) fn quantise(coefficients: &[f64]) -> (Vec<i32>, u32) {
    let largest = coefficients.iter().fold(0f64, |m, c| m.max(c.abs())).max(1e-12);
    let integer_bits = largest.log2().floor() as i32 + 1; // largest < 2^integer_bits
    let shift = (LPC_PRECISION as i32 - 1 - integer_bits).clamp(0, 15);
    let limit = (1i64 << (LPC_PRECISION - 1)) - 1;
    let scale = (1u64 << shift) as f64;
    let q = coefficients
        .iter()
        .map(|&c| ((c * scale).round() as i64).clamp(-limit - 1, limit) as i32)
        .collect();
    (q, shift as u32)
}

pub(crate) fn write_subframe(w: &mut BitWriter, s: &Subframe) {
    let type_code: u64 = match &s.predictor {
        Predictor::Constant(_) => 0,
        Predictor::Verbatim => 1,
        Predictor::Fixed { order } => 8 + *order as u64,
        Predictor::Linear { order, .. } => 32 + (*order as u64 - 1),
    };
    w.put(0, 1); // padding bit
    w.put(type_code as u32, 6);
    if s.wasted > 0 {
        w.put(1, 1);
        w.put(0, s.wasted - 1); // wasted bits - 1 zeros, then a one
        w.put(1, 1);
    } else {
        w.put(0, 1);
    }
    let bps = s.bits_per_sample;
    let block_len = s.samples.len();
    match &s.predictor {
        Predictor::Constant(v) => w.put_signed((*v as i64) as i32, bps),
        Predictor::Verbatim => {
            for &v in &s.samples {
                w.put_signed(v, bps);
            }
        }
        Predictor::Fixed { order } => {
            for &v in &s.samples[..*order] {
                w.put_signed(v, bps);
            }
            write_rice(w, &s.residual, *order, block_len, s.rice.as_ref().unwrap());
        }
        Predictor::Linear {
            order,
            shift,
            coefficients,
        } => {
            for &v in &s.samples[..*order] {
                w.put_signed(v, bps);
            }
            w.put((LPC_PRECISION as u64 - 1) as u32, 4);
            w.put_signed((*shift as i64) as i32, 5);
            for &c in coefficients {
                w.put_signed(c, LPC_PRECISION);
            }
            write_rice(w, &s.residual, *order, block_len, s.rice.as_ref().unwrap());
        }
    }
}
