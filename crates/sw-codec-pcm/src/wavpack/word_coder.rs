//! WavPack's entropy coder for lossless residuals (the "word" coder).
//!
//! Each residual is folded to a magnitude and a sign. The magnitude is placed in one of several bands whose widths
//! follow three running medians per channel; the band number `t` is sent in unary, then the offset inside the band
//! as a truncated binary number, then the sign.
//!
//! Two tricks keep quiet audio cheap, and both shape the stream:
//! * **Zero runs.** While both channels' first medians are tiny, a run of zero residuals is sent as one count.
//! * **Paired band numbers.** After a value, the decoder holds one bit of the *next* band number: whether it is
//!   zero. So a value's unary code is `2 * t + (next band is not zero)`, and a zero band costs no bits at all.
//!   That is why a value is only written once its successor has been looked at.
//!
//! The rules below mirror the decoder step by step; the bit order is least significant bit first.

use crate::le_bits::LeBitWriter;

/// Median state of one channel (three medians, see [`Medians`]).
pub(crate) type Medians = [i32; 3];

/// Unary codes of 16 and more switch to an escape: sixteen ones, then the excess as a run-style count.
const UNARY_ESCAPE: u32 = 16;

/// Where the decoder is in its pairing of band numbers just before it reads a value.
#[derive(Clone, Copy, PartialEq)]
enum Hold {
    /// Nothing held: the next band number is read in full.
    Fresh,
    /// The last value promised a band number of at least one.
    One,
    /// The last value promised a band number of zero, which is then not stored.
    Zero,
}

/// A residual that has been placed in its band but not yet written.
struct Placed {
    band: u32,
    offset: u32,
    span: u32,
    negative: bool,
    hold: Hold,
}

/// What [`encode`] returns.
pub(crate) struct Coded {
    pub bytes: Vec<u8>,
    /// Median state after the last residual (needed for the next block's start values).
    pub medians: [Medians; 2],
}

pub(crate) fn encode(residuals: &[i32], stereo: bool, medians: [Medians; 2]) -> Coded {
    let mut coder = Coder { bits: LeBitWriter::default(), medians, pending: None };
    coder.run(residuals, stereo);
    Coded { bytes: coder.bits.finish(), medians: coder.medians }
}

struct Coder {
    bits: LeBitWriter,
    medians: [Medians; 2],
    pending: Option<Placed>,
}

/// The median macros of the format: `(median >> 4) + 1` is the width of a band.
fn width(median: i32) -> u32 {
    ((median >> 4) + 1) as u32
}

fn shrink(median: &mut i32, stage: u32) {
    let unit = 128i32 >> stage;
    *median = median.wrapping_sub(((median.wrapping_add(unit) - 2) / unit).wrapping_mul(2));
}

fn grow(median: &mut i32, stage: u32) {
    let unit = 128i32 >> stage;
    *median = median.wrapping_add((median.wrapping_add(unit) / unit).wrapping_mul(5));
}

impl Coder {
    fn run(&mut self, residuals: &[i32], stereo: bool) {
        let channels = if stereo { 2 } else { 1 };
        let mut at = 0;
        while at < residuals.len() {
            if self.pending.is_none() && self.runs_allowed() {
                let zeros = residuals[at..].iter().take_while(|&&r| r == 0).count();
                self.put_count(zeros as u32);
                if zeros > 0 {
                    self.medians = [[0; 3]; 2];
                    at += zeros;
                    if at >= residuals.len() {
                        break;
                    }
                }
            }
            let value = self.place(at % channels, residuals[at]);
            self.accept(value);
            at += 1;
        }
        if let Some(last) = self.pending.take() {
            self.write(&last, false);
        }
    }

    /// Zero runs are only used while both channels are very quiet (a mono block has a silent second channel).
    fn runs_allowed(&self) -> bool {
        self.medians[0][0] < 2 && self.medians[1][0] < 2
    }

    /// Finds the band and offset of a residual and moves the medians, as the decoder will.
    fn place(&mut self, channel: usize, residual: i32) -> Placed {
        let negative = residual < 0;
        let magnitude = if negative { !residual } else { residual } as u32;
        let m = &mut self.medians[channel];

        let first = width(m[0]);
        let (band, base, span) = if magnitude < first {
            let span = first - 1;
            shrink(&mut m[0], 0);
            (0, 0, span)
        } else {
            let mut base = first;
            grow(&mut m[0], 0);
            let second = width(m[1]);
            if magnitude - base < second {
                let span = second - 1;
                shrink(&mut m[1], 1);
                (1, base, span)
            } else {
                base += second;
                grow(&mut m[1], 1);
                let third = width(m[2]);
                let band = 2 + (magnitude - base) / third;
                base += (band - 2) * third;
                if band == 2 {
                    shrink(&mut m[2], 2);
                } else {
                    grow(&mut m[2], 2);
                }
                (band, base, third - 1)
            }
        };
        Placed { band, offset: magnitude - base, span, negative, hold: Hold::Fresh }
    }

