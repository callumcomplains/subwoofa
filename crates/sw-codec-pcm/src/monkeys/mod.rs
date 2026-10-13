//! Monkey's Audio (`.ape`), lossless, 16 or 24 bit, mono or stereo, written as a version 3.99 file.
//!
//! The file is a 52-byte descriptor, a 24-byte header, a seek table, then the frames. A frame holds up to
//! `FRAME_BLOCKS` samples per channel and is independent of the others (every adaptive state starts fresh).
//! Inside a frame the signal goes through
//! 1. the two-stage predictor ([`predictor`]), which for stereo first turns left/right into side/mid,
//! 2. the neural filters of the chosen compression level ([`neural`]),
//! 3. the range coder ([`entropy`], [`range_coder`]).
//!
//! A frame starts with a CRC of the original samples. Its bytes are stored swapped in 32-bit words, and padded to
//! a whole number of words. The format is described by the reference decoder; FFmpeg's decoder checks the result.

mod entropy;
mod neural;
mod predictor;
mod range_coder;

use crate::crc::SampleCrc;
use crate::{take_frames, to_integers, BlockWriter, Params, SampleFormat};
use entropy::ValueCoder;
use neural::{Chain, Spec};
use predictor::Predictor;
use range_coder::RangeEncoder;
use rayon::prelude::*;
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

/// File version written: 3.99.
const VERSION: u16 = 3990;

const DESCRIPTOR_LEN: u32 = 52;
const HEADER_LEN: u32 = 24;

/// Samples per channel in a full frame (the size the reference encoder uses from version 3.95 on).
const FRAME_BLOCKS: usize = 73_728 * 4;

/// "The decoder creates the WAV header", so none is stored.
const FLAG_CREATE_WAV_HEADER: u16 = 32;

/// Frames are encoded in batches of this many, one per core, so a batch is held in memory (about 10 MB per frame).
const BATCH: usize = 8;

/// The compression level written: "extra high" (4000). The format also has fast (1000, no filters), normal (2000, one
/// 16-tap filter), high (3000, one 64-tap filter) and insane (5000, three filters up to 1280 taps); on film audio
/// the differences were under 0.2%, so one level does.
const LEVEL: u16 = 4000;

/// The neural filters of that level, in the order the decoder applies them.
const FILTERS: [Spec; 2] = [Spec { order: 32, shift: 10 }, Spec { order: 256, shift: 13 }];

pub struct MonkeysWriter {
    out: BufWriter<File>,
    params: Params,
    bits: u32,
    pending: Vec<Vec<i32>>,
    /// Size in bytes of every frame written so far.
    frame_sizes: Vec<u32>,
    frame_count: usize,
    header_len: u64,
}

impl MonkeysWriter {
    pub fn create(path: &Path, params: Params) -> std::io::Result<MonkeysWriter> {
        let bad = |what: &str| std::io::Error::new(std::io::ErrorKind::InvalidInput, what.to_string());
        let bits = match params.format {
            SampleFormat::Int16 => 16,
            SampleFormat::Int24 => 24,
            _ => return Err(bad("Monkey's Audio holds 16 or 24 bit integer samples")),
        };
        if params.channels() > 2 {
            return Err(bad("Monkey's Audio holds one or two channels"));
        }
        if params.total_frames > u32::MAX as u64 {
            return Err(bad("Monkey's Audio files hold at most 2^32 samples per channel"));
        }
        let frame_count = (params.total_frames as usize).div_ceil(FRAME_BLOCKS);
        let header_len = (DESCRIPTOR_LEN + HEADER_LEN) as u64 + 4 * frame_count as u64;
        let mut out = BufWriter::new(File::create(path)?);
        out.write_all(&vec![0u8; header_len as usize])?; // filled in at the end
        Ok(MonkeysWriter {
            out,
            pending: vec![Vec::new(); params.channels()],
            params,
            bits,
            frame_sizes: Vec::new(),
            frame_count,
            header_len,
        })
    }

    /// Encodes a batch of frames side by side (they do not depend on each other), then writes them in order.
    fn write_frames(&mut self, frames: &[Vec<Vec<i32>>]) -> std::io::Result<()> {
        let bits = self.bits;
        let coded: Vec<Vec<u8>> = frames.par_iter().map(|channels| encode_frame(channels, bits)).collect();
        for bytes in coded {
            self.frame_sizes.push(bytes.len() as u32);
            self.out.write_all(&bytes)?;
        }
        Ok(())
    }

