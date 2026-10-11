//! The signal processing of PCM to DSD: sample rate conversion up to the DSD rate, and the sigma-delta modulator that
//! turns the oversampled signal into one bit per sample.

/// Zeroth-order modified Bessel function, for the Kaiser window.
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..80 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < 1e-16 * sum {
            break;
        }
    }
    sum
}

/// Low-pass filter for a polyphase resampler, designed at the intermediate (zero-stuffed) rate.
///
/// `pass_edge` and `stop_edge` are in Hz, `attenuation` in dB. Returns the taps (an odd count, symmetric, scaled by
/// `up` so the passband gain is one) and how many taps belong to each phase.
fn design_filter(intermediate_rate: f64, up: usize, pass_edge: f64, stop_edge: f64, attenuation: f64) -> (Vec<f64>, usize) {
    let transition = 2.0 * std::f64::consts::PI * (stop_edge - pass_edge) / intermediate_rate;
    let beta = if attenuation > 50.0 { 0.1102 * (attenuation - 8.7) } else { 0.5842 * (attenuation - 21.0).powf(0.4) + 0.07886 * (attenuation - 21.0) };
    let length = ((attenuation - 8.0) / (2.285 * transition)).ceil() as usize;
    // whole phases, and an odd total so the filter has a centre tap
    let per_phase = length.div_ceil(up).max(2);
    let taps_count = per_phase * up + 1;
    let centre = (taps_count - 1) as f64 / 2.0;
    let cutoff = (pass_edge + stop_edge) / 2.0 / intermediate_rate; // cycles per sample at the intermediate rate
    let norm = bessel_i0(beta);
    let taps = (0..taps_count)
        .map(|i| {
            let m = i as f64 - centre;
            let sinc = if m == 0.0 { 2.0 * cutoff } else { (2.0 * std::f64::consts::PI * cutoff * m).sin() / (std::f64::consts::PI * m) };
            let r = m / centre;
            let window = bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / norm;
            sinc * window * up as f64
        })
        .collect();
    (taps, per_phase)
}

/// Streaming polyphase resampler by the factor `up / down`.
pub struct Resampler {
    up: usize,
    down: usize,
    taps: Vec<f64>,
    per_phase: usize,
    /// Input samples still needed (and some older ones), `buffer[0]` being input number `buffer_start`.
    buffer: Vec<f64>,
    buffer_start: usize,
    /// Outputs produced so far.
    produced: usize,
    delay_seconds: f64,
}

impl Resampler {
    /// `input_rate` in Hz; the band up to `pass_edge` is kept, everything from `stop_edge` on is removed.
    pub fn new(input_rate: f64, up: usize, down: usize, pass_edge: f64, stop_edge: f64) -> Resampler {
        let intermediate = input_rate * up as f64;
        let (taps, per_phase) = design_filter(intermediate, up, pass_edge, stop_edge, 100.0);
        let delay_seconds = (taps.len() - 1) as f64 / 2.0 / intermediate;
        Resampler { up, down, taps, per_phase, buffer: vec![0.0; per_phase], buffer_start: 0, produced: 0, delay_seconds }
    }

    /// Time the filter delays the signal by.
    pub fn delay_seconds(&self) -> f64 {
        self.delay_seconds
    }

    /// Feeds input samples and returns every output sample that can now be computed.
    pub fn process(&mut self, input: &[f64]) -> Vec<f64> {
        // the buffer starts `per_phase` zeros before input 0 (the signal is silent before the start)
        self.buffer.extend_from_slice(input);
        let available = self.buffer_start + self.buffer.len() - self.per_phase; // inputs received so far
        let mut out = Vec::with_capacity(input.len() * self.up / self.down + 2);
        loop {
            let position = self.produced * self.down; // in units of 1/up input samples
            let newest = position / self.up; // newest input sample the output depends on
            if newest >= available {
                break;
            }
            let phase = position % self.up;
            let mut sum = 0.0;
            for k in 0..self.per_phase {
                let tap = self.taps[phase + k * self.up];
                // input number `newest - k`, at buffer index (that + per_phase - buffer_start)
                let index = (newest + self.per_phase) as isize - k as isize - self.buffer_start as isize;
                if index >= 0 {
                    sum += tap * self.buffer[index as usize];
                }
            }
            out.push(sum);
            self.produced += 1;
        }
        // forget what no later output needs: the oldest input still reached by the next output, in buffer coordinates
        let next_newest = (self.produced * self.down) / self.up;
        let keep_from = next_newest + 1;
        if keep_from > self.buffer_start + (1 << 16) {
            let cut = (keep_from - self.buffer_start).min(self.buffer.len());
            self.buffer.drain(..cut);
            self.buffer_start += cut;
        }
        out
    }
}

