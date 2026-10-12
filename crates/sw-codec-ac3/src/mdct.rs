//! 512-point windowed MDCT (256 coefficients per block) via a 128-point complex FFT.

use std::f64::consts::PI;

pub const M: usize = 256; // coefficients per block
pub const N: usize = 512; // window length

/// Zeroth-order modified Bessel function of the first kind.
fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term, mut k) = (1.0, 1.0, 1.0);
    let q = x * x / 4.0;
    while term > sum * 1e-17 {
        term *= q / (k * k);
        sum += term;
        k += 1.0;
    }
    sum
}

/// Kaiser-Bessel-derived window of length `n` (the AC-3 window uses alpha = 5, n = 512).
pub fn kbd_window(alpha: f64, n: usize) -> Vec<f64> {
    let half = n / 2;
    let kaiser: Vec<f64> = (0..=half)
        .map(|i| {
            let r = 2.0 * i as f64 / half as f64 - 1.0;
            bessel_i0(PI * alpha * (1.0 - r * r).sqrt())
        })
        .collect();
    let total: f64 = kaiser.iter().sum();
    let mut acc = 0.0;
    let mut w = vec![0.0; n];
    for i in 0..half {
        acc += kaiser[i];
        w[i] = (acc / total).sqrt();
        w[n - 1 - i] = w[i];
    }
    w
}

#[derive(Clone, Copy)]
struct C {
    re: f64,
    im: f64,
}

pub struct Mdct {
    window: Vec<f64>,
    fft_tw: Vec<C>, // e^{-2 pi i k / 128}
    pre: Vec<C>,    // e^{-i pi (8n+1) / (8M)}
    bitrev: Vec<usize>,
    scale: f64,
}

impl Mdct {
    /// `scale` multiplies every output coefficient.
    pub fn new(scale: f64) -> Mdct {
        let h = M / 2;
        let fft_tw = (0..h / 2)
            .map(|k| {
                let a = -2.0 * PI * k as f64 / h as f64;
                C {
                    re: a.cos(),
                    im: a.sin(),
                }
            })
            .collect();
        let pre = (0..h)
            .map(|n| {
                let a = -PI * (8 * n + 1) as f64 / (8 * M) as f64;
                C {
                    re: a.cos(),
                    im: a.sin(),
                }
            })
            .collect();
        let bits = h.trailing_zeros();
        let bitrev = (0..h).map(|i| i.reverse_bits() >> (usize::BITS - bits)).collect();
        Mdct {
            window: kbd_window(5.0, N),
            fft_tw,
            pre,
            bitrev,
            scale,
        }
    }

    pub fn window(&self) -> &[f64] {
        &self.window
    }

    fn fft(&self, a: &mut [C]) {
        let n = a.len();
        for i in 0..n {
            let j = self.bitrev[i];
            if i < j {
                a.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..len / 2 {
                    let w = self.fft_tw[k * step];
                    let (u, v) = (a[start + k], a[start + k + len / 2]);
                    let t = C {
                        re: v.re * w.re - v.im * w.im,
                        im: v.re * w.im + v.im * w.re,
                    };
                    a[start + k] = C {
                        re: u.re + t.re,
                        im: u.im + t.im,
                    };
                    a[start + k + len / 2] = C {
                        re: u.re - t.re,
                        im: u.im - t.im,
                    };
                }
            }
            len *= 2;
        }
    }

    /// DCT-IV of length M: X[k] = sum u[n] cos(pi/M (n+1/2)(k+1/2)).
    fn dct4(&self, u: &[f64], out: &mut [f64]) {
        let h = M / 2;
        let mut z: Vec<C> = (0..h)
            .map(|n| {
                let (re, im) = (u[2 * n], u[M - 1 - 2 * n]);
                let t = self.pre[n];
                C {
                    re: re * t.re - im * t.im,
                    im: re * t.im + im * t.re,
                }
            })
            .collect();
        self.fft(&mut z);
        for k in 0..h {
            let t = self.pre[k];
            let y = C {
                re: z[k].re * t.re - z[k].im * t.im,
                im: z[k].re * t.im + z[k].im * t.re,
            };
            out[2 * k] = y.re;
            out[M - 1 - 2 * k] = -y.im;
        }
    }

    /// Window and transform 512 input samples into 256 coefficients.
    pub fn forward(&self, x: &[f32], out: &mut [f32]) {
        debug_assert!(x.len() == N && out.len() == M);
        let xw: Vec<f64> = (0..N).map(|i| x[i] as f64 * self.window[i]).collect();
        // fold 2M windowed samples into M for the DCT-IV
        let mut u = vec![0.0; M];
        for n in 0..M / 2 {
            u[n] = -xw[3 * M / 2 - 1 - n] - xw[3 * M / 2 + n];
            u[M / 2 + n] = xw[n] - xw[M - 1 - n];
        }
        let mut c = vec![0.0; M];
        self.dct4(&u, &mut c);
        for k in 0..M {
            out[k] = (c[k] * self.scale) as f32;
        }
    }
}

/// Reference O(N^2) MDCT used by tests: X[k] = sum x[n] w[n] cos(pi/M (n + 1/2 + M/2)(k + 1/2)).
#[cfg(test)]
pub fn direct_mdct(x: &[f32], window: &[f64]) -> Vec<f64> {
    (0..M)
        .map(|k| {
            (0..N)
                .map(|n| {
                    x[n] as f64
                        * window[n]
                        * (PI / M as f64 * (n as f64 + 0.5 + M as f64 / 2.0) * (k as f64 + 0.5)).cos()
                })
                .sum()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kbd_is_power_complementary() {
        let w = kbd_window(5.0, N);
        for n in 0..M {
            assert!((w[n] * w[n] + w[n + M] * w[n + M] - 1.0).abs() < 1e-9, "n={n}");
        }
    }

    #[test]
    fn dct4_matches_direct() {
        let m = Mdct::new(1.0);
        let u: Vec<f64> = (0..M).map(|i| ((i * 53 % 97) as f64 - 48.0) / 48.0).collect();
        let mut fast = vec![0.0; M];
        m.dct4(&u, &mut fast);
        let err = (0..M)
            .map(|k| {
                let d: f64 = (0..M)
                    .map(|n| u[n] * (PI / M as f64 * (n as f64 + 0.5) * (k as f64 + 0.5)).cos())
                    .sum();
                (d - fast[k]).abs()
            })
            .fold(0.0, f64::max);
        assert!(err < 1e-9, "dct4 max error {err}");
    }

    #[test]
    fn fast_mdct_matches_direct() {
        let m = Mdct::new(1.0);
        let x: Vec<f32> = (0..N)
            .map(|i| ((i * 37 % 101) as f32 - 50.0) / 50.0 + (i as f32 * 0.07).sin())
            .collect();
        let mut fast = vec![0f32; M];
        m.forward(&x, &mut fast);
        let direct = direct_mdct(&x, m.window());
        let err = fast
            .iter()
            .zip(&direct)
            .map(|(a, b)| (*a as f64 - b).abs())
            .fold(0.0, f64::max);
        assert!(err < 1e-3, "max error {err}");
    }
}