    /// The descriptor, header and seek table, now that the frame sizes are known.
    fn header(&self) -> Vec<u8> {
        let audio_bytes: u64 = self.frame_sizes.iter().map(|&s| s as u64).sum();
        let blocks = self.params.total_frames;
        let last_blocks = (blocks - FRAME_BLOCKS as u64 * (self.frame_count as u64 - 1)) as u32;

        let mut h = Vec::new();
        h.extend_from_slice(b"MAC ");
        h.extend_from_slice(&VERSION.to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        h.extend_from_slice(&DESCRIPTOR_LEN.to_le_bytes());
        h.extend_from_slice(&HEADER_LEN.to_le_bytes());
        h.extend_from_slice(&(4 * self.frame_count as u32).to_le_bytes());
        h.extend_from_slice(&0u32.to_le_bytes()); // stored WAV header bytes
        h.extend_from_slice(&(audio_bytes as u32).to_le_bytes());
        h.extend_from_slice(&((audio_bytes >> 32) as u32).to_le_bytes());
        h.extend_from_slice(&0u32.to_le_bytes()); // stored WAV tail bytes
        h.extend_from_slice(&[0u8; 16]); // MD5: not stored
        h.extend_from_slice(&LEVEL.to_le_bytes());
        h.extend_from_slice(&FLAG_CREATE_WAV_HEADER.to_le_bytes());
        h.extend_from_slice(&(FRAME_BLOCKS as u32).to_le_bytes());
        h.extend_from_slice(&last_blocks.to_le_bytes());
        h.extend_from_slice(&(self.frame_count as u32).to_le_bytes());
        h.extend_from_slice(&(self.bits as u16).to_le_bytes());
        h.extend_from_slice(&(self.params.channels() as u16).to_le_bytes());
        h.extend_from_slice(&self.params.rate.to_le_bytes());
        // seek table: where each frame starts in the file
        let mut at = self.header_len;
        for &size in &self.frame_sizes {
            h.extend_from_slice(&(at as u32).to_le_bytes());
            at += size as u64;
        }
        h
    }
}

impl BlockWriter for MonkeysWriter {
    fn write_block(&mut self, planar: &[Vec<f32>], frames: usize) -> std::io::Result<()> {
        let converted = to_integers(planar, &self.params.order, frames, self.bits);
        for (pending, new) in self.pending.iter_mut().zip(converted) {
            pending.extend(new);
        }
        let frames = take_frames(&mut self.pending, FRAME_BLOCKS, BATCH);
        self.write_frames(&frames)
    }

    fn finish(mut self: Box<Self>) -> std::io::Result<u64> {
        let frames = take_frames(&mut self.pending, FRAME_BLOCKS, 1);
        self.write_frames(&frames)?;
        if !self.pending[0].is_empty() {
            let frame: Vec<Vec<i32>> = self.pending.iter_mut().map(std::mem::take).collect();
            self.write_frames(&[frame])?;
        }
        debug_assert_eq!(self.frame_sizes.len(), self.frame_count);
        self.out.flush()?;
        let header = self.header();
        let total = header.len() as u64 + self.frame_sizes.iter().map(|&s| s as u64).sum::<u64>();
        let file = self.out.get_mut();
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&header)?;
        file.flush()?;
        Ok(total)
    }
}

/// One frame: CRC of the samples, then the range-coded residuals, swapped in 32-bit words.
fn encode_frame(channels: &[Vec<i32>], bits: u32) -> Vec<u8> {
    let mut crc = SampleCrc::new(bits);
    crc.update(channels);
    let crc = crc.raw();
    let stereo = channels.len() == 2;
    let blocks = channels[0].len();

    let mut predictor = Predictor::new();
    let mut filters: Vec<Chain> = channels.iter().map(|_| Chain::new(&FILTERS)).collect();
    let mut coders: Vec<ValueCoder> = channels.iter().map(|_| ValueCoder::new()).collect();
    let mut enc = RangeEncoder::new();

    for n in 0..blocks {
        if stereo {
            let (first, second) = predictor.stereo(channels[0][n], channels[1][n]);
            for (c, residual) in [first, second].into_iter().enumerate() {
                coders[c].encode(&mut enc, filters[c].encode(residual));
            }
        } else {
            let residual = predictor.mono(channels[0][n]);
            coders[0].encode(&mut enc, filters[0].encode(residual));
        }
    }

    let mut bytes = ((!crc) >> 1).to_be_bytes().to_vec();
    bytes.extend(enc.finish());
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
    for word in bytes.chunks_exact_mut(4) {
        word.reverse();
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_whole_words() {
        let tone: Vec<i32> = (0..5000).map(|n| ((n as f64 * 0.05).sin() * 3000.0) as i32).collect();
        assert_eq!(encode_frame(&[tone], 16).len() % 4, 0);
    }
}
