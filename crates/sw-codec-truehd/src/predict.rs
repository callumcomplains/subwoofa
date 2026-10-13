//! Per-channel analysis: choosing the quantiser step, the predictor filters and the cross-channel prediction
//! that make a channel's residual cheapest to code.

use crate::coding::{best_coding, choose_codings, coding_candidates, Coding};
use crate::filter::{apply_filters, quantize_fir, quantize_pair, Fir};
use crate::MAX_ORDER;

pub(crate) struct ChannelAnalysis {
    /// Cross-channel prediction chosen for this channel: 2.14 coefficients of channels up to itself.
    pub(crate) decor: Option<Vec<i32>>,
    pub(crate) quant: u8,
    pub(crate) fir: Fir,
    pub(crate) iir: Fir,
    /// Residual per sample (full scale, a multiple of 2^quant); the first 8 samples are raw.
    pub(crate) residual: Vec<i32>,
    /// Exact entropy-coding choice for every block, `[access unit][block]` (two blocks in the first AU).
    pub(crate) codings: Vec<Vec<Coding>>,
}

/// The predictor families the analysis of a channel tries. The plain LPC filters and the covariance-method LPC
/// are always tried; these two cost most of the time and pay least. Measured on film and music at 48 and 96 kHz,
/// leaving both out makes files 0.0% to 0.6% larger and the analysis 3 to 4 times faster.
#[derive(Clone, Copy)]
pub(crate) struct Tools {
    /// LPC behind a fixed two-pole filter, which helps band-limited high-rate material (up to 0.6%).
    pub(crate) fixed_iir: bool,
    /// LPC with a short moving-average stage fitted to its residual (under 0.1%).
    pub(crate) moving_average: bool,
}

impl Tools {
    pub(crate) fn for_effort(effort: sw_core::Effort) -> Tools {
        let all = effort == sw_core::Effort::Best;
        Tools {
            fixed_iir: all,
            moving_average: all,
        }
    }
}

/// Analyses one channel's samples (`signal`, split into access units of `au`) at quantiser step `quant`: finds the
/// predictor that leaves the cheapest residual, then the exact codebook for every block of it.
pub(crate) fn analyze_channel(signal: &[i32], au: usize, quant: u8, tools: Tools) -> ChannelAnalysis {
    let best = if signal.len() > 64 {
        search_filters(signal, au, quant, tools)
    } else {
        Candidate::unfiltered(signal, quant, au)
    };
    ChannelAnalysis {
        decor: None,
        quant,
        fir: best.fir,
        iir: best.iir,
        codings: choose_block_codings(&best.residual, au, quant),
        residual: best.residual,
    }
}

/// A predictor and what it leaves, with the estimated cost of coding that residual and the filters themselves.
struct Candidate {
    fir: Fir,
    iir: Fir,
    residual: Vec<i32>,
    cost: f64,
}

impl Candidate {
    /// No prediction at all: the samples are the residual.
    fn unfiltered(signal: &[i32], quant: u8, au: usize) -> Candidate {
        Candidate {
            fir: Fir::default(),
            iir: Fir::default(),
            residual: signal.to_vec(),
            cost: estimate_bits(signal, quant, au),
        }
    }
}

/// The search over predictor families for one channel: every filter pair offered is run over the signal, and the
/// cheapest result so far is kept.
struct FilterSearch<'a> {
    signal: &'a [i32],
    /// The samples with their unused low bits shifted away, as floating point (what the analysis works on).
    scaled: Vec<f64>,
    quant: u8,
    au: usize,
    best: Candidate,
}

