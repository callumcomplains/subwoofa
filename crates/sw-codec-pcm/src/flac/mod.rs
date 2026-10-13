//! FLAC (lossless), 16 or 24 bits, up to 8 channels.
//!
//! Audio is cut into blocks of equal length. In every block each channel is coded with the cheapest of: a constant, the
//! raw samples, a fixed polynomial predictor (orders 0 to 4), or a linear predictor fitted to the block (orders 2 to
//! 12). The prediction error is then Rice coded in partitions, each with its own parameter. Stereo blocks also try
//! mid/side and left/side and right/side pairs and keep the smallest.

mod check;
mod frame;
mod predict;
mod rice;
mod search;

use crate::ogg::{build_page, PacketWriter};
use crate::{take_frames, to_integers, BlockWriter, Params, SampleFormat};
use frame::encode_frame;
use rayon::prelude::*;
use search::Search;
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

// ---------------------------------------------------------------------------------------------------------------
// bit and checksum helpers

// ---------------------------------------------------------------------------------------------------------------
// Rice coding of a residual

// ---------------------------------------------------------------------------------------------------------------
// one channel of one block

// ---------------------------------------------------------------------------------------------------------------
// frames

// ---------------------------------------------------------------------------------------------------------------
// the file

/// Where the frames go: a plain `.flac` file, or packets of an Ogg stream.
enum Sink {
    Plain(BufWriter<File>),
    Ogg(PacketWriter<BufWriter<File>>),
}

pub struct FlacWriter {
    /// `None` only after `finish` has taken it.
    sink: Option<Sink>,
    /// Samples (per channel) written so far (the Ogg granule position).
    samples_done: u64,
    params: Params,
    bits: u32,
    block_size: usize,
    block_size_code: u8,
    /// Samples collected for the next block, per file channel.
    pending: Vec<Vec<i32>>,
    next_frame: u64,
    md5: md5::Context,
    min_frame_bytes: u32,
    max_frame_bytes: u32,
    bytes: u64,
}

