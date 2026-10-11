//! DSD (Direct Stream Digital, the audio of SACD) as files: `.dsf` and `.dff` (DSDIFF).
//!
//! Each channel is converted from PCM like this: sample rate conversion up to the DSD rate (a rational step to the
//! 44.1 kHz family when the source is in the 48 kHz family, then doublings), a gain of one half so the loop stays
//! stable, and a fifth-order sigma-delta modulator. The bits are then packed the way the file format wants them.
//!
//! Only the audio is written: a disc image of an SACD needs the proprietary Scarlet Book structures and DST
//! compression, which are not covered.

mod dsp;

use crate::{BlockWriter, Params};
use dsp::{Modulator, Resampler};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

/// DSD64 is 64 times 44.1 kHz.
const BASE_RATE: u32 = 44_100;
/// Input level that keeps the one-bit loop stable (half of full scale, -6 dB).
const INPUT_GAIN: f64 = 0.5;
/// Bytes per channel in a DSF block.
const DSF_BLOCK: usize = 4096;
/// A pattern of bits that decodes to silence, used to pad the end.
const IDLE_PATTERN: u8 = 0x69;

/// The conversion of one channel.
struct ChannelChain {
    stages: Vec<Resampler>,
    modulator: Modulator,
    /// Output bits still to be dropped (the delay of the filters).
    skip: usize,
    /// Bits produced so far, after the skip.
    produced: usize,
    total_bits: usize,
}

impl ChannelChain {
    fn new(input_rate: u32, dsd_rate: u32, total_bits: usize) -> Result<ChannelChain, String> {
        let mut stages = Vec::new();
        // bring the source to the 44.1 kHz family
        let (mut rate, edge_family) = if input_rate % BASE_RATE == 0 {
            (input_rate as f64, input_rate as f64)
        } else if input_rate % 48_000 == 0 {
            let target = input_rate as f64 * 147.0 / 160.0;
            stages.push(Resampler::new(input_rate as f64, 147, 160, 0.9 * target / 2.0, target / 2.0));
            (target, target)
        } else {
            return Err(format!("{input_rate} Hz is not a multiple of 44.1 or 48 kHz"));
        };
        // double up to the DSD rate
        let pass = 0.9 * edge_family / 2.0;
        while (rate - dsd_rate as f64).abs() > 1.0 {
            if rate > dsd_rate as f64 {
                return Err(format!("a {input_rate} Hz source is too fast for DSD at {dsd_rate} Hz"));
            }
            stages.push(Resampler::new(rate, 2, 1, pass, rate - pass));
            rate *= 2.0;
        }
        let delay: f64 = stages.iter().map(|s| s.delay_seconds()).sum();
        Ok(ChannelChain { stages, modulator: Modulator::new(), skip: (delay * dsd_rate as f64).round() as usize, produced: 0, total_bits })
    }

    /// Converts input samples to bits (as 0/1 bytes), up to the total the file wants.
    fn process(&mut self, input: &[f64], bits: &mut Vec<u8>) {
        let mut signal: Vec<f64> = input.iter().map(|v| v * INPUT_GAIN).collect();
        for stage in &mut self.stages {
            signal = stage.process(&signal);
        }
        for x in signal {
            let bit = self.modulator.step(x);
            if self.skip > 0 {
                self.skip -= 1;
                continue;
            }
            if self.produced < self.total_bits {
                bits.push(u8::from(bit));
                self.produced += 1;
            }
        }
    }

    fn missing(&self) -> usize {
        self.total_bits - self.produced
    }
}

/// How the channels of a layout are numbered in the two file formats.
pub struct DsdLayout {
    /// Layout channel index for every channel of the file, in file order.
    pub order: Vec<usize>,
    /// DSF channel type number.
    pub dsf_type: u32,
    /// DSDIFF channel identifiers.
    pub dff_ids: Vec<&'static [u8; 4]>,
}

