//! The adaptive linear predictor: fitting starting coefficients to a frame and running the sign-sign LMS filter.

use crate::alac::entropy::entropy_code;
use crate::alac::PREDICTOR_SHIFT;
use crate::bitwriter::BitWriter;

/// `value` taken as a signed number of `bits` bits.
pub(crate) fn sign_extend(value: i32, bits: u32) -> i32 {
    let shift = 32 - bits;
    (value << shift) >> shift
}

/// Residual of the adaptive predictor for a channel. `coefficients` are the starting values (the decoder adapts its
/// own copy in step); `bits` is the width the residual is wrapped to.
pub(crate) fn predict(samples: &[i32], starting: &[i32], bits: u32) -> Vec<i32> {
    let n = samples.len();
    let order = starting.len();
    let mut residual = vec![0i32; n];
    residual[0] = samples[0];
    if order == 0 {
        residual[1..].copy_from_slice(&samples[1..]);
        return residual;
    }
    let mut coefficients = starting.to_vec();
    for i in 1..=order.min(n - 1) {
        residual[i] = sign_extend(samples[i].wrapping_sub(samples[i - 1]), bits);
    }
    for i in order + 1..n {
        let top = samples[i - order - 1];
        let mut sum = 1i32 << (PREDICTOR_SHIFT - 1);
        for (j, &c) in coefficients.iter().enumerate() {
            sum = sum.wrapping_add(samples[i - 1 - j].wrapping_sub(top).wrapping_mul(c));
        }
        let predicted = (sum >> PREDICTOR_SHIFT).wrapping_add(top);
        let error = sign_extend(samples[i].wrapping_sub(predicted), bits);
        residual[i] = error;
        // adapt the coefficients: nudge each by the sign of its input, oldest first, until the error has been used up
        if error != 0 {
            let negative = error < 0;
            let mut left = error;
            for index in (0..order).rev() {
                if (negative && left >= 0) || (!negative && left <= 0) {
                    break;
                }
                let diff = top.wrapping_sub(samples[i - 1 - index]);
                let mut sign = diff.signum();
                if negative {
                    sign = -sign;
                }
                coefficients[index] -= sign;
                let moved = diff.wrapping_mul(sign);
                left = left.wrapping_sub((moved >> PREDICTOR_SHIFT).wrapping_mul((order - index) as i32));
            }
        }
    }
    residual
}

/// Starting coefficients for a predictor of `order` taps: the least-squares fit of each sample's difference from the
/// sample `order + 1` back, on the differences of the `order` samples in between (which is the form the decoder uses).
pub(crate) fn fit_coefficients(samples: &[i32], order: usize) -> Option<Vec<i32>> {
    let n = samples.len();
    if n < order * 4 + 8 {
        return None;
    }
    let mut flat = vec![0f64; order * order];
    let mut b = vec![0f64; order];
    let mut e = vec![0f64; order];
    for i in order + 1..n {
        let top = samples[i - order - 1] as f64;
        for (k, v) in e.iter_mut().enumerate() {
            *v = samples[i - 1 - k] as f64 - top;
        }
        let d = samples[i] as f64 - top;
        for p in 0..order {
            b[p] += e[p] * d;
            for (acc, eq) in flat[p * order + p..(p + 1) * order].iter_mut().zip(&e[p..]) {
                *acc += e[p] * eq;
            }
        }
    }
    let mut a: Vec<Vec<f64>> = flat.chunks_exact(order).map(|row| row.to_vec()).collect();
    for p in 0..order {
        for q in 0..p {
            a[p][q] = a[q][p];
        }
        a[p][p] += a[p][p] * 1e-9 + 1e-3;
    }
    let solution = solve(a, b)?;
    let limit = i16::MAX as f64;
    Some(
        solution
            .iter()
            .map(|c| (c * (1u32 << PREDICTOR_SHIFT) as f64).round().clamp(-limit, limit) as i32)
            .collect(),
    )
}

/// Gaussian elimination with partial pivoting.
pub(crate) fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let pivot = (col..n).max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs()))?;
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        for row in col + 1..n {
            let f = a[row][col] / a[col][col];
            for c in col..n {
                a[row][c] -= f * a[col][c];
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = vec![0f64; n];
    for row in (0..n).rev() {
        let mut v = b[row];
        for c in row + 1..n {
            v -= a[row][c] * x[c];
        }
        x[row] = v / a[row][row];
    }
    Some(x)
}

/// One channel's chosen predictor and what it leaves.
pub(crate) struct Predicted {
    pub(crate) coefficients: Vec<i32>,
    pub(crate) residual: Vec<i32>,
    pub(crate) bits: usize,
}

/// Tries the predictor orders on a channel and keeps the one that codes smallest.
pub(crate) fn best_prediction(samples: &[i32], bits: u32, orders: &[usize]) -> Predicted {
    let mut best: Option<Predicted> = None;
    for &order in orders {
        let coefficients = if order == 0 {
            Vec::new()
        } else {
            match fit_coefficients(samples, order) {
                Some(c) => c,
                None => continue,
            }
        };
        let residual = predict(samples, &coefficients, bits);
        let mut w = BitWriter::default();
        entropy_code(&mut w, &residual, bits);
        let total = w.bytes.len() * 8 + order * 16;
        if best.as_ref().is_none_or(|b| total < b.bits) {
            best = Some(Predicted {
                coefficients,
                residual,
                bits: total,
            });
        }
    }
    best.unwrap_or_else(|| {
        let residual = predict(samples, &[], bits);
        Predicted {
            coefficients: Vec::new(),
            residual,
            bits: usize::MAX,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alac::PREDICTOR_SHIFT;

    #[test]
    fn predictor_residual_is_what_the_decoder_undoes() {
        // decode the residual with the decoder's rule and compare with the input
        let samples: Vec<i32> = (0..500)
            .map(|i| ((i as f64 * 0.07).sin() * 9000.0) as i32 + (i * 7 % 13))
            .collect();
        let coefficients = fit_coefficients(&samples, 8).unwrap();
        let residual = predict(&samples, &coefficients, 17);
        let order = 8usize;
        let mut c = coefficients.clone();
        let mut out = vec![0i32; samples.len()];
        out[0] = residual[0];
        for i in 1..=order {
            out[i] = sign_extend(out[i - 1] + residual[i], 17);
        }
        for i in order + 1..samples.len() {
            let top = out[i - order - 1];
            let mut sum = 1i32 << (PREDICTOR_SHIFT - 1);
            for j in 0..order {
                sum += (out[i - 1 - j] - top) * c[j];
            }
            let value = sign_extend((sum >> PREDICTOR_SHIFT) + top + residual[i], 17);
            out[i] = value;
            let err = residual[i];
            if err != 0 {
                let neg = err < 0;
                let mut left = err;
                for index in (0..order).rev() {
                    if (neg && left >= 0) || (!neg && left <= 0) {
                        break;
                    }
                    let d = top - out[i - 1 - index];
                    let mut s = d.signum();
                    if neg {
                        s = -s;
                    }
                    c[index] -= s;
                    left -= ((d * s) >> PREDICTOR_SHIFT) * (order - index) as i32;
                }
            }
        }
        assert_eq!(out, samples);
    }
}
