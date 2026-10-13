//! The range coder behind Monkey's Audio's entropy stage.
//!
//! It is a carry-propagating range coder over a 31-bit window: the interval (`low`, `range`) shrinks with every
//! symbol, and whenever `range` falls to 2^23 or less the top byte of the window is shifted out. A carry out of the
//! window is added to the byte already waiting (and turns a run of waiting 0xFF bytes into zeros).
//!
//! The decoder skips the first byte of a stream and reads the code from the second one on. The first byte this
//! encoder emits is the empty "waiting byte" it starts with, which gives exactly that layout.

/// The window is 31 bits; `range` starts at its top.
const TOP: u32 = 1 << 31;

/// Shift a byte out whenever the range is at most this.
const BOTTOM: u32 = TOP >> 8;

/// Bit position of the top byte of the window (the bit above it is the carry).
const BYTE_SHIFT: u32 = 23;

pub(crate) struct RangeEncoder {
    low: u64,
    range: u32,
    /// The byte that may still receive a carry.
    waiting: u8,
    /// How many 0xFF bytes follow it (they would turn to zero on a carry).
    ff_run: u64,
    out: Vec<u8>,
}

impl RangeEncoder {
    pub fn new() -> RangeEncoder {
        RangeEncoder {
            low: 0,
            range: TOP,
            waiting: 0,
            ff_run: 0,
            out: Vec::new(),
        }
    }

    /// Codes the interval `[start, start + size)` out of `total` equal parts of the current range.
    pub fn encode(&mut self, start: u32, size: u32, total: u32) {
        self.normalize();
        let unit = self.range / total;
        self.low += unit as u64 * start as u64;
        self.range = unit * size;
    }

    /// Codes `value` as `bits` plain bits (`bits` up to 16).
    pub fn encode_bits(&mut self, value: u32, bits: u32) {
        debug_assert!(bits <= 16 && value < (1 << bits));
        self.normalize();
        let unit = self.range >> bits;
        self.low += unit as u64 * value as u64;
        self.range = unit;
    }

    fn normalize(&mut self) {
        while self.range <= BOTTOM {
            self.shift_out();
            self.range <<= 8;
        }
    }

    /// Moves the top byte of the window into the output (or onto the waiting run, if it might still change).
    fn shift_out(&mut self) {
        let carry = (self.low >> 31) as u8 & 1;
        if self.low < 0xFFu64 << BYTE_SHIFT || carry != 0 {
            self.out.push(self.waiting.wrapping_add(carry));
            let filler = 0xFFu8.wrapping_add(carry);
            self.out.extend(std::iter::repeat_n(filler, self.ff_run as usize));
            self.ff_run = 0;
            self.waiting = (self.low >> BYTE_SHIFT) as u8;
        } else {
            self.ff_run += 1;
        }
        self.low = (self.low & ((1 << BYTE_SHIFT) - 1)) << 8;
    }

    /// Flushes the window and returns the stream.
    pub fn finish(mut self) -> Vec<u8> {
        for _ in 0..5 {
            self.shift_out();
        }
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decoder side, written from the format's description, to check the encoder against.
    struct Decoder<'a> {
        bytes: &'a [u8],
        at: usize,
        low: u32,
        range: u32,
        buffer: u32,
        unit: u32,
    }

    impl<'a> Decoder<'a> {
        fn new(bytes: &'a [u8]) -> Decoder<'a> {
            // the first byte is skipped, the second starts the code
            let buffer = bytes[1] as u32;
            Decoder {
                bytes,
                at: 2,
                low: buffer >> 1,
                range: 1 << 7,
                buffer,
                unit: 0,
            }
        }

        fn normalize(&mut self) {
            while self.range <= BOTTOM {
                self.buffer = (self.buffer << 8) | *self.bytes.get(self.at).unwrap_or(&0) as u32;
                self.at += 1;
                self.low = (self.low << 8) | ((self.buffer >> 1) & 0xFF);
                self.range <<= 8;
            }
        }

        fn target(&mut self, total: u32) -> u32 {
            self.normalize();
            self.unit = self.range / total;
            self.low / self.unit
        }

        fn take(&mut self, start: u32, size: u32) {
            self.low -= self.unit * start;
            self.range = self.unit * size;
        }
    }

    #[test]
    fn symbols_come_back() {
        let mut seed = 12345u32;
        let mut next = move |n: u32| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) % n
        };
        let symbols: Vec<(u32, u32)> = (0..5000).map(|_| (next(1000), 1 + next(900))).collect();
        let mut enc = RangeEncoder::new();
        for &(v, total) in &symbols {
            let v = v % total;
            enc.encode(v, 1, total);
            enc.encode_bits(v & 0xFF, 8);
        }
        let bytes = enc.finish();
        let mut dec = Decoder::new(&bytes);
        for &(v, total) in &symbols {
            let v = v % total;
            let got = dec.target(total);
            dec.take(got, 1);
            assert_eq!(got, v);
            let bits = dec.target(256);
            dec.take(bits, 1);
            assert_eq!(bits, v & 0xFF);
        }
    }
}
