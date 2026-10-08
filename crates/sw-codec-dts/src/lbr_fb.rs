//! The hybrid filterbank of the DTS Express (LBR) codec, written as the exact adjoint of the decoder
//! synthesis: a 128-band MDCT, a butterfly stage between neighbouring bands at high frequencies, and an
//! 8-point lapped transform over time inside every group of four coefficients. The encoder output is the
//! "time sample" domain the stream carries (32 bands x 128 samples per 4096-sample frame at 48 kHz).

use crate::lbr_tables::{BANK_COEFF, LONG_WINDOW};
use std::f64::consts::PI;

pub const BANDS: usize = 32;
pub const HOP: usize = 128;
/// Time samples per band in one frame.
pub const TS: usize = 128;

fn coeffs() -> ([f64; 4], [f64; 4], f64, f64) {
    let c = BANK_COEFF;
    ([c[0] as f64, c[1] as f64, c[2] as f64, c[3] as f64], [c[4] as f64, c[5] as f64, c[6] as f64, c[7] as f64], c[8] as f64, c[9] as f64)
}

/// The 4x8 stage of the decoder: `src[0..8]` are the time samples at positions -4..=3 around the step, the
/// result the four coefficients of the band (before the butterflies).
pub fn bank_forward(src: &[f64; 8]) -> [f64; 4] {
    let (sw, c, _, _) = coeffs();
    let a = src[0] * sw[0] - src[3] * sw[3];
    let b = src[1] * sw[1] - src[2] * sw[2];
    let cc = src[6] * sw[1] + src[5] * sw[2];
    let d = src[7] * sw[0] + src[4] * sw[3];
    [
        c[0] * b - c[1] * cc + c[3] * a - c[2] * d,
        c[0] * d - c[1] * a - c[3] * b - c[2] * cc,
        c[2] * b + c[1] * d - c[3] * cc + c[0] * a,
        c[2] * a - c[1] * b + c[3] * d - c[0] * cc,
    ]
}

/// Transpose of `bank_forward`, scaled so that the overlapped steps invert it (the stage is orthogonal up
/// to a factor of four).
pub fn bank_adjoint(o: &[f64; 4]) -> [f64; 8] {
    let (sw, c, _, _) = coeffs();
    // transpose of the 4x4 stage (rows: o0 = c3 a + c0 b - c1 cc - c2 d, ...)
    let a = 4.0 * (c[3] * o[0] - c[1] * o[1] + c[0] * o[2] + c[2] * o[3]);
    let b = 4.0 * (c[0] * o[0] - c[3] * o[1] + c[2] * o[2] - c[1] * o[3]);
    let cc = 4.0 * (-c[1] * o[0] - c[2] * o[1] - c[3] * o[2] - c[0] * o[3]);
    let d = 4.0 * (-c[2] * o[0] + c[0] * o[1] + c[1] * o[2] + c[3] * o[3]);
    let mut s = [0f64; 8];
    s[0] += sw[0] * a;
    s[3] += -sw[3] * a;
    s[1] += sw[1] * b;
    s[2] += -sw[2] * b;
    s[6] += sw[1] * cc;
    s[5] += sw[2] * cc;
    s[7] += sw[0] * d;
    s[4] += sw[3] * d;
    s
}

/// Butterflies of the decoder between neighbouring bands (on the coefficients `v[band][4]`).
pub fn butterflies_forward(v: &mut [[f64; 4]; BANDS]) {
    let (_, _, al1, al2) = coeffs();
    for i in 12..BANDS - 1 {
        let a = v[i][3] * al1;
        let b = v[i + 1][0] * al1;
        v[i][3] += b - a;
        v[i + 1][0] -= b + a;
        let a = v[i][2] * al2;
        let b = v[i + 1][1] * al2;
        v[i][2] += b - a;
        v[i + 1][1] -= b + a;
    }
}

/// Inverse of `butterflies_forward` (each pair is a 2x2 map).
pub fn butterflies_inverse(v: &mut [[f64; 4]; BANDS]) {
    let (_, _, al1, al2) = coeffs();
    let inv = |x2: f64, y2: f64, al: f64| -> (f64, f64) {
        // forward: x' = (1 - al) x + al y ; y' = -al x + (1 - al) y
        let det = (1.0 - al) * (1.0 - al) + al * al;
        (((1.0 - al) * x2 - al * y2) / det, (al * x2 + (1.0 - al) * y2) / det)
    };
    for i in 12..BANDS - 1 {
        let (x, y) = inv(v[i][3], v[i + 1][0], al1);
        v[i][3] = x;
        v[i + 1][0] = y;
        let (x, y) = inv(v[i][2], v[i + 1][1], al2);
        v[i][2] = x;
        v[i + 1][1] = y;
    }
}

