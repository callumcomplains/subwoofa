//! Prediction of a channel's samples: quantized reflection coefficients (PARCOR) from the autocorrelation or
//! covariance method, the direct-form filter the decoder derives from them, and the residual it leaves.

use crate::lossless::xll::arith::{clip23, mul16, norm16};
use crate::lossless::xll::table::REFL_COEFF;

/// Direct form coefficients (error filter `1 + sum c_k z^-k`) from quantized reflection coefficients,
/// exactly as the decoder derives them.
pub(crate) fn lattice_to_direct(rc: &[i32]) -> Vec<i32> {
    let order = rc.len();
    let mut coeff = vec![0i32; order];
    for j in 0..order {
        let r = rc[j];
        for k in 0..j.div_ceil(2) {
            let t1 = coeff[k];
            let t2 = coeff[j - k - 1];
            coeff[k] = t1.wrapping_add(mul16(r, t2));
            coeff[j - k - 1] = t2.wrapping_add(mul16(r, t1));
        }
        coeff[j] = r;
    }
    coeff
}

/// Quantize a reflection coefficient (|k| < 1) to a table index with sign.
pub(crate) fn quantize_rc(k: f64) -> i32 {
    let target = (k.abs() * 65536.0).min(65491.0);
    let mut best = 0usize;
    let mut bd = f64::MAX;
    for (i, v) in REFL_COEFF.iter().enumerate() {
        let d = (*v as f64 - target).abs();
        if d < bd {
            bd = d;
            best = i;
        }
    }
    if k < 0.0 {
        -(best as i32)
    } else {
        best as i32
    }
}

pub(crate) fn rc_value(idx: i32) -> i32 {
    if idx < 0 {
        -(REFL_COEFF[(-idx) as usize] as i32)
    } else {
        REFL_COEFF[idx as usize] as i32
    }
}

/// The fraction of a frame the autocorrelation window tapers over (a Tukey window).
pub(crate) const WINDOW_TAPER: f64 = 0.3;

/// Levinson-Durbin: reflection coefficients of the error filter `1 + sum a_k z^-k` up to `max_order`.
pub(crate) fn reflection_coefficients(x: &[i32], max_order: usize) -> Vec<f64> {
    let n = x.len();
    // a Tukey window: flat in the middle, cosine ramps over the outer `WINDOW_TAPER / 2` at each end
    let xs: Vec<f64> = x
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let t = (i as f64 + 0.5) / n as f64;
            let edge = t.min(1.0 - t);
            let w = if edge < WINDOW_TAPER / 2.0 {
                0.5 - 0.5 * (std::f64::consts::PI * (2.0 * edge / WINDOW_TAPER)).cos()
            } else {
                1.0
            };
            *v as f64 * w
        })
        .collect();
    // all lags in one pass; each lag still adds its products in time order. The reversed copy makes the lags of
    // one sample contiguous, so the inner loop is a plain vector multiply-add.
    let reversed: Vec<f64> = xs.iter().rev().copied().collect();
    let mut r = vec![0f64; max_order + 1];
    for (i, xi) in xs.iter().enumerate() {
        let lags = i.min(max_order);
        for (acc, past) in r[..=lags].iter_mut().zip(&reversed[n - 1 - i..]) {
            *acc += xi * past;
        }
    }
    r[0] = r[0] * (1.0 + 1e-9) + 1e-3;
    let mut a = vec![0f64; max_order + 1];
    let mut ks = Vec::new();
    let mut err = r[0];
    for i in 1..=max_order {
        let mut acc = r[i];
        for j in 1..i {
            acc += a[j] * r[i - j];
        }
        let k = if err.abs() < 1e-9 { 0.0 } else { -acc / err };
        let k = k.clamp(-0.9995, 0.9995);
        let prev = a.clone();
        for j in 1..i {
            a[j] = prev[j] + k * prev[i - j];
        }
        a[i] = k;
        err *= 1.0 - k * k;
        ks.push(k);
    }
    ks
}

