//! Turning prediction residuals into range-coded symbols (Monkey's Audio 3.99).
//!
//! A residual is folded to a non-negative number `x` (positive `v` becomes `2v - 1`, the rest `-2v`). It is split
//! around a *pivot* that follows a running average of recent values: the number of whole pivots goes through a
//! fixed probability model (small numbers are likely), the remainder is sent as a plain uniform number.

use super::range_coder::RangeEncoder;

/// The model has 64 symbols; the last one means "the real count follows as 32 raw bits".
const MODEL_SYMBOLS: u32 = 64;

/// Start of each probability interval out of 65536, for the 21 likeliest counts (Monkey's Audio 3.98 model).
const STARTS: [u16; 22] = [
    0, 19578, 36160, 48417, 56323, 60899, 63265, 64435, 64971, 65232, 65351, 65416, 65447, 65466, 65476, 65482, 65485,
    65488, 65490, 65491, 65492, 65493,
];

/// Width of each of those intervals.
const WIDTHS: [u16; 21] = [19578, 16582, 12257, 7906, 4576, 2366, 1170, 536, 261, 119, 65, 31, 19, 10, 6, 3, 3, 2, 1, 1, 1];

/// Counts of 21 and above share the last 43 slots of 65536, one slot each.
const FIRST_TAIL_SLOT: u32 = 65493 - 21;

/// Start value of the running sum: a typical level of 2^10.
const SUM_START: u32 = (1 << 10) * 16;

/// Largest pivot that fits the uniform coder in one go (it codes at most 16 bits).
const PIVOT_LIMIT: u32 = 1 << 16;

/// One channel's running sum of recent magnitudes.
pub(crate) struct ValueCoder {
    sum: u32,
}

impl ValueCoder {
    pub fn new() -> ValueCoder {
        ValueCoder { sum: SUM_START }
    }

    pub fn encode(&mut self, enc: &mut RangeEncoder, residual: i32) {
        let x = fold(residual);
        let pivot = (self.sum >> 5).max(1);
        let (count, remainder) = (x / pivot, x % pivot);

        put_count(enc, count);
        put_remainder(enc, remainder, pivot);

        // follow the signal: add this value, forget a 32nd of the old sum
        self.sum = self.sum.wrapping_add(x.div_ceil(2).wrapping_sub((self.sum + 16) >> 5));
    }
}

/// Positive `v` becomes the odd number `2v - 1`, zero and negative the even number `-2v`.
fn fold(residual: i32) -> u32 {
    if residual > 0 {
        (residual as u32) * 2 - 1
    } else {
        residual.unsigned_abs() * 2
    }
}

fn put_count(enc: &mut RangeEncoder, count: u32) {
    if count >= MODEL_SYMBOLS - 1 {
        put_symbol(enc, MODEL_SYMBOLS - 1);
        enc.encode_bits(count >> 16, 16);
        enc.encode_bits(count & 0xFFFF, 16);
    } else {
        put_symbol(enc, count);
    }
}

fn put_symbol(enc: &mut RangeEncoder, symbol: u32) {
    if symbol < 21 {
        enc.encode(STARTS[symbol as usize] as u32, WIDTHS[symbol as usize] as u32, 1 << 16);
    } else {
        enc.encode(symbol + FIRST_TAIL_SLOT, 1, 1 << 16);
    }
}

/// A number below `pivot`, uniformly; pivots of 65536 and more are split into a high and a low part.
fn put_remainder(enc: &mut RangeEncoder, remainder: u32, pivot: u32) {
    if pivot < PIVOT_LIMIT {
        enc.encode(remainder, 1, pivot);
    } else {
        let extra_bits = 32 - (pivot >> 16).leading_zeros();
        let high_total = (pivot >> extra_bits) + 1;
        enc.encode(remainder >> extra_bits, 1, high_total);
        enc.encode(remainder & ((1 << extra_bits) - 1), 1, 1 << extra_bits);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding_alternates_signs() {
        let folded: Vec<u32> = [0, 1, -1, 2, -2].iter().map(|&v| fold(v)).collect();
        assert_eq!(folded, [0, 1, 2, 3, 4]);
    }

    #[test]
    fn model_tables_are_consistent() {
        for i in 0..21 {
            assert_eq!(STARTS[i] as u32 + WIDTHS[i] as u32, STARTS[i + 1] as u32);
        }
    }

    #[test]
    fn quiet_values_are_cheap() {
        let size = |v: i32| {
            let mut enc = RangeEncoder::new();
            let mut coder = ValueCoder::new();
            for _ in 0..4000 {
                coder.encode(&mut enc, v);
            }
            enc.finish().len()
        };
        assert!(size(1) < size(3000));
    }
}
