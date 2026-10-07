//! Bit writer and the checksums used by MLP / TrueHD framing.

/// MSB-first bit writer that can also expose the partially filled last byte
/// (the restart-header checksum is computed over bits that are still being written).
pub struct BitWriter {
    buf: Vec<u8>,
    acc: u32,
    nacc: u32,
}

impl BitWriter {
    pub fn new() -> Self {
        BitWriter { buf: Vec::with_capacity(2048), acc: 0, nacc: 0 }
    }

    /// Append the low `n` bits of `v`, `n <= 32`.
    pub fn put(&mut self, v: u32, n: u32) {
        debug_assert!(n <= 32);
        let mut n = n;
        while n > 0 {
            let take = n.min(8 - self.nacc);
            let bits = (v >> (n - take)) & ((1u32 << take) - 1);
            self.acc = (self.acc << take) | bits;
            self.nacc += take;
            n -= take;
            if self.nacc == 8 {
                self.buf.push(self.acc as u8);
                self.acc = 0;
                self.nacc = 0;
            }
        }
    }

    /// Append a signed value in `n` bits (two's complement).
    pub fn put_signed(&mut self, v: i32, n: u32) {
        let mask = if n >= 32 { u32::MAX } else { (1u32 << n) - 1 };
        self.put(v as u32 & mask, n);
    }

    pub fn bit_len(&self) -> usize {
        self.buf.len() * 8 + self.nacc as usize
    }

    /// Bytes written so far, with the partial last byte (if any) left-aligned and zero padded.
    pub fn snapshot(&self) -> Vec<u8> {
        let mut v = self.buf.clone();
        if self.nacc > 0 {
            v.push((self.acc << (8 - self.nacc)) as u8);
        }
        v
    }

    /// Pad with zero bits to a multiple of `unit` bits.
    pub fn align(&mut self, unit: usize) {
        let rem = self.bit_len() % unit;
        if rem != 0 {
            let mut pad = unit - rem;
            while pad > 0 {
                let n = pad.min(32);
                self.put(0, n as u32);
                pad -= n;
            }
        }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        assert_eq!(self.nacc, 0, "stream must be byte aligned");
        self.buf
    }
}

fn crc8_table(poly: u8) -> [u8; 256] {
    let mut t = [0u8; 256];
    for i in 0..256usize {
        let mut c = i as u8;
        for _ in 0..8 {
            c = if c & 0x80 != 0 { (c << 1) ^ poly } else { c << 1 };
        }
        t[i] = c;
    }
    t
}

fn crc16_table(poly: u16) -> [u16; 256] {
    let mut t = [0u16; 256];
    for i in 0..256usize {
        let mut c = (i as u16) << 8;
        for _ in 0..8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ poly } else { c << 1 };
        }
        t[i] = c;
    }
    t
}

fn crc8(table: &[u8; 256], init: u8, data: &[u8]) -> u8 {
    data.iter().fold(init, |c, b| table[(c ^ b) as usize])
}

/// XOR of all bytes.
pub fn parity(data: &[u8]) -> u8 {
    data.iter().fold(0, |a, b| a ^ b)
}

/// 8-bit checksum over a substream's data (everything before the parity/checksum bytes).
pub fn checksum8(data: &[u8]) -> u8 {
    let t = crc8_table(0x63);
    let (last, head) = data.split_last().expect("non-empty");
    crc8(&t, 0x3c, head) ^ last
}

/// 16-bit checksum of the major sync header (bytes 0..26), stored little endian at 26..28.
pub fn major_sync_checksum(header26: &[u8]) -> u16 {
    let t = crc16_table(0x002D);
    let body = &header26[..24];
    let crc = body.iter().fold(0u16, |c, b| t[((c >> 8) as u8 ^ b) as usize] ^ (c << 8));
    // the reference implementation yields the standard CRC with its bytes swapped
    crc.swap_bytes() ^ u16::from_le_bytes([header26[24], header26[25]])
}

/// Checksum over a restart header. `buf` starts with the two flag bits that precede the header;
/// `bit_size` counts the header bits excluding those two and excluding the checksum itself.
pub fn restart_checksum(buf: &[u8], bit_size: usize) -> u8 {
    let t = crc8_table(0x1D);
    let num_bytes = (bit_size + 2) / 8;
    // the two flag bits are not part of the checksum: seed with the top bits masked off
    let mut crc = crc8(&t, buf[0] & 0xC0, &buf[..num_bytes - 1]) as u32;
    crc ^= buf[num_bytes - 1] as u32;
    for i in 0..((bit_size + 2) & 7) {
        crc <<= 1;
        if crc & 0x100 != 0 {
            crc ^= 0x11D;
        }
        crc ^= ((buf[num_bytes] >> (7 - i)) & 1) as u32;
    }
    crc as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_packs_msb_first() {
        let mut w = BitWriter::new();
        w.put(0b101, 3);
        w.put(0xABC, 12);
        assert_eq!(w.bit_len(), 15);
        assert_eq!(w.snapshot(), vec![0b1011_0101, 0b0111_1000]);
        w.align(16);
        assert_eq!(w.into_bytes(), vec![0b1011_0101, 0b0111_1000]);
    }

    #[test]
    fn signed_values() {
        let mut w = BitWriter::new();
        w.put_signed(-3, 4);
        w.put_signed(5, 4);
        assert_eq!(w.into_bytes(), vec![0xD5]);
    }

    #[test]
    fn major_sync_checksum_matches_a_real_stream() {
        // 5.1 TrueHD major sync written by an independent encoder: the stored checksum is d1e4
        let h = hex("f8726fba0007800fb752000000008c7f203c00000000000000e4d1");
        // header26 = first 26 bytes of the real header (the real header has 28 bytes)
        let real = hex("f8726fba0007800fb752000000008c7f203c0000000000000000e4d1");
        assert_eq!(real.len(), 28);
        let c = major_sync_checksum(&real[..26]);
        assert_eq!(c.to_le_bytes(), [real[26], real[27]]);
        let _ = h;
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn parity_and_checksums_are_stable() {
        assert_eq!(parity(&[1, 2, 4]), 7);
        assert_eq!(checksum8(&[0x00]), 0x3c ^ 0x00 ^ 0x00 | 0); // single byte: crc over empty = init, ^ byte
        let h = [0u8; 26];
        let c = major_sync_checksum(&h);
        assert_eq!(c, 0);
    }
}