/// The channel arrangements the two formats define, or `None` for other layouts.
pub fn dsd_layout(layout: &sw_core::ChannelLayout) -> Option<DsdLayout> {
    use sw_core::Channel::*;
    let (wanted, dsf_type, ids): (&[sw_core::Channel], u32, Vec<&'static [u8; 4]>) = match layout.channels.len() {
        1 => (&[C], 1, vec![b"C   "]),
        2 => (&[L, R], 2, vec![b"SLFT", b"SRGT"]),
        3 => (&[L, R, C], 3, vec![b"MLFT", b"MRGT", b"C   "]),
        4 if layout.channels.contains(&Lfe) => (&[L, R, C, Lfe], 5, vec![b"MLFT", b"MRGT", b"C   ", b"LFE "]),
        4 => (&[L, R, Ls, Rs], 4, vec![b"MLFT", b"MRGT", b"LS  ", b"RS  "]),
        5 => (&[L, R, C, Ls, Rs], 6, vec![b"MLFT", b"MRGT", b"C   ", b"LS  ", b"RS  "]),
        6 => (&[L, R, C, Lfe, Ls, Rs], 7, vec![b"MLFT", b"MRGT", b"C   ", b"LFE ", b"LS  ", b"RS  "]),
        _ => return None,
    };
    let order: Option<Vec<usize>> = wanted.iter().map(|w| layout.channels.iter().position(|c| c == w)).collect();
    order.map(|order| DsdLayout { order, dsf_type, dff_ids: ids })
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DsdFile {
    Dsf,
    Dff,
}

pub struct DsdWriter {
    out: BufWriter<File>,
    file: DsdFile,
    params: Params,
    chains: Vec<ChannelChain>,
    /// Bits waiting to be packed, per file channel.
    queues: Vec<Vec<u8>>,
    total_bits: usize,
    bytes_written: u64,
    header_len: u64,
}

impl DsdWriter {
    pub fn create(path: &Path, params: Params, file: DsdFile, layout: &DsdLayout, multiplier: u32) -> std::io::Result<DsdWriter> {
        let dsd_rate = BASE_RATE * multiplier;
        let total_bits = (params.total_frames as u128 * dsd_rate as u128 + params.rate as u128 / 2) as u128 / params.rate as u128;
        let total_bits = total_bits as usize;
        let chains: Result<Vec<ChannelChain>, String> = (0..params.channels()).map(|_| ChannelChain::new(params.rate, dsd_rate, total_bits)).collect();
        let chains = chains.map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
        let mut out = BufWriter::new(File::create(path)?);
        let header = match file {
            DsdFile::Dsf => dsf_header(&params, layout, dsd_rate, total_bits),
            DsdFile::Dff => dff_header(&params, layout, dsd_rate, total_bits),
        };
        out.write_all(&header)?;
        let channels = params.channels();
        Ok(DsdWriter { out, file, params, chains, queues: vec![Vec::new(); channels], total_bits, bytes_written: 0, header_len: header.len() as u64 })
    }

    /// Writes whatever whole blocks (DSF) or bytes (DSDIFF) the queues hold; with `last`, pads the rest with idle bits
    /// and writes that too.
    fn drain(&mut self, last: bool) -> std::io::Result<()> {
        let channels = self.queues.len();
        match self.file {
            DsdFile::Dsf => {
                let block_bits = DSF_BLOCK * 8;
                // whole blocks, one per channel in turn
                while self.queues.iter().all(|q| q.len() >= block_bits) {
                    self.write_dsf_block(block_bits)?;
                }
                if last && !self.queues[0].is_empty() {
                    for q in self.queues.iter_mut() {
                        // idle bits, in the order an LSB-first byte holds them
                        while q.len() < block_bits {
                            let n = q.len();
                            q.push((IDLE_PATTERN >> (n % 8)) & 1);
                        }
                    }
                    self.write_dsf_block(block_bits)?;
                }
            }
            DsdFile::Dff => {
                let take = if last { self.queues[0].len().div_ceil(8) } else { self.queues.iter().map(|q| q.len()).min().unwrap_or(0) / 8 };
                for q in self.queues.iter_mut() {
                    while last && q.len() < take * 8 {
                        let n = q.len();
                        q.push((IDLE_PATTERN >> (7 - n % 8)) & 1);
                    }
                }
                let mut bytes = Vec::with_capacity(take * channels);
                for i in 0..take {
                    for q in &self.queues {
                        // most significant bit first
                        bytes.push(q[i * 8..i * 8 + 8].iter().fold(0u8, |acc, &bit| (acc << 1) | bit));
                    }
                }
                for q in self.queues.iter_mut() {
                    q.drain(..take * 8);
                }
                self.out.write_all(&bytes)?;
                self.bytes_written += bytes.len() as u64;
            }
        }
        Ok(())
    }

    /// One DSF block per channel: 4096 bytes, the first sample in the lowest bit.
    fn write_dsf_block(&mut self, block_bits: usize) -> std::io::Result<()> {
        for q in self.queues.iter_mut() {
            let bytes: Vec<u8> = q[..block_bits].chunks(8).map(|b| b.iter().enumerate().fold(0u8, |acc, (i, &bit)| acc | (bit << i))).collect();
            q.drain(..block_bits);
            self.out.write_all(&bytes)?;
            self.bytes_written += bytes.len() as u64;
        }
        Ok(())
    }
}

impl BlockWriter for DsdWriter {
    fn write_block(&mut self, planar: &[Vec<f32>], frames: usize) -> std::io::Result<()> {
        use rayon::prelude::*;
        let order = self.params.order.clone();
        let results: Vec<Vec<u8>> = self
            .chains
            .par_iter_mut()
            .zip(order.par_iter())
            .map(|(chain, &src)| {
                let input: Vec<f64> = planar[src][..frames].iter().map(|&v| v as f64).collect();
                let mut bits = Vec::new();
                chain.process(&input, &mut bits);
                bits
            })
            .collect();
        for (q, bits) in self.queues.iter_mut().zip(results) {
            q.extend(bits);
        }
        self.drain(false)
    }

    fn finish(mut self: Box<Self>) -> std::io::Result<u64> {
        use rayon::prelude::*;
        // push zeros through until the filters have delivered the tail
        while self.chains[0].missing() > 0 {
            let more = vec![0f64; 4096];
            let results: Vec<Vec<u8>> = self
                .chains
                .par_iter_mut()
                .map(|chain| {
                    let mut bits = Vec::new();
                    chain.process(&more, &mut bits);
                    bits
                })
                .collect();
            for (q, bits) in self.queues.iter_mut().zip(results) {
                q.extend(bits);
            }
        }
        self.drain(false)?;
        self.drain(true)?;
        if self.file == DsdFile::Dff && self.bytes_written % 2 == 1 {
            self.out.write_all(&[0])?; // chunks have an even length
            self.bytes_written += 1;
        }
        self.out.flush()?;
        Ok(self.header_len + self.bytes_written)
    }
}

/// DSF: a `DSD ` chunk, a `fmt ` chunk and the `data` chunk with the channels in blocks of 4096 bytes, bits least
/// significant first.
fn dsf_header(p: &Params, layout: &DsdLayout, dsd_rate: u32, total_bits: usize) -> Vec<u8> {
    let channels = p.channels();
    let bytes_per_channel = total_bits.div_ceil(8);
    let blocks = bytes_per_channel.div_ceil(DSF_BLOCK);
    let data_bytes = (blocks * DSF_BLOCK * channels) as u64;
    let file_size = 28 + 52 + 12 + data_bytes;
    let mut h = Vec::new();
    h.extend_from_slice(b"DSD ");
    h.extend_from_slice(&28u64.to_le_bytes());
    h.extend_from_slice(&file_size.to_le_bytes());
    h.extend_from_slice(&0u64.to_le_bytes()); // no metadata chunk
    h.extend_from_slice(b"fmt ");
    h.extend_from_slice(&52u64.to_le_bytes());
    h.extend_from_slice(&1u32.to_le_bytes()); // format version
    h.extend_from_slice(&0u32.to_le_bytes()); // raw DSD
    h.extend_from_slice(&layout.dsf_type.to_le_bytes());
    h.extend_from_slice(&(channels as u32).to_le_bytes());
    h.extend_from_slice(&dsd_rate.to_le_bytes());
    h.extend_from_slice(&1u32.to_le_bytes()); // bits per sample: 1 means least significant bit first
    h.extend_from_slice(&(total_bits as u64).to_le_bytes());
    h.extend_from_slice(&(DSF_BLOCK as u32).to_le_bytes());
    h.extend_from_slice(&0u32.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&(12 + data_bytes).to_le_bytes());
    h
}

/// DSDIFF: a `FRM8` form holding the version, the properties (rate, channels, no compression) and the `DSD ` chunk
/// with one byte per channel in turn, bits most significant first.
fn dff_header(p: &Params, layout: &DsdLayout, dsd_rate: u32, total_bits: usize) -> Vec<u8> {
    let channels = p.channels();
    let data_bytes = (total_bits.div_ceil(8) * channels) as u64;
    let chunk = |id: &[u8; 4], body: &[u8]| -> Vec<u8> {
        let mut c = Vec::new();
        c.extend_from_slice(id);
        c.extend_from_slice(&(body.len() as u64).to_be_bytes());
        c.extend_from_slice(body);
        if body.len() % 2 == 1 {
            c.push(0);
        }
        c
    };
    let mut chnl = Vec::new();
    chnl.extend_from_slice(&(channels as u16).to_be_bytes());
    for id in &layout.dff_ids {
        chnl.extend_from_slice(*id);
    }
    let mut cmpr = Vec::new();
    cmpr.extend_from_slice(b"DSD ");
    let name = b"not compressed";
    cmpr.push(name.len() as u8);
    cmpr.extend_from_slice(name);
    let mut prop = Vec::new();
    prop.extend_from_slice(b"SND ");
    prop.extend_from_slice(&chunk(b"FS  ", &dsd_rate.to_be_bytes()));
    prop.extend_from_slice(&chunk(b"CHNL", &chnl));
    prop.extend_from_slice(&chunk(b"CMPR", &cmpr));
    let mut body = Vec::new();
    body.extend_from_slice(b"DSD ");
    body.extend_from_slice(&chunk(b"FVER", &0x0105_0000u32.to_be_bytes()));
    body.extend_from_slice(&chunk(b"PROP", &prop));
    // the DSD chunk: its header only, the data follows
    let padded = data_bytes + data_bytes % 2;
    let form_size = body.len() as u64 + 12 + padded;
    let mut h = Vec::new();
    h.extend_from_slice(b"FRM8");
    h.extend_from_slice(&form_size.to_be_bytes());
    h.extend_from_slice(&body);
    h.extend_from_slice(b"DSD ");
    h.extend_from_slice(&data_bytes.to_be_bytes());
    h
}
