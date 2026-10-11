//! WAV: RIFF, or RF64 when the audio is too long for a 32-bit size. Always carries a `bext` chunk (Broadcast Wave)
//! when the source had a time reference, and the extensible `fmt ` chunk whenever the plain one cannot say enough.

use crate::{interleave, pad_even, BlockWriter, Params};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

/// Tail of the sub-format GUIDs, shared by PCM and IEEE float (the first two bytes are the format tag).
const GUID_TAIL: [u8; 14] = [0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71];

pub struct WavWriter {
    out: BufWriter<File>,
    params: Params,
    scratch: Vec<u8>,
    data_bytes: u64,
    written: u64,
    file_bytes: u64,
}

impl WavWriter {
    pub fn create(path: &Path, params: Params) -> std::io::Result<WavWriter> {
        let mut out = BufWriter::new(File::create(path)?);
        let data_bytes = params.total_frames * params.channels() as u64 * params.format.bytes() as u64;
        let header = header(&params, data_bytes);
        out.write_all(&header)?;
        let file_bytes = header.len() as u64;
        Ok(WavWriter { out, params, scratch: Vec::new(), data_bytes, written: 0, file_bytes })
    }
}

impl BlockWriter for WavWriter {
    fn write_block(&mut self, planar: &[Vec<f32>], frames: usize) -> std::io::Result<()> {
        interleave(planar, &self.params.order, frames, self.params.format, false, &mut self.scratch);
        self.out.write_all(&self.scratch)?;
        self.written += self.scratch.len() as u64;
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> std::io::Result<u64> {
        // the header was written for the planned length; a shorter source would leave it wrong
        if self.written != self.data_bytes {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "the input ended before the planned length"));
        }
        pad_even(&mut self.out, self.data_bytes)?;
        self.out.flush()?;
        self.file_bytes += self.data_bytes + self.data_bytes % 2;
        Ok(self.file_bytes)
    }
}

/// Every byte in front of the audio.
fn header(p: &Params, data_bytes: u64) -> Vec<u8> {
    let rf64 = data_bytes > 0xFFFF_0000;
    let mut chunks: Vec<u8> = Vec::new();
    if rf64 {
        // ds64: the 64-bit sizes the 32-bit fields cannot hold
        let mut ds64 = Vec::new();
        ds64.extend_from_slice(&0u64.to_le_bytes()); // RIFF size, patched below
        ds64.extend_from_slice(&data_bytes.to_le_bytes());
        ds64.extend_from_slice(&p.total_frames.to_le_bytes());
        ds64.extend_from_slice(&0u32.to_le_bytes()); // no table entries
        chunk(&mut chunks, b"ds64", &ds64);
    }
    if let Some(reference) = p.time_reference {
        chunk(&mut chunks, b"bext", &bext(reference));
    }
    chunk(&mut chunks, b"fmt ", &fmt(p));
    let data_header = [b"data".as_slice(), &(if rf64 { u32::MAX } else { data_bytes as u32 }).to_le_bytes()].concat();
    let riff_size = 4 + chunks.len() as u64 + data_header.len() as u64 + data_bytes + data_bytes % 2;
    if rf64 {
        // patch the RIFF size into ds64 (its first eight bytes after the chunk header)
        chunks[8..16].copy_from_slice(&riff_size.to_le_bytes());
    }
    let mut out = Vec::new();
    out.extend_from_slice(if rf64 { b"RF64" } else { b"RIFF" });
    out.extend_from_slice(&(if rf64 { u32::MAX } else { riff_size as u32 }).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(&chunks);
    out.extend_from_slice(&data_header);
    out
}

fn chunk(out: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    if body.len() % 2 == 1 {
        out.push(0);
    }
}

/// The `fmt ` chunk. The plain 16-byte form is only used where every player understands it (up to stereo,
/// integer, 16 bits); everything else gets the extensible form with the channel mask.
fn fmt(p: &Params) -> Vec<u8> {
    let channels = p.channels() as u16;
    let bits = p.format.bits() as u16;
    let block_align = channels * (bits / 8);
    let plain = channels <= 2 && !p.format.is_float() && bits == 16;
    let tag: u16 = if p.format.is_float() { 3 } else { 1 };
    let mut f = Vec::new();
    f.extend_from_slice(&(if plain { tag } else { 0xFFFE }).to_le_bytes());
    f.extend_from_slice(&channels.to_le_bytes());
    f.extend_from_slice(&p.rate.to_le_bytes());
    f.extend_from_slice(&(p.rate * block_align as u32).to_le_bytes());
    f.extend_from_slice(&block_align.to_le_bytes());
    f.extend_from_slice(&bits.to_le_bytes());
    if !plain {
        f.extend_from_slice(&22u16.to_le_bytes()); // size of the extension
        f.extend_from_slice(&bits.to_le_bytes()); // valid bits per sample
        f.extend_from_slice(&p.mask.to_le_bytes());
        f.extend_from_slice(&tag.to_le_bytes());
        f.extend_from_slice(&GUID_TAIL);
    }
    f
}

/// Broadcast Wave `bext` chunk (version 1) holding only the time reference.
fn bext(time_reference: u64) -> Vec<u8> {
    let mut b = vec![0u8; 256 + 32 + 32 + 10 + 8]; // description, originator, reference, date, time
    b[256..256 + 8].copy_from_slice(b"Subwoofa");
    b.extend_from_slice(&(time_reference as u32).to_le_bytes());
    b.extend_from_slice(&((time_reference >> 32) as u32).to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // version
    b.extend_from_slice(&[0u8; 64]); // UMID
    b.extend_from_slice(&[0u8; 10]); // loudness values
    b.extend_from_slice(&[0u8; 180]); // reserved
    b
}
