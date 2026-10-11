//! FLAC (lossless), 16 or 24 bits, up to 8 channels.
//!
//! Audio is cut into blocks of equal length. In every block each channel is coded with the cheapest of: a constant, the
//! raw samples, a fixed polynomial predictor (orders 0 to 4), or a linear predictor fitted to the block (orders 2 to
//! 12). The prediction error is then Rice coded in partitions, each with its own parameter. Stereo blocks also try
//! mid/side and left/side and right/side pairs and keep the smallest.

use crate::bitwriter::BitWriter;
use crate::ogg::{build_page, PacketWriter};
use crate::{to_integers, BlockWriter, Params, SampleFormat};
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

/// Highest linear predictor order tried.
const MAX_LPC_ORDER: usize = 12;
/// Predictor orders fitted to each block.
const LPC_ORDERS: [usize; 4] = [2, 4, 8, 12];
/// Precision (bits) of the quantised predictor coefficients.
const LPC_PRECISION: u32 = 15;
/// Largest partition order tried (blocks are split into up to 2^8 Rice partitions).
const MAX_PARTITION_ORDER: u32 = 8;

// ---------------------------------------------------------------------------------------------------------------
// bit and checksum helpers

fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
        }
    }
    crc
}

fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x8005 } else { crc << 1 };
        }
    }
    crc
}

/// Zigzag mapping of a signed residual to an unsigned number (0, -1, 1, -2 ... becomes 0, 1, 2, 3 ...).
fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

// ---------------------------------------------------------------------------------------------------------------
// Rice coding of a residual

/// How a residual is split into partitions and what Rice parameter each uses.
struct RiceCoding {
    partition_order: u32,
    parameters: Vec<u32>,
    /// Parameters take 5 bits instead of 4 (needed when one exceeds 14).
    wide: bool,
    bits: usize,
}

/// The cheapest partitioning of `residual`, which follows `predictor_order` warm-up samples in a block of
/// `block_len` samples.
fn rice_code(residual: &[i32], predictor_order: usize, block_len: usize) -> RiceCoding {
    let coded: Vec<u64> = residual.iter().map(|&r| zigzag(r as i64)).collect();
    let mut best: Option<RiceCoding> = None;
    for order in 0..=MAX_PARTITION_ORDER {
        let parts = 1usize << order;
        if block_len % parts != 0 || block_len / parts <= predictor_order {
            break;
        }
        let size = block_len / parts;
        let mut parameters = Vec::with_capacity(parts);
        let mut payload = 0usize;
        let mut start = 0usize;
        for p in 0..parts {
            let end = if p == 0 { size - predictor_order } else { start + size };
            let part = &coded[start..end];
            let (k, bits) = best_parameter(part);
            parameters.push(k);
            payload += bits;
            start = end;
        }
        let wide = parameters.iter().any(|&k| k > 14);
        let bits = 2 + 4 + parts * (4 + usize::from(wide)) + payload;
        if best.as_ref().map_or(true, |b| bits < b.bits) {
            best = Some(RiceCoding { partition_order: order, parameters, wide, bits });
        }
    }
    best.expect("partition order 0 always fits")
}

/// Rice parameter minimising the size of `values` and that size in bits.
fn best_parameter(values: &[u64]) -> (u32, usize) {
    let sum: u64 = values.iter().sum();
    let mean = sum / values.len().max(1) as u64;
    let guess = 64 - mean.leading_zeros(); // about log2 of the mean
    let mut best = (0u32, usize::MAX);
    for k in guess.saturating_sub(2)..=(guess + 1).min(30) {
        let bits: usize = values.iter().map(|&v| (v >> k) as usize + 1 + k as usize).sum();
        if bits < best.1 {
            best = (k, bits);
        }
    }
    best
}

fn write_rice(w: &mut BitWriter, residual: &[i32], predictor_order: usize, block_len: usize, coding: &RiceCoding) {
    w.put(u64::from(coding.wide), 2);
    w.put(coding.partition_order as u64, 4);
    let parts = 1usize << coding.partition_order;
    let size = block_len / parts;
    let mut start = 0usize;
    for p in 0..parts {
        let end = if p == 0 { size - predictor_order } else { start + size };
        let k = coding.parameters[p];
        w.put(k as u64, if coding.wide { 5 } else { 4 });
        for &r in &residual[start..end] {
            w.put_rice(zigzag(r as i64), k);
        }
        start = end;
    }
}

