//! Block switching: the 256-sample transform pair of A/52 clause 7.9.
//!
//! The decoder side (clause 7.9.4) is written out literally. The encoder transform for a switched block is
//! its adjoint: AC-3's lapped transforms are orthogonal, so analysing with the transpose of the synthesis
//! matrix gives perfect reconstruction. The two matrices (512 windowed samples from 256 interleaved
//! coefficients) are built once.

use crate::mdct::{kbd_window, Mdct, M, N};
use std::f64::consts::PI;

/// Literal clause 7.9.4.1 / 7.9.4.2 inverse transform: 256 coefficients to 512 windowed samples.
pub fn imdct_spec(xk: &[f64], short: bool, w: &[f64]) -> Vec<f64> {
    let n = N;
    let mut x = vec![0.0; n];
    if !short {
        let tw: Vec<(f64, f64)> = (0..n / 4)
            .map(|k| {
                let a = 2.0 * PI * (8 * k + 1) as f64 / (8 * n) as f64;
                (-a.cos(), -a.sin())
            })
            .collect();
        let z0: Vec<(f64, f64)> = (0..n / 4)
            .map(|k| {
                let (re, im) = (xk[n / 2 - 2 * k - 1], xk[2 * k]);
                (re * tw[k].0 - im * tw[k].1, im * tw[k].0 + re * tw[k].1)
            })
            .collect();
        let y = post(&z0, &tw, 8.0 * PI / n as f64);
        for i in 0..n / 8 {
            x[2 * i] = -y[n / 8 + i].1 * w[2 * i];
            x[2 * i + 1] = y[n / 8 - i - 1].0 * w[2 * i + 1];
            x[n / 4 + 2 * i] = -y[i].0 * w[n / 4 + 2 * i];
            x[n / 4 + 2 * i + 1] = y[n / 4 - i - 1].1 * w[n / 4 + 2 * i + 1];
            x[n / 2 + 2 * i] = -y[n / 8 + i].0 * w[n / 2 - 2 * i - 1];
            x[n / 2 + 2 * i + 1] = y[n / 8 - i - 1].1 * w[n / 2 - 2 * i - 2];
            x[3 * n / 4 + 2 * i] = y[i].1 * w[n / 4 - 2 * i - 1];
            x[3 * n / 4 + 2 * i + 1] = -y[n / 4 - i - 1].0 * w[n / 4 - 2 * i - 2];
        }
    } else {
        let tw: Vec<(f64, f64)> = (0..n / 8)
            .map(|k| {
                let a = 2.0 * PI * (8 * k + 1) as f64 / (4 * n) as f64;
                (-a.cos(), -a.sin())
            })
            .collect();
        let mut ys: Vec<Vec<(f64, f64)>> = Vec::new();
        for half in 0..2 {
            let xs: Vec<f64> = (0..n / 4).map(|k| xk[2 * k + half]).collect();
            let z0: Vec<(f64, f64)> = (0..n / 8)
                .map(|k| {
                    let (re, im) = (xs[n / 4 - 2 * k - 1], xs[2 * k]);
                    (re * tw[k].0 - im * tw[k].1, im * tw[k].0 + re * tw[k].1)
                })
                .collect();
            ys.push(post(&z0, &tw, 16.0 * PI / n as f64));
        }
        let (y1, y2) = (&ys[0], &ys[1]);
        for i in 0..n / 8 {
            x[2 * i] = -y1[i].1 * w[2 * i];
            x[2 * i + 1] = y1[n / 8 - i - 1].0 * w[2 * i + 1];
            x[n / 4 + 2 * i] = -y1[i].0 * w[n / 4 + 2 * i];
            x[n / 4 + 2 * i + 1] = y1[n / 8 - i - 1].1 * w[n / 4 + 2 * i + 1];
            x[n / 2 + 2 * i] = -y2[i].0 * w[n / 2 - 2 * i - 1];
            x[n / 2 + 2 * i + 1] = y2[n / 8 - i - 1].1 * w[n / 2 - 2 * i - 2];
            x[3 * n / 4 + 2 * i] = y2[i].1 * w[n / 4 - 2 * i - 1];
            x[3 * n / 4 + 2 * i + 1] = -y2[n / 8 - i - 1].0 * w[n / 4 - 2 * i - 2];
        }
    }
    x
}