/// Reflection coefficients from a covariance-method (windowless least squares) predictor, by step-down.
pub(crate) fn covariance_reflection(x: &[i32], order: usize) -> Option<Vec<f64>> {
    let n = x.len();
    if n <= 4 * order {
        return None;
    }
    // `back[k]` is the sample k places before the end, so the `order` samples before time t are a contiguous
    // window and the inner loop is a plain vector multiply-add (each cell still adds in time order).
    let back: Vec<f64> = x.iter().rev().map(|v| *v as f64).collect();
    let mut flat = vec![0f64; order * order];
    let mut r = vec![0f64; order];
    for t in order..n {
        let from_end = n - t;
        let window = &back[from_end..from_end + order]; // window[j] is the sample j + 1 before t
        let now = back[from_end - 1];
        for i in 0..order {
            let xi = window[i];
            r[i] -= xi * now;
            for (acc, w) in flat[i * order + i..(i + 1) * order].iter_mut().zip(&window[i..]) {
                *acc += xi * w;
            }
        }
    }
    let mut m: Vec<Vec<f64>> = flat.chunks_exact(order).map(|row| row.to_vec()).collect();
    let mut trace = 0.0;
    for i in 0..order {
        for j in 0..i {
            m[i][j] = m[j][i];
        }
        trace += m[i][i];
    }
    if trace < 1.0 {
        return None;
    }
    for i in 0..order {
        m[i][i] += trace * 1e-10;
    }
    // Gaussian elimination
    let mut a = m;
    let mut b = r;
    for col in 0..order {
        let piv = (col..order).max_by(|&p, &q| a[p][col].abs().total_cmp(&a[q][col].abs()))?;
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for row in col + 1..order {
            let f = a[row][col] / a[col][col];
            for c in col..order {
                a[row][c] -= f * a[col][c];
            }
            b[row] -= f * b[col];
        }
    }
    let mut coef = vec![0f64; order];
    for row in (0..order).rev() {
        let mut v = b[row];
        for c in row + 1..order {
            v -= a[row][c] * coef[c];
        }
        coef[row] = v / a[row][row];
    }
    // step down: coef[k-1] multiplies x[n-k]
    let mut ks = vec![0f64; order];
    let mut cur = coef;
    for i in (0..order).rev() {
        let k = cur[i];
        if !k.is_finite() {
            return None;
        }
        ks[i] = k.clamp(-0.9995, 0.9995);
        if i == 0 {
            break;
        }
        let d = 1.0 - k * k;
        if d.abs() < 1e-9 {
            return None;
        }
        let mut next = vec![0f64; i];
        for j in 0..i {
            next[j] = (cur[j] - k * cur[i - 1 - j]) / d;
        }
        cur = next;
    }
    Some(ks)
}

/// The order of the linear predictor `whiten` takes out of a channel.
pub(crate) const WHITEN_ORDER: usize = 2;

/// The channel with its predictable part taken out (an order-2 linear predictor fitted to it).
pub(crate) fn whiten(x: &[i32]) -> Vec<f64> {
    let order = WHITEN_ORDER;
    if x.len() < 64 || x.iter().all(|v| *v == 0) {
        return x.iter().map(|v| *v as f64).collect();
    }
    let ks = reflection_coefficients(x, order);
    let rcq: Vec<i32> = ks.iter().map(|k| rc_value(quantize_rc(*k))).collect();
    let coeff = lattice_to_direct(&rcq);
    predict(x, &coeff).iter().map(|v| *v as f64).collect()
}

/// Prediction residual of `x` with the decoder's arithmetic (first `order` samples stay raw).
pub(crate) fn predict(x: &[i32], coeff: &[i32]) -> Vec<i32> {
    let order = coeff.len();
    let n = x.len();
    let mut res = x.to_vec();
    for j in 0..n.saturating_sub(order) {
        let mut acc = 0i64;
        for k in 0..order {
            acc += x[j + k] as i64 * coeff[order - k - 1] as i64;
        }
        let xhat = clip23(norm16(acc));
        res[j + order] = x[j + order].wrapping_sub(xhat.wrapping_neg());
    }
    res
}