// ---------------------------------------------------------------------------------------------------------------
// one channel of one block

enum Predictor {
    Constant(i32),
    Verbatim,
    Fixed { order: usize },
    Linear { order: usize, shift: u32, coefficients: Vec<i32> },
}

/// A channel of a block, coded.
struct Subframe {
    /// Low bits that were zero in every sample and are sent once instead.
    wasted: u32,
    /// Bits per sample after removing the wasted ones.
    bits_per_sample: u32,
    predictor: Predictor,
    samples: Vec<i32>,
    residual: Vec<i32>,
    rice: Option<RiceCoding>,
    /// Size in bits, header included.
    bits: usize,
}

/// Codes one channel with the smallest predictor. `sample_bits` is the sample width of this channel (one more for the
/// side channel of a stereo pair).
fn code_channel(samples: &[i32], sample_bits: u32) -> Subframe {
    let n = samples.len();
    let or_all = samples.iter().fold(0i32, |a, &s| a | s);
    if or_all == 0 {
        return Subframe { wasted: 0, bits_per_sample: sample_bits, predictor: Predictor::Constant(0), samples: Vec::new(), residual: Vec::new(), rice: None, bits: 8 + sample_bits as usize };
    }
    let wasted = or_all.trailing_zeros();
    let x: Vec<i32> = samples.iter().map(|&s| s >> wasted).collect();
    let bps = sample_bits - wasted;
    let header = 8 + wasted as usize; // type byte (with the wasted flag) and the unary wasted-bits count
    let mut best = Subframe { wasted, bits_per_sample: bps, predictor: Predictor::Verbatim, samples: x.clone(), residual: Vec::new(), rice: None, bits: header + n * bps as usize };
    if x.iter().all(|&v| v == x[0]) {
        best.predictor = Predictor::Constant(x[0]);
        best.bits = header + bps as usize;
        return best;
    }
    // fixed polynomial predictors
    for order in 0..=4usize.min(n.saturating_sub(1)) {
        let residual = fixed_residual(&x, order);
        let rice = rice_code(&residual, order, n);
        let bits = header + order * bps as usize + rice.bits;
        if bits < best.bits {
            best = Subframe { wasted, bits_per_sample: bps, predictor: Predictor::Fixed { order }, samples: x.clone(), residual, rice: Some(rice), bits };
        }
    }
    // linear predictors fitted to this block
    if n >= 64 {
        for (order, coefficients, shift) in fit_linear(&x) {
            let residual = linear_residual(&x, &coefficients, shift);
            let rice = rice_code(&residual, order, n);
            let bits = header + 4 + 5 + order * LPC_PRECISION as usize + order * bps as usize + rice.bits;
            if bits < best.bits {
                best = Subframe { wasted, bits_per_sample: bps, predictor: Predictor::Linear { order, shift, coefficients }, samples: x.clone(), residual, rice: Some(rice), bits };
            }
        }
    }
    best
}

fn fixed_residual(x: &[i32], order: usize) -> Vec<i32> {
    // coefficients of the fixed predictors, newest sample first
    const COEFFICIENTS: [&[i64]; 5] = [&[], &[1], &[2, -1], &[3, -3, 1], &[4, -6, 4, -1]];
    let c = COEFFICIENTS[order];
    (order..x.len())
        .map(|i| {
            let prediction: i64 = c.iter().enumerate().map(|(j, &cj)| cj * x[i - 1 - j] as i64).sum();
            (x[i] as i64 - prediction) as i32
        })
        .collect()
}

fn linear_residual(x: &[i32], coefficients: &[i32], shift: u32) -> Vec<i32> {
    let order = coefficients.len();
    (order..x.len())
        .map(|i| {
            let prediction: i64 = coefficients.iter().enumerate().map(|(j, &c)| c as i64 * x[i - 1 - j] as i64).sum();
            (x[i] as i64 - (prediction >> shift)) as i32
        })
        .collect()
}

