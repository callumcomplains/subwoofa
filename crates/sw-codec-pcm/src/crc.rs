//! The CRC-32 over the original samples that the lossless audio formats store to prove a decode was exact: IEEE
//! polynomial, reflected, samples as little-endian bytes (2 per sample for 16 bit, 3 for 24 bit), channels
//! interleaved.

pub(crate) struct SampleCrc {
    state: u32,
    width: usize,
}

impl SampleCrc {
    pub fn new(bits: u32) -> SampleCrc {
        SampleCrc { state: 0xFFFF_FFFF, width: bits as usize / 8 }
    }

    /// Adds a stretch of samples (`channels[channel][frame]`).
    pub fn update(&mut self, channels: &[Vec<i32>]) {
        for n in 0..channels[0].len() {
            for channel in channels {
                for &byte in &channel[n].to_le_bytes()[..self.width] {
                    self.state ^= byte as u32;
                    for _ in 0..8 {
                        self.state = if self.state & 1 != 0 { (self.state >> 1) ^ 0xEDB8_8320 } else { self.state >> 1 };
                    }
                }
            }
        }
    }

    /// The register as it is (what Monkey's Audio inverts and shifts).
    pub fn raw(&self) -> u32 {
        self.state
    }

    /// The usual CRC-32 value.
    pub fn value(&self) -> u32 {
        !self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_check_value() {
        let mut crc = SampleCrc::new(8);
        crc.update(&[b"123456789".iter().map(|&b| b as i32).collect()]);
        assert_eq!(crc.value(), 0xCBF4_3926);
    }
}
