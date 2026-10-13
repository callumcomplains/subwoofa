//! Coding one channel of a frame for a quantizer profile: scale factors, quantized samples, the predictor of the
//! lowest bands, and the grids that carry the scale factors in the bitstream.

use crate::express::encoder::LbrEncoder;
use crate::express::levels::{alphabet, nearest, step_gain, AMP_MAX};
use crate::express::tables::{GRID_1_TO_SCF, GRID_1_WEIGHTS, QUANT_AMP, SCF_TO_GRID_1, SCF_TO_GRID_2};

/// Everything derived from the time samples of one channel for one frame and quantizer profile.
pub(crate) struct ChanCoding {
    /// Squared error against the input time samples (noise-filled bands count twice their energy).
    pub(crate) err: f64,
    /// Second grid: per group of bands, 64 temporal attenuation steps (two samples each) shared by its bands.
    pub(crate) g2: Vec<[u8; 64]>,
    /// The last eight samples the decoder will hold in each of the three predicted bands.
    pub(crate) tails: [[f64; 8]; 3],
    /// Which of the three predicted bands use their predictor in this frame.
    pub(crate) lpc_used: [bool; 3],
    /// Quantized symbol indexes `[band][slot][16]`.
    pub(crate) q: Vec<Vec<Vec<i32>>>,
    pub(crate) grids: Grids,
}

pub(crate) struct Grids {
    pub(crate) g1: Vec<[i32; 8]>,
    pub(crate) avg: Vec<i32>,
    pub(crate) g3: Vec<[i32; 8]>,
}

impl LbrEncoder {
    /// Quantize one channel for a set of levels. Bands at or above `k` are not coded; they get the scale
    /// factor that makes the decoder's noise match their level.
    /// Bands below `k` carry data; bands from `k` up to `kw` are written as zero symbols (a channel pair has to
    /// describe the whole mono region); above `kw` the decoder fills noise at the signalled level.
    pub(crate) fn code_channel(
        &self,
        ch: usize,
        x: &[Vec<f64>],
        levels: &[usize],
        pos: &[usize],
        k: usize,
        kw: usize,
        g2: Vec<[u8; 64]>,
    ) -> ChanCoding {
        let nsb = self.geo.nsb;
        let mut q = vec![vec![Vec::new(); 8]; nsb];
        let mut scf = vec![[0usize; 8]; nsb];
        let noise_gain = 0.453 * self.br_scale();
        let mut err = 0.0;
        let mut tails = [[0f64; 8]; 3];
        let mut lpc_used = [false; 3];
        for sb in 0..nsb {
            // the quantizer level belongs to the position a band is coded at (which need not be its own index)
            let alpha = alphabet(levels[pos[sb].min(nsb - 1)]);
            let lmax = alpha.iter().fold(0f32, |m, a| m.max(a.abs()));
            let group = SCF_TO_GRID_2[sb] as usize;
            if sb < 3 && pos[sb] < k {
                // the lowest bands are predicted: a closed loop over what the decoder will have produced
                // with the predictor of the frame, or without it (the first samples still follow the last frame's)
                let off = self.lpc_band(ch, sb, &x[sb], &alpha, lmax, false);
                let on = if levels[sb] >= 3 {
                    Some(self.lpc_band(ch, sb, &x[sb], &alpha, lmax, true))
                } else {
                    None
                };
                let ((s8, q8, e, tail), used) = match on {
                    Some(o) if o.2 < off.2 => (o, true),
                    _ => (off, false),
                };
                lpc_used[sb] = used;
                err += e;
                tails[sb] = tail;
                scf[sb] = s8;
                q[sb].clone_from_slice(&q8);
                continue;
            }
            for slot in 0..8 {
                let seg = &x[sb][slot * 16..slot * 16 + 16];
                // attenuation steps of the eight sample pairs of the slot (none for the lowest bands)
                let steps: [u8; 8] = if sb >= 4 && group < g2.len() {
                    g2[group][slot * 8..slot * 8 + 8].try_into().unwrap()
                } else {
                    [0; 8]
                };
                if pos[sb] < k {
                    let (s, qq, e) = self.pick_scale(seg, &alpha, lmax, &steps);
                    err += e;
                    scf[sb][slot] = s;
                    q[sb][slot] = qq;
                } else if pos[sb] < kw {
                    scf[sb][slot] = 0;
                    err += seg.iter().map(|v| v * v).sum::<f64>();
                    let zero = nearest(&alpha, 0.0) as i32;
                    q[sb][slot] = vec![zero; 16];
                } else {
                    // decoder noise: uniform, rms 0.453 * br scale * amplitude
                    let energy = seg.iter().map(|v| v * v).sum::<f64>();
                    err += 2.0 * energy;
                    let rms = (energy / 16.0).sqrt();
                    let want = rms / noise_gain;
                    let mut s = 0usize;
                    // the noise is shaped by the attenuation steps as well
                    let shaped = |s: usize| (steps.iter().map(|g| step_gain(s, *g).powi(2)).sum::<f64>() / 8.0).sqrt();
                    while s < AMP_MAX - 1 && shaped(s) < want {
                        s += 1;
                    }
                    scf[sb][slot] = if want < 1e-6 { 0 } else { s };
                    q[sb][slot] = Vec::new();
                }
            }
        }
        ChanCoding {
            err,
            q,
            grids: self.grids(&scf),
            g2,
            tails,
            lpc_used,
        }
    }