/// Window of the 256-point MDCT (rising half from the table, mirrored).
fn window() -> [f64; 2 * HOP] {
    let mut w = [0f64; 2 * HOP];
    for i in 0..HOP {
        w[i] = LONG_WINDOW[i] as f64;
        w[2 * HOP - 1 - i] = LONG_WINDOW[i] as f64;
    }
    w
}

/// Analysis window: the decoder window is not power complementary by itself (it deviates by up to 2%), so
/// the analysis uses its biorthogonal partner, `w / (w[i]^2 + w[127 - i]^2)`, which makes the pair perfect
/// reconstruction.
fn analysis_window() -> [f64; 2 * HOP] {
    let mut w = [0f64; 2 * HOP];
    for i in 0..HOP {
        let ws = LONG_WINDOW[i] as f64;
        let wp = LONG_WINDOW[HOP - 1 - i] as f64;
        let a = ws / (ws * ws + wp * wp);
        w[i] = a;
        w[2 * HOP - 1 - i] = a;
    }
    w
}

fn cos_table() -> Vec<f64> {
    let n = HOP;
    let mut cos = Vec::with_capacity(2 * n * n);
    for i in 0..2 * n {
        for k in 0..n {
            cos.push((PI / n as f64 * (i as f64 + 0.5 + n as f64 / 2.0) * (k as f64 + 0.5)).cos());
        }
    }
    cos
}

/// Streaming analysis of one channel.
///
/// Absolute sample numbering: hop `h` is the MDCT of `x[128h .. 128h + 256)`; band time sample `N` is
/// touched by the steps with `4s - 4 <= N <= 4s + 3`. The decoder starts with zero history, so samples
/// before 0 are not representable.
pub struct Analysis {
    win: [f64; 2 * HOP],
    cos: Vec<f64>,
    /// Time samples being assembled; `ts[b][0]` is sample `base`.
    ts: Vec<Vec<f64>>,
    base: isize,
    /// Next hop to compute.
    hop: usize,
    /// Gain applied to the MDCT coefficients (the decoder IMDCT scale is -2^-17).
    scale: f64,
}

impl Analysis {
    pub fn new() -> Analysis {
        let scale = (2.0 / HOP as f64) * 131072.0;
        Analysis { win: analysis_window(), cos: cos_table(), ts: vec![vec![0.0; 4]; BANDS], base: -4, hop: 0, scale }
    }

    /// MDCT of `x[128h .. 128h + 256)` followed by the adjoint of the band stages; accumulates into the
    /// time samples.
    pub fn push_hop(&mut self, block: &[f64]) {
        debug_assert_eq!(block.len(), 2 * HOP);
        let mut c = vec![0f64; HOP];
        for (i, (x, w)) in block.iter().zip(self.win.iter()).enumerate() {
            let xw = x * w;
            if xw == 0.0 {
                continue;
            }
            let row = &self.cos[i * HOP..(i + 1) * HOP];
            for (o, cc) in c.iter_mut().zip(row) {
                *o += xw * cc;
            }
        }
        let mut v = [[0f64; 4]; BANDS];
        for b in 0..BANDS {
            for j in 0..4 {
                v[b][j] = c[b * 4 + j] * self.scale;
            }
        }
        butterflies_inverse(&mut v);
        let s = self.hop as isize;
        self.hop += 1;
        for b in 0..BANDS {
            let contrib = bank_adjoint(&v[b]);
            let first = (4 * s - 4 - self.base) as usize;
            let t = &mut self.ts[b];
            if first + 8 > t.len() {
                t.resize(first + 8, 0.0);
            }
            for (k, x) in contrib.iter().enumerate() {
                t[first + k] += *x;
            }
        }
    }

    /// The next frame of time samples (`N = 128 f .. 128 f + 128` of every band), once every step up to
    /// `32 f + 32` was pushed.
    pub fn take_frame(&mut self) -> Vec<Vec<f64>> {
        let start = 128 * (self.base + 4).div_euclid(128);
        let off = (start - self.base) as usize;
        let mut out = Vec::with_capacity(BANDS);
        for b in 0..BANDS {
            let t = &mut self.ts[b];
            if t.len() < off + HOP {
                t.resize(off + HOP, 0.0);
            }
            out.push(t[off..off + HOP].to_vec());
            t.drain(..off + HOP);
        }
        self.base = start + HOP as isize;
        out
    }

    pub fn hops_pushed(&self) -> usize {
        self.hop
    }
}

