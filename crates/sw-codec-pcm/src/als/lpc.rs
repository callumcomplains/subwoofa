//! Linear prediction for MPEG-4 ALS: analysis, quantised PARCOR coefficients, and the exact integer filter the
//! decoder will run.
//!
//! ALS sends *reflection* (PARCOR) coefficients, not filter taps. The first two are quantised with a square-root
//! law (fine near +-1, where they matter most), the rest uniformly in steps of 1/64 and rebuilt at the middle of
//! their step. Both sides then turn the quantised values into direct taps with the same integer recursion, so the
//! encoder's residual is exactly what the decoder inverts.

/// 1.0 in the 20-bit fixed point of the format.
pub(crate) const ONE: i64 = 1 << 20;

/// Quantiser indices run over `-64..=63`.
pub(crate) const INDEX_MIN: i32 = -64;
pub(crate) const INDEX_MAX: i32 = 63;

/// The reconstruction value of index `i` (0..=127) for the first two coefficients, in 1/2^20 units.
fn companded(i: i64) -> i64 {
    32 + ((i * (i + 1)) << 7) - ONE
}

/// Quantiser index (`-64..=63`) of the first or second coefficient: the nearest value on the companded scale.
pub(crate) fn companded_index(value: f64) -> i32 {
    let target = value * ONE as f64;
    let mut best = 0i64;
    for i in 1..128 {
        if (companded(i) as f64 - target).abs() < (companded(best) as f64 - target).abs() {
            best = i;
        }
    }
    best as i32 + INDEX_MIN
}

/// Quantiser index of a later coefficient: steps of 1/64.
pub(crate) fn uniform_index(value: f64) -> i32 {
    ((value * 64.0).floor() as i32).clamp(INDEX_MIN, INDEX_MAX)
}

/// The fixed-point coefficient a quantiser index stands for (what the decoder rebuilds).
pub(crate) fn dequantise(position: usize, index: i32) -> i32 {
    match position {
        0 => companded((index - INDEX_MIN) as i64) as i32,
        1 => -(companded((index - INDEX_MIN) as i64) as i32),
        _ => index * (1 << 14) + (1 << 13),
    }
}

/// Reflection coefficients and the prediction error after each order, from the autocorrelation (Levinson-Durbin).
pub(crate) struct Analysis {
    /// `reflection[m]` is the coefficient that takes order `m` to `m + 1`.
    pub reflection: Vec<f64>,
    /// `error[m]` is the residual energy at order `m`.
    pub error: Vec<f64>,
}

pub(crate) fn analyse(samples: &[i32], max_order: usize) -> Analysis {
    let n = samples.len();
    let windowed: Vec<f64> = samples
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let x = (i as f64 + 0.5) / n as f64;
            s as f64 * (0.5 - 0.5 * (2.0 * std::f64::consts::PI * x).cos())
        })
        .collect();
    let order = max_order.min(n.saturating_sub(1));
    let mut r = vec![0.0f64; order + 1];
    for (lag, slot) in r.iter_mut().enumerate() {
        *slot = windowed[lag..].iter().zip(&windowed).map(|(a, b)| a * b).sum();
    }
    r[0] = r[0] * (1.0 + 1e-9) + 1e-9;

    let mut taps = vec![0.0f64; order + 1];
    let mut error = vec![r[0]];
    let mut reflection = Vec::with_capacity(order);
    for m in 0..order {
        let mut acc = r[m + 1];
        for j in 1..=m {
            acc += taps[j] * r[m + 1 - j];
        }
        let k = (-acc / error[m]).clamp(-0.9999, 0.9999);
        let before = taps.clone();
        for j in 1..=m {
            taps[j] = before[j] + k * before[m + 1 - j];
        }
        taps[m + 1] = k;
        reflection.push(k);
        error.push(error[m] * (1.0 - k * k));
    }
    Analysis { reflection, error }
}

/// The direct taps of the filter from fixed-point reflection coefficients (the decoder's integer recursion).
pub(crate) fn taps_from(par: &[i32]) -> Vec<i32> {
    let mut cof = vec![0i32; par.len()];
    let half = |a: i32, b: i32| ((a as i64 * b as i64 + (1 << 19)) >> 20) as i32;
    for k in 0..par.len() {
        let (mut i, mut j) = (0isize, k as isize - 1);
        while i < j {
            let low = half(par[k], cof[j as usize]);
            cof[j as usize] = cof[j as usize].wrapping_add(half(par[k], cof[i as usize]));
            cof[i as usize] = cof[i as usize].wrapping_add(low);
            i += 1;
            j -= 1;
        }
        if i == j {
            cof[i as usize] = cof[i as usize].wrapping_add(half(par[k], cof[j as usize]));
        }
        cof[k] = par[k];
    }
    cof
}

/// Residual of `signal[start..]`, predicted from the `taps.len()` samples before each one.
pub(crate) fn residual(signal: &[i32], start: usize, taps: &[i32]) -> Vec<i32> {
    (start..signal.len())
        .map(|n| {
            let mut y = 1i64 << 19;
            for (j, &t) in taps.iter().enumerate() {
                y += t as i64 * signal[n - 1 - j] as i64;
            }
            signal[n].wrapping_add((y >> 20) as i32)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn companded_scale_matches_the_format() {
        assert_eq!(companded(0), -1_048_544);
        assert_eq!(companded(127), 1_032_224);
        assert_eq!(companded_index(0.0), 90 - 64); // the entry closest to zero
        assert_eq!(dequantise(1, companded_index(0.5)).signum(), -1);
    }

    #[test]
    fn uniform_steps_rebuild_at_the_middle() {
        assert_eq!(dequantise(5, uniform_index(0.0)), 1 << 13);
        assert_eq!(uniform_index(5.0), INDEX_MAX);
        assert_eq!(uniform_index(-5.0), INDEX_MIN);
    }

    #[test]
    fn a_tone_is_predicted_well() {
        let tone: Vec<i32> = (0..2048).map(|n| ((n as f64 * 0.05).sin() * 10_000.0) as i32).collect();
        let a = analyse(&tone, 8);
        assert!(a.error[8] < a.error[0] / 1000.0);
    }
}
