//! Writing bits into a byte vector, least significant bit first. TTA and WavPack both pack their streams this way.

#[derive(Default)]
pub(crate) struct LeBitWriter {
    bytes: Vec<u8>,
    pending: u64,
    pending_bits: u32,
}

impl LeBitWriter {
    /// Appends the low `count` bits of `value` (`count` up to 32).
    pub fn put(&mut self, value: u32, count: u32) {
        debug_assert!(count <= 32);
        if count == 0 {
            return;
        }
        self.pending |= ((value as u64) & ((1u64 << count) - 1)) << self.pending_bits;
        self.pending_bits += count;
        while self.pending_bits >= 8 {
            self.bytes.push(self.pending as u8);
            self.pending >>= 8;
            self.pending_bits -= 8;
        }
    }

    /// `count` one bits.
    pub fn put_ones(&mut self, mut count: u32) {
        while count > 0 {
            let step = count.min(31);
            self.put((1u32 << step) - 1, step);
            count -= step;
        }
    }

    /// Number of bits written so far.
    #[cfg(test)]
    pub fn bit_len(&self) -> usize {
        self.bytes.len() * 8 + self.pending_bits as usize
    }

    /// The bytes so far, with a last partial byte padded by zero bits.
    pub fn finish(mut self) -> Vec<u8> {
        if self.pending_bits > 0 {
            self.bytes.push(self.pending as u8);
        }
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_bits_first() {
        let mut w = LeBitWriter::default();
        w.put(0b1, 1);
        w.put(0b10, 2); // bits 1..2 hold 0, 1
        w.put_ones(3);
        assert_eq!(w.finish(), vec![0b0011_1101]);
    }
}