/// Linear predictors of the orders in `LPC_ORDERS`: the block is windowed, its autocorrelation is turned into
/// predictor coefficients with the Levinson-Durbin recursion, and the coefficients are rounded to integers with a
/// common shift. Returns (order, coefficients, shift) for each.
fn fit_linear(x: &[i32]) -> Vec<(usize, Vec<i32>, u32)> {
    let n = x.len();
    // Tukey window (flat in the middle, cosine ramps over the outer quarter at each end)
    let ramp = (n / 4).max(1);
    let windowed: Vec<f64> = x
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let edge = i.min(n - 1 - i);
            let w = if edge < ramp { 0.5 - 0.5 * (std::f64::consts::PI * edge as f64 / ramp as f64).cos() } else { 1.0 };
            s as f64 * w
        })
        .collect();
    let mut autocorrelation = [0f64; MAX_LPC_ORDER + 1];
    for (lag, a) in autocorrelation.iter_mut().enumerate() {
        *a = windowed[lag..].iter().zip(&windowed).map(|(p, q)| p * q).sum();
    }
    if autocorrelation[0] <= 0.0 {
        return Vec::new();
    }
    autocorrelation[0] *= 1.0 + 1e-9; // keeps the recursion stable on very tonal blocks

    // Levinson-Durbin: coefficient sets for every order up to the maximum
    let mut sets: Vec<Vec<f64>> = vec![Vec::new()];
    let mut current: Vec<f64> = Vec::new();
    let mut error = autocorrelation[0];
    for order in 1..=MAX_LPC_ORDER {
        let acc: f64 = autocorrelation[order] - current.iter().enumerate().map(|(j, c)| c * autocorrelation[order - 1 - j]).sum::<f64>();
        let reflection = acc / error;
        let mut next = vec![0f64; order];
        for j in 0..order - 1 {
            next[j] = current[j] - reflection * current[order - 2 - j];
        }
        next[order - 1] = reflection;
        error *= 1.0 - reflection * reflection;
        current = next;
        sets.push(current.clone());
        if error <= 0.0 {
            break;
        }
    }
    LPC_ORDERS
        .iter()
        .filter(|&&order| order < sets.len() && order < n)
        .map(|&order| {
            let (coefficients, shift) = quantise(&sets[order]);
            (order, coefficients, shift)
        })
        .collect()
}

/// Rounds predictor coefficients to `LPC_PRECISION`-bit integers sharing one right shift.
fn quantise(coefficients: &[f64]) -> (Vec<i32>, u32) {
    let largest = coefficients.iter().fold(0f64, |m, c| m.max(c.abs())).max(1e-12);
    let integer_bits = largest.log2().floor() as i32 + 1; // largest < 2^integer_bits
    let shift = (LPC_PRECISION as i32 - 1 - integer_bits).clamp(0, 15);
    let limit = (1i64 << (LPC_PRECISION - 1)) - 1;
    let scale = (1u64 << shift) as f64;
    let q = coefficients.iter().map(|&c| ((c * scale).round() as i64).clamp(-limit - 1, limit) as i32).collect();
    (q, shift as u32)
}

