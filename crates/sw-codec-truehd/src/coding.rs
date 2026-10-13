//! Entropy coding of a block of residuals: the Huffman codebooks, how many low bits are sent raw, and the choice
//! for every block of an access unit by shortest path (switching codebook costs bits).

use crate::tables::HUFFMAN;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Coding {
    pub(crate) codebook: u8,
    /// raw bits per sample
    pub(crate) lsb_bits: u32,
    pub(crate) bits: usize,
}

/// Value of `huff_offset`-corrected symbol input for sample `v` under `(codebook, lsb_bits)`.
/// Returns the non-negative number to split into (symbol, raw bits).
#[inline]
pub(crate) fn shifted(v: i32, codebook: u8, l: u32) -> i64 {
    let sign_shift: i32 = l as i32 + if codebook != 0 { 2 - codebook as i32 } else { -1 };
    let mut off = 0i64;
    if codebook > 0 {
        off -= 7i64 << l;
    }
    if sign_shift >= 0 {
        off -= 1i64 << sign_shift;
    }
    v as i64 - off
}

pub(crate) fn cost_of(v: &[i32], codebook: u8, l: u32) -> Option<usize> {
    let mut bits = 0usize;
    for &s in v {
        let y = shifted(s, codebook, l);
        if y < 0 {
            return None;
        }
        if codebook == 0 {
            if (y >> l) != 0 {
                return None;
            }
            bits += l as usize;
        } else {
            let sym = (y >> l) as usize;
            if sym >= 18 {
                return None;
            }
            let len = HUFFMAN[codebook as usize - 1][sym].1;
            if len == 0 {
                return None;
            }
            bits += len as usize + l as usize;
        }
    }
    Some(bits)
}

/// Pick the cheapest (codebook, lsb_bits) for a block of already-shifted residual values.
pub(crate) fn best_coding(v: &[i32], quant: u8) -> Coding {
    let max_l = 24u32.saturating_sub(quant as u32).min(24);
    let peak = v.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    // raw coding: smallest l that holds every value
    let mut best: Option<Coding> = None;
    if peak == 0 {
        return Coding {
            codebook: 0,
            lsb_bits: 0,
            bits: 0,
        };
    }
    let need = 32 - peak.leading_zeros(); // bits for the magnitude
    for l in need..=(need + 1).min(31) {
        if let Some(b) = cost_of(v, 0, l) {
            best = Some(Coding {
                codebook: 0,
                lsb_bits: l,
                bits: b,
            });
            break;
        }
    }
    let mean = v.iter().map(|s| s.unsigned_abs() as u64).sum::<u64>() / v.len().max(1) as u64;
    let l0 = (64 - mean.leading_zeros()) as i32; // about log2(mean)+1
    for l in (l0 - 3).max(0)..=(l0 + 1) {
        let l = l as u32;
        if l > max_l {
            continue;
        }
        for cb in 1..=3u8 {
            if let Some(b) = cost_of(v, cb, l) {
                if best.is_none_or(|c| b < c.bits) {
                    best = Some(Coding {
                        codebook: cb,
                        lsb_bits: l,
                        bits: b,
                    });
                }
            }
        }
    }
    best.unwrap_or(Coding {
        codebook: 0,
        lsb_bits: (need + 1).min(31),
        bits: v.len() * (need as usize + 1),
    })
}

/// Bits a parameter update costs for one channel whose codebook or raw-bit count changes (change flag, three
/// sub-flags, codebook, raw bits); the block-level flags are shared and not counted.
pub(crate) const CODING_SWITCH_BITS: usize = 10;

/// Every feasible (codebook, raw bits) near the best choice for a block, with its exact bit count.
pub(crate) fn coding_candidates(v: &[i32], quant: u8) -> Vec<Coding> {
    let peak = v.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    if peak == 0 {
        return vec![Coding {
            codebook: 0,
            lsb_bits: 0,
            bits: 0,
        }];
    }
    let max_l = 24u32.saturating_sub(quant as u32).min(24);
    let mut out: Vec<Coding> = Vec::new();
    let need = 32 - peak.leading_zeros();
    for l in need..=(need + 1).min(31) {
        if let Some(b) = cost_of(v, 0, l) {
            out.push(Coding {
                codebook: 0,
                lsb_bits: l,
                bits: b,
            });
            break;
        }
    }
    let mean = v.iter().map(|s| s.unsigned_abs() as u64).sum::<u64>() / v.len().max(1) as u64;
    let l0 = (64 - mean.leading_zeros()) as i32;
    for l in (l0 - 3).max(0)..=(l0 + 2) {
        let l = l as u32;
        if l > max_l {
            continue;
        }
        for cb in 1..=3u8 {
            if let Some(b) = cost_of(v, cb, l) {
                out.push(Coding {
                    codebook: cb,
                    lsb_bits: l,
                    bits: b,
                });
            }
        }
    }
    if out.is_empty() {
        out.push(best_coding(v, quant));
    }
    // keep the useful ones: within a few switch costs of the best
    let best = out.iter().map(|c| c.bits).min().unwrap_or(0);
    out.retain(|c| c.bits <= best + 4 * CODING_SWITCH_BITS);
    out
}

/// Choose one coding per block so that bits plus parameter updates are smallest: a shortest path through the
/// candidates of the blocks, where changing the codebook or raw-bit count from one block to the next costs an update.
/// `start` is the coding the decoder holds before the first block.
pub(crate) fn choose_codings(cands: &[Vec<Coding>], start: Coding) -> Vec<Coding> {
    let same = |a: &Coding, b: &Coding| a.codebook == b.codebook && a.lsb_bits == b.lsb_bits;
    let n = cands.len();
    let mut cost: Vec<Vec<usize>> = Vec::with_capacity(n);
    let mut from: Vec<Vec<usize>> = Vec::with_capacity(n);
    for (i, cs) in cands.iter().enumerate() {
        let mut ci = Vec::with_capacity(cs.len());
        let mut fi = Vec::with_capacity(cs.len());
        for c in cs {
            if i == 0 {
                ci.push(c.bits + if same(c, &start) { 0 } else { CODING_SWITCH_BITS });
                fi.push(0);
            } else {
                let (mut best, mut arg) = (usize::MAX, 0);
                for (j, p) in cands[i - 1].iter().enumerate() {
                    let t = cost[i - 1][j] + if same(p, c) { 0 } else { CODING_SWITCH_BITS };
                    if t < best {
                        best = t;
                        arg = j;
                    }
                }
                ci.push(best + c.bits);
                fi.push(arg);
            }
        }
        cost.push(ci);
        from.push(fi);
    }
    let mut out = vec![start; n];
    if n == 0 {
        return out;
    }
    let mut k = (0..cands[n - 1].len()).min_by_key(|&j| cost[n - 1][j]).unwrap_or(0);
    for i in (0..n).rev() {
        out[i] = cands[i][k];
        k = from[i][k];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_values_prefer_a_huffman_codebook_to_raw_coding() {
        let v: Vec<i32> = (0..40).map(|i| ((i * 7) % 5) - 2).collect();
        let c = best_coding(&v, 0);
        assert!(c.bits < 40 * 4, "{c:?}");
    }

    #[test]
    fn digital_silence_costs_nothing() {
        assert_eq!(best_coding(&[0; 40], 0).bits, 0);
    }
}