    /// Closed-loop quantization of one predicted band: scale factors, symbols, squared error and the decoder's last
    /// eight samples. The coefficient set follows the position in the frame as the decoder's synthesis does.
    pub(crate) fn lpc_band(
        &self,
        ch: usize,
        sb: usize,
        y: &[f64],
        alpha: &[f32],
        lmax: f32,
        on: bool,
    ) -> ([usize; 8], Vec<Vec<i32>>, f64, [f64; 8]) {
        let plan_all = self.lpc_plan.read().unwrap();
        let state_all = self.lpc_state.read().unwrap();
        let (plan, st) = (&plan_all[ch], &state_all[ch]);
        let none = [0f64; 8];
        let coef = |n: usize| -> &[f64; 8] {
            if !on && n >= 16 {
                &none
            } else if sb < 2 {
                if n < 16 {
                    &st.last[sb]
                } else if n < 80 {
                    &plan.cur[sb][0]
                } else {
                    &plan.cur[sb][1]
                }
            } else if n < 16 {
                &st.last[2]
            } else {
                &plan.cur[2][0]
            }
        };
        let mut h: Vec<f64> = st.hist[sb].to_vec();
        let mut scf = [0usize; 8];
        let mut q: Vec<Vec<i32>> = vec![Vec::new(); 8];
        let mut err = 0.0;
        let zero = nearest(alpha, 0.0) as i32;
        for slot in 0..8 {
            let base = h.len() - 8;
            // peak of the open-loop residual picks the scale factors to try
            let mut peak = 0f64;
            {
                let mut ctx: Vec<f64> = h[base..].to_vec();
                for i in 0..16 {
                    let n = slot * 16 + i;
                    let c = coef(n);
                    let pred: f64 = -(0..8).map(|j| c[j] * ctx[ctx.len() - 1 - j]).sum::<f64>();
                    peak = peak.max((y[n] - pred).abs());
                    ctx.push(y[n]);
                }
            }
            let mut best: Option<(f64, usize, Vec<i32>, Vec<f64>)> = None;
            if peak < 1e-9 {
                // silence: zero symbols, the decoder's prediction rings on
                let mut dec = h[base..].to_vec();
                let mut e = 0.0;
                for i in 0..16 {
                    let n = slot * 16 + i;
                    let c = coef(n);
                    let pred: f64 = -(0..8).map(|j| c[j] * dec[dec.len() - 1 - j]).sum::<f64>();
                    e += (y[n] - pred).powi(2);
                    dec.push(pred);
                }
                best = Some((e, 0, vec![zero; 16], dec[8..].to_vec()));
            } else {
                let mut s_peak = 0usize;
                while s_peak < AMP_MAX - 1 && (QUANT_AMP[s_peak] * lmax) as f64 + 1e-12 < peak {
                    s_peak += 1;
                }
                for s in s_peak.saturating_sub(4)..=(s_peak + 2).min(AMP_MAX - 1) {
                    let a = QUANT_AMP[s] as f64;
                    let mut dec = h[base..].to_vec();
                    let mut syms = Vec::with_capacity(16);
                    let mut e = 0.0;
                    for i in 0..16 {
                        let n = slot * 16 + i;
                        let c = coef(n);
                        let pred: f64 = -(0..8).map(|j| c[j] * dec[dec.len() - 1 - j]).sum::<f64>();
                        let qi = nearest(alpha, ((y[n] - pred) / a) as f32);
                        let out = pred + a * alpha[qi] as f64;
                        e += (y[n] - out).powi(2);
                        syms.push(qi as i32);
                        dec.push(out);
                    }
                    if best.as_ref().is_none_or(|b| e < b.0) {
                        best = Some((e, s, syms, dec[8..].to_vec()));
                    }
                }
            }
            let (e, s, syms, dec) = best.unwrap();
            err += e;
            scf[slot] = s;
            q[slot] = syms;
            h.extend(dec);
        }
        let mut tail = [0f64; 8];
        tail.copy_from_slice(&h[h.len() - 8..]);
        (scf, q, err, tail)
    }

