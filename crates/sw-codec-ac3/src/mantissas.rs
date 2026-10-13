//! Mantissas: quantizing a coefficient to its `bap` and writing a block's mantissas, with the grouped
//! quantizers (bap 1, 2 and 4) packed several to a word.

use crate::bits::BitWriter;

/// Mantissa bits per bap for the ungrouped quantizers (bap 1, 2 and 4 are grouped).
pub(crate) const BAP_BITS: [u32; 16] = [0, 0, 0, 3, 0, 4, 5, 6, 7, 8, 9, 10, 11, 12, 14, 16];

pub(crate) const GROUPED_LEVELS: [i32; 6] = [0, 3, 5, 7, 11, 15];

/// Quantize a mantissa in (-1, 1) for the given bap. Grouped/symmetric quantizers (bap 1-5)
/// return an unsigned level index; bap >= 6 return a two's-complement integer.
pub(crate) fn quantize(bap: usize, m: f32) -> i32 {
    if bap <= 5 {
        let l = GROUPED_LEVELS[bap];
        let half = (l - 1) / 2;
        ((m * l as f32 / 2.0).round() as i32).clamp(-half, half) + half
    } else {
        let nbits = BAP_BITS[bap];
        let scale = (1i64 << (nbits - 1)) as f32;
        ((m * scale).round() as i64).clamp(-(1 << (nbits - 1)), (1 << (nbits - 1)) - 1) as i32
    }
}

/// Write one block's mantissas. Groups of bap 1 (3 values, 5 bits), bap 2 (3 values, 7 bits) and
/// bap 4 (2 values, 7 bits) are emitted as one word at the position of the group's first value.
pub(crate) fn write_mantissas(w: &mut BitWriter, items: &[(usize, i32)]) {
    let mut codes: [Vec<i32>; 5] = Default::default();
    for &(b, c) in items {
        if matches!(b, 1 | 2 | 4) {
            codes[b].push(c);
        }
    }
    let mut occ = [0usize; 5];
    for &(b, c) in items {
        match b {
            1 | 2 | 4 => {
                let (size, mid, bits) = match b {
                    1 => (3, 1, 5),
                    2 => (3, 2, 7),
                    _ => (2, 5, 7),
                };
                if occ[b] % size == 0 {
                    let g = &codes[b];
                    let at = |k: usize| g.get(occ[b] + k).copied().unwrap_or(mid);
                    let word = match b {
                        1 => 9 * at(0) + 3 * at(1) + at(2),
                        2 => 25 * at(0) + 5 * at(1) + at(2),
                        _ => 11 * at(0) + at(1),
                    };
                    w.put(word as u32, bits);
                }
                occ[b] += 1;
            }
            3 => w.put(c as u32, 3),
            5 => w.put(c as u32, 4),
            _ => {
                let nbits = BAP_BITS[b];
                w.put((c as i64 & ((1i64 << nbits) - 1)) as u32, nbits);
            }
        }
    }
}
