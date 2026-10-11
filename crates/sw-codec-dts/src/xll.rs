//! DTS-HD lossless extension (XLL): frame encoder.
//!
//! Per frame and channel set: optional pairwise channel decorrelation, adaptive prediction through
//! quantized reflection coefficients (or a fixed polynomial predictor), and Rice / linear coding of the
//! residual in segments (ETSI TS 102 114 clause 8).

use crate::bits::BitWriter;
use crate::xll_table::REFL_COEFF;

pub const SYNC_XLL: u32 = 0x41A2_9547;

/// One row of an embedded downmix matrix: how a channel of a lower set was scaled and what the channels
/// of this set were added to it with (9-bit codes: bit 8 set = positive, low 8 bits index `DMIX`).
#[derive(Debug, Clone)]
pub struct DmixRow {
    pub scale_code: u16,
    pub coeff_codes: Vec<u16>,
}

#[derive(Debug, Clone)]
pub struct DmixHeader {
    /// One row per channel of the lower-indexed sets.
    pub rows: Vec<DmixRow>,
}

#[derive(Debug, Clone)]
pub struct SetCfg {
    pub channels: usize,
    /// Speaker position bit mask of the set (bit 0 = C, 1 = L, 2 = R, 3 = Ls, 4 = Rs, 5 = LFE1, ...).
    pub ch_mask: u32,
    /// Residual of a core (true) or the original audio (false).
    pub residual_of_core: bool,
    /// Present for the non-primary sets of a hierarchy.
    pub dmix: Option<DmixHeader>,
    /// The primary set can describe how a decoder mixes its channels down to stereo: the matrix type (1 = Lo/Ro,
    /// 2 = Lt/Rt) and, for each of the two outputs, a 9-bit code for every channel of the set.
    pub matrix: Option<(u8, Vec<Vec<u16>>)>,
}

#[derive(Debug, Clone)]
pub struct XllConfig {
    pub sample_rate: u32,
    /// Resolution of the coded samples (16, 20 or 24 bits).
    pub bit_res: u32,
    /// Samples per channel in one frame.
    pub frame_samples: usize,
    pub sets: Vec<SetCfg>,
}

pub fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

fn zigzag(v: i32) -> u32 {
    if v >= 0 { (v as u32) << 1 } else { (((-(v as i64)) as u32) << 1) - 1 }
}

#[inline]
fn norm16(v: i64) -> i64 {
    (v + (1 << 15)) >> 16
}

#[inline]
fn mul16(a: i32, b: i32) -> i32 {
    norm16(a as i64 * b as i64) as i32
}

#[inline]
fn clip23(v: i64) -> i32 {
    v.clamp(-(1 << 23), (1 << 23) - 1) as i32
}

/// Direct form coefficients (error filter `1 + sum c_k z^-k`) from quantized reflection coefficients,
/// exactly as the decoder derives them.
fn lattice_to_direct(rc: &[i32]) -> Vec<i32> {
    let order = rc.len();
    let mut coeff = vec![0i32; order];
    for j in 0..order {
        let r = rc[j];
        for k in 0..(j + 1) / 2 {
            let t1 = coeff[k];
            let t2 = coeff[j - k - 1];
            coeff[k] = t1.wrapping_add(mul16(r, t2));
            coeff[j - k - 1] = t2.wrapping_add(mul16(r, t1));
        }
        coeff[j] = r;
    }
    coeff
}

/// Quantize a reflection coefficient (|k| < 1) to a table index with sign.
fn quantize_rc(k: f64) -> i32 {
    let target = (k.abs() * 65536.0).min(65491.0);
    let mut best = 0usize;
    let mut bd = f64::MAX;
    for (i, v) in REFL_COEFF.iter().enumerate() {
        let d = (*v as f64 - target).abs();
        if d < bd {
            bd = d;
            best = i;
        }
    }
    if k < 0.0 { -(best as i32) } else { best as i32 }
}

fn rc_value(idx: i32) -> i32 {
    if idx < 0 { -(REFL_COEFF[(-idx) as usize] as i32) } else { REFL_COEFF[idx as usize] as i32 }
}