/// Sigma-delta modulator: shapes the quantisation noise of a one-bit quantiser away from the audio band.
///
/// The noise transfer function has `ORDER` zeros at zero frequency and Butterworth poles, with the cutoff set so the
/// out-of-band noise gain is `NOISE_GAIN` (about 1.5, the usual limit for a stable one-bit loop).
pub struct Modulator {
    numerator: [f64; ORDER],   // b_1 .. b_ORDER of NTF = (1 + sum b z^-k) / (1 + sum a z^-k)
    denominator: [f64; ORDER], // a_1 .. a_ORDER
    quantiser_error: [f64; ORDER], // e[n-1] ..: output minus quantiser input
    shaped_error: [f64; ORDER],    // es[n-1] ..: output minus input
    overloads: u64,
}

const ORDER: usize = 5;
/// Largest out-of-band gain of the noise transfer function.
const NOISE_GAIN: f64 = 1.5;

impl Modulator {
    pub fn new() -> Modulator {
        let (numerator, denominator) = noise_transfer_function();
        Modulator { numerator, denominator, quantiser_error: [0.0; ORDER], shaped_error: [0.0; ORDER], overloads: 0 }
    }

    /// Times the loop had to be reset because the input overloaded it.
    #[cfg(test)]
    pub fn overloads(&self) -> u64 {
        self.overloads
    }

    /// One input sample (-1.0 to 1.0, best kept below about 0.7) to one output bit.
    #[inline]
    pub fn step(&mut self, x: f64) -> bool {
        let mut v = x;
        for k in 0..ORDER {
            v += self.numerator[k] * self.quantiser_error[k] - self.denominator[k] * self.shaped_error[k];
        }
        let y = if v >= 0.0 { 1.0 } else { -1.0 };
        let e = y - v;
        let es = y - x;
        if !v.is_finite() || v.abs() > 50.0 {
            // the loop has run away: start again from rest rather than output noise for ever
            self.quantiser_error = [0.0; ORDER];
            self.shaped_error = [0.0; ORDER];
            self.overloads += 1;
            return y > 0.0;
        }
        for k in (1..ORDER).rev() {
            self.quantiser_error[k] = self.quantiser_error[k - 1];
            self.shaped_error[k] = self.shaped_error[k - 1];
        }
        self.quantiser_error[0] = e;
        self.shaped_error[0] = es;
        y > 0.0
    }
}

/// `(1 - z^-1)^ORDER` over the denominator of a digital Butterworth low-pass whose cutoff gives `NOISE_GAIN`.
fn noise_transfer_function() -> ([f64; ORDER], [f64; ORDER]) {
    // zeros: the coefficients of (1 - z^-1)^ORDER
    let mut zeros = vec![1.0f64];
    for _ in 0..ORDER {
        let mut next = vec![0.0; zeros.len() + 1];
        for (i, &c) in zeros.iter().enumerate() {
            next[i] += c;
            next[i + 1] -= c;
        }
        zeros = next;
    }
    // find the cutoff (fraction of the sample rate) whose peak noise gain is NOISE_GAIN
    let (mut low, mut high) = (1e-4f64, 0.45f64);
    for _ in 0..60 {
        let mid = 0.5 * (low + high);
        let poles = butterworth_denominator(mid);
        if peak_gain(&zeros, &poles) > NOISE_GAIN {
            high = mid;
        } else {
            low = mid;
        }
    }
    let poles = butterworth_denominator(low);
    let mut b = [0.0; ORDER];
    let mut a = [0.0; ORDER];
    for k in 0..ORDER {
        b[k] = zeros[k + 1];
        a[k] = poles[k + 1];
    }
    (b, a)
}

