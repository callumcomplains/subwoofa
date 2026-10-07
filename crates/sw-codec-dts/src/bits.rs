//! MSB-first bit writer.

#[derive(Default, Clone)]
pub struct BitWriter {
    bytes: Vec<u8>,
    cur: u64,
    nbits: u32,
}

impl BitWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append the low `n` bits (0..=32) of `value`.
    pub fn put(&mut self, value: u32, n: u32) {
        debug_assert!(n <= 32);
        if n == 0 {
            return;
        }
        debug_assert!(n == 32 || value >> n == 0, "value {value} does not fit {n} bits");
        self.cur = (self.cur << n) | (value as u64 & ((1u64 << n) - 1));
        self.nbits += n;
        while self.nbits >= 8 {
            self.nbits -= 8;
            self.bytes.push((self.cur >> self.nbits) as u8);
        }
        self.cur &= (1u64 << self.nbits) - 1;
    }

    /// Append a two's complement value in `n` bits.
    pub fn put_signed(&mut self, value: i32, n: u32) {
        self.put((value as u32) & if n == 32 { u32::MAX } else { (1u32 << n) - 1 }, n);
    }

    pub fn bit_len(&self) -> usize {
        self.bytes.len() * 8 + self.nbits as usize
    }

    /// Pad with zero bits up to a multiple of `bits`.
    pub fn align(&mut self, bits: usize) {
        while self.bit_len() % bits != 0 {
            self.put(0, 1);
        }
    }

    pub fn into_bytes(mut self) -> Vec<u8> {
        self.align(8);
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_msb_first() {
        let mut w = BitWriter::new();
        w.put(0b101, 3);
        w.put(0xFF, 8);
        w.put_signed(-2, 4);
        assert_eq!(w.bit_len(), 15);
        assert_eq!(w.into_bytes(), vec![0b1011_1111, 0b1111_1100]);
    }
}
