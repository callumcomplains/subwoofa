//! ALAC (Apple Lossless) in an M4A file: 16 or 24 bit, 1 to 8 channels in Apple's channel arrangements.
//!
//! Audio is cut into frames of 4096 samples. Each channel (or channel pair) goes through three stages:
//!
//! 1. *Stereo decorrelation* (pairs only): left/right, left/side, right/side or mid/side, chosen by trial.
//! 2. *Prediction*: a short adaptive linear predictor. Its starting coefficients are fitted to the frame and sent;
//!    both encoder and decoder then adjust them sample by sample (sign-sign LMS), so the decoder must repeat the
//!    encoder's arithmetic exactly. 24-bit audio keeps its lowest byte aside, raw, as the format prescribes.
//! 3. *Entropy coding*: adaptive Golomb-Rice codes whose parameter follows a running average of recent magnitudes,
//!    with a run-length code for stretches of zeros.

mod entropy;
mod frame;
mod layout;
mod predict;
mod search;
mod stereo;

pub use layout::alac_order;

use crate::mp4_audio::{self, atom};
use crate::{take_frames, to_integers, BlockWriter, Params, SampleFormat};
use frame::encode_frame;
use rayon::prelude::*;
use search::Search;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

/// Samples per frame.
const FRAME_SAMPLES: usize = 4096;
/// Rice coder settings stored in the file header and used by every frame.
const HISTORY_MULT: u32 = 40;
const INITIAL_HISTORY: u32 = 10;
const K_MODIFIER: u32 = 14;
/// Per-frame scaling of the history multiplier, in quarters (4 = use `HISTORY_MULT` as it is).
const RICE_MODIFIER: u32 = 4;
/// Fractional bits of the predictor coefficients.
const PREDICTOR_SHIFT: u32 = 9;
// ---------------------------------------------------------------------------------------------------------------
// entropy coding

// ---------------------------------------------------------------------------------------------------------------
// prediction

// ---------------------------------------------------------------------------------------------------------------
// stereo decorrelation

// ---------------------------------------------------------------------------------------------------------------
// frames

// ---------------------------------------------------------------------------------------------------------------
// the M4A file

pub struct AlacWriter {
    out: BufWriter<File>,
    params: Params,
    bits: u32,
    /// Samples collected for the next frame, per channel in ALAC order.
    pending: Vec<Vec<i32>>,
    frame_sizes: Vec<u32>,
    start: mp4_audio::Start,
    layout_tag: u32,
    samples_done: u64,
}

impl AlacWriter {
    pub fn create(path: &Path, params: Params, layout_tag: u32) -> std::io::Result<AlacWriter> {
        let bits = match params.format {
            SampleFormat::Int16 => 16,
            SampleFormat::Int24 => 24,
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "ALAC holds 16 or 24 bit integer samples",
                ))
            }
        };
        let mut out = BufWriter::new(File::create(path)?);
        let start = mp4_audio::write_start(&mut out, b"M4A ")?;
        Ok(AlacWriter {
            out,
            bits,
            pending: vec![Vec::new(); params.channels()],
            frame_sizes: Vec::new(),
            start,
            layout_tag,
            samples_done: 0,
            params,
        })
    }

    /// Encodes a batch of frames side by side, then writes them in order.
    fn write_frames(&mut self, blocks: &[Vec<Vec<i32>>]) -> std::io::Result<()> {
        let bits = self.bits;
        let search = Search::for_effort(self.params.effort);
        let frames: Vec<Vec<u8>> = blocks
            .par_iter()
            .map(|block| encode_frame(block, bits, search))
            .collect();
        for (block, bytes) in blocks.iter().zip(frames) {
            self.frame_sizes.push(bytes.len() as u32);
            self.samples_done += block[0].len() as u64;
            self.out.write_all(&bytes)?;
        }
        Ok(())
    }

    /// The boxes inside the sample entry: the `alac` description (Apple's cookie) and a channel layout box.
    fn entry_boxes(&self) -> Vec<u8> {
        let p = &self.params;
        let total = self.samples_done;
        let max_frame = self.frame_sizes.iter().copied().max().unwrap_or(0);
        let bytes: u64 = self.frame_sizes.iter().map(|&s| s as u64).sum();
        let average_bitrate = (bytes * 8 * p.rate as u64).checked_div(total).unwrap_or(0) as u32;

        let mut cookie = Vec::new();
        cookie.extend_from_slice(&0u32.to_be_bytes()); // version and flags
        cookie.extend_from_slice(&(FRAME_SAMPLES as u32).to_be_bytes());
        cookie.push(0); // compatible version
        cookie.push(self.bits as u8);
        cookie.push(HISTORY_MULT as u8);
        cookie.push(INITIAL_HISTORY as u8);
        cookie.push(K_MODIFIER as u8);
        cookie.push(p.channels() as u8);
        cookie.extend_from_slice(&255u16.to_be_bytes()); // longest run
        cookie.extend_from_slice(&max_frame.to_be_bytes());
        cookie.extend_from_slice(&average_bitrate.to_be_bytes());
        cookie.extend_from_slice(&p.rate.to_be_bytes());
        let mut boxes = atom(b"alac", &cookie);
        if p.channels() > 2 {
            let mut chan = Vec::new();
            chan.extend_from_slice(&0u32.to_be_bytes());
            chan.extend_from_slice(&self.layout_tag.to_be_bytes());
            chan.extend_from_slice(&0u32.to_be_bytes()); // bitmap
            chan.extend_from_slice(&0u32.to_be_bytes()); // no descriptions
            boxes.extend_from_slice(&atom(b"chan", &chan));
        }
        boxes
    }
}

impl BlockWriter for AlacWriter {
    fn write_block(&mut self, planar: &[Vec<f32>], frames: usize) -> std::io::Result<()> {
        let converted = to_integers(planar, &self.params.order, frames, self.bits);
        for (pending, new) in self.pending.iter_mut().zip(converted) {
            pending.extend(new);
        }
        let blocks = take_frames(&mut self.pending, FRAME_SAMPLES, 1);
        self.write_frames(&blocks)
    }

    fn finish(mut self: Box<Self>) -> std::io::Result<u64> {
        if !self.pending[0].is_empty() {
            let block: Vec<Vec<i32>> = self.pending.iter_mut().map(std::mem::take).collect();
            self.write_frames(&[block])?;
        }
        let track = mp4_audio::Track {
            rate: self.params.rate,
            channels: self.params.channels() as u16,
            bits: self.bits as u16,
            frame_samples: FRAME_SAMPLES as u32,
            total_samples: self.samples_done,
            frame_sizes: &self.frame_sizes,
            entry_name: b"alac",
            entry_boxes: self.entry_boxes(),
        };
        mp4_audio::finish(&mut self.out, &self.start, &track)
    }
}
