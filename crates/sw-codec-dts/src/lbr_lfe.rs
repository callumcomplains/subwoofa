//! LFE channel of DTS Express: the signal is low-passed and decimated to 750 Hz (64 samples per frame) and
//! coded with a 4-bit ADPCM; the decoder interpolates with a fixed IIR filter.

use crate::lbr::BitsLe;
use crate::lbr_tables::{LFE_DELTA_INDEX_16, LFE_IIR, LFE_STEP_SIZE_16};

pub const DECIM: usize = 64;
const HALF: usize = 640; // FIR half length in input samples
const CUTOFF_HZ: f64 = 110.0;
const DECODER_SCALE: f64 = 64.0 * 0.0000078265894;

/// DC gain of the decoder's interpolation filter for a unit pulse train (one pulse every `DECIM` samples).
fn pulse_gain() -> f64 {
    let mut hist = [[0.0f64; 2]; 5];
    let mut last = 0.0;
    for _ in 0..400 {
        let mut res = 1.0;
        let mut acc = 0.0;
        for _ in 0..DECIM {
            for k in 0..5 {
                let tmp = hist[k][0] * LFE_IIR[k][0] as f64 + hist[k][1] * LFE_IIR[k][1] as f64 + res;
                res = hist[k][0] * LFE_IIR[k][2] as f64 + hist[k][1] * LFE_IIR[k][3] as f64 + tmp;
                hist[k][0] = hist[k][1];
                hist[k][1] = tmp;
            }
            acc += res;
            res = 0.0;
        }
        last = acc / DECIM as f64;
    }
    last
}

pub struct LfeEncoder {
    taps: Vec<f64>,
    /// Output value per coded unit at DC, so that a constant input `x` decodes to `x`.
    unit: f64,
    value: f64,
    step_i: usize,
}

impl LfeEncoder {
    pub fn new(sample_rate: u32) -> LfeEncoder {
        let fc = CUTOFF_HZ / sample_rate as f64;
        let n = 2 * HALF + 1;
        let mut taps: Vec<f64> = (0..n)
            .map(|i| {
                let x = i as f64 - HALF as f64;
                let sinc = if x == 0.0 { 2.0 * fc } else { (2.0 * std::f64::consts::PI * fc * x).sin() / (std::f64::consts::PI * x) };
                let w = 0.5 + 0.5 * (std::f64::consts::PI * x / (HALF as f64 + 1.0)).cos();
                sinc * w
            })
            .collect();
        let sum: f64 = taps.iter().sum();
        for t in taps.iter_mut() {
            *t /= sum;
        }
        // a constant input of 1.0 must come out as 1.0: the pulse train carries value*DECIM per pulse
        let unit = DECODER_SCALE * pulse_gain();
        LfeEncoder { taps, unit, value: 0.0, step_i: 0 }
    }

    /// Samples of history the filter needs on each side of a coded sample.
    pub fn reach(&self) -> usize {
        HALF
    }

    /// Code one frame: `x(i)` gives the input sample at position `i` relative to the start of the frame
    /// (it may be asked for positions outside 0..4096; return 0 beyond the ends). Returns the chunk body.
    pub fn code_frame(&mut self, x: &dyn Fn(isize) -> f64) -> BitsLe {
        let mut w = BitsLe::new();
        let start = self.value.clamp(-1.0, 1.0);
        let mag = (start.abs() * 0x7fff as f64).round() as u32;
        w.put(mag | if start < 0.0 { 0x8000 } else { 0 }, 16);
        self.value = if start < 0.0 { -(mag as f64) / 0x7fff as f64 } else { mag as f64 / 0x7fff as f64 };
        w.put(self.step_i as u32, 8);
        for j in 0..64usize {
            let centre = (j * DECIM) as isize;
            let mut acc = 0.0;
            for (i, t) in self.taps.iter().enumerate() {
                acc += t * x(centre + i as isize - HALF as isize);
            }
            let target = (acc / self.unit).clamp(-2.9, 2.9);
            let step = LFE_STEP_SIZE_16[self.step_i] as f64;
            let mut best = (f64::MAX, 0u32, self.value);
            for code in 0..16u32 {
                let mut delta = step * 0.125;
                if code & 4 != 0 {
                    delta += step;
                }
                if code & 2 != 0 {
                    delta += step * 0.5;
                }
                if code & 1 != 0 {
                    delta += step * 0.25;
                }
                let v = if code & 8 != 0 { (self.value - delta).max(-3.0) } else { (self.value + delta).min(3.0) };
                let e = (v - target).abs();
                if e < best.0 {
                    best = (e, code, v);
                }
            }
            w.put(best.1, 4);
            self.value = best.2;
            let ni = self.step_i as i32 + LFE_DELTA_INDEX_16[(best.1 & 7) as usize] as i32;
            self.step_i = ni.clamp(0, LFE_STEP_SIZE_16.len() as i32 - 1) as usize;
        }
        w
    }
}