impl FlacWriter {
    pub fn create(path: &Path, params: Params, in_ogg: bool) -> std::io::Result<FlacWriter> {
        let bits = match params.format {
            SampleFormat::Int16 => 16,
            SampleFormat::Int24 => 24,
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "FLAC holds 16 or 24 bit integer samples",
                ))
            }
        };
        if params.channels() > 8 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "FLAC holds at most 8 channels",
            ));
        }
        let (block_size, block_size_code) = if params.rate <= 48_000 {
            (4096, 12)
        } else if params.rate <= 96_000 {
            (8192, 13)
        } else {
            (16384, 14)
        };
        let out = BufWriter::new(File::create(path)?);
        let sink = if in_ogg {
            Sink::Ogg(PacketWriter::new(out, OGG_SERIAL))
        } else {
            Sink::Plain(out)
        };
        // STREAMINFO is rewritten at the end, when the totals are known
        let mut writer = FlacWriter {
            sink: Some(sink),
            samples_done: 0,
            bits,
            block_size,
            block_size_code,
            pending: vec![Vec::new(); params.channels()],
            next_frame: 0,
            md5: md5::Context::new(),
            min_frame_bytes: u32::MAX,
            max_frame_bytes: 0,
            bytes: 0,
            params,
        };
        let comment = writer.channel_mask_comment(in_ogg);
        let mut header_bytes = 0u64;
        match writer.sink.as_mut().unwrap() {
            Sink::Plain(out) => {
                out.write_all(b"fLaC")?;
                out.write_all(&metadata_header(0, comment.is_none(), 34))?;
                out.write_all(&[0u8; 34])?;
                header_bytes = 4 + 4 + 34;
                if let Some(c) = comment {
                    out.write_all(&metadata_header(4, true, c.len() as u32))?;
                    out.write_all(&c)?;
                    header_bytes += 4 + c.len() as u64;
                }
            }
            Sink::Ogg(ogg) => {
                // packet 1 (alone on the first page): identification header with STREAMINFO; packet 2: the comment
                ogg.packet(&ogg_first_packet(&[0u8; 34]), 0)?;
                ogg.flush(false)?;
                let c = comment.expect("an Ogg FLAC stream always has a comment block");
                let mut packet = metadata_header(4, true, c.len() as u32).to_vec();
                packet.extend_from_slice(&c);
                ogg.packet(&packet, 0)?;
                ogg.flush(false)?; // audio starts on a fresh page
            }
        }
        writer.bytes = header_bytes;
        Ok(writer)
    }

    /// A Vorbis comment carrying the channel mask, unless the channels are in the default FLAC order for their count
    /// (an Ogg stream always has a comment block, so it gets one without entries).
    fn channel_mask_comment(&self, always: bool) -> Option<Vec<u8>> {
        const DEFAULT_MASKS: [u32; 8] = [0x4, 0x3, 0x7, 0x33, 0x37, 0x3F, 0x70F, 0x63F];
        let mask = self.params.mask;
        let needed = mask != 0 && mask != DEFAULT_MASKS[self.params.channels() - 1];
        if !needed && !always {
            return None;
        }
        let vendor = b"Subwoofa";
        let mut c = Vec::new();
        c.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        c.extend_from_slice(vendor);
        if needed {
            let entry = format!("WAVEFORMATEXTENSIBLE_CHANNEL_MASK=0x{mask:X}");
            c.extend_from_slice(&1u32.to_le_bytes());
            c.extend_from_slice(&(entry.len() as u32).to_le_bytes());
            c.extend_from_slice(entry.as_bytes());
        } else {
            c.extend_from_slice(&0u32.to_le_bytes());
        }
        Some(c)
    }

    /// Feeds the checksum with the blocks and, meanwhile, encodes them side by side; then writes them in order.
    fn write_frames(&mut self, blocks: &[Vec<Vec<i32>>]) -> std::io::Result<()> {
        let bits = self.bits;
        let framing = Framing {
            size: self.block_size,
            size_code: self.block_size_code,
            bits,
            first: self.next_frame,
            search: Search::for_effort(self.params.effort),
        };
        let md5 = &mut self.md5;
        let (_, frames) = rayon::join(
            || {
                for block in blocks {
                    update_md5(md5, bits, block);
                }
            },
            || encode_blocks(blocks, framing),
        );
        for (block, frame) in blocks.iter().zip(frames) {
            self.store_frame(frame, block[0].len())?;
        }
        Ok(())
    }

    fn store_frame(&mut self, frame: Vec<u8>, n: usize) -> std::io::Result<()> {
        self.next_frame += 1;
        self.min_frame_bytes = self.min_frame_bytes.min(frame.len() as u32);
        self.max_frame_bytes = self.max_frame_bytes.max(frame.len() as u32);
        self.bytes += frame.len() as u64;
        self.samples_done += n as u64;
        match self.sink.as_mut().unwrap() {
            Sink::Plain(out) => out.write_all(&frame),
            Sink::Ogg(ogg) => ogg.packet(&frame, self.samples_done),
        }
    }
}

/// What encoding a batch of blocks needs to know about the stream.
#[derive(Clone, Copy)]
struct Framing {
    size: usize,
    size_code: u8,
    bits: u32,
    /// Number of the first frame of the batch.
    first: u64,
    search: Search,
}

/// The frames for a batch of blocks, encoded side by side.
fn encode_blocks(blocks: &[Vec<Vec<i32>>], framing: Framing) -> Vec<Vec<u8>> {
    let Framing {
        size,
        size_code,
        bits,
        first,
        search,
    } = framing;
    blocks
        .par_iter()
        .enumerate()
        .map(|(i, block)| {
            let n = block[0].len();
            let code = if n == size {
                size_code
            } else if n <= 256 {
                6
            } else {
                7
            };
            encode_frame(block, bits, first + i as u64, code, search)
        })
        .collect()
}

