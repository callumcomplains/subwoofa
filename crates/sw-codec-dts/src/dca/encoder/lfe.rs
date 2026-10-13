//! The low-frequency effects channel: decimating it to the 128-sample-per-frame rate and quantizing it.

use crate::dca::encoder::Encoder;
use crate::dca::tables::{LFE_FIR_64, SCALE_7};

/// Same calibration for the LFE channel (the decoder's LFE path has its own scaling).
pub(crate) const LFE_GAIN: f64 = 1.1541;

/// Samples the LFE path is delayed to line up with the subband path.
pub(crate) const LFE_ALIGN: usize = 32;

/// 512-tap decimation filter (unit DC gain) for the LFE channel. The decoder's interpolation filter already
/// rolls off (-2.5 dB at 80 Hz, -3.9 dB at 100 Hz), so the encoder side is designed as its inverse up to
/// 110 Hz (boost capped at 6 dB), tapering to zero by 260 Hz: the LFE comes back flat instead of doubly
/// attenuated.
pub(crate) fn lfe_decimator() -> Vec<f64> {
    use std::f64::consts::PI;
    let n = 512usize;
    let decoder = |f: f64| -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, h) in LFE_FIR_64.iter().enumerate() {
            let a = -2.0 * PI * f / 48_000.0 * i as f64;
            re += h * a.cos();
            im += h * a.sin();
        }
        (re * re + im * im).sqrt() / 64.0
    };
    let dc = decoder(0.0);
    let want = |f: f64| -> f64 {
        if f <= 110.0 {
            (dc / decoder(f)).min(2.0)
        } else if f < 260.0 {
            let t = (f - 110.0) / 150.0;
            (dc / decoder(110.0)).min(2.0) * 0.5 * (1.0 + (PI * t).cos())
        } else {
            0.0
        }
    };
    // sample the wanted magnitude, linear phase with the filter centre at (n - 1) / 2
    let centre = (n as f64 - 1.0) / 2.0;
    let grid = 4096usize;
    let mut h = vec![0.0f64; n];
    for (i, v) in h.iter_mut().enumerate() {
        let t = i as f64 - centre;
        let mut acc = want(0.0);
        for k in 1..grid / 2 {
            let f = 48_000.0 * k as f64 / grid as f64;
            if f > 300.0 {
                break;
            }
            acc += 2.0 * want(f) * (2.0 * PI * k as f64 / grid as f64 * t).cos();
        }
        let w = 0.5 * (1.0 + (2.0 * PI * (i as f64 + 0.5 - n as f64 / 2.0) / n as f64).cos());
        *v = acc / grid as f64 * w;
    }
    let sum: f64 = h.iter().sum();
    h.iter().map(|v| v / sum).collect()
}

impl Encoder {
    /// Decimated, quantizable LFE samples for this frame: (samples, scale index).
    pub(crate) fn lfe_samples(&mut self, pcm: &[f64]) -> (Vec<i32>, u8) {
        let mut input = std::mem::take(&mut self.lfe_pre);
        input.extend_from_slice(pcm);
        self.lfe_pre = input.split_off(pcm.len());
        let pcm = &input[..];
        let n_out = pcm.len() / self.lfe_decim;
        let mut vals = Vec::with_capacity(n_out);
        for d in 0..n_out {
            self.lfe_hist.drain(..self.lfe_decim);
            self.lfe_hist
                .extend_from_slice(&pcm[d * self.lfe_decim..(d + 1) * self.lfe_decim]);
            let y: f64 = self.lfe_hist.iter().zip(self.lfe_fir.iter()).map(|(x, h)| x * h).sum();
            vals.push(y * LFE_GAIN);
        }
        let peak = vals.iter().fold(0f64, |m, v| m.max(v.abs()));
        let mut idx = 0usize;
        while idx < 124 && SCALE_7[idx] as f64 * 0.035 * 127.0 < peak {
            idx += 1;
        }
        let step = SCALE_7[idx] as f64 * 0.035;
        let q = vals
            .iter()
            .map(|v| ((v / step).round() as i32).clamp(-128, 127))
            .collect();
        (q, idx as u8)
    }
}