/// Denominator `1 + a1 z^-1 + ...` of the digital Butterworth low-pass of order `ORDER` with cutoff `fc` (cycles per
/// sample), by the bilinear transform of the analogue prototype.
fn butterworth_denominator(fc: f64) -> Vec<f64> {
    let warped = (std::f64::consts::PI * fc).tan();
    // poles of the analogue prototype on the unit circle, scaled to the warped cutoff, then mapped with z = (1+s)/(1-s)
    let mut poly: Vec<(f64, f64)> = vec![(1.0, 0.0)]; // coefficients of the polynomial in z^-1, complex
    for i in 0..ORDER {
        let angle = std::f64::consts::PI * (2 * i + 1 + ORDER) as f64 / (2 * ORDER) as f64;
        let s = (warped * angle.cos(), warped * angle.sin());
        // z-plane pole p = (1 + s) / (1 - s)
        let denom = (1.0 - s.0).powi(2) + s.1 * s.1;
        let p = (((1.0 + s.0) * (1.0 - s.0) - s.1 * s.1) / denom, (s.1 * (1.0 - s.0) + s.1 * (1.0 + s.0)) / denom);
        // multiply poly by (1 - p z^-1)
        let mut next = vec![(0.0, 0.0); poly.len() + 1];
        for (k, &c) in poly.iter().enumerate() {
            next[k].0 += c.0;
            next[k].1 += c.1;
            next[k + 1].0 -= c.0 * p.0 - c.1 * p.1;
            next[k + 1].1 -= c.0 * p.1 + c.1 * p.0;
        }
        poly = next;
    }
    poly.iter().map(|c| c.0).collect()
}

/// Largest magnitude of `numerator(z) / denominator(z)` on the unit circle.
fn peak_gain(numerator: &[f64], denominator: &[f64]) -> f64 {
    let eval = |poly: &[f64], w: f64| -> (f64, f64) {
        let (mut re, mut im) = (0.0, 0.0);
        for (k, c) in poly.iter().enumerate() {
            re += c * (w * k as f64).cos();
            im -= c * (w * k as f64).sin();
        }
        (re, im)
    };
    let mut peak = 0f64;
    for i in 1..=2000 {
        let w = std::f64::consts::PI * i as f64 / 2000.0;
        let n = eval(numerator, w);
        let d = eval(denominator, w);
        let g = (n.0 * n.0 + n.1 * n.1).sqrt() / (d.0 * d.0 + d.1 * d.1).sqrt().max(1e-300);
        peak = peak.max(g);
    }
    peak
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_transfer_function_is_monic_and_flat_far_from_the_audio_band() {
        let (b, a) = noise_transfer_function();
        let mut num = vec![1.0];
        num.extend_from_slice(&b);
        let mut den = vec![1.0];
        den.extend_from_slice(&a);
        let peak = peak_gain(&num, &den);
        assert!((peak - NOISE_GAIN).abs() < 0.02, "peak noise gain {peak}");
        // a hundredth of the sample rate (28 kHz at DSD64): the noise is already well under the quantiser level
        let w = 2.0 * std::f64::consts::PI * 0.01;
        let g = {
            let eval = |p: &[f64]| -> f64 {
                let (re, im) = p.iter().enumerate().fold((0.0, 0.0), |(r, i), (k, c)| (r + c * (w * k as f64).cos(), i - c * (w * k as f64).sin()));
                (re * re + im * im).sqrt()
            };
            eval(&num) / eval(&den)
        };
        assert!(g < 5e-3, "noise gain at fs/100 is {g}");
    }

    #[test]
    fn a_resampler_keeps_a_tone_and_scales_the_rate() {
        // 1 kHz tone at 48 kHz, up by 147/160 to 44.1 kHz
        let mut r = Resampler::new(48_000.0, 147, 160, 20_000.0, 22_050.0);
        let input: Vec<f64> = (0..48_000).map(|i| (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 48_000.0).sin()).collect();
        let mut out = Vec::new();
        for chunk in input.chunks(5000) {
            out.extend(r.process(chunk));
        }
        assert!((out.len() as i64 - 44_100).abs() <= 2, "{} samples out", out.len());
        // amplitude of the steady part
        let peak = out[10_000..40_000].iter().fold(0f64, |m, v| m.max(v.abs()));
        assert!((peak - 1.0).abs() < 0.01, "peak {peak}");
    }

    #[test]
    fn the_modulator_codes_a_tone_with_its_average_density() {
        let mut m = Modulator::new();
        let n = 64 * 20_000;
        let mut acc = 0.0;
        let mut ones = 0usize;
        let mut reference = 0.0;
        // a slow ramp: the average of the +-1 output over a window follows the input
        for i in 0..n {
            let x = 0.5 * (2.0 * std::f64::consts::PI * 700.0 * i as f64 / 2_822_400.0).sin();
            let bit = m.step(x);
            acc += if bit { 1.0 } else { -1.0 };
            reference += x;
            ones += usize::from(bit);
        }
        assert_eq!(m.overloads(), 0);
        let _ = ones;
        assert!((acc - reference).abs() / (n as f64) < 0.01, "mean {} against {}", acc / n as f64, reference / n as f64);
    }
}