fn write_subframe(w: &mut BitWriter, s: &Subframe) {
    let type_code: u64 = match &s.predictor {
        Predictor::Constant(_) => 0,
        Predictor::Verbatim => 1,
        Predictor::Fixed { order } => 8 + *order as u64,
        Predictor::Linear { order, .. } => 32 + (*order as u64 - 1),
    };
    w.put(0, 1); // padding bit
    w.put(type_code, 6);
    if s.wasted > 0 {
        w.put(1, 1);
        w.put(0, s.wasted - 1); // wasted bits - 1 zeros, then a one
        w.put(1, 1);
    } else {
        w.put(0, 1);
    }
    let bps = s.bits_per_sample;
    let block_len = s.samples.len();
    match &s.predictor {
        Predictor::Constant(v) => w.put_signed(*v as i64, bps),
        Predictor::Verbatim => {
            for &v in &s.samples {
                w.put_signed(v as i64, bps);
            }
        }
        Predictor::Fixed { order } => {
            for &v in &s.samples[..*order] {
                w.put_signed(v as i64, bps);
            }
            write_rice(w, &s.residual, *order, block_len, s.rice.as_ref().unwrap());
        }
        Predictor::Linear { order, shift, coefficients } => {
            for &v in &s.samples[..*order] {
                w.put_signed(v as i64, bps);
            }
            w.put(LPC_PRECISION as u64 - 1, 4);
            w.put_signed(*shift as i64, 5);
            for &c in coefficients {
                w.put_signed(c as i64, LPC_PRECISION);
            }
            write_rice(w, &s.residual, *order, block_len, s.rice.as_ref().unwrap());
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// frames

/// How the channels of a stereo frame are stored.
#[derive(Clone, Copy)]
enum Assignment {
    Independent,
    LeftSide,
    RightSide,
    MidSide,
}

/// A frame (all channels of one block) as bytes.
fn encode_frame(channels: &[Vec<i32>], bits: u32, frame_number: u64, block_size_code: u8) -> Vec<u8> {
    let n = channels[0].len();
    let (assignment, subframes) = if channels.len() == 2 {
        choose_stereo(&channels[0], &channels[1], bits)
    } else {
        (Assignment::Independent, channels.iter().map(|c| code_channel(c, bits)).collect())
    };
    let mut w = BitWriter::default();
    w.put(0xFFF8, 16); // sync code and fixed block size
    w.put(block_size_code as u64, 4);
    w.put(0, 4); // sample rate from STREAMINFO
    w.put(
        match assignment {
            Assignment::Independent => channels.len() as u64 - 1,
            Assignment::LeftSide => 8,
            Assignment::RightSide => 9,
            Assignment::MidSide => 10,
        },
        4,
    );
    w.put(0, 3); // sample size from STREAMINFO
    w.put(0, 1);
    put_utf8_number(&mut w, frame_number);
    match block_size_code {
        6 => w.put(n as u64 - 1, 8),
        7 => w.put(n as u64 - 1, 16),
        _ => {}
    }
    let crc = crc8(&w.bytes);
    w.put(crc as u64, 8);
    for s in &subframes {
        write_subframe(&mut w, s);
    }
    w.align();
    let crc = crc16(&w.bytes);
    w.put(crc as u64, 16);
    w.bytes
}

/// Codes a stereo block as left/right, left/side, right/side or mid/side, whichever is smallest.
fn choose_stereo(left: &[i32], right: &[i32], bits: u32) -> (Assignment, Vec<Subframe>) {
    let side: Vec<i32> = left.iter().zip(right).map(|(l, r)| l - r).collect();
    let mid: Vec<i32> = left.iter().zip(right).map(|(l, r)| ((*l as i64 + *r as i64) >> 1) as i32).collect();
    let l = code_channel(left, bits);
    let r = code_channel(right, bits);
    let s = code_channel(&side, bits + 1);
    let m = code_channel(&mid, bits);
    let options = [(Assignment::Independent, l.bits + r.bits), (Assignment::LeftSide, l.bits + s.bits), (Assignment::RightSide, s.bits + r.bits), (Assignment::MidSide, m.bits + s.bits)];
    let (assignment, _) = options.iter().copied().min_by_key(|&(_, b)| b).unwrap();
    let subframes = match assignment {
        Assignment::Independent => vec![l, r],
        Assignment::LeftSide => vec![l, s],
        Assignment::RightSide => vec![s, r],
        Assignment::MidSide => vec![m, s],
    };
    (assignment, subframes)
}

/// FLAC numbers frames with a UTF-8 style variable-length code.
fn put_utf8_number(w: &mut BitWriter, value: u64) {
    if value < 0x80 {
        w.put(value, 8);
        return;
    }
    let continuation_bytes = if value < 0x800 { 1 } else if value < 0x1_0000 { 2 } else if value < 0x20_0000 { 3 } else if value < 0x400_0000 { 4 } else if value < 0x8000_0000 { 5 } else { 6 };
    let lead_mark = (0xFF00u32 >> (continuation_bytes + 1)) as u8; // 110xxxxx, 1110xxxx ...
    let lead_bits = value >> (6 * continuation_bytes);
    w.put(lead_mark as u64 | lead_bits, 8);
    for i in (0..continuation_bytes).rev() {
        w.put(0x80 | ((value >> (6 * i)) & 0x3F), 8);
    }
}

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
            _ => return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "FLAC holds 16 or 24 bit integer samples")),
        };
        if params.channels() > 8 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "FLAC holds at most 8 channels"));
        }
        let (block_size, block_size_code) = if params.rate <= 48_000 { (4096, 12) } else if params.rate <= 96_000 { (8192, 13) } else { (16384, 14) };
        let out = BufWriter::new(File::create(path)?);
        let sink = if in_ogg { Sink::Ogg(PacketWriter::new(out, OGG_SERIAL)) } else { Sink::Plain(out) };
        // STREAMINFO is rewritten at the end, when the totals are known
        let mut writer = FlacWriter { sink: Some(sink), samples_done: 0, bits, block_size, block_size_code, pending: vec![Vec::new(); params.channels()], next_frame: 0, md5: md5::Context::new(), min_frame_bytes: u32::MAX, max_frame_bytes: 0, bytes: 0, params };
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

    fn write_frame(&mut self, block: &[Vec<i32>]) -> std::io::Result<()> {
        let n = block[0].len();
        let code = if n == self.block_size { self.block_size_code } else if n <= 256 { 6 } else { 7 };
        let frame = encode_frame(block, self.bits, self.next_frame, code);
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

    /// Feeds the checksum with the samples as a decoder would output them: interleaved little-endian integers.
    fn update_md5(&mut self, block: &[Vec<i32>]) {
        let width = (self.bits / 8) as usize;
        let mut raw = Vec::with_capacity(block[0].len() * block.len() * width);
        for i in 0..block[0].len() {
            for channel in block {
                raw.extend_from_slice(&channel[i].to_le_bytes()[..width]);
            }
        }
        self.md5.consume(&raw);
    }
}