/// Naive complex IFFT (z[n] = sum Z[k] e^{+j ang k n}) followed by the post-twiddle.
fn post(z0: &[(f64, f64)], tw: &[(f64, f64)], ang: f64) -> Vec<(f64, f64)> {
    let len = z0.len();
    (0..len)
        .map(|n| {
            let (mut re, mut im) = (0.0, 0.0);
            for (k, z) in z0.iter().enumerate() {
                let a = ang * (k * n) as f64;
                let (c, s) = (a.cos(), a.sin());
                re += z.0 * c - z.1 * s;
                im += z.0 * s + z.1 * c;
            }
            (re * tw[n].0 - im * tw[n].1, im * tw[n].0 + re * tw[n].1)
        })
        .collect()
}

/// Forward transforms for both block types, consistent with `Mdct::forward` for long blocks.
pub struct Switched {
    /// `at[type][k][n]`: coefficient k from input sample n (window included), already scaled.
    at: [Vec<f32>; 2],
}

impl Switched {
    pub fn new(mdct_scale: f64) -> Switched {
        let full = kbd_window(5.0, N);
        let w = &full[..M];
        let mats: Vec<Vec<f64>> = [false, true]
            .iter()
            .map(|&short| {
                let mut at = vec![0.0; M * N];
                for k in 0..M {
                    let mut unit = vec![0.0; M];
                    unit[k] = 1.0;
                    let col = imdct_spec(&unit, short, w);
                    at[k * N..(k + 1) * N].copy_from_slice(&col);
                }
                at
            })
            .collect();
        // calibrate the scale against the fast long-block forward transform
        let m = Mdct::new(mdct_scale);
        let x: Vec<f32> = (0..N)
            .map(|i| ((i * 37 % 101) as f32 - 50.0) / 50.0 + (i as f32 * 0.07).sin())
            .collect();
        let mut fast = vec![0f32; M];
        m.forward(&x, &mut fast);
        let raw: Vec<f64> = (0..M)
            .map(|k| (0..N).map(|n| mats[0][k * N + n] * x[n] as f64).sum())
            .collect();
        let k = raw.iter().zip(&fast).map(|(r, f)| r * *f as f64).sum::<f64>() / raw.iter().map(|r| r * r).sum::<f64>();
        let scaled = |m: &Vec<f64>| m.iter().map(|v| (v * k) as f32).collect::<Vec<f32>>();
        // the short synthesis columns have half the energy of the long ones (norm 64 against 128)
        let short: Vec<f32> = mats[1].iter().map(|v| (v * k * 2.0) as f32).collect();
        Switched {
            at: [scaled(&mats[0]), short],
        }
    }

