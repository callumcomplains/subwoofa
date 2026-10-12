//! WavPack (`.wv`), lossless, 16 or 24 bit, any number of channels.
//!
//! A WavPack file is a plain run of *blocks*. Each block holds a stretch of samples for one or two channels: a
//! 32-byte header, then sub-blocks (decorrelation terms, weights, histories, entropy state, the coded samples).
//! More than two channels are split into pairs (and a lone last channel); the blocks of one stretch of time are
//! written together, the first marked "initial" and the last "final".
//!
//! Per block and channel group the encoder
//! 1. optionally turns a stereo pair into mid/side,
//! 2. runs the signal through the decorrelation chain ([`predictor`]),
//! 3. codes the residuals ([`word_coder`]).
//!
//! The format is described in the WavPack file format documentation; the decoder in FFmpeg checks the result.

mod log_tables;
mod predictor;
mod word_coder;

use crate::{to_integers, BlockWriter, Params, SampleFormat};
use log_tables::log2;
use predictor::{Chain, Term};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use word_coder::Medians;

/// Version number written in every block header (4.x streams: 0x410).
const VERSION: u16 = 0x410;

/// Sample rates the header can name directly; anything else is sent as a 24-bit number.
const RATES: [u32; 15] = [6000, 8000, 9600, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000, 64000, 88200, 96000, 192000];
const CUSTOM_RATE: u32 = 15;

/// Header flag bits.
mod flag {
    pub const MONO: u32 = 0x4;
    pub const JOINT_STEREO: u32 = 0x10;
    pub const CROSS_DECORRELATION: u32 = 0x20;
    pub const INITIAL: u32 = 0x800;
    pub const FINAL: u32 = 0x1000;
    pub const MAGNITUDE_SHIFT: u32 = 18;
    pub const RATE_SHIFT: u32 = 23;
}

/// Sub-block ids.
mod id {
    pub const TERMS: u8 = 0x2;
    pub const WEIGHTS: u8 = 0x3;
    pub const HISTORY: u8 = 0x4;
    pub const ENTROPY: u8 = 0x5;
    pub const DATA: u8 = 0xA;
    pub const CHANNELS: u8 = 0xD;
    pub const RATE: u8 = 0x27;
    pub const ODD_SIZE: u8 = 0x40;
    pub const LONG: u8 = 0x80;
}

/// A mono channel or a stereo pair, with the state that carries over from block to block.
struct Group {
    /// Positions in the file's channel order.
    channels: Vec<usize>,
    chain: Chain,
    medians: [Medians; 2],
}

pub struct WavPackWriter {
    out: BufWriter<File>,
    params: Params,
    bits: u32,
    groups: Vec<Group>,
    /// Frames per block.
    span: usize,
    pending: Vec<Vec<i32>>,
    next_index: u64,
    written: u64,
}