impl FilterSearch<'_> {
    /// Tries a filter pair; keeps it if the residual plus the cost of sending the filters is the cheapest so far.
    fn offer(&mut self, fir: Fir, iir: Fir) {
        let mask = !((1i32 << self.quant) - 1);
        let residual = apply_filters(self.signal, &fir, &iir, mask);
        let iir_cost = if iir.order > 0 {
            40.0 + iir.order as f64 * 14.0
        } else {
            0.0
        };
        let cost = estimate_bits(&residual, self.quant, self.au) + 40.0 + fir.order as f64 * 14.0 + iir_cost;
        if cost < self.best.cost {
            self.best = Candidate {
                fir,
                iir,
                residual,
                cost,
            };
        }
    }

    /// The plain linear predictors from the autocorrelation method.
    fn plain_lpc(&mut self, lpc: &[Vec<f64>]) {
        for order in [2usize, 4, 8] {
            if let Some(fir) = quantize_fir(&lpc[order]) {
                self.offer(fir, Fir::default());
            }
        }
    }

    /// Fixed two-pole IIR stages (poles close to the unit circle) boost the empty top of the spectrum of
    /// band-limited high-rate material; the FIR then only has to deal with the rest.
    fn behind_fixed_iir(&mut self) {
        for &(b1, b2) in FIXED_IIR {
            let mut filtered = vec![0f64; self.scaled.len()];
            for i in 0..self.scaled.len() {
                let mut v = self.scaled[i];
                if i >= 1 {
                    v -= b1 * filtered[i - 1];
                }
                if i >= 2 {
                    v -= b2 * filtered[i - 2];
                }
                filtered[i] = v;
            }
            for order in [6usize, 5] {
                if let Some(coeffs) = lpc_covariance(&filtered, order) {
                    if let Some((fir, iir)) = quantize_pair(&coeffs, &[b1, b2]) {
                        self.offer(fir, iir);
                    }
                }
            }
            let lpc = levinson(&filtered, 6);
            if let Some((fir, iir)) = quantize_pair(&lpc[6], &[b1, b2]) {
                self.offer(fir, iir);
            }
        }
    }

    /// Predictors from the covariance method, which needs no window and so can be steeper.
    fn covariance_lpc(&mut self) {
        for order in [2usize, 4, 8] {
            if let Some(coeffs) = lpc_covariance(&self.scaled, order) {
                if let Some(fir) = quantize_fir(&coeffs) {
                    self.offer(fir, Fir::default());
                }
            }
        }
    }

    /// An LPC filter plus a short moving-average (IIR) stage fitted to its residual.
    fn with_moving_average(&mut self, lpc: &[Vec<f64>]) {
        for (order, taps) in [(6usize, 2usize), (4, 4), (5, 2)] {
            let coeffs = &lpc[order];
            let fir_residual: Vec<f64> = (0..self.scaled.len())
                .map(|i| {
                    let predicted: f64 = (0..order.min(i)).map(|k| coeffs[k] * self.scaled[i - 1 - k]).sum();
                    self.scaled[i] - predicted
                })
                .collect();
            if let Some(average) = fit_moving_average(&fir_residual, taps) {
                if let Some((fir, iir)) = quantize_pair(coeffs, &average) {
                    self.offer(fir, iir);
                }
            }
        }
    }
}

/// The cheapest predictor for a channel of more than a few samples.
fn search_filters(signal: &[i32], au: usize, quant: u8, tools: Tools) -> Candidate {
    let scaled: Vec<f64> = signal.iter().map(|v| (*v >> quant) as f64).collect();
    let lpc = levinson(&scaled, MAX_ORDER);
    let mut search = FilterSearch {
        signal,
        scaled,
        quant,
        au,
        best: Candidate::unfiltered(signal, quant, au),
    };
    search.plain_lpc(&lpc);
    if tools.fixed_iir {
        search.behind_fixed_iir();
    }
    search.covariance_lpc();
    if tools.moving_average {
        search.with_moving_average(&lpc);
    }
    search.best
}

/// The exact coding choice for the real block layout: the first access unit is 8 priming samples and the rest, each
/// coded as cheaply as possible; the other access units are chosen by shortest path, so that switching codebooks is
/// paid for.
fn choose_block_codings(residual: &[i32], au: usize, quant: u8) -> Vec<Vec<Coding>> {
    let n_au = residual.len() / au;
    if n_au == 0 {
        return Vec::new();
    }
    let block =
        |start: usize, len: usize| -> Vec<i32> { residual[start..start + len].iter().map(|r| r >> quant).collect() };
    let first: Vec<Coding> = [(0usize, 8usize), (8, au - 8)]
        .into_iter()
        .map(|(start, len)| best_coding(&block(start, len), quant))
        .collect();
    let hold = first[1];
    let candidates: Vec<Vec<Coding>> = (1..n_au)
        .map(|a| coding_candidates(&block(a * au, au), quant))
        .collect();
    let mut codings = vec![first];
    codings.extend(choose_codings(&candidates, hold).into_iter().map(|c| vec![c]));
    codings
}

/// Two-pole IIR stages tried in front of the FIR (poles at `b1`, `b2` in the denominator).
pub(crate) const FIXED_IIR: &[(f64, f64)] = &[(0.5, 0.9375), (0.0, 0.9375), (-0.5, 0.9375), (0.5, 0.75)];

/// Total bits of the exact entropy-coding choices of an analysis.
pub(crate) fn analysis_bits(a: &ChannelAnalysis) -> usize {
    a.codings.iter().flatten().map(|c| c.bits).sum()
}

