//! DTS-ES matrix helpers: mixing the centre surround into the surround pair, with the optional
//! +/-45 degree phase shift of the surrounds and the 3 dB rear attenuation of the legacy encoder.

use std::f64::consts::PI;

/// Group delay of the Hilbert transformer, in samples. Every channel is delayed by this much while the
/// phase shift is on, so the channels stay aligned.
pub const SHIFT_DELAY: usize = 127;

#[derive(Debug, Clone, Copy, Default)]
pub struct EsOptions {
    /// Mark the stream as DTS-ES (extended surround flag in the header).
    pub flag: bool,
    /// Add the centre surround to the surround pair (layouts that carry a separate Cs).
    pub mix_cs: bool,
    /// Also send the centre surround as its own channel in the XCH extension (ES discrete).
    pub xch: bool,
    /// Rotate the left/right surround by +/-45 degrees before the Cs is added.
    pub phase_shift: bool,
    /// Attenuate Ls, Rs and Cs by 3 dB first.
    pub rear_minus_3db: bool,
}

impl EsOptions {
    /// Samples of delay the mixer adds to the programme.
    pub fn delay(&self) -> usize {
        if self.phase_shift {
            SHIFT_DELAY
        } else {
            0
        }
    }
}

fn hilbert_taps() -> Vec<f64> {
    let n = 2 * SHIFT_DELAY + 1;
    (0..n)
        .map(|i| {
            let k = i as f64 - SHIFT_DELAY as f64;
            if (k as i64) % 2 == 0 {
                return 0.0;
            }
            let w = 0.42 - 0.5 * (2.0 * PI * i as f64 / (n - 1) as f64).cos()
                + 0.08 * (4.0 * PI * i as f64 / (n - 1) as f64).cos();
            2.0 / (PI * k) * w
        })
        .collect()
}

/// Streaming mixer over planar blocks of the layout's channels.
pub struct EsMixer {
    opts: EsOptions,
    ls: usize,
    rs: usize,
    cs: Option<usize>,
    taps: Vec<f64>,
    /// Per-channel input history (the last `2 * SHIFT_DELAY` samples).
    hist: Vec<Vec<f64>>,
}

impl EsMixer {
    /// `channels` is the layout's channel list (must contain Ls and Rs; Cs when `mix_cs`).
    pub fn new(opts: EsOptions, channels: &[sw_core::Channel]) -> Option<EsMixer> {
        use sw_core::Channel::*;
        let pos = |c| channels.iter().position(|x| *x == c);
        let ls = pos(Ls)?;
        let rs = pos(Rs)?;
        let cs = pos(Cs);
        if opts.mix_cs && cs.is_none() {
            return None;
        }
        let keep = if opts.phase_shift { 2 * SHIFT_DELAY } else { 0 };
        Some(EsMixer {
            opts,
            ls,
            rs,
            cs,
            taps: hilbert_taps(),
            hist: vec![vec![0.0; keep]; channels.len()],
        })
    }