impl WavPackWriter {
    pub fn create(path: &Path, params: Params) -> std::io::Result<WavPackWriter> {
        let bits = match params.format {
            SampleFormat::Int16 => 16,
            SampleFormat::Int24 => 24,
            _ => return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "WavPack here holds 16 or 24 bit integer samples")),
        };
        if params.total_frames > u32::MAX as u64 - 1 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "WavPack files hold at most 2^32 samples per channel"));
        }
        let groups = (0..params.channels())
            .collect::<Vec<_>>()
            .chunks(2)
            .map(|pair| {
                let stereo = pair.len() == 2;
                Group { channels: pair.to_vec(), chain: Chain::new(&predictor::preset(stereo), stereo), medians: [[0; 3]; 2] }
            })
            .collect();
        let span = (params.rate as usize / 2).clamp(4096, 100_000);
        Ok(WavPackWriter {
            out: BufWriter::new(File::create(path)?),
            pending: vec![Vec::new(); params.channels()],
            params,
            bits,
            groups,
            span,
            next_index: 0,
            written: 0,
        })
    }

    /// Writes the blocks of one stretch of time.
    fn write_frame(&mut self, frame: &[Vec<i32>]) -> std::io::Result<()> {
        let frames = frame[0].len() as u32;
        let last_group = self.groups.len() - 1;
        for g in 0..self.groups.len() {
            let mut flags = 0;
            if g == 0 {
                flags |= flag::INITIAL;
            }
            if g == last_group {
                flags |= flag::FINAL;
            }
            let block = self.build_block(g, frame, flags, frames);
            self.out.write_all(&block)?;
            self.written += block.len() as u64;
        }
        self.next_index += frames as u64;
        Ok(())
    }

    fn build_block(&mut self, g: usize, frame: &[Vec<i32>], mut flags: u32, frames: u32) -> Vec<u8> {
        let rate = self.params.rate;
        let total_samples = self.params.total_frames as u32;
        let channel_count = self.params.channels();
        let channel_mask = self.params.mask;
        let bits = self.bits;
        let group = &mut self.groups[g];
        let stereo = group.channels.len() == 2;

        let samples: Vec<&[i32]> = group.channels.iter().map(|&c| &frame[c][..]) .collect();
        let max_abs = samples.iter().flat_map(|s| s.iter()).map(|v| v.unsigned_abs()).max().unwrap_or(0);
        let crc = block_crc(&samples);

        flags |= (bits / 8 - 1) | (32 - max_abs.leading_zeros()).min(31) << flag::MAGNITUDE_SHIFT;
        flags |= rate_code(rate) << flag::RATE_SHIFT;
        if !stereo {
            flags |= flag::MONO;
        }

        let start = group.chain.start_block();
        if group.chain.terms().any(|t| t.delay < 0) {
            flags |= flag::CROSS_DECORRELATION;
        }
        // the decoder rebuilds the medians from their logarithms, so start from the rounded values
        for median in group.medians.iter_mut().flatten() {
            *median = log_tables::rounded(*median);
        }
        let entropy_start = group.medians;

        let residuals = if stereo {
            let joint = prefers_joint(samples[0], samples[1]);
            if joint {
                flags |= flag::JOINT_STEREO;
            }
            let mut out = Vec::with_capacity(samples[0].len() * 2);
            for (&l, &r) in samples[0].iter().zip(samples[1]) {
                let (l, r) = if joint { to_mid_side(l, r) } else { (l, r) };
                let (rl, rr) = group.chain.pair(l, r);
                out.push(rl);
                out.push(rr);
            }
            out
        } else {
            samples[0].iter().map(|&s| group.chain.mono(s)).collect()
        };
        let coded = word_coder::encode(&residuals, stereo, entropy_start);
        group.medians = coded.medians;

        let mut sub = Vec::new();
        if flags & flag::INITIAL != 0 && channel_count > 2 {
            push_sub_block(&mut sub, id::CHANNELS, &channel_info(channel_count, channel_mask));
        }
        if rate_code(rate) == CUSTOM_RATE {
            push_sub_block(&mut sub, id::RATE, &rate.to_le_bytes()[..3]);
        }
        push_sub_block(&mut sub, id::TERMS, &start.terms);
        push_sub_block(&mut sub, id::WEIGHTS, &start.weights);
        push_sub_block(&mut sub, id::HISTORY, &start.history);
        push_sub_block(&mut sub, id::ENTROPY, &entropy_bytes(&entropy_start, stereo));
        push_sub_block(&mut sub, id::DATA, &coded.bytes);

        let mut block = Vec::with_capacity(32 + sub.len());
        block.extend_from_slice(b"wvpk");
        block.extend_from_slice(&((24 + sub.len()) as u32).to_le_bytes());
        block.extend_from_slice(&VERSION.to_le_bytes());
        block.push((self.next_index >> 32) as u8);
        block.push((self.params.total_frames >> 32) as u8);
        block.extend_from_slice(&total_samples.to_le_bytes());
        block.extend_from_slice(&(self.next_index as u32).to_le_bytes());
        block.extend_from_slice(&frames.to_le_bytes());
        block.extend_from_slice(&flags.to_le_bytes());
        block.extend_from_slice(&crc.to_le_bytes());
        block.extend_from_slice(&sub);
        block
    }
}