/// Analyse channel `c` of `coded`, trying to predict it from the channels below it (a primitive
/// matrix the decoder undoes) and keeping the prediction only when it saves bits.
pub(crate) fn analyze_with_prediction(
    coded: &[Vec<i32>],
    c: usize,
    n: usize,
    au: usize,
    forced: Option<u8>,
    decorrelate: bool,
    tools: Tools,
) -> (ChannelAnalysis, Option<(ChannelAnalysis, usize)>) {
    let x = &coded[c][..n];
    let or_all = x.iter().fold(0i32, |a, v| a | v);
    let quant = match forced {
        Some(q) => q,
        None if or_all == 0 => 0,
        None => (or_all.trailing_zeros() as u8).min(15),
    };
    let base = analyze_channel(x, au, quant, tools);
    if !decorrelate || c == 0 || or_all == 0 || n < 256 {
        return (base, None);
    }
    let regs: Vec<&[i32]> = (0..c).map(|j| &coded[j][..n]).collect();
    // fit on signals whitened by the target's own predictor: what is left to remove is the part the
    // per-channel filters cannot (loud bass would otherwise dominate the fit)
    let xs: Vec<f64> = x.iter().map(|v| (*v >> quant) as f64).collect();
    let lpc = levinson(&xs, MAX_ORDER).pop().unwrap_or_default();
    let whiten = |sig: &[i32]| -> Vec<f64> {
        (0..n)
            .map(|i| {
                let pred: f64 = (0..lpc.len().min(i)).map(|k| lpc[k] * sig[i - 1 - k] as f64).sum();
                sig[i] as f64 - pred
            })
            .collect()
    };
    let wt = whiten(x);
    let wr: Vec<Vec<f64>> = regs.iter().map(|r| whiten(r)).collect();
    let Some(coef) = predict_coefficients(&wr, &wt) else {
        return (base, None);
    };
    let mask = !((1i32 << quant) - 1);
    let mut res = Vec::with_capacity(n);
    for i in 0..n {
        let s: i64 = coef.iter().zip(&regs).map(|(k, r)| *k as i64 * r[i] as i64).sum();
        let r = x[i] - (((s >> 14) as i32) & mask);
        if r.unsigned_abs() >= 1 << 23 {
            return (base, None);
        }
        res.push(r);
    }
    let mut with = analyze_channel(&res, au, quant, tools);
    let side_cost = 17 * (c + 1) + 40;
    let (b0, b1) = (analysis_bits(&base), analysis_bits(&with) + side_cost);
    if b1 < b0 {
        let mut full = coef;
        full.push(1 << 14); // the channel's own coefficient (index c)
        with.decor = Some(full);
        (base, Some((with, b0 - b1)))
    } else {
        (base, None)
    }
}

/// Least-squares prediction of `target` from `regs` (instantaneous), as 2.14 coefficients.
pub(crate) fn predict_coefficients(regs: &[Vec<f64>], target: &[f64]) -> Option<Vec<i32>> {
    let k = regs.len();
    let mut a = vec![vec![0f64; k]; k];
    let mut b = vec![0f64; k];
    for i in 0..target.len() {
        let t = target[i];
        for p in 0..k {
            let rp = regs[p][i];
            b[p] += rp * t;
            for q in p..k {
                a[p][q] += rp * regs[q][i];
            }
        }
    }
    for p in 0..k {
        for q in 0..p {
            a[p][q] = a[q][p];
        }
    }
    let trace: f64 = (0..k).map(|p| a[p][p]).sum();
    if trace < 1.0 {
        return None;
    }
    for p in 0..k {
        a[p][p] += trace * 1e-9 + 1e-9;
    }
    // Gaussian elimination with partial pivoting
    for col in 0..k {
        let piv = (col..k).max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs()))?;
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for row in col + 1..k {
            let f = a[row][col] / a[col][col];
            for cc in col..k {
                a[row][cc] -= f * a[col][cc];
            }
            b[row] -= f * b[col];
        }
    }
    let mut sol = vec![0f64; k];
    for row in (0..k).rev() {
        let mut v = b[row];
        for cc in row + 1..k {
            v -= a[row][cc] * sol[cc];
        }
        sol[row] = v / a[row][row];
    }
    let q: Vec<i32> = sol
        .iter()
        .map(|v| {
            if v.abs() < 0.004 {
                0
            } else {
                (v.clamp(-1.99, 1.99) * 16384.0).round() as i32
            }
        })
        .collect();
    q.iter().any(|v| *v != 0).then_some(q)
}

