//! Choosing predictors: the pairing of channels, and for each channel the order and coefficients that code it
//! cheapest.

use crate::lossless::xll::predict::{
    covariance_reflection, lattice_to_direct, predict, quantize_rc, rc_value, reflection_coefficients, whiten,
};
use crate::lossless::xll::residual::{segment_cost, zigzag};
use crate::lossless::xll::{BandPlan, ChannelPlan, SetPlan, XllEncoder};

impl XllEncoder {
    /// Choose channel pairs and coefficients. Returns (orig_order, coeffs per pair, coded channels).
    pub(crate) fn decorrelate(&self, pcm: &[Vec<i32>]) -> (Vec<usize>, Vec<i32>, Vec<Vec<i32>>) {
        let n = pcm.len();
        let len = pcm[0].len();
        // pairing and coefficients are judged on whitened channels: that is what the predictors then see
        let white: Vec<Vec<f64>> = pcm.iter().map(|c| whiten(c)).collect();
        let energy: Vec<f64> = white.iter().map(|c| c.iter().map(|v| v * v).sum::<f64>()).collect();
        let mut used = vec![false; n];
        let mut order: Vec<usize> = Vec::new();
        let mut coeffs: Vec<i32> = Vec::new();
        // greedy pairing by normalised correlation
        let corr = |a: usize, b: usize| -> f64 {
            let xy: f64 = white[a].iter().zip(&white[b]).map(|(x, y)| x * y).sum();
            xy / (energy[a] * energy[b]).sqrt().max(1e-9)
        };
        while used.iter().filter(|u| !**u).count() >= 2 {
            let mut best = (0.0f64, 0usize, 0usize);
            for a in 0..n {
                for b in a + 1..n {
                    if used[a] || used[b] {
                        continue;
                    }
                    let c = corr(a, b).abs();
                    if best.1 == best.2 || c > best.0 {
                        best = (c, a, b);
                    }
                }
            }
            let (_, a, b) = best;
            used[a] = true;
            used[b] = true;
            // basis = the louder channel
            let (basis, dec) = if energy[a] >= energy[b] { (a, b) } else { (b, a) };
            let xy: f64 = white[basis].iter().zip(&white[dec]).map(|(x, y)| x * y).sum();
            let ratio = if energy[basis] > 0.0 { xy / energy[basis] } else { 0.0 };
            let q = (ratio * 8.0).round().clamp(-64.0, 63.0) as i32;
            order.push(basis);
            order.push(dec);
            coeffs.push(q);
        }
        if let Some(i) = used.iter().position(|u| !*u) {
            order.push(i);
        }
        // apply
        let mut coded: Vec<Vec<i32>> = order.iter().map(|&c| pcm[c].clone()).collect();
        for (p, q) in coeffs.iter().enumerate() {
            if *q != 0 {
                let (b, d) = (2 * p, 2 * p + 1);
                for i in 0..len {
                    let pred = ((coded[b][i] as i64 * *q as i64 + 4) >> 3) as i32;
                    coded[d][i] = coded[d][i].wrapping_sub(pred);
                }
            }
        }
        (order, coeffs, coded)
    }

    pub(crate) fn plan_channel(&self, x: Vec<i32>) -> ChannelPlan {
        let n = x.len();
        let max_order = self.max_order.min(n / 8);
        let mut best: Option<(u64, ChannelPlan)> = None;
        let mut consider = |cost: u64, plan: ChannelPlan| {
            if best.as_ref().is_none_or(|b| cost < b.0) {
                best = Some((cost, plan));
            }
        };
        // fixed polynomial predictors
        for fo in 0..=3usize {
            let mut res = x.clone();
            for _ in 0..fo {
                for k in (1..n).rev() {
                    res[k] = res[k].wrapping_sub(res[k - 1]);
                }
            }
            let u: Vec<u32> = res.iter().map(|v| zigzag(*v)).collect();
            let cost = segment_cost(&u, 0, n);
            consider(
                cost,
                ChannelPlan {
                    adapt_order: 0,
                    fixed_order: fo,
                    rc_idx: Vec::new(),
                    residual: res,
                },
            );
        }
        if max_order > 0 && x.iter().any(|v| *v != 0) {
            let ks = reflection_coefficients(&x, max_order);
            // the covariance method costs half the time of the whole encode and, in most material, gains a few
            // hundredths of a per cent; Fast also tries fewer orders
            let covariance = self.cfg.effort == sw_core::Effort::Best;
            let orders: &[usize] = match self.cfg.effort {
                sw_core::Effort::Fast => &[2, 4, 8, 12, 15],
                _ => &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
            };
            for (cov, order) in orders.iter().flat_map(|&o| [(false, o), (true, o)]) {
                if order > max_order || (cov && !covariance) {
                    continue;
                }
                let idx: Vec<i32> = if cov {
                    match covariance_reflection(&x, order) {
                        Some(k) => k.iter().map(|k| quantize_rc(*k)).collect(),
                        None => continue,
                    }
                } else {
                    ks[..order].iter().map(|k| quantize_rc(*k)).collect()
                };
                let rcq: Vec<i32> = idx.iter().map(|i| rc_value(*i)).collect();
                let coeff = lattice_to_direct(&rcq);
                let res = predict(&x, &coeff);
                let u: Vec<u32> = res.iter().map(|v| zigzag(*v)).collect();
                let cost = segment_cost(&u, order, n) + 8 * order as u64;
                consider(
                    cost,
                    ChannelPlan {
                        adapt_order: order,
                        fixed_order: 0,
                        rc_idx: idx,
                        residual: res,
                    },
                );
            }
        }
        best.unwrap().1
    }

    /// Choose predictors for every channel of one channel set.
    pub(crate) fn plan_band(&self, pcm: &[Vec<i32>]) -> BandPlan {
        let nch = pcm.len();
        let (orig_order, coeffs, coded) = if nch >= 2 {
            self.decorrelate(pcm)
        } else {
            (vec![0], vec![], pcm.to_vec())
        };
        let decor = coeffs.iter().any(|c| *c != 0);
        let (orig_order, coeffs, coded) = if decor || nch < 2 {
            (orig_order, coeffs, coded)
        } else {
            ((0..nch).collect(), vec![0; nch / 2], pcm.to_vec())
        };
        let me: &XllEncoder = self;
        let plans: Vec<ChannelPlan> = {
            use rayon::prelude::*;
            coded.into_par_iter().map(|x| me.plan_channel(x)).collect()
        };
        BandPlan {
            plans,
            orig_order,
            coeffs,
            decor,
        }
    }

    pub(crate) fn plan_set(&self, pcm: &[Vec<i32>], core0: Option<&[Vec<i32>]>) -> SetPlan {
        if self.nbands() == 1 {
            return SetPlan {
                bands: vec![self.plan_band(pcm)],
                history: Vec::new(),
            };
        }
        let mut lo = Vec::new();
        let mut hi = Vec::new();
        let mut history = Vec::new();
        for (k, ch) in pcm.iter().enumerate() {
            let (mut b0, b1, h) = crate::lossless::xll::bands::split(ch);
            // a core under the lossless data covers band 0 only: what band 0 carries is the difference
            if let Some(core) = core0 {
                for (v, c) in b0.iter_mut().zip(&core[k]) {
                    *v -= *c;
                }
            }
            lo.push(b0);
            hi.push(b1);
            history.push(h);
        }
        let (b0, b1) = rayon::join(|| self.plan_band(&lo), || self.plan_band(&hi));
        SetPlan {
            bands: vec![b0, b1],
            history,
        }
    }
}