    /// Transform 512 raw input samples into 256 coefficients; `short` interleaves the two half transforms.
    pub fn forward(&self, x: &[f32], short: bool, out: &mut [f32]) {
        let m = &self.at[usize::from(short)];
        for k in 0..M {
            let row = &m[k * N..(k + 1) * N];
            out[k] = row.iter().zip(x).map(|(a, b)| a * b).sum();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MDCT_SCALE;

    /// Decode a block sequence like a decoder would (clause 7.9.4.1 step 6): overlap and add with factor 2.
    fn decode(coefs: &[Vec<f32>], pattern: &[bool], w: &[f64]) -> Vec<f64> {
        let mut out = Vec::new();
        let mut delay = vec![0.0; M];
        for (c, &short) in coefs.iter().zip(pattern) {
            let xk: Vec<f64> = c.iter().map(|v| *v as f64).collect();
            let x = imdct_spec(&xk, short, w);
            for n in 0..M {
                out.push(2.0 * (x[n] + delay[n]));
                delay[n] = x[M + n];
            }
        }
        out
    }

    #[test]
    fn long_matrix_matches_fast_transform() {
        let s = Switched::new(MDCT_SCALE);
        let m = Mdct::new(MDCT_SCALE);
        let x: Vec<f32> = (0..N).map(|i| ((i * 91 % 113) as f32 - 56.0) / 56.0).collect();
        let (mut a, mut b) = (vec![0f32; M], vec![0f32; M]);
        s.forward(&x, false, &mut a);
        m.forward(&x, &mut b);
        let err = a.iter().zip(&b).map(|(p, q)| (p - q).abs()).fold(0.0, f32::max);
        assert!(err < 1e-4, "{err}");
    }

    #[test]
    fn mixed_block_sequences_reconstruct() {
        let s = Switched::new(MDCT_SCALE);
        let full = kbd_window(5.0, N);
        let w = &full[..M];
        let blocks = 14;
        let mut seed = 7u32;
        let sig: Vec<f32> = (0..(blocks + 1) * M)
            .map(|i| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                ((seed >> 9) as f32 / 4_194_304.0 - 1.0) * 0.5 + (i as f32 * 0.03).sin() * 0.3
            })
            .collect();
        let pattern: Vec<bool> = [
            false, false, true, false, true, true, false, false, true, false, false, true, true, false,
        ]
        .to_vec();
        let mut coefs = Vec::new();
        for b in 0..blocks {
            let mut c = vec![0f32; M];
            s.forward(&sig[b * M..b * M + N], pattern[b], &mut c);
            coefs.push(c);
        }
        let out = decode(&coefs, &pattern, w);
        // block b's output (samples b*M..(b+1)*M of `out`) is the signal delayed by one block
        let err = (M..blocks * M)
            .map(|i| (out[i] - sig[i] as f64).abs())
            .fold(0.0, f64::max);
        assert!(err < 1e-3, "max reconstruction error {err}");
    }
}

#[cfg(test)]
mod probe {
    use super::*;
    use crate::MDCT_SCALE;

    fn run(pattern: &[bool]) -> f64 {
        let s = Switched::new(MDCT_SCALE);
        let full = kbd_window(5.0, N);
        let w = &full[..M];
        let blocks = pattern.len();
        let sig: Vec<f32> = (0..(blocks + 1) * M)
            .map(|i| (i as f32 * 0.03).sin() * 0.5 + (i as f32 * 0.31).cos() * 0.2)
            .collect();
        let mut out = Vec::new();
        let mut delay = vec![0.0; M];
        for b in 0..blocks {
            let mut c = vec![0f32; M];
            s.forward(&sig[b * M..b * M + N], pattern[b], &mut c);
            let x = imdct_spec(&c.iter().map(|v| *v as f64).collect::<Vec<_>>(), pattern[b], w);
            for n in 0..M {
                out.push(2.0 * (x[n] + delay[n]));
                delay[n] = x[M + n];
            }
        }
        (M..blocks * M)
            .map(|i| (out[i] - sig[i] as f64).abs())
            .fold(0.0, f64::max)
    }

    #[test]
    fn patterns() {
        println!("all long {}", run(&[false; 6]));
        println!("all short {}", run(&[true; 6]));
        println!("L S L {}", run(&[false, false, true, false, false, false]));
        println!("S S L L {}", run(&[true, true, false, false, false, false]));
    }
}

#[cfg(test)]
mod probe2 {
    use super::*;
    use crate::MDCT_SCALE;