impl Default for Analysis {
    fn default() -> Self {
        Self::new()
    }
}

/// The decoder synthesis of one channel from time samples (for tests and for measuring quantization
/// noise): PCM from sample 0.
pub fn synthesize(ts: &[Vec<Vec<f64>>]) -> Vec<f64> {
    let scale = 131072f64.recip();
    let win = window();
    let cos = cos_table();
    let frames = ts.len();
    let mut out = vec![0f64; frames * 4096 + 4096];
    let at = |b: usize, idx: isize| -> f64 {
        if idx < 0 {
            return 0.0;
        }
        let f = idx as usize / TS;
        if f >= frames {
            return 0.0;
        }
        ts[f][b][idx as usize % TS]
    };
    for s in 0..frames * 32 {
        let mut v = [[0f64; 4]; BANDS];
        for b in 0..BANDS {
            let mut src = [0f64; 8];
            for (k, x) in src.iter_mut().enumerate() {
                *x = at(b, 4 * s as isize - 4 + k as isize);
            }
            v[b] = bank_forward(&src);
        }
        butterflies_forward(&mut v);
        let mut coef = [0f64; HOP];
        for b in 0..BANDS {
            for j in 0..4 {
                coef[b * 4 + j] = v[b][j];
            }
        }
        for i in 0..2 * HOP {
            let row = &cos[i * HOP..(i + 1) * HOP];
            let y: f64 = coef.iter().zip(row).map(|(c, r)| c * r).sum::<f64>() * scale * win[i];
            out[s * HOP + i] += y;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn butterflies_roundtrip() {
        let mut v = [[0f64; 4]; BANDS];
        for (i, row) in v.iter_mut().enumerate() {
            for (j, x) in row.iter_mut().enumerate() {
                *x = ((i * 7 + j * 3) % 11) as f64 - 5.0;
            }
        }
        let orig = v;
        butterflies_forward(&mut v);
        butterflies_inverse(&mut v);
        for i in 0..BANDS {
            for j in 0..4 {
                assert!((v[i][j] - orig[i][j]).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn bank_stage_is_perfect_reconstruction() {
        let steps = 40;
        let mut seed = 5u32;
        let c: Vec<[f64; 4]> = (0..steps)
            .map(|_| {
                let mut r = [0f64; 4];
                for x in r.iter_mut() {
                    seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    *x = (seed >> 8) as f64 / 8_388_608.0 - 1.0;
                }
                r
            })
            .collect();
        let mut t = vec![0f64; 4 * steps + 8];
        for (s, cs) in c.iter().enumerate() {
            let a = bank_adjoint(cs);
            for k in 0..8 {
                t[4 * s + k] += a[k]; // sample index 4s - 4 + k, stored with an offset of 4
            }
        }
        let mut worst = 0f64;
        for s in 2..steps - 2 {
            let mut src = [0f64; 8];
            for k in 0..8 {
                src[k] = t[4 * s + k];
            }
            let o = bank_forward(&src);
            for j in 0..4 {
                worst = worst.max((o[j] - c[s][j]).abs());
            }
        }
        println!("bank stage worst error {worst:e}");
        assert!(worst < 1e-5, "{worst}");
    }

    /// analysis followed by the decoder synthesis reproduces the signal
    #[test]
    fn analysis_synthesis_reconstructs() {
        let frames = 4;
        let n = frames * 4096 + 1024;
        let mut seed = 99u32;
        let x: Vec<f64> = (0..n)
            .map(|i| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let noise = (seed >> 8) as f64 / 8_388_608.0 - 1.0;
                0.3 * (2.0 * PI * 440.0 * i as f64 / 48_000.0).sin() + 0.1 * noise
            })
            .collect();
        let mut an = Analysis::new();
        let mut frames_ts: Vec<Vec<Vec<f64>>> = Vec::new();
        let mut next_hop = 0;
        for f in 0..frames {
            while next_hop <= 32 * f + 32 {
                let start = next_hop * HOP;
                let block: Vec<f64> = (0..2 * HOP).map(|i| x.get(start + i).copied().unwrap_or(0.0)).collect();
                an.push_hop(&block);
                next_hop += 1;
            }
            frames_ts.push(an.take_frame());
        }
        let y = synthesize(&frames_ts);
        let (mut e, mut s) = (0.0, 0.0);
        for i in 1024..frames * 4096 - 1024 {
            e += (y[i] - x[i]).powi(2);
            s += x[i] * x[i];
        }
        let db = 10.0 * (s / e).log10();
        println!("LBR filterbank round trip: {db:.1} dB");
        assert!(db > 80.0, "{db}");
    }
}
