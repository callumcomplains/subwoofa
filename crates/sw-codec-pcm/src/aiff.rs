//! AIFF (integer samples) and AIFF-C (floating point), both big-endian. The format limits files to 4 GB.

use crate::{interleave, pad_even, BlockWriter, Params};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

pub struct AiffWriter {
    out: BufWriter<File>,
    params: Params,
    scratch: Vec<u8>,
    data_bytes: u64,
    written: u64,
    file_bytes: u64,
}

impl AiffWriter {
    pub fn create(path: &Path, params: Params) -> std::io::Result<AiffWriter> {
        let data_bytes = params.total_frames * params.channels() as u64 * params.format.bytes() as u64;
        let header = header(&params, data_bytes)?;
        let mut out = BufWriter::new(File::create(path)?);
        out.write_all(&header)?;
        let file_bytes = header.len() as u64;
        Ok(AiffWriter { out, params, scratch: Vec::new(), data_bytes, written: 0, file_bytes })
    }
}

impl BlockWriter for AiffWriter {
    fn write_block(&mut self, planar: &[Vec<f32>], frames: usize) -> std::io::Result<()> {
        interleave(planar, &self.params.order, frames, self.params.format, true, &mut self.scratch);
        self.out.write_all(&self.scratch)?;
        self.written += self.scratch.len() as u64;
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> std::io::Result<u64> {
        if self.written != self.data_bytes {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "the input ended before the planned length"));
        }
        pad_even(&mut self.out, self.data_bytes)?;
        self.out.flush()?;
        Ok(self.file_bytes + self.data_bytes + self.data_bytes % 2)
    }
}

fn header(p: &Params, data_bytes: u64) -> std::io::Result<Vec<u8>> {
    let float = p.format.is_float();
    let mut comm = Vec::new();
    comm.extend_from_slice(&(p.channels() as u16).to_be_bytes());
    comm.extend_from_slice(&(p.total_frames as u32).to_be_bytes());
    comm.extend_from_slice(&(p.format.bits() as u16).to_be_bytes());
    comm.extend_from_slice(&extended80(p.rate));
    if float {
        // AIFF-C names its compression (fl32 or fl64), then a text label: a length byte and the text, padded to even
        comm.extend_from_slice(if p.format.bits() == 32 { b"fl32" } else { b"fl64" });
        let name: &[u8] = if p.format.bits() == 32 { b"IEEE 32-bit float" } else { b"IEEE 64-bit float" };
        comm.push(name.len() as u8);
        comm.extend_from_slice(name);
        if (1 + name.len()) % 2 == 1 {
            comm.push(0);
        }
    }
    let mut chunks = Vec::new();
    if float {
        chunk(&mut chunks, b"FVER", &0xA280_5140u32.to_be_bytes()); // AIFF-C version 1
    }
    chunk(&mut chunks, b"COMM", &comm);
    let ssnd_size = 8 + data_bytes;
    let form_size = 4 + chunks.len() as u64 + 8 + ssnd_size + ssnd_size % 2;
    if form_size > u32::MAX as u64 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "AIFF files cannot hold more than 4 GB: use WAV (RF64), CAF or FLAC"));
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"FORM");
    out.extend_from_slice(&(form_size as u32).to_be_bytes());
    out.extend_from_slice(if float { b"AIFC" } else { b"AIFF" });
    out.extend_from_slice(&chunks);
    out.extend_from_slice(b"SSND");
    out.extend_from_slice(&(ssnd_size as u32).to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes()); // offset of the first sample
    out.extend_from_slice(&0u32.to_be_bytes()); // block size
    Ok(out)
}

fn chunk(out: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(body);
    if body.len() % 2 == 1 {
        out.push(0);
    }
}

/// A whole number as the 80-bit IEEE extended float AIFF uses for the sample rate.
fn extended80(rate: u32) -> [u8; 10] {
    let top_bit = 31 - rate.leading_zeros(); // position of the highest set bit
    let exponent = 16383 + top_bit as u16;
    let mantissa = (rate as u64) << (63 - top_bit);
    let mut out = [0u8; 10];
    out[..2].copy_from_slice(&exponent.to_be_bytes());
    out[2..].copy_from_slice(&mantissa.to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::extended80;

    #[test]
    fn rate_48000_in_extended_form() {
        // 48000 = 1.46484375 * 2^15: exponent 0x400E, mantissa 0xBB80 << 48
        assert_eq!(extended80(48_000), [0x40, 0x0E, 0xBB, 0x80, 0, 0, 0, 0, 0, 0]);
    }
}
