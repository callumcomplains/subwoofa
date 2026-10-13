//! True peak (ITU-R BS.1770-4 Annex 2): the highest level the reconstructed analogue signal reaches, found by
//! oversampling 4x at rates below 96 kHz, 2x up to 192 kHz and not at all above, with a 12-tap interpolator per phase.

use std::f64::consts::PI;

/// Taps of the interpolating filter for each output phase.
const TAPS: usize = 12;

/// Zeroth-order modified Bessel function, for the Kaiser window.
fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term) = (1.0, 1.0);
    for k in 1..40 {
        term *= (x / (2.0 * k as f64)).powi(2);
        sum += term;
    }
    sum
}

/// How much to oversample at a sample rate.
fn factor_for(rate: u32) -> usize {
    match rate {
        0..=95_999 => 4,
        96_000..=191_999 => 2,
        _ => 1,
    }
}

/// Peak finder for one channel.
#[derive(Clone)]
pub(crate) struct TruePeak {
    factor: usize,
    /// `phases[p][k]`: weight of the sample `k` positions before the interpolation point, for phase `p`.
    phases: Vec<[f64; TAPS]>,
    history: [f64; TAPS],
    pub(crate) peak: f64,
}

impl TruePeak {
    pub(crate) fn new(rate: u32) -> TruePeak {
        let factor = factor_for(rate);
        let beta = 7.0;
        let norm = bessel_i0(beta);
        let phases = (0..factor)
            .map(|p| {
                let frac = p as f64 / factor as f64;
                let mut taps = [0.0; TAPS];
                for (k, tap) in taps.iter_mut().enumerate() {
                    // distance in samples from the interpolation point to the sample `k` places before the newest one;
                    // the point sits `frac` after the sample TAPS / 2 - 1 places before the newest
                    let d = (TAPS as f64 / 2.0 - 1.0 - k as f64) - frac;
                    let sinc = if d == 0.0 { 1.0 } else { (PI * d).sin() / (PI * d) };
                    let r = d / (TAPS as f64 / 2.0);
                    let window = if r.abs() < 1.0 {
                        bessel_i0(beta * (1.0 - r * r).sqrt()) / norm
                    } else {
                        0.0
                    };
                    *tap = sinc * window;
                }
                taps
            })
            .collect();
        TruePeak {
            factor,
            phases,
            history: [0.0; TAPS],
            peak: 0.0,
        }
    }

    pub(crate) fn push(&mut self, x: f64) {
        self.history.rotate_right(1);
        self.history[0] = x;
        if self.factor == 1 {
            self.peak = self.peak.max(x.abs());
            return;
        }
        // the history holds the newest sample first; the interpolation points lie between the sample TAPS / 2 - 1
        // back and the one after it
        for taps in &self.phases {
            let y: f64 = taps.iter().zip(&self.history).map(|(t, h)| t * h).sum();
            self.peak = self.peak.max(y.abs());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sine_between_samples_is_found_above_its_sample_peak() {
        // a quarter-rate sine at 45 degrees: every sample is 0.7071 of the peak, the waveform reaches 1.0
        let mut t = TruePeak::new(48_000);
        let mut sample_peak: f64 = 0.0;
        for i in 0..4800 {
            let x = (2.0 * PI * 0.25 * i as f64 + PI / 4.0).sin();
            sample_peak = sample_peak.max(x.abs());
            t.push(x);
        }
        assert!((sample_peak - 0.7071).abs() < 0.001);
        assert!(t.peak > 0.98 && t.peak < 1.03, "true peak {}", t.peak);
    }

    #[test]
    fn no_oversampling_at_192_khz() {
        let mut t = TruePeak::new(192_000);
        t.push(0.5);
        t.push(-0.9);
        assert_eq!(t.peak, 0.9);
    }
}
