//! MSB-first bit writer and the AC-3 CRC-16 (x^16 + x^15 + x^2 + 1).

pub struct BitWriter {
    buf: Vec<u8>,
    acc: u64,
    nacc: u32,
}

impl Default for BitWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl BitWriter {
    pub fn new() -> Self {
        BitWriter {
            buf: Vec::with_capacity(4096),
            acc: 0,
            nacc: 0,
        }
    }

    /// Append the low `n` bits of `v` (n <= 32).
    pub fn put(&mut self, v: u32, n: u32) {
        debug_assert!(n <= 32 && (n == 32 || v >> n == 0), "value {v} does not fit {n} bits");
        self.acc = (self.acc << n) | v as u64;
        self.nacc += n;
        while self.nacc >= 8 {
            self.nacc -= 8;
            self.buf.push((self.acc >> self.nacc) as u8);
        }
        self.acc &= (1u64 << self.nacc) - 1;
    }

    pub fn bit_len(&self) -> usize {
        self.buf.len() * 8 + self.nacc as usize
    }

    /// Pad with zero bits to `total_bits`, then return the bytes. Panics if already longer.
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
        debug_assert_eq!(self.nacc, 0);
        self.buf
    }
}

const POLY: u32 = 0x18005;

/// CRC of `data` (MSB first, zero initial value, no final xor).
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u32 = 0;
    for &b in data {
        crc ^= (b as u32) << 8;
        for _ in 0..8 {
            crc <<= 1;
            if crc & 0x10000 != 0 {
                crc ^= POLY;
            }
        }
    }
    crc as u16
}

fn mulmod(mut a: u32, mut b: u32) -> u32 {
    let mut r = 0;
    while b != 0 {
        if b & 1 != 0 {
            r ^= a;
        }
        b >>= 1;
        a <<= 1;
        if a & 0x10000 != 0 {
            a ^= POLY;
        }
    }
    r
}

fn powmod(mut base: u32, mut e: u64) -> u32 {
    let mut r = 1;
    while e != 0 {
        if e & 1 != 0 {
            r = mulmod(r, base);
        }
        base = mulmod(base, base);
        e >>= 1;
    }
    r
}

/// Value for the `crc1` field so that the CRC over the first 5/8 of the frame (starting at the
/// crc1 field itself) is zero. `crc_after` is `crc16` of the bytes after the field up to the
/// end of that region, and `region_bytes` is the byte length of the whole region including the field.
pub fn crc1_field(crc_after: u16, region_bytes: usize) -> u16 {
    // x^-1 modulo the polynomial is (POLY >> 1) because the constant term is 1
    let inv_x = POLY >> 1;
    let bits = 8 * region_bytes as u64;
    mulmod(crc_after as u32, powmod(inv_x, bits)) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_packs_msb_first() {
        let mut w = BitWriter::new();
        w.put(0b101, 3);
        w.put(0b1, 1);
        w.put(0xAB, 8);
        w.put(0, 4);
        assert_eq!(w.bit_len(), 16);
        assert_eq!(w.finish_padded(24), vec![0b1011_1010, 0b1011_0000, 0]);
    }

    #[test]
    fn crc_appended_gives_zero_residual() {
        let data: Vec<u8> = (0..100u32).map(|i| (i * 7 + 3) as u8).collect();
        let c = crc16(&data);
        let mut with = data.clone();
        with.extend_from_slice(&c.to_be_bytes());
        assert_eq!(crc16(&with), 0);
    }

    #[test]
    fn crc1_field_zeroes_the_leading_region() {
        // region = [crc1 (2 bytes)] + rest, as in an AC-3 frame after the sync word
        let rest: Vec<u8> = (0..70u32).map(|i| (i * 31 + 5) as u8).collect();
        let field = crc1_field(crc16(&rest), rest.len() + 2);
        let mut region = field.to_be_bytes().to_vec();
        region.extend_from_slice(&rest);
        assert_eq!(crc16(&region), 0);
    }
}