    /// Mix one block (all channels the same length). Output has the same shape; the Cs channel is left in
    /// place (the caller leaves it out of the core).
    pub fn process(&mut self, block: &[Vec<f32>]) -> Vec<Vec<f32>> {
        let n = block[0].len();
        let att = if self.opts.rear_minus_3db {
            std::f64::consts::FRAC_1_SQRT_2
        } else {
            1.0
        };
        let d = self.opts.delay();
        let cs_gain = std::f64::consts::FRAC_1_SQRT_2 * att;
        let mut out: Vec<Vec<f32>> = Vec::with_capacity(block.len());
        for (c, chan) in block.iter().enumerate() {
            // extended signal: history followed by the new block
            let mut ext: Vec<f64> = Vec::with_capacity(self.hist[c].len() + n);
            ext.extend_from_slice(&self.hist[c]);
            ext.extend(chan.iter().map(|v| *v as f64));
            let h = self.hist[c].len();
            let sample = |i: usize| ext[h + i - d];
            let mut y: Vec<f64> = (0..n)
                .map(|i| if d == 0 { chan[i] as f64 } else { sample(i) })
                .collect();
            if c == self.ls || c == self.rs {
                let theta = if c == self.ls { PI / 4.0 } else { -PI / 4.0 };
                if self.opts.phase_shift {
                    let (ct, st) = (theta.cos(), theta.sin());
                    for (i, v) in y.iter_mut().enumerate() {
                        let mut acc = 0.0;
                        // output i has the centre tap at input i (delayed by D overall)
                        let base = h + i - 2 * SHIFT_DELAY;
                        for (k, t) in self.taps.iter().enumerate() {
                            if *t != 0.0 {
                                acc += t * ext[base + k];
                            }
                        }
                        *v = ct * *v + st * acc;
                    }
                }
                for v in y.iter_mut() {
                    *v *= att;
                }
            }
            if h > 0 {
                let keep = h;
                self.hist[c] = ext[ext.len() - keep..].to_vec();
            }
            out.push(y.into_iter().map(|v| v as f32).collect());
        }
        if self.opts.mix_cs {
            if let Some(cs) = self.cs {
                let cs_sig: Vec<f64> = (0..n)
                    .map(|i| {
                        // the Cs follows the same delay as the other channels
                        out[cs][i] as f64
                    })
                    .collect();
                for i in 0..n {
                    out[self.ls][i] += (cs_gain * cs_sig[i]) as f32;
                    out[self.rs][i] += (cs_gain * cs_sig[i]) as f32;
                }
                if self.opts.rear_minus_3db {
                    for v in out[cs].iter_mut() {
                        *v *= att as f32;
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sw_core::Channel::*;

    #[test]
    fn matrix_mix_adds_cs_at_minus_3_db() {
        let chans = [L, R, C, Lfe, Ls, Rs, Cs];
        let mut m = EsMixer::new(
            EsOptions {
                flag: true,
                mix_cs: true,
                xch: false,
                phase_shift: false,
                rear_minus_3db: false,
            },
            &chans,
        )
        .unwrap();
        let block: Vec<Vec<f32>> = (0..7).map(|c| vec![if c == 6 { 0.5 } else { 0.0 }; 8]).collect();
        let out = m.process(&block);
        assert!((out[4][0] - 0.5 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert!((out[5][3] - 0.5 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert_eq!(out[0][0], 0.0);
    }

    #[test]
    fn phase_shift_rotates_surrounds_in_quadrature() {
        let chans = [L, R, C, Lfe, Ls, Rs];
        let mut m = EsMixer::new(
            EsOptions {
                flag: true,
                mix_cs: false,
                xch: false,
                phase_shift: true,
                rear_minus_3db: false,
            },
            &chans,
        )
        .unwrap();
        let n = 4096;
        let f = 0.01f64; // cycles per sample
        let sig: Vec<f32> = (0..n).map(|i| (2.0 * PI * f * i as f64).sin() as f32).collect();
        let block: Vec<Vec<f32>> = (0..6).map(|_| sig.clone()).collect();
        let out = m.process(&block);
        // Ls and Rs lead / lag the delayed input by 45 degrees: their sum is sqrt(2) times the delayed input
        for i in 1000..3000 {
            let delayed = sig[i - SHIFT_DELAY] as f64;
            let sum = (out[4][i] + out[5][i]) as f64;
            assert!(
                (sum - 2.0 * std::f64::consts::FRAC_1_SQRT_2 * delayed).abs() < 0.01,
                "sample {i}: {sum} vs {}",
                2.0 * delayed * std::f64::consts::FRAC_1_SQRT_2
            );
        }
        // and each differs from the delayed input in amplitude only by phase (unit gain)
        let e: f64 = (1000..3000).map(|i| (out[4][i] as f64).powi(2)).sum::<f64>() / 2000.0;
        assert!((e - 0.5).abs() < 0.02);
    }
}