impl BlockWriter for WavPackWriter {
    fn write_block(&mut self, planar: &[Vec<f32>], frames: usize) -> std::io::Result<()> {
        let converted = to_integers(planar, &self.params.order, frames, self.bits);
        for (pending, new) in self.pending.iter_mut().zip(converted) {
            pending.extend(new);
        }
        while self.pending[0].len() >= self.span {
            let frame: Vec<Vec<i32>> = self.pending.iter_mut().map(|p| p.drain(..self.span).collect()).collect();
            self.write_frame(&frame)?;
        }
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> std::io::Result<u64> {
        if !self.pending[0].is_empty() {
            let frame: Vec<Vec<i32>> = self.pending.iter_mut().map(std::mem::take).collect();
            self.write_frame(&frame)?;
        }
        self.out.flush()?;
        Ok(self.written)
    }
}

/// Mid/side as WavPack defines it: `side = left - right`, `mid = right + side / 2` (undone exactly by the decoder).
fn to_mid_side(left: i32, right: i32) -> (i32, i32) {
    let side = left.wrapping_sub(right);
    (side, right.wrapping_add(side >> 1))
}

/// Joint stereo helps when the channels are alike: compare the size of the plain and the mid/side signals.
fn prefers_joint(left: &[i32], right: &[i32]) -> bool {
    let size = |a: i64, b: i64| (a + b).max(1);
    let plain = size(left.iter().map(|&v| log2_cost(v)).sum(), right.iter().map(|&v| log2_cost(v)).sum());
    let joint = size(
        left.iter().zip(right).map(|(&l, &r)| log2_cost(to_mid_side(l, r).0)).sum(),
        left.iter().zip(right).map(|(&l, &r)| log2_cost(to_mid_side(l, r).1)).sum(),
    );
    joint < plain
}

/// A rough price in bits of coding a value without prediction.
fn log2_cost(value: i32) -> i64 {
    (32 - value.unsigned_abs().leading_zeros()) as i64
}

/// The block checksum the decoder recomputes from the final samples.
fn block_crc(samples: &[&[i32]]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for n in 0..samples[0].len() {
        for channel in samples {
            crc = crc.wrapping_mul(3).wrapping_add(channel[n] as u32);
        }
    }
    crc
}

fn rate_code(rate: u32) -> u32 {
    RATES.iter().position(|&r| r == rate).map_or(CUSTOM_RATE, |i| i as u32)
}

/// Channel count and speaker mask for blocks of files with more than two channels.
fn channel_info(count: usize, mask: u32) -> Vec<u8> {
    let mut bytes = vec![count as u8];
    bytes.extend_from_slice(&mask.to_le_bytes()[..3]);
    bytes
}

/// Medians as stored: the logarithm of each, channel by channel.
fn entropy_bytes(medians: &[Medians; 2], stereo: bool) -> Vec<u8> {
    let channels = if stereo { 2 } else { 1 };
    medians[..channels].iter().flatten().flat_map(|&m| (log2(m.max(0) as u32) as u16).to_le_bytes()).collect()
}

/// Appends a sub-block: id, size in 16-bit words, the data, padded to a whole word.
fn push_sub_block(out: &mut Vec<u8>, id: u8, data: &[u8]) {
    let words = data.len().div_ceil(2);
    let mut id = id;
    if data.len() % 2 == 1 {
        id |= self::id::ODD_SIZE;
    }
    if words > 255 {
        out.push(id | self::id::LONG);
        out.extend_from_slice(&(words as u32).to_le_bytes()[..3]);
    } else {
        out.push(id);
        out.push(words as u8);
    }
    out.extend_from_slice(data);
    if data.len() % 2 == 1 {
        out.push(0);
    }
}