    /// Takes the next value in line. The one before it can now be written, because its code depends on this band.
    fn accept(&mut self, mut value: Placed) {
        match self.pending.take() {
            None => {
                self.pending = Some(value);
            }
            Some(previous) => {
                let next_is_nonzero = value.band >= 1;
                self.write(&previous, next_is_nonzero);
                if next_is_nonzero {
                    value.hold = Hold::One;
                    self.pending = Some(value);
                } else {
                    // a zero band is promised, so it needs no code and can be written straight away
                    value.hold = Hold::Zero;
                    self.write(&value, false);
                }
            }
        }
    }

    /// Writes a value: band number (unless held), offset inside the band, sign.
    fn write(&mut self, value: &Placed, next_is_nonzero: bool) {
        match value.hold {
            Hold::Zero => debug_assert_eq!(value.band, 0),
            Hold::Fresh => self.put_unary(value.band * 2 + next_is_nonzero as u32),
            Hold::One => self.put_unary((value.band - 1) * 2 + next_is_nonzero as u32),
        }
        self.put_offset(value.offset, value.span);
        self.bits.put(value.negative as u32, 1);
    }

    /// Unary `count` (ones, then a zero), with the escape for 16 and above.
    fn put_unary(&mut self, count: u32) {
        if count < UNARY_ESCAPE {
            self.bits.put_ones(count);
            self.bits.put(0, 1);
        } else {
            self.bits.put_ones(UNARY_ESCAPE);
            self.bits.put(0, 1);
            self.put_count(count - UNARY_ESCAPE);
        }
    }

    /// A count that is usually small: 0 and 1 are unary, larger ones are an Elias-gamma style number.
    fn put_count(&mut self, count: u32) {
        if count < 2 {
            self.bits.put_ones(count);
            self.bits.put(0, 1);
        } else {
            let size = 32 - count.leading_zeros();
            self.bits.put_ones(size);
            self.bits.put(0, 1);
            self.bits.put(count & ((1 << (size - 1)) - 1), size - 1);
        }
    }

    /// `offset` in `0..=span` as a truncated binary number (short codes for the smaller offsets).
    fn put_offset(&mut self, offset: u32, span: u32) {
        if span == 0 {
            return;
        }
        let bits = 31 - span.leading_zeros();
        let short = (2u32 << bits) - span - 1;
        if offset < short {
            self.bits.put(offset, bits);
        } else {
            let shifted = offset + short;
            self.bits.put(shifted >> 1, bits);
            self.bits.put(shifted & 1, 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_is_one_count() {
        let coded = encode(&vec![0; 10_000], true, [[0; 3]; 2]);
        assert!(coded.bytes.len() <= 4, "{} bytes", coded.bytes.len());
    }

    #[test]
    fn louder_costs_more() {
        let wave = |amp: i32| -> Vec<i32> { (0..4000).map(|n| ((n as f64 * 0.3).sin() * amp as f64) as i32).collect() };
        let quiet = encode(&wave(50), false, [[0; 3]; 2]).bytes.len();
        let loud = encode(&wave(5000), false, [[0; 3]; 2]).bytes.len();
        assert!(quiet < loud);
    }

    #[test]
    fn offsets_are_truncated_binary() {
        // span 4 has five offsets: the first three take 2 bits, the last two take 3
        let lengths: Vec<usize> = (0..5)
            .map(|offset| {
                let mut c = Coder { bits: LeBitWriter::default(), medians: [[0; 3]; 2], pending: None };
                c.put_offset(offset, 4);
                c.bits.bit_len()
            })
            .collect();
        assert_eq!(lengths, [2, 2, 2, 3, 3]);
    }

    #[test]
    fn counts_grow_as_gamma_codes() {
        let bits_for = |count: u32| {
            let mut c = Coder { bits: LeBitWriter::default(), medians: [[0; 3]; 2], pending: None };
            c.put_count(count);
            c.bits.finish().len()
        };
        assert_eq!(bits_for(0), 1);
        assert_eq!(bits_for(1), 1);
        assert!(bits_for(1_000_000) > bits_for(100));
    }
}
