//! Adaptive Golomb-Rice coding of the prediction error, with a run-length code for zeros.

use crate::alac::{HISTORY_MULT, INITIAL_HISTORY, K_MODIFIER};
use crate::bitwriter::BitWriter;

/// Floor of log2 for positive numbers (0 gives 0).
pub(crate) fn log2_floor(v: u32) -> u32 {
    if v == 0 {
        0
    } else {
        31 - v.leading_zeros()
    }
}

/// One value with the adaptive Golomb code: a unary quotient (of the divisor 2^k - 1), then the remainder; a quotient
/// above 8 is replaced by nine ones and the value itself in `escape_bits` bits.
pub(crate) fn put_scalar(w: &mut BitWriter, value: u32, k: u32, escape_bits: u32) {
    let k = k.min(K_MODIFIER);
    let divisor = (1u32 << k) - 1;
    let quotient = value / divisor;
    let remainder = value % divisor;
    if quotient > 8 {
        w.put(0x1FF, 9);
        w.put(value, escape_bits);
        return;
    }
    if quotient > 0 {
        w.put(((1u64 << quotient) - 1) as u32, quotient);
    }
    w.put(0, 1);
    if k != 1 {
        if remainder > 0 {
            w.put((remainder as u64 + 1) as u32, k);
        } else {
            w.put(0, k - 1);
        }
    }
}

/// Codes the prediction residual of one channel. `escape_bits` is the width of an escaped value.
pub(crate) fn entropy_code(w: &mut BitWriter, residual: &[i32], escape_bits: u32) {
    let n = residual.len();
    let mut history = INITIAL_HISTORY;
    let mut sign_modifier = 0u32;
    let mut i = 0usize;
    while i < n {
        let k = log2_floor((history >> 9) + 3);
        // zigzag: 0, -1, 1, -2 ... becomes 0, 1, 2, 3 ... as 2|v| (positive) or 2|v| - 1 (negative)
        let x = {
            let v = 2i32.wrapping_mul(residual[i]).wrapping_neg().wrapping_sub(1);
            (v ^ (v >> 31)) as u32
        };
        i += 1;
        put_scalar(w, x - sign_modifier, k, escape_bits);
        history = history
            .wrapping_add(x.wrapping_mul(HISTORY_MULT))
            .wrapping_sub(history.wrapping_mul(HISTORY_MULT) >> 9);
        sign_modifier = 0;
        if x > 0xFFFF {
            history = 0xFFFF;
        }
        if history < 128 && i < n {
            // the average has collapsed: send the length of the run of zeros that follows
            let k = (7 + ((history + 16) >> 6)).saturating_sub(log2_floor(history));
            let mut run = 0u32;
            while i < n && residual[i] == 0 {
                i += 1;
                run += 1;
            }
            put_scalar(w, run, k, 16);
            sign_modifier = u32::from(run <= 0xFFFF);
            history = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitwriter::BitWriter;

    #[test]
    fn scalar_codes_match_the_published_examples() {
        // k = 1: the divisor is 1, so the code is the unary quotient and nothing else
        let mut w = BitWriter::default();
        put_scalar(&mut w, 3, 1, 16);
        w.align(8);
        assert_eq!(w.bytes(), vec![0b1110_0000]);
        // k = 3: divisor 7; 9 = 1 * 7 + 2 -> unary 10, then (2 + 1) in 3 bits = 011
        let mut w = BitWriter::default();
        put_scalar(&mut w, 9, 3, 16);
        w.align(8);
        assert_eq!(w.bytes(), vec![0b1001_1000]);
        // a zero remainder is k - 1 zero bits
        let mut w = BitWriter::default();
        put_scalar(&mut w, 7, 3, 16);
        w.align(8);
        assert_eq!(w.bytes(), vec![0b1000_0000]);
    }
}