/// Feeds the checksum with the samples as a decoder would output them: interleaved little-endian integers.
fn update_md5(md5: &mut md5::Context, bits: u32, block: &[Vec<i32>]) {
    let width = (bits / 8) as usize;
    let mut raw = Vec::with_capacity(block[0].len() * block.len() * width);
    for i in 0..block[0].len() {
        for channel in block {
            raw.extend_from_slice(&channel[i].to_le_bytes()[..width]);
        }
    }
    md5.consume(&raw);
}

impl BlockWriter for FlacWriter {
    fn write_block(&mut self, planar: &[Vec<f32>], frames: usize) -> std::io::Result<()> {
        let converted = to_integers(planar, &self.params.order, frames, self.bits);
        for (pending, new) in self.pending.iter_mut().zip(converted) {
            pending.extend(new);
        }
        let blocks = take_frames(&mut self.pending, self.block_size, 1);
        self.write_frames(&blocks)
    }

    fn finish(mut self: Box<Self>) -> std::io::Result<u64> {
        if !self.pending[0].is_empty() {
            let block: Vec<Vec<i32>> = self.pending.iter_mut().map(std::mem::take).collect();
            self.write_frames(&[block])?;
        }
        // now the totals are known: STREAMINFO is rewritten
        let info = self.stream_info();
        match self.sink.take().unwrap() {
            Sink::Plain(mut out) => {
                out.flush()?;
                out.seek(SeekFrom::Start(8))?; // after "fLaC" and the metadata block header
                out.write_all(&info)?;
                out.flush()?;
                Ok(self.bytes)
            }
            Sink::Ogg(ogg) => {
                let mut out = ogg.finish()?;
                // the first page again, with the real STREAMINFO (same length, new checksum)
                let packet = ogg_first_packet(&info);
                let first = build_page(OGG_SERIAL, 0, 0x02, 0, &[packet.len() as u8], &packet);
                out.seek(SeekFrom::Start(0))?;
                out.write_all(&first)?;
                out.flush()?;
                Ok(out.get_ref().metadata()?.len())
            }
        }
    }
}

impl FlacWriter {
    /// The 34 bytes of STREAMINFO.
    fn stream_info(&mut self) -> Vec<u8> {
        let mut info = Vec::with_capacity(34);
        // block sizes: the last block is left out of the minimum, so a one-block file reports its own length
        let largest_block = self.block_size.min(self.params.total_frames as usize) as u16;
        info.extend_from_slice(&largest_block.to_be_bytes());
        info.extend_from_slice(&largest_block.to_be_bytes());
        info.extend_from_slice(&self.min_frame_bytes.to_be_bytes()[1..]);
        info.extend_from_slice(&self.max_frame_bytes.to_be_bytes()[1..]);
        let packed: u64 = ((self.params.rate as u64) << 44)
            | (((self.params.channels() as u64) - 1) << 41)
            | (((self.bits as u64) - 1) << 36)
            | self.params.total_frames;
        info.extend_from_slice(&packed.to_be_bytes());
        let digest = std::mem::replace(&mut self.md5, md5::Context::new()).finalize();
        info.extend_from_slice(&digest.0);
        info
    }
}

/// Stream serial number of the single logical Ogg stream.
const OGG_SERIAL: u32 = 0x5357_4F46;

/// The first Ogg packet of a FLAC stream: mapping signature and version, the number of further header packets,
/// then the native stream marker and the STREAMINFO block.
fn ogg_first_packet(stream_info: &[u8]) -> Vec<u8> {
    let mut p = vec![0x7F];
    p.extend_from_slice(b"FLAC");
    p.extend_from_slice(&[1, 0]); // mapping version 1.0
    p.extend_from_slice(&1u16.to_be_bytes()); // one more header packet (the comment)
    p.extend_from_slice(b"fLaC");
    p.extend_from_slice(&metadata_header(0, false, 34));
    p.extend_from_slice(stream_info);
    p
}

/// Metadata block header: type, whether it is the last block, and the body length.
fn metadata_header(block_type: u8, last: bool, length: u32) -> [u8; 4] {
    let l = length.to_be_bytes();
    [block_type | if last { 0x80 } else { 0 }, l[1], l[2], l[3]]
}