impl BlockWriter for FlacWriter {
    fn write_block(&mut self, planar: &[Vec<f32>], frames: usize) -> std::io::Result<()> {
        let converted = to_integers(planar, &self.params.order, frames, self.bits);
        for (pending, new) in self.pending.iter_mut().zip(converted) {
            pending.extend(new);
        }
        while self.pending[0].len() >= self.block_size {
            let block: Vec<Vec<i32>> = self.pending.iter_mut().map(|p| p.drain(..self.block_size).collect()).collect();
            self.update_md5(&block);
            self.write_frame(&block)?;
        }
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> std::io::Result<u64> {
        if !self.pending[0].is_empty() {
            let block: Vec<Vec<i32>> = self.pending.iter_mut().map(std::mem::take).collect();
            self.update_md5(&block);
            self.write_frame(&block)?;
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
        let packed: u64 = ((self.params.rate as u64) << 44) | (((self.params.channels() as u64) - 1) << 41) | (((self.bits as u64) - 1) << 36) | self.params.total_frames;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_frame_numbers() {
        let mut w = BitWriter::default();
        put_utf8_number(&mut w, 0x7F);
        put_utf8_number(&mut w, 0x80);
        put_utf8_number(&mut w, 0x7FF);
        put_utf8_number(&mut w, 0x800);
        assert_eq!(w.bytes, vec![0x7F, 0xC2, 0x80, 0xDF, 0xBF, 0xE0, 0xA0, 0x80]);
    }

    #[test]
    fn crc_values() {
        assert_eq!(crc8(b"123456789"), 0xF4);
        assert_eq!(crc16(b"123456789"), 0xFEE8);
    }

    #[test]
    fn rice_roundtrip_size() {
        let residual: Vec<i32> = (0..256).map(|i| ((i * 37) % 11) as i32 - 5).collect();
        let coding = rice_code(&residual, 0, 256);
        let mut w = BitWriter::default();
        write_rice(&mut w, &residual, 0, 256, &coding);
        w.align();
        assert_eq!(w.bytes.len(), coding.bits.div_ceil(8));
    }
}
