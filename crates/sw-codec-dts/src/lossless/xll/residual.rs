//! Entropy coding of the residual in segments: Rice, linear and hybrid Rice codes, how many bits each takes and
//! how they are written.

use crate::bits::BitWriter;
use crate::lossless::xll::NSEG;

pub(crate) fn zigzag(v: i32) -> u32 {
    if v >= 0 {
        (v as u32) << 1
    } else {
        (((-(v as i64)) as u32) << 1) - 1
    }
}

/// Rice parameter minimising the code length of `u`, and that length in bits.
pub(crate) fn best_rice(u: &[u32]) -> (u32, u64) {
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
pub(crate) fn linear_bits(u: &[u32]) -> (u32, u64) {
    let max = u.iter().copied().max().unwrap_or(0);
    if max == 0 {
        return (0, 0);
    }
    let w = (32 - max.leading_zeros()).max(2);
    (w, w as u64 * u.len() as u64)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Code {
    Rice(u32),
    Linear(u32),
    /// Rice code with parameter `k` for most samples; the `c` largest (isolated) samples are sent as `l`-bit
    /// values at listed positions instead of long unary runs.
    Hybrid {
        k: u32,
        l: u32,
        c: u32,
    },
}

pub(crate) fn choose_code(u: &[u32]) -> (Code, u64) {
    let (k, rb) = best_rice(u);
    let (w, lb) = linear_bits(u);
    // linear data need no unary part; compare including the larger cost of an all-zero block
    if lb <= rb {
        (Code::Linear(w), lb)
    } else {
        (Code::Rice(k), rb)
    }
}

/// Hybrid coding is only tried for a segment whose largest value is at least this many times its mean (output
/// is identical to trying it always on the 192 kHz test material, 14% faster).
pub(crate) const HYBRID_GATE: f64 = 8.0;

/// Indices of the `c` largest values (ties: lowest index first), in ascending index order.
pub(crate) fn isolated_positions(u: &[u32], c: usize) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..u.len()).collect();
    idx.sort_by(|&a, &b| u[b].cmp(&u[a]).then(a.cmp(&b)));
    idx.truncate(c);
    idx.sort_unstable();
    idx
}

/// Hybrid Rice coding of part B of a segment: the best (k, l, count) and its length in bits, including what it adds
/// to the segment header (the hybrid flag, the length field of `nabits` bits, the count of `posbits` bits).
pub(crate) fn hybrid_code(u: &[u32], posbits: u32, nabits: u32) -> Option<(Code, u64)> {
    let n = u.len();
    if n < 16 {
        return None;
    }
    // outliers make hybrid coding pay; without a value well above the typical one it cannot
    let (mut mx, mut sum) = (0u32, 0u64);
    for v in u {
        mx = mx.max(*v);
        sum += *v as u64;
    }
    if (mx as f64) < HYBRID_GATE * (sum as f64 / n as f64 + 1.0) {
        return None;
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
    order.select_nth_unstable_by(top.saturating_sub(1).min(n - 1), |&a, &b| {
        u[b].cmp(&u[a]).then(a.cmp(&b))
    });
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
            if best.is_none_or(|b| bits < b.1) {
                best = Some((Code::Hybrid { k, l, c: c as u32 }, bits));
            }
        }
    }
    best
}

/// Choice of coding for part B of a segment (Rice, linear or hybrid Rice).
pub(crate) fn choose_code_b(u: &[u32], posbits: u32, nabits: u32) -> (Code, u64) {
    let plain = choose_code(u);
    match hybrid_code(u, posbits, nabits) {
        Some(h) if h.1 < plain.1 => h,
        _ => plain,
    }
}

pub(crate) fn signed_to_raw(v: i32, bits: u32) -> u32 {
    let raw = if v >= 0 { (v as u32) << 1 } else { ((-v) as u32) * 2 - 1 };
    debug_assert!(raw < (1 << bits));
    raw
}

/// Stored value of the coding parameter field.
pub(crate) fn stored_param(code: Code, _nabits: u32, rice: bool) -> u32 {
    match (code, rice) {
        (Code::Rice(k), true) => k,
        (Code::Hybrid { k, .. }, _) => k,
        (Code::Linear(w), false) => w.saturating_sub(1).max(if w == 0 { 0 } else { 1 }),
        (Code::Linear(w), true) => w, // not reachable: coerced by `recode`
        (Code::Rice(k), false) => k,
    }
}

/// Re-choose a coding of the same family (Rice or linear) for another block of residuals.
pub(crate) fn recode(res: &[i32], rice: bool) -> Code {
    let u: Vec<u32> = res.iter().map(|v| zigzag(*v)).collect();
    if rice {
        Code::Rice(best_rice(&u).0)
    } else {
        Code::Linear(linear_bits(&u).0)
    }
}

pub(crate) fn put_values(w: &mut BitWriter, vals: &[i32], code: Code) {
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
pub(crate) fn put_rice(w: &mut BitWriter, u: u32, k: u32) {
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

/// Estimated bits of a channel's residual over the whole frame in `NSEG` segments (part A of segment 0
/// holds the first `order` samples).
pub(crate) fn segment_cost(u: &[u32], order: usize, n: usize) -> u64 {
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
