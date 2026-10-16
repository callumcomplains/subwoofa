//! Writing bit fields into bytes, the one way every encoder here needs: `BitWriter` packs most significant bit first
//! (AC-3, E-AC-3, DTS, TrueHD, MLP, FLAC, ALAC, MPEG-4 ALS), `LeBitWriter` least significant bit first (DTS Express,
//! TTA, WavPack). Both take fields of up to 32 bits, keep only the low bits of the value, and pad with zero bits.

/// Bit writer, most significant bit first.
#[derive(Default, Clone)]
pub struct BitWriter {
    bytes: Vec<u8>,
    /// Bits not yet in `bytes`, right-aligned; fewer than 8 between calls.
    pending: u64,
    pending_bits: u32,
}

impl BitWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends the low `n` bits (0 to 32) of `value`.
    pub fn put(&mut self, value: u32, n: u32) {
        debug_assert!(n <= 32);
        if n == 0 {
            return;
        }
        self.pending = (self.pending << n) | (value as u64 & ((1u64 << n) - 1));
        self.pending_bits += n;
        while self.pending_bits >= 8 {
            self.pending_bits -= 8;
            self.bytes.push((self.pending >> self.pending_bits) as u8);
        }
        self.pending &= (1u64 << self.pending_bits) - 1;
    }

    /// Appends `value` as `n` bits of two's complement.
    pub fn put_signed(&mut self, value: i32, n: u32) {
        self.put(value as u32, n);
    }

    /// Bits written so far.
    pub fn bit_len(&self) -> usize {
        self.bytes.len() * 8 + self.pending_bits as usize
    }

    /// Pads with zero bits up to a multiple of `unit` bits.
    pub fn align(&mut self, unit: usize) {
        let mut pad = (unit - self.bit_len() % unit) % unit;
        while pad > 0 {
            let n = pad.min(32);
            self.put(0, n as u32);
            pad -= n;
        }
    }

    /// The whole bytes written so far (a partly filled last byte is not among them).
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The bytes written so far with a partly filled last byte padded by zero bits, leaving the writer as it is (a
    /// checksum over bits still being written).
    pub fn snapshot(&self) -> Vec<u8> {
        let mut v = self.bytes.clone();
        if self.pending_bits > 0 {
            v.push((self.pending << (8 - self.pending_bits)) as u8);
        }
        v
    }

    /// The bytes, the last one padded by zero bits.
    pub fn into_bytes(mut self) -> Vec<u8> {
        self.align(8);
        self.bytes
    }

    /// The bytes padded by zero bits to exactly `total_bits` (a whole number of bytes). Panics when more than that was
    /// written, which is a bug in the caller's size sums.
    pub fn finish_padded(mut self, total_bits: usize) -> Vec<u8> {
        assert!(
            self.bit_len() <= total_bits,
            "bit overflow: {} > {total_bits}",
            self.bit_len()
        );
        while self.bit_len() < total_bits {
            let n = (total_bits - self.bit_len()).min(32) as u32;
            self.put(0, n);
        }
        self.bytes
    }
}

/// Bit writer, least significant bit first.
#[derive(Default, Clone)]
pub struct LeBitWriter {
    bytes: Vec<u8>,
    /// Bits not yet in `bytes`, from bit 0 up; fewer than 8 between calls.
    pending: u64,
    pending_bits: u32,
}

impl LeBitWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends the low `n` bits (0 to 32) of `value`.
    pub fn put(&mut self, value: u32, n: u32) {
        debug_assert!(n <= 32);
        if n == 0 {
            return;
        }
        self.pending |= (value as u64 & ((1u64 << n) - 1)) << self.pending_bits;
        self.pending_bits += n;
        while self.pending_bits >= 8 {
            self.bytes.push(self.pending as u8);
            self.pending >>= 8;
            self.pending_bits -= 8;
        }
    }

    /// Appends `count` one bits.
    pub fn put_ones(&mut self, mut count: u32) {
        while count > 0 {
            let step = count.min(32);
            self.put(u32::MAX, step);
            count -= step;
        }
    }

    /// Bits written so far.
    pub fn bit_len(&self) -> usize {
        self.bytes.len() * 8 + self.pending_bits as usize
    }

    /// The bytes, the last one padded by zero bits.
    pub fn into_bytes(mut self) -> Vec<u8> {
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
    fn packs_msb_first() {
        let mut w = BitWriter::new();
        w.put(0b101, 3);
        w.put(0xFF, 8);
        w.put_signed(-2, 4);
        assert_eq!(w.bit_len(), 15);
        assert_eq!(w.snapshot(), vec![0b1011_1111, 0b1111_1100]);
        assert_eq!(w.bytes(), [0b1011_1111]);
        assert_eq!(w.into_bytes(), vec![0b1011_1111, 0b1111_1100]);
    }

    #[test]
    fn high_bits_dropped() {
        let mut w = BitWriter::new();
        w.put(0xFFFF_FFF5, 4);
        w.put(u32::MAX, 32);
        w.put(0, 4);
        assert_eq!(w.into_bytes(), vec![0x5F, 0xFF, 0xFF, 0xFF, 0xF0]);
    }

    #[test]
    fn pads_to_size() {
        let mut w = BitWriter::new();
        w.put(1, 1);
        assert_eq!(w.clone().finish_padded(24), vec![0x80, 0, 0]);
        w.align(16);
        assert_eq!(w.bit_len(), 16);
    }

    #[test]
    fn packs_lsb_first() {
        let mut w = LeBitWriter::new();
        w.put(0b1, 1);
        w.put(0b10, 2);
        w.put_ones(3);
        assert_eq!(w.bit_len(), 6);
        assert_eq!(w.into_bytes(), vec![0b0011_1101]);
        let mut long = LeBitWriter::new();
        long.put_ones(40);
        assert_eq!(long.into_bytes(), vec![0xFF; 5]);
    }
}