    /// Pick the scale factor index and the quantized symbols of one 16-sample slot.
    pub(crate) fn pick_scale(&self, seg: &[f64], alpha: &[f32], lmax: f32, steps: &[u8; 8]) -> (usize, Vec<i32>, f64) {
        let peak = seg.iter().fold(0f64, |m, v| m.max(v.abs()));
        if peak < 1e-9 {
            let zero = nearest(alpha, 0.0);
            return (0, vec![zero as i32; 16], 0.0);
        }
        // the smallest scale factor at which every sample fits the alphabet with its own attenuation
        let mut s_peak = 0usize;
        for (i, v) in seg.iter().enumerate() {
            let mut s = steps[i / 2] as usize;
            while s < AMP_MAX - 1 && step_gain(s, steps[i / 2]) * lmax as f64 + 1e-12 < v.abs() {
                s += 1;
            }
            s_peak = s_peak.max(s);
        }
        let mut best = (f64::MAX, s_peak, Vec::new());
        for s in s_peak.saturating_sub(5)..=(s_peak + 1).min(AMP_MAX - 1) {
            let mut err = 0.0;
            let q: Vec<i32> = seg
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let a = step_gain(s, steps[i / 2]);
                    let qi = if a > 0.0 {
                        nearest(alpha, (*v / a) as f32)
                    } else {
                        nearest(alpha, 0.0)
                    };
                    err += (v - a * alpha[qi] as f64).powi(2);
                    qi as i32
                })
                .collect();
            if err < best.0 {
                best = (err, s, q);
            }
        }
        (best.1, best.2, best.0)
    }

    /// The temporal attenuation of every group of bands from the signal's own envelope: the loudest sample pair
    /// of a slot keeps its scale factor, quieter ones are turned down in steps of 3 dB (the decoder subtracts
    /// the steps from the scale factor of the slot, and a negative result silences the pair).
    /// Grid 1 values (11 groups x 8 slots), the third grid averages and per-band offsets.
    pub(crate) fn grids(&self, scf: &[[usize; 8]]) -> Grids {
        let nsb = self.geo.nsb;
        let mut g1 = vec![[0i32; 8]; 12];
        for (g, row) in g1.iter_mut().enumerate().take(self.geo.ng1) {
            let sb = (GRID_1_TO_SCF[g] as usize).min(nsb - 1);
            for i in 0..8 {
                let mut m = scf[sb][i] as i32;
                for sb2 in 0..nsb {
                    if GRID_1_WEIGHTS[g][sb2] > 64 {
                        m = m.max(scf[sb2][i] as i32);
                    }
                }
                row[i] = m;
            }
        }
        let mut avg = vec![0i32; nsb];
        let mut g3 = vec![[0i32; 8]; nsb];
        for sb in 4..nsb {
            let a = SCF_TO_GRID_1[sb] as usize;
            let (w1, w2) = (GRID_1_WEIGHTS[a][sb] as i32, GRID_1_WEIGHTS[a + 1][sb] as i32);
            let mut diffs = [0i32; 8];
            for i in 0..8 {
                let interp = (w1 * g1[a][i] + w2 * g1[a + 1][i]) >> 7;
                diffs[i] = interp - scf[sb][i] as i32;
            }
            let mean = (diffs.iter().sum::<i32>() as f64 / 8.0).round() as i32;
            avg[sb - 4] = mean;
            if sb < self.g3_avg_only_start() {
                for i in 0..8 {
                    g3[sb][i] = (diffs[i] - mean).clamp(-6, 5);
                }
            }
        }
        Grids { g1, avg, g3 }
    }

    /// Bands in the order they are coded: the first six in place, the others by falling energy.
    pub(crate) fn band_order(&self, x: &[Vec<f64>]) -> Vec<usize> {
        let nsb = self.geo.nsb;
        let mut rest: Vec<(f64, usize)> = (6..nsb).map(|b| (x[b].iter().map(|v| v * v).sum::<f64>(), b)).collect();
        rest.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
        (0..6.min(nsb)).chain(rest.into_iter().map(|r| r.1)).collect()
    }
}