/// Levinson-Durbin: reflection coefficients of the error filter `1 + sum a_k z^-k` up to `max_order`.
fn reflection_coefficients(x: &[i32], max_order: usize) -> Vec<f64> {
    let n = x.len();
    let taper: f64 = std::env::var("SW_TAPER").ok().and_then(|v| v.parse().ok()).unwrap_or(0.3);
    let xs: Vec<f64> = x
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let t = (i as f64 + 0.5) / n as f64;
            let w = if taper > 0.0 { let e = t.min(1.0 - t); if e < taper / 2.0 { 0.5 - 0.5 * (std::f64::consts::PI * (2.0 * e / taper)).cos() } else { 1.0 } } else { 1.0 };
            *v as f64 * w
        })
        .collect();
    let mut r = vec![0f64; max_order + 1];
    for lag in 0..=max_order {
        let mut s = 0.0;
        for i in lag..n {
            s += xs[i] * xs[i - lag];
        }
        r[lag] = s;
    }
    r[0] = r[0] * (1.0 + 1e-9) + 1e-3;
    let mut a = vec![0f64; max_order + 1];
    let mut ks = Vec::new();
    let mut err = r[0];
    for i in 1..=max_order {
        let mut acc = r[i];
        for j in 1..i {
            acc += a[j] * r[i - j];
        }
        let k = if err.abs() < 1e-9 { 0.0 } else { -acc / err };
        let k = k.clamp(-0.9995, 0.9995);
        let prev = a.clone();
        for j in 1..i {
            a[j] = prev[j] + k * prev[i - j];
        }
        a[i] = k;
        err *= 1.0 - k * k;
        ks.push(k);
    }
    ks
}

/// Reflection coefficients from a covariance-method (windowless least squares) predictor, by step-down.
fn covariance_reflection(x: &[i32], order: usize) -> Option<Vec<f64>> {
    let n = x.len();
    if n <= 4 * order {
        return None;
    }
    let xs: Vec<f64> = x.iter().map(|v| *v as f64).collect();
    let mut m = vec![vec![0f64; order]; order];
    let mut r = vec![0f64; order];
    for t in order..n {
        for i in 0..order {
            let xi = xs[t - 1 - i];
            r[i] -= xi * xs[t];
            for j in i..order {
                m[i][j] += xi * xs[t - 1 - j];
            }
        }
    }
    let mut trace = 0.0;
    for i in 0..order {
        for j in 0..i {
            m[i][j] = m[j][i];
        }
        trace += m[i][i];
    }
    if trace < 1.0 {
        return None;
    }
    for i in 0..order {
        m[i][i] += trace * 1e-10;
    }
    // Gaussian elimination
    let mut a = m;
    let mut b = r;
    for col in 0..order {
        let piv = (col..order).max_by(|&p, &q| a[p][col].abs().total_cmp(&a[q][col].abs()))?;
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for row in col + 1..order {
            let f = a[row][col] / a[col][col];
            for c in col..order {
                a[row][c] -= f * a[col][c];
            }
            b[row] -= f * b[col];
        }
    }
    let mut coef = vec![0f64; order];
    for row in (0..order).rev() {
        let mut v = b[row];
        for c in row + 1..order {
            v -= a[row][c] * coef[c];
        }
        coef[row] = v / a[row][row];
    }
    // step down: coef[k-1] multiplies x[n-k]
    let mut ks = vec![0f64; order];
    let mut cur = coef;
    for i in (0..order).rev() {
        let k = cur[i];
        if !k.is_finite() {
            return None;
        }
        ks[i] = k.clamp(-0.9995, 0.9995);
        if i == 0 {
            break;
        }
        let d = 1.0 - k * k;
        if d.abs() < 1e-9 {
            return None;
        }
        let mut next = vec![0f64; i];
        for j in 0..i {
            next[j] = (cur[j] - k * cur[i - 1 - j]) / d;
        }
        cur = next;
    }
    Some(ks)
}

/// Rice parameter minimising the code length of `u`, and that length in bits.
fn best_rice(u: &[u32]) -> (u32, u64) {
    if u.is_empty() {
        return (0, 0);
    }
    let mut best = (0u32, u64::MAX);
    for k in 0..=24u32 {
        let mut bits = u.len() as u64 * (k as u64 + 1);
        for v in u {
            bits += (*v >> k) as u64;
        }
        if bits < best.1 {
            best = (k, bits);
        }
        // the cost is convex in k: stop once it rises clearly
        if bits > best.1 + 64 && k > 2 {
            break;
        }
    }
    best
}

