//! Analysis bank matched to the 64-band synthesis filter bank the decoder uses for 96 kHz output.
//!
//! Built from the impulse responses of the decoder's own (bit-exact) synthesis: every band's response is
//! measured once, and subband samples are the energy-normalised correlation of the input with it.

use crate::dca::core_exact::FixedSynth;

pub const BANDS: usize = 64;

pub struct Analysis64 {
    resp: Vec<Vec<f64>>,
    len: usize,
    hist: Vec<f64>,
}

impl Analysis64 {
    pub fn new() -> Analysis64 {
        const AMP: i32 = 1 << 20;
        let blocks = 40;
        let mut resp: Vec<Vec<_>> = (0..BANDS).map(|_| Vec::with_capacity(blocks * BANDS)).collect();
        for band in 0..BANDS {
            let mut sy = FixedSynth::new(false, true);
            for blk in 0..blocks {
                let mut input = [0i32; 64];
                if blk == 0 {
                    input[band] = AMP;
                }
                let mut out = [0i32; 64];
                sy.step64_full(&input, &mut out);
                resp[band].extend(out.iter().map(|v| *v as f64 / AMP as f64));
            }
        }
        let last = resp
            .iter()
            .map(|r| r.iter().rposition(|v| v.abs() > 1e-6).unwrap_or(0))
            .max()
            .unwrap_or(0);
        let len = (last + 1).div_ceil(BANDS) * BANDS;
        for r in resp.iter_mut() {
            r.truncate(len);
            let e: f64 = r.iter().map(|v| v * v).sum();
            for v in r.iter_mut() {
                *v /= e;
            }
        }
        Analysis64 {
            resp,
            len,
            hist: vec![0.0; len],
        }
    }

    /// Samples between the input and the decoder output the analysed subbands produce.
    pub fn delay(&self) -> usize {
        self.len - BANDS
    }

    /// Feed 64 new samples (oldest first) and produce one sample per subband.
    pub fn step(&mut self, input: &[f64], out: &mut [f64; BANDS]) {
        debug_assert_eq!(input.len(), BANDS);
        self.hist.copy_within(BANDS.., 0);
        let n = self.len;
        self.hist[n - BANDS..].copy_from_slice(input);
        for band in 0..BANDS {
            out[band] = self.resp[band].iter().zip(&self.hist).map(|(f, x)| f * x).sum();
        }
    }
}

/// Dot product with independent accumulators, so the compiler can use vector instructions.
fn dot(a: &[f64], b: &[f64]) -> f64 {
    let mut s = [0f64; 8];
    for (ca, cb) in a.chunks_exact(8).zip(b.chunks_exact(8)) {
        for k in 0..8 {
            s[k] += ca[k] * cb[k];
        }
    }
    s.iter().sum()
}

impl Analysis64 {
    /// Feed `input.len() / 64` blocks of 64 samples (oldest first) and return `[band][block]`. The same as calling
    /// `step` for every block, but a band's coefficients are used for all the blocks in a row, so they stay in the
    /// processor's cache.
    pub fn analyse_blocks(&mut self, input: &[f64]) -> Vec<Vec<f64>> {
        debug_assert_eq!(input.len() % BANDS, 0);
        let nb = input.len() / BANDS;
        let n = self.len;
        let mut buf = Vec::with_capacity(n + input.len());
        buf.extend_from_slice(&self.hist);
        buf.extend_from_slice(input);
        let mut out = vec![vec![0f64; nb]; BANDS];
        for (band, o) in out.iter_mut().enumerate() {
            let row = &self.resp[band];
            for (b, v) in o.iter_mut().enumerate() {
                *v = dot(row, &buf[(b + 1) * BANDS..(b + 1) * BANDS + n]);
            }
        }
        self.hist.copy_from_slice(&buf[nb * BANDS..nb * BANDS + n]);
        out
    }
}

impl Default for Analysis64 {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Analysis followed by the decoder's synthesis returns the input.
    #[test]
    fn analysis_then_synthesis_reconstructs() {
        let mut an = Analysis64::new();
        let mut sy = FixedSynth::new(false, true);
        let n_blocks = 120;
        let mut seed = 777u32;
        let x: Vec<f64> = (0..n_blocks * 64)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                ((seed >> 8) as f64 / 8_388_608.0 - 1.0) * 1_000_000.0
            })
            .collect();
        let mut y = Vec::new();
        for b in 0..n_blocks {
            let mut sub = [0f64; BANDS];
            an.step(&x[b * 64..b * 64 + 64], &mut sub);
            let mut input = [0i32; 64];
            for (i, v) in sub.iter().enumerate() {
                input[i] = v.round() as i32;
            }
            let mut out = [0i32; 64];
            sy.step64_full(&input, &mut out);
            y.extend(out.iter().map(|v| *v as f64));
        }
        let delay = an.delay();
        let mut err = 0.0;
        let mut sig = 0.0;
        for i in 2000..x.len() - 2000 {
            err += (y[i + delay] - x[i]).powi(2);
            sig += x[i] * x[i];
        }
        let db = 10.0 * (sig / err).log10();
        println!("64-band analysis+synthesis: delay {delay}, SNR {db:.1} dB");
        assert!(db > 60.0, "{db}");
    }
}
