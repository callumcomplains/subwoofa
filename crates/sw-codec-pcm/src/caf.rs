//! CAF (Core Audio Format): linear PCM, little-endian, with a channel label for every channel. No 4 GB limit.

use crate::{interleave, BlockWriter, Params};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

pub struct CafWriter {
    out: BufWriter<File>,
    params: Params,
    scratch: Vec<u8>,
    data_bytes: u64,
    written: u64,
    file_bytes: u64,
}

impl CafWriter {
    pub fn create(path: &Path, params: Params) -> std::io::Result<CafWriter> {
        let data_bytes = params.total_frames * params.channels() as u64 * params.format.bytes() as u64;
        let header = header(&params, data_bytes);
        let mut out = BufWriter::new(File::create(path)?);
        out.write_all(&header)?;
        let file_bytes = header.len() as u64;
        Ok(CafWriter { out, params, scratch: Vec::new(), data_bytes, written: 0, file_bytes })
    }
}

impl BlockWriter for CafWriter {
    fn write_block(&mut self, planar: &[Vec<f32>], frames: usize) -> std::io::Result<()> {
        interleave(planar, &self.params.order, frames, self.params.format, false, &mut self.scratch);
        self.out.write_all(&self.scratch)?;
        self.written += self.scratch.len() as u64;
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> std::io::Result<u64> {
        if self.written != self.data_bytes {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "the input ended before the planned length"));
        }
        self.out.flush()?;
        Ok(self.file_bytes + self.data_bytes)
    }
}

fn header(p: &Params, data_bytes: u64) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"caff");
    out.extend_from_slice(&1u16.to_be_bytes()); // version
    out.extend_from_slice(&0u16.to_be_bytes()); // flags

    // audio description
    let bytes_per_frame = (p.channels() * p.format.bytes()) as u32;
    let format_flags: u32 = u32::from(p.format.is_float()) | 2; // bit 0 float, bit 1 little-endian
    let mut desc = Vec::new();
    desc.extend_from_slice(&(p.rate as f64).to_be_bytes());
    desc.extend_from_slice(b"lpcm");
    desc.extend_from_slice(&format_flags.to_be_bytes());
    desc.extend_from_slice(&bytes_per_frame.to_be_bytes()); // bytes per packet (one frame per packet)
    desc.extend_from_slice(&1u32.to_be_bytes()); // frames per packet
    desc.extend_from_slice(&(p.channels() as u32).to_be_bytes());
    desc.extend_from_slice(&p.format.bits().to_be_bytes());
    chunk(&mut out, b"desc", &desc);

    // channel layout: one description (label, flags, three coordinates) per channel
    let mut chan = Vec::new();
    chan.extend_from_slice(&0u32.to_be_bytes()); // tag 0: use the channel descriptions
    chan.extend_from_slice(&0u32.to_be_bytes()); // bitmap (unused)
    chan.extend_from_slice(&(p.channels() as u32).to_be_bytes());
    for &label in &p.labels {
        chan.extend_from_slice(&label.to_be_bytes());
        chan.extend_from_slice(&0u32.to_be_bytes());
        chan.extend_from_slice(&[0u8; 12]);
    }
    chunk(&mut out, b"chan", &chan);

    // audio data: a 4-byte edit count, then the samples
    out.extend_from_slice(b"data");
    out.extend_from_slice(&((4 + data_bytes) as i64).to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out
}

fn chunk(out: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(body.len() as i64).to_be_bytes());
    out.extend_from_slice(body);
}