/// Bits needed by a linear code of the zigzag values (the stored parameter is `bits - 1`, 0 = all zero).
fn linear_bits(u: &[u32]) -> (u32, u64) {
    let max = u.iter().copied().max().unwrap_or(0);
    if max == 0 {
        return (0, 0);
    }
    let w = (32 - max.leading_zeros()).max(2);
    (w, w as u64 * u.len() as u64)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Code {
    Rice(u32),
    Linear(u32),
    /// Rice code with parameter `k` for most samples; the `c` largest (isolated) samples are sent as `l`-bit
    /// values at listed positions instead of long unary runs.
    Hybrid { k: u32, l: u32, c: u32 },
}

fn choose_code(u: &[u32]) -> (Code, u64) {
    let (k, rb) = best_rice(u);
    let (w, lb) = linear_bits(u);
    // linear data need no unary part; compare including the larger cost of an all-zero block
    if lb <= rb { (Code::Linear(w), lb) } else { (Code::Rice(k), rb) }
}

/// Hybrid coding is only tried for a segment whose largest value is at least this many times its mean (output
/// is identical to trying it always on the 192 kHz test material, 14% faster; 0 turns hybrid coding off).
fn hybrid_gate() -> Option<f64> {
    static G: std::sync::OnceLock<Option<f64>> = std::sync::OnceLock::new();
    *G.get_or_init(|| Some(std::env::var("SW_HYB_GATE").ok().and_then(|v| v.parse().ok()).unwrap_or(8.0)))
}

/// The channel with its predictable part taken out (an order-2 linear predictor fitted to it; `SW_WHITEN` sets the
/// order, 0 = the samples as they are).
fn whiten(x: &[i32]) -> Vec<f64> {
    let order: usize = std::env::var("SW_WHITEN").ok().and_then(|v| v.parse().ok()).unwrap_or(2);
    if order == 0 || x.len() < 64 || x.iter().all(|v| *v == 0) {
        return x.iter().map(|v| *v as f64).collect();
    }
    let ks = reflection_coefficients(x, order);
    let rcq: Vec<i32> = ks.iter().map(|k| rc_value(quantize_rc(*k))).collect();
    let coeff = lattice_to_direct(&rcq);
    predict(x, &coeff).iter().map(|v| *v as f64).collect()
}

/// Indices of the `c` largest values (ties: lowest index first), in ascending index order.
fn isolated_positions(u: &[u32], c: usize) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..u.len()).collect();
    idx.sort_by(|&a, &b| u[b].cmp(&u[a]).then(a.cmp(&b)));
    idx.truncate(c);
    idx.sort_unstable();
    idx
}

/// Hybrid Rice coding of part B of a segment: the best (k, l, count) and its length in bits, including what it adds
/// to the segment header (the hybrid flag, the length field of `nabits` bits, the count of `posbits` bits).
fn hybrid_code(u: &[u32], posbits: u32, nabits: u32) -> Option<(Code, u64)> {
    let n = u.len();
    if n < 16 {
        return None;
    }
    if let Some(g) = hybrid_gate() {
        if g == 0.0 {
            return None;
        }
        // outliers make hybrid coding pay; without a value well above the typical one it cannot
        let (mut mx, mut sum) = (0u32, 0u64);
        for v in u {
            mx = mx.max(*v);
            sum += *v as u64;
        }
        if (mx as f64) < g * (sum as f64 / n as f64 + 1.0) {
            return None;
        }
    }
    let max_c = ((1usize << posbits.min(20)) - 1).min(n - 1).min(32);
    // sums of u >> k over everything, then per candidate set only the isolated samples are taken out
    let mut sums = [0u64; 25];
    for v in u {
        for (k, s) in sums.iter_mut().enumerate() {
            *s += (*v >> k) as u64;
        }
    }
    let mut order: Vec<usize> = (0..n).collect();
    let top = max_c.min(n);
    order.select_nth_unstable_by(top.saturating_sub(1).min(n - 1), |&a, &b| u[b].cmp(&u[a]).then(a.cmp(&b)));
    let mut top_idx: Vec<usize> = order[..top].to_vec();
    top_idx.sort_by(|&a, &b| u[b].cmp(&u[a]).then(a.cmp(&b)));
    let mut best: Option<(Code, u64)> = None;
    for &c in &[1usize, 2, 3, 4, 6, 8, 12, 16, 24, 32] {
        if c > top_idx.len() {
            break;
        }
        let iso = &top_idx[..c];
        let big = u[iso[0]];
        if big == 0 {
            break;
        }
        let l = 32 - big.leading_zeros();
        if l > (1 << nabits) {
            continue;
        }
        let fixed = 1 + nabits as u64 + posbits as u64 + c as u64 * (posbits as u64 + l as u64);
        for k in 0..=24u32 {
            let taken: u64 = iso.iter().map(|&i| (u[i] >> k) as u64).sum();
            let bits = (n - c) as u64 * (k as u64 + 1) + (sums[k as usize] - taken) + fixed;
            if best.map_or(true, |b| bits < b.1) {
                best = Some((Code::Hybrid { k, l, c: c as u32 }, bits));
            }
        }
    }
    best
}

