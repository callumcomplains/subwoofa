//! Writing values of up to 32 bits into a byte vector, most significant bit first.

#[derive(Default)]
pub(crate) struct BitWriter {
    pub bytes: Vec<u8>,
    /// Bits not yet in `bytes`, right-aligned in `pending`.
    pending: u64,
    pending_bits: u32,
}

impl BitWriter {
    /// Appends the low `count` bits of `value` (`count` up to 32).
    pub fn put(&mut self, value: u64, count: u32) {
        debug_assert!(count <= 32);
        if count == 0 {
            return;
        }
        self.pending = (self.pending << count) | (value & ((1u64 << count) - 1));
        self.pending_bits += count;
        while self.pending_bits >= 8 {
            self.pending_bits -= 8;
            self.bytes.push((self.pending >> self.pending_bits) as u8);
        }
        self.pending &= (1u64 << self.pending_bits) - 1;
    }

    /// Appends a signed value as `count` bits of two's complement.
    pub fn put_signed(&mut self, value: i64, count: u32) {
        self.put(value as u64, count);
    }

    /// Rice code: the quotient in unary (that many zeros, then a one) and the remainder in `k` bits.
    pub fn put_rice(&mut self, zigzag: u64, k: u32) {
        let mut zeros = zigzag >> k;
        while zeros >= 32 {
            self.put(0, 32);
            zeros -= 32;
        }
        self.put(0, zeros as u32);
        self.put(1, 1);
        self.put(zigzag & ((1u64 << k) - 1), k);
    }

    pub fn align(&mut self) {
        if self.pending_bits > 0 {
            self.put(0, 8 - self.pending_bits);
        }
    }
}

