//! The 32-band cosine-modulated filter bank of the DTS core.
//!
//! `Synthesis` is the decoder's reconstruction bank written straight from the pseudocode of
//! ETSI TS 102 114 clause C.3.6 (used by the tests and to measure quantization noise); `Analysis` is
//! the matching encoder-side bank built on the same prototype filter.

use crate::tables::{QMF_NPR, QMF_PR};
use std::f64::consts::PI;

pub const BANDS: usize = 32;
const TAPS: usize = 512;

fn prototype(perfect: bool) -> &'static [f64; 512] {
    if perfect {
        &QMF_PR
    } else {
        &QMF_NPR
    }
}

/// Reconstruction filter bank: 32 subband samples in, 32 PCM samples out.
pub struct Synthesis {
    coeff: &'static [f64; 512],
    cos_mod: Vec<f64>,
    x: [f64; TAPS],
    z: [f64; 64],
}

impl Synthesis {
    pub fn new(perfect: bool) -> Synthesis {
        let mut cos_mod = Vec::with_capacity(544);
        for k in 0..16 {
            for i in 0..16 {
                cos_mod.push(((2 * i + 1) as f64 * (2 * k + 1) as f64 * PI / 64.0).cos());
            }
        }
        for k in 0..16 {
            for i in 0..16 {
                cos_mod.push((i as f64 * (2 * k + 1) as f64 * PI / 32.0).cos());
            }
        }
        for k in 0..16 {
            cos_mod.push(0.25 / (2.0 * ((2 * k + 1) as f64 * PI / 128.0).cos()));
        }
        for k in 0..16 {
            cos_mod.push(-0.25 / (2.0 * ((2 * k + 1) as f64 * PI / 128.0).sin()));
        }
        Synthesis { coeff: prototype(perfect), cos_mod, x: [0.0; TAPS], z: [0.0; 64] }
    }

    /// One time step: `sub` holds one sample of every subband (bands >= `active` count as zero).
    pub fn step(&mut self, sub: &[f64; BANDS], out: &mut [f64; 32]) {
        let cm = &self.cos_mod;
        let mut a = [0f64; 16];
        let mut b = [0f64; 16];
        for k in 0..16 {
            let mut acc = 0.0;
            for i in 0..16 {
                acc += (sub[2 * i] + sub[2 * i + 1]) * cm[k * 16 + i];
            }
            a[k] = acc;
            let mut acc = 0.0;
            for i in 0..16 {
                let v = if i > 0 { sub[2 * i] + sub[2 * i - 1] } else { sub[0] };
                acc += v * cm[256 + k * 16 + i];
            }
            b[k] = acc;
        }
        // shift history by one block, then store the new one
        self.x.copy_within(0..TAPS - 32, 32);
        for k in 0..16 {
            self.x[k] = cm[512 + k] * (a[k] + b[k]);
            self.x[32 - k - 1] = cm[528 + k] * (a[k] - b[k]);
        }
        let c = self.coeff;
        for i in 0..32 {
            let k = 31 - i;
            let mut j = 0;
            while j < 512 {
                self.z[i] += c[i + j] * (self.x[i + j] - self.x[j + k]);
                self.z[32 + i] += c[32 + i + j] * (-self.x[i + j] - self.x[j + k]);
                j += 64;
            }
        }
        out.copy_from_slice(&self.z[..32]);
        for i in 0..32 {
            self.z[i] = self.z[i + 32];
            self.z[i + 32] = 0.0;
        }
    }
}

/// Analysis filter bank: 32 PCM samples in, one sample of every subband out.
///
/// Built as the matched filter of the synthesis bank: the impulse response of every synthesis band is
/// measured once, and subband samples are the (normalised) correlation of the input with it.
pub struct Analysis {
    resp: Vec<Vec<f64>>,
    len: usize,
    hist: Vec<f64>,
}

impl Analysis {
    pub fn new(perfect: bool) -> Analysis {
        let blocks = 20;
        let mut resp = vec![vec![0f64; blocks * 32]; BANDS];
        for band in 0..BANDS {
            let mut sy = Synthesis::new(perfect);
            for blk in 0..blocks {
                let mut sub = [0f64; BANDS];
                if blk == 0 {
                    sub[band] = 1.0;
                }
                let mut out = [0f64; 32];
                sy.step(&sub, &mut out);
                resp[band][blk * 32..blk * 32 + 32].copy_from_slice(&out);
            }
        }
        let last = resp.iter().map(|r| r.iter().rposition(|v| v.abs() > 1e-12).unwrap_or(0)).max().unwrap_or(0);
        let len = (last + 1).div_ceil(32) * 32;
        for r in resp.iter_mut() {
            r.truncate(len);
            let e: f64 = r.iter().map(|v| v * v).sum();
            for v in r.iter_mut() {
                *v /= e;
            }
        }
        Analysis { resp, len, hist: vec![0.0; len] }
    }

    /// Delay in samples between input and the synthesis output of the analysed subbands.
    pub fn delay(&self) -> usize {
        self.len - 32
    }

    /// Feed 32 new PCM samples (oldest first) and produce one sample per subband.
    pub fn step(&mut self, input: &[f64], out: &mut [f64; BANDS]) {
        debug_assert_eq!(input.len(), 32);
        self.hist.copy_within(32.., 0);
        let n = self.len;
        self.hist[n - 32..].copy_from_slice(input);
        for band in 0..BANDS {
            out[band] = self.resp[band].iter().zip(&self.hist).map(|(f, x)| f * x).sum();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Analysis followed by synthesis returns the input up to a fixed delay and gain.
    fn roundtrip(perfect: bool) -> (f64, usize, f64) {
        let n_blocks = 200;
        let mut x = vec![0f64; n_blocks * 32];
        let mut seed = 12345u32;
        for v in x.iter_mut() {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            *v = ((seed >> 8) as f64 / 8_388_608.0 - 1.0) * 1000.0;
        }
        let mut an = Analysis::new(perfect);
        let mut sy = Synthesis::new(perfect);
        let mut y = Vec::new();
        for b in 0..n_blocks {
            let mut sub = [0f64; BANDS];
            an.step(&x[b * 32..b * 32 + 32], &mut sub);
            let mut out = [0f64; 32];
            sy.step(&sub, &mut out);
            y.extend_from_slice(&out);
        }
        // find delay and gain by correlation
        let mut best = (0.0, 0usize, f64::MAX);
        for delay in 0..1100usize {
            let n = x.len() - delay - 2000;
            let (mut xy, mut yy) = (0.0, 0.0);
            for i in 1000..1000 + n {
                xy += x[i] * y[i + delay];
                yy += y[i + delay] * y[i + delay];
            }
            let xx: f64 = x[1000..1000 + n].iter().map(|v| v * v).sum();
            let gain = xy / xx;
            let err = 1.0 - xy * xy / (xx * yy.max(1e-30));
            if err < best.2 {
                best = (gain, delay, err);
            }
        }
        best
    }

    #[test]
    fn analysis_then_synthesis_reconstructs() {
        for perfect in [false, true] {
            let (gain, delay, rel_err) = roundtrip(perfect);
            println!("perfect={perfect}: gain {gain:.6} delay {delay} relative error {rel_err:.3e}");
            assert!(rel_err < 1e-3, "poor reconstruction (perfect={perfect}): {rel_err}");
        }
    }
}