    #[test]
    fn gram() {
        let full = kbd_window(5.0, N);
        let w = &full[..M];
        let cols: Vec<Vec<f64>> = (0..M)
            .map(|k| {
                let mut u = vec![0.0; M];
                u[k] = 1.0;
                imdct_spec(&u, true, w)
            })
            .collect();
        let dot = |a: &Vec<f64>, b: &Vec<f64>| a.iter().zip(b).map(|(x, y)| x * y).sum::<f64>();
        let mut offmax = 0.0f64;
        let mut diag = Vec::new();
        for i in 0..M {
            diag.push(dot(&cols[i], &cols[i]));
            for j in 0..M {
                if i != j {
                    offmax = offmax.max(dot(&cols[i], &cols[j]).abs());
                }
            }
        }
        println!(
            "diag min {} max {} offmax {}",
            diag.iter().cloned().fold(f64::MAX, f64::min),
            diag.iter().cloned().fold(0.0, f64::max),
            offmax
        );
        let colsl: Vec<Vec<f64>> = (0..M)
            .map(|k| {
                let mut u = vec![0.0; M];
                u[k] = 1.0;
                imdct_spec(&u, false, w)
            })
            .collect();
        println!(
            "long diag {} {}",
            dot(&colsl[0], &colsl[0]),
            dot(&colsl[100], &colsl[100])
        );
        let _ = MDCT_SCALE;
    }
}

/// Transient detector of clause 8.2.2: an 8 kHz high-pass, then peak levels over a three-level tree of
/// segments of the 256 new samples; a block is switched when a segment peak jumps by more than the level's
/// threshold over its predecessor.
pub struct Detector {
    /// Two cascaded biquads: (b0, b1, b2, a1, a2) and transposed direct form II state.
    coef: [[f64; 5]; 2],
    state: [[f64; 2]; 2],
    /// Last segment peak of each level from the previous block.
    prev: [f64; 3],
}

const THRESHOLD: [f64; 3] = [0.1, 0.075, 0.05];
const SILENCE: f64 = 100.0 / 32768.0;

impl Detector {
    pub fn new(sample_rate: u32) -> Detector {
        // 2nd-order Butterworth high-pass at 8 kHz, bilinear transform
        let k = (PI * 8000.0 / sample_rate as f64).tan();
        let q = std::f64::consts::FRAC_1_SQRT_2;
        let norm = 1.0 / (1.0 + k / q + k * k);
        let b0 = norm;
        let c = [
            b0,
            -2.0 * b0,
            b0,
            2.0 * (k * k - 1.0) * norm,
            (1.0 - k / q + k * k) * norm,
        ];
        Detector {
            coef: [c, c],
            state: [[0.0; 2]; 2],
            prev: [0.0; 3],
        }
    }

    fn filter(&mut self, x: f64) -> f64 {
        let mut v = x;
        for (c, s) in self.coef.iter().zip(self.state.iter_mut()) {
            let y = c[0] * v + s[0];
            s[0] = c[1] * v - c[3] * y + s[1];
            s[1] = c[2] * v - c[4] * y;
            v = y;
        }
        v
    }

    /// Feed the 256 new samples of one block; true when the block should use the two short transforms.
    pub fn block(&mut self, new: &[f32]) -> bool {
        let f: Vec<f64> = new.iter().map(|v| self.filter(*v as f64).abs()).collect();
        let n = f.len();
        let mut transient = false;
        let mut peaks_last = [0.0; 3];
        let mut overall = 0.0f64;
        for level in 0..3 {
            let segs = 1usize << level;
            let len = n / segs;
            let mut before = self.prev[level];
            for s in 0..segs {
                let p = f[s * len..(s + 1) * len].iter().cloned().fold(0.0, f64::max);
                if level == 0 {
                    overall = p;
                }
                if p * THRESHOLD[level] > before {
                    transient = true;
                }
                before = p;
                if s == segs - 1 {
                    peaks_last[level] = p;
                }
            }
        }
        self.prev = peaks_last;
        transient && overall >= SILENCE
    }
}