/// Choice of coding for part B of a segment (Rice, linear or hybrid Rice).
fn choose_code_b(u: &[u32], posbits: u32, nabits: u32) -> (Code, u64) {
    let plain = choose_code(u);
    match hybrid_code(u, posbits, nabits) {
        Some(h) if h.1 < plain.1 => h,
        _ => plain,
    }
}

struct BandPlan {
    plans: Vec<ChannelPlan>,
    orig_order: Vec<usize>,
    coeffs: Vec<i32>,
    decor: bool,
}

/// A channel set: one band below 192 kHz, two above (plus the band-0 history the decoder needs).
struct SetPlan {
    bands: Vec<BandPlan>,
    history: Vec<[i32; crate::xll_bands::HISTORY]>,
}

struct ChannelPlan {
    /// Coded samples of the channel (after decorrelation).
    coded: Vec<i32>,
    adapt_order: usize,
    fixed_order: usize,
    rc_idx: Vec<i32>,
    residual: Vec<i32>,
}

pub struct XllEncoder {
    cfg: XllConfig,
    pub max_order: usize,
}

/// Segment count assumed while choosing predictors.
const NSEG: usize = 4;

impl XllEncoder {
    pub fn new(cfg: XllConfig) -> XllEncoder {
        let max_order = if cfg.sample_rate > 48_000 { 15 } else { 12 };
        XllEncoder { cfg, max_order }
    }

    fn nabits(&self) -> u32 {
        let n = if self.cfg.bit_res > 16 { 5 } else if self.cfg.bit_res > 8 { 4 } else { 3 };
        // room for the embedded downmix and the decimator saturation
        if (self.cfg.sets.len() > 1 || self.nbands() > 1) && n < 5 { n + 1 } else { n }
    }

    /// Frequency bands: above 96 kHz the signal is split in two.
    fn nbands(&self) -> usize {
        if self.cfg.sample_rate > 96_000 { 2 } else { 1 }
    }

    /// Samples per channel and band in one frame.
    fn band_len(&self) -> usize {
        self.cfg.frame_samples / self.nbands()
    }

