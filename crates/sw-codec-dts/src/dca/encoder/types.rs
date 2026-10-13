//! What the core encoder is configured with, and what the extensions that sit on top of it hand it.

use crate::bits::BitWriter;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Unsupported(String),
}

#[derive(Debug, Clone)]
pub struct Config {
    pub sample_rate: u32,
    /// AMODE code (Table 5-4).
    pub amode: u8,
    /// Primary (full range) channels, 1..=5.
    pub channels: usize,
    pub lfe: bool,
    /// Core bit rate in bits per second.
    pub bitrate: u32,
    /// Dialog normalisation in dB, -31..=0.
    pub dialnorm: i32,
    /// Source resolution flag: 16, 20 or 24.
    pub pcm_bits: u32,
    /// DTS-ES (matrix) flag.
    pub es: bool,
    /// DTS-ES discrete: a sixth full-range channel (the centre surround) travels in the XCH extension at
    /// the end of every frame. `encode_frame` then takes it after the primary channels.
    pub xch: bool,
    /// Perfect-reconstruction filter bank (FILTS flag): needed when a residual extension (XBR) refines
    /// the core.
    pub perfect: bool,
    /// Subband sample blocks per frame: 16 (512 samples) or 32 (1024 samples).
    pub blocks: usize,
}

pub const MAX_PRIMARY: usize = 5;

/// Downmix coefficients in the auxiliary data of a core frame: the matrix type (1 Lo/Ro, 2 Lt/Rt) and one 9-bit
/// code (sign bit 8, table index) per channel for each of the two outputs, channels in the core's order with the
/// LFE last.
#[derive(Debug, Clone)]
pub struct AuxDownmix {
    pub kind: u8,
    pub rows: Vec<Vec<u16>>,
}

impl AuxDownmix {
    /// Sync word, the data, and the checksum of the data.
    pub(crate) fn block(&self) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.put(0, 1); // no decoding time stamp
        w.put(1, 1); // dynamic downmix present
        w.put(u32::from(self.kind), 3);
        for row in &self.rows {
            for c in row {
                w.put(*c as u32, 9);
            }
        }
        let payload = w.into_bytes();
        let mut out = vec![0x9A, 0x11, 0x05, 0xA0];
        out.extend_from_slice(&payload);
        out.extend_from_slice(&crate::lossless::xll::crc16(&payload).to_be_bytes());
        out
    }

    /// Bytes from the sync word to the end of the checksum.
    pub(crate) fn size(&self) -> usize {
        4 + (5 + 9 * self.rows.iter().map(Vec::len).sum::<usize>()).div_ceil(8) + 2
    }
}

/// An XXCH extension (DTS-HD HRA 6.1/7.1): channels beyond the core's five, coded like core channels in the
/// extension substream. All masks use the DTS loudspeaker bit positions (C 0, L 1, R 2, Ls 3, Rs 4, LFE 5, Cs 6,
/// Lsr 7, Rsr 8, Lss 9, Rss 10, then Lc 11, Rc 12, Lh 13, Ch 14, Rh 15, LFE2 16, Lw 17, Rw 18, Oh 19, Lhs 20, Rhs 21).
#[derive(Debug, Clone)]
pub struct XxchParams {
    /// Speakers of the core (with Ls/Rs written as Lss/Rss when the core carries the sides).
    pub core_mask: u32,
    /// Speakers of the channels coded here, in coding order.
    pub spk_mask: u32,
    pub mask_bits: u32,
    /// Embedded downmix: the pre-scaling code (6 bits) applied to every core channel and, per extension
    /// channel, the core speakers it was added to with the 7-bit coefficient code (sign bit 6, magnitude 0..=63).
    pub dmix: Option<(u32, Vec<Vec<(u32, u32)>>)>,
}
