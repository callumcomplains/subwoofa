//! A DTS stream inside a WAV file: the form a CD player or an S/PDIF output carries as if it were PCM.

use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

/// A 16-bit stereo WAV file being written; the sizes in the header are filled in by `finish`.
pub(crate) struct WavOut {
    out: BufWriter<std::fs::File>,
    rate: u32,
    written: u64,
}

impl WavOut {
    pub(crate) fn create(path: &Path, rate: u32) -> std::io::Result<WavOut> {
        let mut out = BufWriter::new(std::fs::File::create(path)?);
        out.write_all(&wav_header(rate, 0))?;
        Ok(WavOut { out, rate, written: 0 })
    }

    pub(crate) fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.out.write_all(data)?;
        self.written += data.len() as u64;
        Ok(())
    }

    /// Pads the data to whole stereo samples and writes the real sizes into the header.
    pub(crate) fn finish(mut self) -> std::io::Result<()> {
        let pad = (4 - self.written % 4) % 4;
        self.out.write_all(&vec![0u8; pad as usize])?;
        self.out.flush()?;
        let mut file = self.out.into_inner().map_err(|e| e.into_error())?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&wav_header(self.rate, self.written + pad))?;
        file.flush()
    }
}

/// Canonical 44-byte WAV header for 16-bit stereo PCM holding `data_len` bytes (a DTS stream in PCM).
fn wav_header(rate: u32, data_len: u64) -> Vec<u8> {
    let mut h = Vec::with_capacity(44);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&((36 + data_len).min(u32::MAX as u64) as u32).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes());
    h.extend_from_slice(&2u16.to_le_bytes());
    h.extend_from_slice(&rate.to_le_bytes());
    h.extend_from_slice(&(rate * 4).to_le_bytes());
    h.extend_from_slice(&4u16.to_le_bytes());
    h.extend_from_slice(&16u16.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&(data_len.min(u32::MAX as u64) as u32).to_le_bytes());
    h
}

/// The stream's 16-bit words byte-swapped into little-endian samples (an odd last byte is padded with a zero).
pub(crate) fn words_to_samples(data: &[u8]) -> Vec<u8> {
    let mut w = data.to_vec();
    if w.len() % 2 == 1 {
        w.push(0);
    }
    for pair in w.chunks_exact_mut(2) {
        pair.swap(0, 1);
    }
    w
}

/// The 14-bit DTS stream format: the bit stream is cut into 14-bit groups, each stored sign-extended in a 16-bit
/// little-endian word (a frame whose size is a multiple of 14 bytes fills whole words).
pub(crate) fn pack_14bit(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 8 / 7 + 2);
    let (mut acc, mut n) = (0u32, 0u32);
    for &b in data {
        acc = (acc << 8) | b as u32;
        n += 8;
        while n >= 14 {
            n -= 14;
            let w = ((acc >> n) & 0x3FFF) as u16;
            let signed = if w & 0x2000 != 0 { w | 0xC000 } else { w };
            out.extend_from_slice(&signed.to_le_bytes());
        }
        acc &= (1 << n) - 1;
    }
    if n > 0 {
        let w = ((acc << (14 - n)) & 0x3FFF) as u16;
        let signed = if w & 0x2000 != 0 { w | 0xC000 } else { w };
        out.extend_from_slice(&signed.to_le_bytes());
    }
    out
}