    /// Choose channel pairs and coefficients. Returns (orig_order, coeffs per pair, coded channels).
    fn decorrelate(&self, pcm: &[Vec<i32>]) -> (Vec<usize>, Vec<i32>, Vec<Vec<i32>>) {
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

    fn plan_channel(&self, x: Vec<i32>) -> ChannelPlan {
        let n = x.len();
        let max_order = self.max_order.min(n / 8);
        let mut best: Option<(u64, ChannelPlan)> = None;
        let mut consider = |cost: u64, plan: ChannelPlan| {
            if best.as_ref().map_or(true, |b| cost < b.0) {
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
            consider(cost, ChannelPlan { coded: x.clone(), adapt_order: 0, fixed_order: fo, rc_idx: Vec::new(), residual: res });
        }
        if max_order > 0 && x.iter().any(|v| *v != 0) {
            let ks = reflection_coefficients(&x, max_order);
            for (cov, order) in (1usize..=15).flat_map(|o| [(false, o), (true, o)]) {
                if order > max_order {
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
                consider(cost, ChannelPlan { coded: x.clone(), adapt_order: order, fixed_order: 0, rc_idx: idx, residual: res });
            }
        }
        best.unwrap().1
    }

    /// Choose predictors for every channel of one channel set.
    fn plan_band(&self, pcm: &[Vec<i32>]) -> BandPlan {
        let nch = pcm.len();
        let (orig_order, coeffs, coded) = if nch >= 2 { self.decorrelate(pcm) } else { (vec![0], vec![], pcm.to_vec()) };
        let decor = coeffs.iter().any(|c| *c != 0);
        let (orig_order, coeffs, coded) = if decor || nch < 2 { (orig_order, coeffs, coded) } else { ((0..nch).collect(), vec![0; nch / 2], pcm.to_vec()) };
        let me: &XllEncoder = self;
        let plans: Vec<ChannelPlan> = {
            use rayon::prelude::*;
            coded.into_par_iter().map(|x| me.plan_channel(x)).collect()
        };
        BandPlan { plans, orig_order, coeffs, decor }
    }

    fn plan_set(&self, pcm: &[Vec<i32>], core0: Option<&[Vec<i32>]>) -> SetPlan {
        if self.nbands() == 1 {
            return SetPlan { bands: vec![self.plan_band(pcm)], history: Vec::new() };
        }
        let mut lo = Vec::new();
        let mut hi = Vec::new();
        let mut history = Vec::new();
        for (k, ch) in pcm.iter().enumerate() {
            let (mut b0, b1, h) = crate::xll_bands::split(ch);
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
        SetPlan { bands: vec![b0, b1], history }
    }

    /// Encode one frame of samples (`bit_res`-bit signed integers, one vector per channel: the channels of
    /// the first set in speaker order, then those of the next set, ...).
    pub fn encode_frame(&mut self, pcm: &[Vec<i32>]) -> Vec<u8> {
        self.encode_frame_core(pcm, None)
    }

    /// Like `encode_frame`, for a stream with a core under the lossless data at 176.4 / 192 kHz: `core0[ch]` is the
    /// core's decoded output at the band rate (half the sample rate) for each channel of the first set, which the
    /// decoder adds to band 0; the band-0 samples are coded as the difference.
    pub fn encode_frame_core(&mut self, pcm: &[Vec<i32>], core0: Option<&[Vec<i32>]>) -> Vec<u8> {
        let cfg = self.cfg.clone();
        let len = cfg.frame_samples;
        assert!(self.nbands() == 1 || cfg.sets.len() == 1, "frequency bands: a single channel set only");
        assert_eq!(pcm.len(), cfg.sets.iter().map(|s| s.channels).sum::<usize>());
        assert!(pcm.iter().all(|c| c.len() == len));
        let mut set_plans = Vec::new();
        let mut at = 0;
        for (si, set) in cfg.sets.iter().enumerate() {
            set_plans.push(self.plan_set(&pcm[at..at + set.channels], if si == 0 { core0 } else { None }));
            at += set.channels;
        }
        // band data: one block per segment; try several segment counts and keep the smallest frame
        let mut best_frame: Option<Vec<u8>> = None;
        for &nseg in &[1usize, 2, 4, 8] {
            let seg_len = self.band_len() / nseg;
            if seg_len < 16 || seg_len > if cfg.sample_rate > 48_000 { 512 } else { 256 } {
                continue;
            }
            let segs: Vec<Vec<Vec<Vec<u8>>>> = set_plans.iter().map(|sp| (0..sp.bands.len()).map(|b| self.set_segments(sp, b, nseg)).collect()).collect();
            let frame = self.assemble(&set_plans, &segs, nseg);
            if best_frame.as_ref().map_or(true, |f| frame.len() < f.len()) {
                best_frame = Some(frame);
            }
        }
        best_frame.expect("no valid segmentation")
    }

    /// The entropy-coded segments of one channel set.
    fn set_segments(&self, set: &SetPlan, band: usize, nseg: usize) -> Vec<Vec<u8>> {
        let plans = &set.bands[band].plans;
        let seg_len = self.band_len() / nseg;
        let nabits = self.nabits();
        let posbits = seg_len.trailing_zeros();
        let mut segs: Vec<Vec<u8>> = Vec::new();
        for seg in 0..nseg {
            let mut w = BitWriter::new();
            // coding choices per channel
            let mut choice_b: Vec<Code> = Vec::new();
            let mut part_a_len: Vec<usize> = Vec::new();
            for p in plans {
                let a_len = if seg == 0 { p.adapt_order } else { 0 };
                let slice = &p.residual[seg * seg_len..(seg + 1) * seg_len];
                let ub: Vec<u32> = slice[a_len..].iter().map(|v| zigzag(*v)).collect();
                choice_b.push(choose_code_b(&ub, posbits, nabits).0);
                part_a_len.push(a_len);
            }
            let reuse = false; // a reused parameter set would keep segment 0's part A length in some decoders
            if seg > 0 {
                w.put(u32::from(reuse), 1);
            }
            if !reuse {
                w.put(0, 1); // distinct parameters per channel
                for c in &choice_b {
                    // flags are written from choice B (rice vs linear applies to both parts)
                    let rice = matches!(c, Code::Rice(_) | Code::Hybrid { .. });
                    w.put(u32::from(rice), 1);
                    if rice {
                        match c {
                            Code::Hybrid { l, .. } => {
                                w.put(1, 1); // hybrid Rice coding
                                w.put(l - 1, nabits);
                            }
                            _ => w.put(0, 1),
                        }
                    }
                }
                for (i, _) in plans.iter().enumerate() {
                    let rice = matches!(choice_b[i], Code::Rice(_) | Code::Hybrid { .. });
                    if seg == 0 {
                        // part A shares the Rice/linear flag with part B
                        let code = recode(&plans[i].residual[..part_a_len[i]], rice);
                        w.put(stored_param(code, nabits, rice), nabits);
                    }
                    w.put(stored_param(choice_b[i], nabits, rice), nabits);
                }
            }
            // residual data
            for (i, p) in plans.iter().enumerate() {
                let rice = matches!(choice_b[i], Code::Rice(_) | Code::Hybrid { .. });
                let slice = &p.residual[seg * seg_len..(seg + 1) * seg_len];
                let a_len = part_a_len[i];
                let code_a = if seg == 0 { recode(&slice[..a_len], rice) } else { Code::Rice(0) };
                if seg == 0 {
                    put_values(&mut w, &slice[..a_len], code_a);
                }
                if let Code::Hybrid { k, l, c } = choice_b[i] {
                    // isolated samples: how many, where, then every sample in order (isolated ones as l-bit values)
                    let b: Vec<u32> = slice[a_len..].iter().map(|v| zigzag(*v)).collect();
                    let iso = isolated_positions(&b, c as usize);
                    w.put(iso.len() as u32, posbits);
                    for &pos in &iso {
                        w.put(pos as u32, posbits);
                    }
                    let mut next = 0usize;
                    for (j, v) in b.iter().enumerate() {
                        if next < iso.len() && iso[next] == j {
                            w.put(*v, l);
                            next += 1;
                        } else {
                            put_rice(&mut w, *v, k);
                        }
                    }
                } else {
                    put_values(&mut w, &slice[a_len..], choice_b[i]);
                }
            }
            if band == 1 && seg == 0 {
                // decimator history: the band-0 samples in front of this frame, as signed values of a common width
                let peak = set.history.iter().flatten().map(|v| v.unsigned_abs()).max().unwrap_or(0);
                let nbits = (33 - peak.leading_zeros()).max(1);
                w.put(nbits - 1, 5);
                for h in &set.history {
                    for v in h {
                        w.put((*v as u32) & if nbits >= 32 { u32::MAX } else { (1u32 << nbits) - 1 }, nbits);
                    }
                }
            }
            w.align(8);
            segs.push(w.into_bytes());
        }
        segs
    }

    fn set_header(&self, index: usize, set_plan: &SetPlan) -> Vec<u8> {
        let cfg = &self.cfg;
        let set = &cfg.sets[index];
        let nch = set.channels;
        let mut h = BitWriter::new();
        h.put(0, 10); // size placeholder, patched below
        h.put(nch as u32 - 1, 4);
        h.put(if set.residual_of_core { 0 } else { (1u32 << nch) - 1 }, nch as u32);
        h.put(cfg.bit_res - 1, 5);
        h.put(cfg.bit_res - 1, 5);
        h.put(fs_index(cfg.sample_rate), 4);
        h.put(0, 2); // no interpolation
        h.put(0, 2); // not a replacement set
        h.put(u32::from(index == 0), 1); // primary channel set
        match (&set.dmix, &set.matrix) {
            (None, Some((kind, rows))) => {
                h.put(1, 1); // downmix coefficients present
                h.put(1, 1); // embedded in the channel set header
                h.put(u32::from(*kind), 3);
                h.put(1, 1); // part of a hierarchy
                for row in rows {
                    for c in row {
                        h.put(*c as u32, 9);
                    }
                }
            }
            (Some(d), _) => {
                h.put(1, 1); // downmix coefficients present
                h.put(1, 1); // and already applied by the encoder
                h.put(1, 1); // part of a hierarchy
                for row in &d.rows {
                    h.put(row.scale_code as u32, 9);
                    for c in &row.coeff_codes {
                        h.put(*c as u32, 9);
                    }
                }
            }
            (None, None) => {
                h.put(0, 1); // no downmix coefficients
                h.put(1, 1); // part of a hierarchy
            }
        }
        h.put(1, 1); // channel mask enabled
        h.put(set.ch_mask, self.mask_bits());
        if cfg.sample_rate > 96_000 {
            h.put(0, 1); // no extra frequency bands beyond two
        }
        for (bi, sp) in set_plan.bands.iter().enumerate() {
            h.put(u32::from(sp.decor), 1);
            let order_bits = if nch <= 1 { 0 } else { 32 - (nch as u32 - 1).leading_zeros() };
            if sp.decor {
                for o in &sp.orig_order {
                    h.put(*o as u32, order_bits);
                }
                for c in &sp.coeffs {
                    if *c != 0 {
                        h.put(1, 1);
                        h.put(signed_to_raw(*c, 7), 7);
                    } else {
                        h.put(0, 1);
                    }
                }
            }
            for p in &sp.plans {
                h.put(p.adapt_order as u32, 4);
            }
            for p in &sp.plans {
                if p.adapt_order == 0 {
                    h.put(p.fixed_order as u32, 2);
                }
            }
            for p in &sp.plans {
                for i in &p.rc_idx {
                    h.put(signed_to_raw(*i, 8), 8);
                }
            }
            if bi > 0 {
                if set.dmix.is_some() {
                    h.put(0, 1); // downmix not applied in the extension band
                }
                h.put(0, 1); // no MSB/LSB split in the extension band
                h.put(0, 1); // no scalable resolution in the extension band
            }
        }
        // reserved: none; byte align; CRC
        h.align(8);
        let mut hb = h.into_bytes();
        let total = hb.len() + 2;
        hb[0] = ((total - 1) >> 2) as u8; // size - 1 in 10 bits: patch top 8 bits
        hb[1] = (hb[1] & 0x3F) | ((((total - 1) & 3) as u8) << 6);
        let crc = crc16(&hb);
        hb.extend_from_slice(&crc.to_be_bytes());
        hb
    }

    fn mask_bits(&self) -> u32 {
        let m = self.cfg.sets.iter().fold(0u32, |m, s| m | s.ch_mask);
        (32 - m.leading_zeros()).max(6)
    }

    fn assemble(&self, set_plans: &[SetPlan], set_segs: &[Vec<Vec<Vec<u8>>>], nseg: usize) -> Vec<u8> {
        let cfg = &self.cfg;
        let mut hb = Vec::new();
        for (i, sp) in set_plans.iter().enumerate() {
            hb.extend_from_slice(&self.set_header(i, sp));
        }

        // ---- NAVI: one size per segment and channel set
        let max_seg = set_segs.iter().flatten().flatten().map(|s| s.len()).max().unwrap();
        let nb4ssize = (32 - (max_seg as u32 - 1).leading_zeros()).max(1);
        let mut nv = BitWriter::new();
        for band in 0..self.nbands() {
            for seg in 0..nseg {
                for segs in set_segs {
                    nv.put(segs[band][seg].len() as u32 - 1, nb4ssize);
                }
            }
        }
        nv.align(8);
        let mut nb = nv.into_bytes();
        let crc = crc16(&nb);
        nb.extend_from_slice(&crc.to_be_bytes());

        // ---- common header (fixed layout, 14 bytes with the CRC)
        let body_len: usize = set_segs.iter().flatten().flatten().map(|s| s.len()).sum();
        let fbits = 16u32;
        let header_len = (74 + fbits as usize).div_ceil(8) + 2;
        let mut frame_len = header_len + hb.len() + nb.len() + body_len;
        frame_len += (4 - frame_len % 4) % 4;
        let mask_bits = self.mask_bits();
        let mut tmp = BitWriter::new();
        tmp.put(SYNC_XLL, 32);
        tmp.put(0, 4); // version 1
        tmp.put((header_len - 1) as u32, 8);
        tmp.put(fbits - 1, 5);
        tmp.put((frame_len - 1) as u32, fbits);
        tmp.put(cfg.sets.len() as u32 - 1, 4);
        tmp.put(nseg.trailing_zeros(), 4);
        tmp.put((self.band_len() / nseg).trailing_zeros(), 4);
        tmp.put(nb4ssize - 1, 5);
        tmp.put(0, 2); // no band CRC
        // The original sets this flag when the stream has downmix information, and then adds fields for the
        // scalable resolution of every channel to the set headers; decoders do not need either, so it stays off.
        tmp.put(0, 1);
        tmp.put(mask_bits - 1, 5);
        tmp.align(8);
        let mut cb = tmp.into_bytes();
        let crc = crc16(&cb[4..]);
        cb.extend_from_slice(&crc.to_be_bytes());
        assert_eq!(cb.len(), header_len);

        let mut out = Vec::with_capacity(frame_len);
        out.extend_from_slice(&cb);
        out.extend_from_slice(&hb);
        out.extend_from_slice(&nb);
        for band in 0..self.nbands() {
            for seg in 0..nseg {
                for segs in set_segs {
                    out.extend_from_slice(&segs[band][seg]);
                }
            }
        }
        out.resize(frame_len, 0);
        out
    }
}

pub fn fs_index(rate: u32) -> u32 {
    match rate {
        8000 => 0,
        16000 => 1,
        32000 => 2,
        64000 => 3,
        128000 => 4,
        22050 => 5,
        44100 => 6,
        88200 => 7,
        176400 => 8,
        352800 => 9,
        12000 => 10,
        24000 => 11,
        48000 => 12,
        96000 => 13,
        192000 => 14,
        384000 => 15,
        _ => 12,
    }
}

fn signed_to_raw(v: i32, bits: u32) -> u32 {
    let raw = if v >= 0 { (v as u32) << 1 } else { ((-v) as u32) * 2 - 1 };
    debug_assert!(raw < (1 << bits));
    raw
}

/// Stored value of the coding parameter field.
fn stored_param(code: Code, _nabits: u32, rice: bool) -> u32 {
    match (code, rice) {
        (Code::Rice(k), true) => k,
        (Code::Hybrid { k, .. }, _) => k,
        (Code::Linear(w), false) => w.saturating_sub(1).max(if w == 0 { 0 } else { 1 }),
        (Code::Linear(w), true) => w, // not reachable: coerced by `recode`
        (Code::Rice(k), false) => k,
    }
}

/// Re-choose a coding of the same family (Rice or linear) for another block of residuals.
fn recode(res: &[i32], rice: bool) -> Code {
    let u: Vec<u32> = res.iter().map(|v| zigzag(*v)).collect();
    if rice { Code::Rice(best_rice(&u).0) } else { Code::Linear(linear_bits(&u).0) }
}

fn put_values(w: &mut BitWriter, vals: &[i32], code: Code) {
    match code {
        Code::Linear(bits) => {
            if bits == 0 {
                return;
            }
            for v in vals {
                w.put(zigzag(*v), bits);
            }
        }
        Code::Rice(k) => {
            for v in vals {
                put_rice(w, zigzag(*v), k);
            }
        }
        Code::Hybrid { .. } => unreachable!("hybrid segments are written by the segment writer"),
    }
}

/// One Rice-coded zigzag value: the quotient in unary (zeros, then a one) and `k` raw bits.
fn put_rice(w: &mut BitWriter, u: u32, k: u32) {
    let mut q = u >> k;
    while q >= 32 {
        w.put(0, 32);
        q -= 32;
    }
    if q > 0 {
        w.put(0, q);
    }
    w.put(1, 1);
    if k > 0 {
        w.put(u & ((1u32 << k) - 1), k);
    }
}

/// Prediction residual of `x` with the decoder's arithmetic (first `order` samples stay raw).
fn predict(x: &[i32], coeff: &[i32]) -> Vec<i32> {
    let order = coeff.len();
    let n = x.len();
    let mut res = x.to_vec();
    for j in 0..n.saturating_sub(order) {
        let mut acc = 0i64;
        for k in 0..order {
            acc += x[j + k] as i64 * coeff[order - k - 1] as i64;
        }
        let xhat = clip23(norm16(acc));
        res[j + order] = x[j + order].wrapping_sub(xhat.wrapping_neg());
    }
    res
}

/// Estimated bits of a channel's residual over the whole frame in `NSEG` segments (part A of segment 0
/// holds the first `order` samples).
fn segment_cost(u: &[u32], order: usize, n: usize) -> u64 {
    let seg = n / NSEG;
    let mut bits = 0u64;
    for s in 0..NSEG {
        let sl = &u[s * seg..(s + 1) * seg];
        let a = if s == 0 { order } else { 0 };
        bits += choose_code(&sl[..a]).1 + choose_code_b(&sl[a..], seg.trailing_zeros(), 5).1;
        bits += 16; // parameters
    }
    bits
}