/// Cheap estimate of the bits needed to code `res` (about log2 of the mean magnitude per sample).
pub(crate) fn estimate_bits(res: &[i32], quant: u8, au: usize) -> f64 {
    let mut total = 0.0;
    for chunk in res.chunks(au) {
        let sum: u64 = chunk.iter().map(|r| ((r >> quant).unsigned_abs()) as u64).sum();
        let mean = sum as f64 / chunk.len() as f64;
        total += chunk.len() as f64 * ((mean + 1.0).log2() + 1.3);
    }
    total
}

/// Least-squares predictor from the data itself (no window, so no spectral leakage limits the
/// steepness of the filter): `w[n] ~ sum c[k] w[n-1-k]`.
pub(crate) fn lpc_covariance(w: &[f64], order: usize) -> Option<Vec<f64>> {
    let n = w.len();
    if n <= order * 4 {
        return None;
    }
    let mut a = vec![vec![0f64; order]; order];
    let mut b = vec![0f64; order];
    for i in order..n {
        let t = w[i];
        for p in 0..order {
            let rp = w[i - 1 - p];
            b[p] += rp * t;
            for q in p..order {
                a[p][q] += rp * w[i - 1 - q];
            }
        }
    }
    let mut trace = 0.0;
    for p in 0..order {
        for q in 0..p {
            a[p][q] = a[q][p];
        }
        trace += a[p][p];
    }
    if trace < 1.0 {
        return None;
    }
    for p in 0..order {
        a[p][p] += trace * 1e-12;
    }
    solve_linear(a, b)
}

/// Levinson-Durbin: returns predictor coefficients for every order 0..=max (x[n] ~ sum c[k] x[n-1-k]).
pub(crate) fn levinson(x: &[f64], max: usize) -> Vec<Vec<f64>> {
    let mut r = vec![0f64; max + 1];
    for lag in 0..=max {
        r[lag] = x.iter().zip(&x[lag..]).map(|(a, b)| a * b).sum();
    }
    r[0] = r[0] * (1.0 + 1e-9) + 1e-9;
    let mut out: Vec<Vec<f64>> = vec![Vec::new()];
    let mut a: Vec<f64> = Vec::new();
    let mut err = r[0];
    for i in 0..max {
        let mut acc = r[i + 1];
        for (j, aj) in a.iter().enumerate() {
            acc -= aj * r[i - j];
        }
        let k = if err.abs() < 1e-12 { 0.0 } else { acc / err };
        let mut na = vec![0f64; i + 1];
        for j in 0..i {
            na[j] = a[j] - k * a[i - 1 - j];
        }
        na[i] = k;
        err *= 1.0 - k * k;
        a = na;
        out.push(a.clone());
    }
    out
}

/// Moving-average coefficients `b` with `r[n] ~ e[n] + sum b[k] e[n-1-k]` for white `e`, fitted by
/// iterated linear regression on the running innovation. Returns `None` if no stable fit helps.
pub(crate) fn fit_moving_average(r: &[f64], q: usize) -> Option<Vec<f64>> {
    let n = r.len();
    let mut b = vec![0f64; q];
    let mut e: Vec<f64> = r.to_vec();
    let base: f64 = r.iter().map(|v| v * v).sum();
    if base < 1.0 {
        return None;
    }
    for _ in 0..4 {
        let mut a = vec![vec![0f64; q]; q];
        let mut rhs = vec![0f64; q];
        for i in q..n {
            for p in 0..q {
                rhs[p] += e[i - 1 - p] * r[i];
                for c in p..q {
                    a[p][c] += e[i - 1 - p] * e[i - 1 - c];
                }
            }
        }
        for p in 0..q {
            for c in 0..p {
                a[p][c] = a[c][p];
            }
            a[p][p] += base * 1e-9;
        }
        b = solve_linear(a, rhs)?;
        // innovation with the new coefficients
        for i in 0..n {
            let mut v = r[i];
            for k in 0..q.min(i) {
                v -= b[k] * e[i - 1 - k];
            }
            e[i] = v;
        }
        if e.iter().any(|v| !v.is_finite()) {
            return None;
        }
    }
    let after: f64 = e.iter().map(|v| v * v).sum();
    (after < base * 0.999 && b.iter().all(|v| v.abs() < 1.9)).then_some(b)
}

pub(crate) fn solve_linear(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let k = b.len();
    for col in 0..k {
        let piv = (col..k).max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs()))?;
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for row in col + 1..k {
            let f = a[row][col] / a[col][col];
            for cc in col..k {
                a[row][cc] -= f * a[col][cc];
            }
            b[row] -= f * b[col];
        }
    }
    let mut sol = vec![0f64; k];
    for row in (0..k).rev() {
        let mut v = b[row];
        for cc in row + 1..k {
            v -= a[row][cc] * sol[cc];
        }
        sol[row] = v / a[row][row];
    }
    Some(sol)
}
