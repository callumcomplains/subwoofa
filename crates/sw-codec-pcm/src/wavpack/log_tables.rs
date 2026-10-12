//! WavPack stores several numbers (entropy medians, the history of each predictor) as 16-bit logarithms with eight
//! fractional bits, which the decoder turns back into integers. The two lookup tables behind that are plain math:
//!
//! * `EXP[i] = round(256 * (2^(i/256) - 1))`
//! * `LOG[i] = round(256 * log2(1 + i/256))`
//!
//! The encoder has to round its own state the same way, so that its idea of the starting values equals the decoder's.

use std::sync::OnceLock;

struct Tables {
    exp: [u8; 256],
    log: [u8; 256],
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut t = Tables { exp: [0; 256], log: [0; 256] };
        for i in 0..256 {
            let x = i as f64 / 256.0;
            t.exp[i] = (256.0 * (2f64.powf(x) - 1.0)).round() as u8;
            t.log[i] = (256.0 * (1.0 + x).log2()).round() as u8;
        }
        t
    })
}

/// The logarithm of a non-negative number: `256 * log2(value)`, roughly.
pub(crate) fn log2(value: u32) -> i32 {
    match value {
        0 => 0,
        1 => 256,
        _ => {
            let value = value + (value >> 9);
            let bits = 32 - value.leading_zeros() as i32;
            let index = if bits < 9 { value << (9 - bits) } else { value >> (bits - 9) };
            (bits << 8) + tables().log[(index & 0xFF) as usize] as i32
        }
    }
}

/// The number a logarithm stands for (the inverse of [`log2`], for any sign).
pub(crate) fn exp2(log: i32) -> i32 {
    let negative = log < 0;
    let log = log.abs();
    let mantissa = tables().exp[(log & 0xFF) as usize] as i32 | 0x100;
    let shift = log >> 8;
    let value = if shift > 9 { mantissa << (shift - 9) } else { mantissa >> (9 - shift) };
    if negative {
        -value
    } else {
        value
    }
}

/// [`log2`] of a signed number, keeping the sign.
pub(crate) fn log2_signed(value: i32) -> i32 {
    if value < 0 {
        -log2(value.unsigned_abs())
    } else {
        log2(value as u32)
    }
}

/// What the decoder will make of `value` after it has been stored as a logarithm.
pub(crate) fn rounded(value: i32) -> i32 {
    exp2(log2_signed(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_match_the_format() {
        // sums taken from the tables in the WavPack reference decoder
        let t = tables();
        assert_eq!(t.exp.iter().map(|&v| v as u32).sum::<u32>(), 28_882);
        assert_eq!(t.log.iter().map(|&v| v as u32).sum::<u32>(), 36_401);
    }

    #[test]
    fn round_trip_is_close() {
        for value in [1, 7, 100, 4096, 70_000, 8_000_000] {
            let back = rounded(value);
            assert!((back - value).abs() * 100 <= value + 100, "{value} came back as {back}");
            assert_eq!(rounded(-value), -back);
        }
        assert_eq!(rounded(0), 0);
    }
}
