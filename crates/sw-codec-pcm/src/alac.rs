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

use crate::bitwriter::BitWriter;
use crate::{to_integers, BlockWriter, Params, SampleFormat};
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;
use sw_core::{Channel, ChannelLayout};

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
/// Predictor orders tried for each channel (0 = no prediction).
const PREDICTOR_ORDERS: [usize; 4] = [0, 4, 8, 16];

/// Syntax element kinds in the frame.
const ELEMENT_SINGLE: u32 = 0;
const ELEMENT_PAIR: u32 = 1;
const ELEMENT_END: u32 = 7;

/// How many single/pair elements make up each channel count (1 to 8), as Apple lays them out.
fn elements_for(channels: usize) -> &'static [u32] {
    const S: u32 = ELEMENT_SINGLE;
    const P: u32 = ELEMENT_PAIR;
    match channels {
        1 => &[S],
        2 => &[P],
        3 => &[S, P],
        4 => &[S, P, S],
        5 => &[S, P, P],
        6 => &[S, P, P, S],
        7 => &[S, P, P, S, S],
        _ => &[S, P, P, P, S],
    }
}

/// Apple's channel order and layout tag for a layout, or `None` when ALAC has no arrangement for it.
/// Returns the layout channel indexes in ALAC order.
pub fn alac_order(layout: &ChannelLayout) -> Option<(Vec<usize>, u32)> {
    use Channel::*;
    let (wanted, tag): (&[Channel], u32) = match layout.channels.len() {
        1 => (&[C], (100 << 16) | 1),
        2 => (&[L, R], (101 << 16) | 2),
        3 => (&[C, L, R], (114 << 16) | 3),
        4 => (&[C, L, R, Cs], (116 << 16) | 4),
        5 => (&[C, L, R, Ls, Rs], (120 << 16) | 5),
        6 => (&[C, L, R, Ls, Rs, Lfe], (124 << 16) | 6),
        7 => (&[C, L, R, Ls, Rs, Cs, Lfe], (142 << 16) | 7),
        8 => (&[C, Lw, Rw, L, R, Ls, Rs, Lfe], (127 << 16) | 8),
        _ => return None,
    };
    let order: Option<Vec<usize>> = wanted.iter().map(|w| layout.channels.iter().position(|c| c == w)).collect();
    order.map(|o| (o, tag))
}

// ---------------------------------------------------------------------------------------------------------------
// entropy coding

/// Floor of log2 for positive numbers (0 gives 0).
fn log2_floor(v: u32) -> u32 {
    if v == 0 { 0 } else { 31 - v.leading_zeros() }
}

/// One value with the adaptive Golomb code: a unary quotient (of the divisor 2^k - 1), then the remainder; a quotient
/// above 8 is replaced by nine ones and the value itself in `escape_bits` bits.
fn put_scalar(w: &mut BitWriter, value: u32, k: u32, escape_bits: u32) {
    let k = k.min(K_MODIFIER);
    let divisor = (1u32 << k) - 1;
    let quotient = value / divisor;
    let remainder = value % divisor;
    if quotient > 8 {
        w.put(0x1FF, 9);
        w.put(value as u64, escape_bits);
        return;
    }
    if quotient > 0 {
        w.put((1u64 << quotient) - 1, quotient);
    }
    w.put(0, 1);
    if k != 1 {
        if remainder > 0 {
            w.put(remainder as u64 + 1, k);
        } else {
            w.put(0, k - 1);
        }
    }
}

/// Codes the prediction residual of one channel. `escape_bits` is the width of an escaped value.
fn entropy_code(w: &mut BitWriter, residual: &[i32], escape_bits: u32) {
    let n = residual.len();
    let mut history = INITIAL_HISTORY;
    let mut sign_modifier = 0u32;
    let mut i = 0usize;
    while i < n {
        let k = log2_floor((history >> 9) + 3);
        // zigzag: 0, -1, 1, -2 ... becomes 0, 1, 2, 3 ... as 2|v| (positive) or 2|v| - 1 (negative)
        let x = {
            let v = 2i32.wrapping_mul(residual[i]).wrapping_neg().wrapping_sub(1);
            (v ^ (v >> 31)) as u32
        };
        i += 1;
        put_scalar(w, x - sign_modifier, k, escape_bits);
        history = history.wrapping_add(x.wrapping_mul(HISTORY_MULT)).wrapping_sub(history.wrapping_mul(HISTORY_MULT) >> 9);
        sign_modifier = 0;
        if x > 0xFFFF {
            history = 0xFFFF;
        }
        if history < 128 && i < n {
            // the average has collapsed: send the length of the run of zeros that follows
            let k = (7 + ((history + 16) >> 6)).saturating_sub(log2_floor(history));
            let mut run = 0u32;
            while i < n && residual[i] == 0 {
                i += 1;
                run += 1;
            }
            put_scalar(w, run, k, 16);
            sign_modifier = u32::from(run <= 0xFFFF);
            history = 0;
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// prediction

/// `value` taken as a signed number of `bits` bits.
fn sign_extend(value: i32, bits: u32) -> i32 {
    let shift = 32 - bits;
    (value << shift) >> shift
}

/// Residual of the adaptive predictor for a channel. `coefficients` are the starting values (the decoder adapts its
/// own copy in step); `bits` is the width the residual is wrapped to.
fn predict(samples: &[i32], starting: &[i32], bits: u32) -> Vec<i32> {
    let n = samples.len();
    let order = starting.len();
    let mut residual = vec![0i32; n];
    residual[0] = samples[0];
    if order == 0 {
        residual[1..].copy_from_slice(&samples[1..]);
        return residual;
    }
    let mut coefficients = starting.to_vec();
    for i in 1..=order.min(n - 1) {
        residual[i] = sign_extend(samples[i].wrapping_sub(samples[i - 1]), bits);
    }
    for i in order + 1..n {
        let top = samples[i - order - 1];
        let mut sum = 1i32 << (PREDICTOR_SHIFT - 1);
        for (j, &c) in coefficients.iter().enumerate() {
            sum = sum.wrapping_add(samples[i - 1 - j].wrapping_sub(top).wrapping_mul(c));
        }
        let predicted = (sum >> PREDICTOR_SHIFT).wrapping_add(top);
        let error = sign_extend(samples[i].wrapping_sub(predicted), bits);
        residual[i] = error;
        // adapt the coefficients: nudge each by the sign of its input, oldest first, until the error has been used up
        if error != 0 {
            let negative = error < 0;
            let mut left = error;
            for index in (0..order).rev() {
                if (negative && left >= 0) || (!negative && left <= 0) {
                    break;
                }
                let diff = top.wrapping_sub(samples[i - 1 - index]);
                let mut sign = diff.signum();
                if negative {
                    sign = -sign;
                }
                coefficients[index] -= sign;
                let moved = diff.wrapping_mul(sign);
                left = left.wrapping_sub((moved >> PREDICTOR_SHIFT).wrapping_mul((order - index) as i32));
            }
        }
    }
    residual
}

/// Starting coefficients for a predictor of `order` taps: the least-squares fit of each sample's difference from the
/// sample `order + 1` back, on the differences of the `order` samples in between (which is the form the decoder uses).
fn fit_coefficients(samples: &[i32], order: usize) -> Option<Vec<i32>> {
    let n = samples.len();
    if n < order * 4 + 8 {
        return None;
    }
    let mut a = vec![vec![0f64; order]; order];
    let mut b = vec![0f64; order];
    let mut e = vec![0f64; order];
    for i in order + 1..n {
        let top = samples[i - order - 1] as f64;
        for (k, v) in e.iter_mut().enumerate() {
            *v = samples[i - 1 - k] as f64 - top;
        }
        let d = samples[i] as f64 - top;
        for p in 0..order {
            b[p] += e[p] * d;
            for q in p..order {
                a[p][q] += e[p] * e[q];
            }
        }
    }
    for p in 0..order {
        for q in 0..p {
            a[p][q] = a[q][p];
        }
        a[p][p] += a[p][p] * 1e-9 + 1e-3;
    }
    let solution = solve(a, b)?;
    let limit = i16::MAX as f64;
    Some(solution.iter().map(|c| (c * (1u32 << PREDICTOR_SHIFT) as f64).round().clamp(-limit, limit) as i32).collect())
}

/// Gaussian elimination with partial pivoting.
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let pivot = (col..n).max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs()))?;
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        for row in col + 1..n {
            let f = a[row][col] / a[col][col];
            for c in col..n {
                a[row][c] -= f * a[col][c];
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = vec![0f64; n];
    for row in (0..n).rev() {
        let mut v = b[row];
        for c in row + 1..n {
            v -= a[row][c] * x[c];
        }
        x[row] = v / a[row][row];
    }
    Some(x)
}

/// One channel's chosen predictor and what it leaves.
struct Predicted {
    coefficients: Vec<i32>,
    residual: Vec<i32>,
    bits: usize,
}

/// Tries the predictor orders on a channel and keeps the one that codes smallest.
fn best_prediction(samples: &[i32], bits: u32, orders: &[usize]) -> Predicted {
    let mut best: Option<Predicted> = None;
    for &order in orders {
        let coefficients = if order == 0 {
            Vec::new()
        } else {
            match fit_coefficients(samples, order) {
                Some(c) => c,
                None => continue,
            }
        };
        let residual = predict(samples, &coefficients, bits);
        let mut w = BitWriter::default();
        entropy_code(&mut w, &residual, bits);
        let total = w.bytes.len() * 8 + order * 16;
        if best.as_ref().map_or(true, |b| total < b.bits) {
            best = Some(Predicted { coefficients, residual, bits: total });
        }
    }
    best.unwrap_or_else(|| {
        let residual = predict(samples, &[], bits);
        Predicted { coefficients: Vec::new(), residual, bits: usize::MAX }
    })
}

// ---------------------------------------------------------------------------------------------------------------
// stereo decorrelation

/// How a stereo pair is stored, with the two numbers the decoder needs.
#[derive(Clone, Copy)]
struct Stereo {
    shift: u32,
    left_weight: u32,
}

const LEFT_RIGHT: Stereo = Stereo { shift: 0, left_weight: 0 };

/// The four ways of storing a pair: left/right, left/side, right/side, mid/side. Returns the changed channels.
fn stereo_variants(left: &[i32], right: &[i32]) -> Vec<(Stereo, Vec<i32>, Vec<i32>)> {
    let n = left.len();
    let mut out = vec![(LEFT_RIGHT, left.to_vec(), right.to_vec())];
    // left and side
    out.push((Stereo { shift: 0, left_weight: 1 }, left.to_vec(), (0..n).map(|i| left[i].wrapping_sub(right[i])).collect()));
    // right and side
    let mut l2 = Vec::with_capacity(n);
    let mut r2 = Vec::with_capacity(n);
    for i in 0..n {
        let side = left[i].wrapping_sub(right[i]);
        r2.push(side);
        l2.push(right[i].wrapping_add(side >> 31));
    }
    out.push((Stereo { shift: 31, left_weight: 1 }, l2, r2));
    // mid and side
    let mut l3 = Vec::with_capacity(n);
    let mut r3 = Vec::with_capacity(n);
    for i in 0..n {
        l3.push(left[i].wrapping_add(right[i]) >> 1);
        r3.push(left[i].wrapping_sub(right[i]));
    }
    out.push((Stereo { shift: 1, left_weight: 1 }, l3, r3));
    out
}

// ---------------------------------------------------------------------------------------------------------------
// frames

/// Writes one element (a single channel or a pair) of a frame.
fn write_element(w: &mut BitWriter, kind: u32, instance: u32, channels: &[&[i32]], bits: u32, verbatim: bool) {
    let n = channels[0].len();
    let extra_bits = if verbatim { 0 } else { bits.saturating_sub(16) };
    w.put(kind as u64, 3);
    w.put(instance as u64, 4);
    w.put(0, 12); // unused
    let has_size = n < FRAME_SAMPLES;
    w.put(u64::from(has_size), 1);
    w.put((extra_bits >> 3) as u64, 2);
    w.put(u64::from(verbatim), 1);
    if has_size {
        w.put(n as u64, 32);
    }
    if verbatim {
        for i in 0..n {
            for ch in channels {
                w.put_signed(ch[i] as i64, bits);
            }
        }
        return;
    }
    let width = bits - extra_bits + channels.len() as u32 - 1; // the side channel needs one more bit
    // the low bytes are kept aside; the rest is what gets predicted
    let mask = (1i32 << extra_bits) - 1;
    let low: Vec<Vec<i32>> = channels.iter().map(|c| c.iter().map(|v| v & mask).collect()).collect();
    let high: Vec<Vec<i32>> = channels.iter().map(|c| c.iter().map(|v| v >> extra_bits).collect()).collect();
    let (stereo, coded): (Stereo, Vec<Vec<i32>>) = if channels.len() == 2 {
        // try every way of storing the pair with a quick predictor, keep the smallest
        let mut best: Option<(usize, Stereo, Vec<Vec<i32>>)> = None;
        for (s, l, r) in stereo_variants(&high[0], &high[1]) {
            let cost = best_prediction(&l, width, &[8]).bits.saturating_add(best_prediction(&r, width, &[8]).bits);
            if best.as_ref().map_or(true, |b| cost < b.0) {
                best = Some((cost, s, vec![l, r]));
            }
        }
        let (_, s, c) = best.unwrap();
        (s, c)
    } else {
        (LEFT_RIGHT, high)
    };
    w.put(stereo.shift as u64, 8);
    w.put(stereo.left_weight as u64, 8);
    let predictions: Vec<Predicted> = coded.iter().map(|c| best_prediction(c, width, &PREDICTOR_ORDERS)).collect();
    for p in &predictions {
        w.put(0, 4); // prediction type
        w.put(PREDICTOR_SHIFT as u64, 4);
        w.put(RICE_MODIFIER as u64, 3);
        w.put(p.coefficients.len() as u64, 5);
        // the decoder reads them oldest tap last: the stream starts with the newest
        for &c in &p.coefficients {
            w.put_signed(c as i64, 16);
        }
    }
    if extra_bits > 0 {
        for i in 0..n {
            for l in &low {
                w.put(l[i] as u64, extra_bits);
            }
        }
    }
    for p in &predictions {
        entropy_code(w, &p.residual, width);
    }
}

/// A whole frame (all elements) as bytes.
fn encode_frame(channels: &[Vec<i32>], bits: u32) -> Vec<u8> {
    let n = channels[0].len();
    let max_bytes = {
        let header_bits = 23 + 32 * usize::from(n < FRAME_SAMPLES);
        (header_bits + bits as usize * channels.len() * n + 3).div_ceil(8)
    };
    for verbatim in [false, true] {
        let mut w = BitWriter::default();
        let (mut singles, mut pairs, mut ch) = (0u32, 0u32, 0usize);
        for &kind in elements_for(channels.len()) {
            if kind == ELEMENT_PAIR {
                write_element(&mut w, kind, pairs, &[&channels[ch], &channels[ch + 1]], bits, verbatim);
                pairs += 1;
                ch += 2;
            } else {
                write_element(&mut w, kind, singles, &[&channels[ch]], bits, verbatim);
                singles += 1;
                ch += 1;
            }
        }
        w.put(ELEMENT_END as u64, 3);
        w.align();
        if verbatim || w.bytes.len() <= max_bytes {
            return w.bytes;
        }
    }
    unreachable!("the verbatim frame is always accepted")
}

// ---------------------------------------------------------------------------------------------------------------
// the M4A file

pub struct AlacWriter {
    out: BufWriter<File>,
    params: Params,
    bits: u32,
    /// Samples collected for the next frame, per channel in ALAC order.
    pending: Vec<Vec<i32>>,
    frame_sizes: Vec<u32>,
    /// Offset of the first byte of audio data in the file.
    data_start: u64,
    /// Offset of the 64-bit size field of the `mdat` box.
    mdat_size_at: u64,
    layout_tag: u32,
    samples_done: u64,
}

impl AlacWriter {
    pub fn create(path: &Path, params: Params, layout_tag: u32) -> std::io::Result<AlacWriter> {
        let bits = match params.format {
            SampleFormat::Int16 => 16,
            SampleFormat::Int24 => 24,
            _ => return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "ALAC holds 16 or 24 bit integer samples")),
        };
        let mut out = BufWriter::new(File::create(path)?);
        let mut ftyp = Vec::new();
        ftyp.extend_from_slice(b"M4A ");
        ftyp.extend_from_slice(&0u32.to_be_bytes());
        for brand in [b"M4A ", b"mp42", b"isom"] {
            ftyp.extend_from_slice(brand);
        }
        ftyp.extend_from_slice(&0u32.to_be_bytes());
        let ftyp = atom(b"ftyp", &ftyp);
        out.write_all(&ftyp)?;
        // `mdat` with a 64-bit size, patched when the length is known
        let mdat_size_at = ftyp.len() as u64 + 8;
        out.write_all(&1u32.to_be_bytes())?;
        out.write_all(b"mdat")?;
        out.write_all(&0u64.to_be_bytes())?;
        let data_start = ftyp.len() as u64 + 16;
        Ok(AlacWriter { out, bits, pending: vec![Vec::new(); params.channels()], frame_sizes: Vec::new(), data_start, mdat_size_at, layout_tag, samples_done: 0, params })
    }

    fn write_frame(&mut self, block: &[Vec<i32>]) -> std::io::Result<()> {
        let bytes = encode_frame(block, self.bits);
        self.frame_sizes.push(bytes.len() as u32);
        self.samples_done += block[0].len() as u64;
        self.out.write_all(&bytes)
    }

    /// The `moov` box: everything a player needs to find and decode the audio.
    fn moov(&self, data_end: u64) -> Vec<u8> {
        let p = &self.params;
        let rate = p.rate;
        let total = self.samples_done;
        let frames = self.frame_sizes.len() as u32;
        let max_frame = self.frame_sizes.iter().copied().max().unwrap_or(0);
        let bytes: u64 = self.frame_sizes.iter().map(|&s| s as u64).sum();
        let average_bitrate = if total > 0 { (bytes * 8 * rate as u64 / total) as u32 } else { 0 };

        // sample entry: the 'alac' description (Apple's cookie) and a channel layout box
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
        cookie.extend_from_slice(&rate.to_be_bytes());
        let mut entry = Vec::new();
        entry.extend_from_slice(&[0u8; 6]);
        entry.extend_from_slice(&1u16.to_be_bytes()); // data reference
        entry.extend_from_slice(&[0u8; 8]); // version, revision, vendor
        entry.extend_from_slice(&(p.channels() as u16).to_be_bytes());
        entry.extend_from_slice(&(self.bits as u16).to_be_bytes());
        entry.extend_from_slice(&[0u8; 4]); // compression id, packet size
        entry.extend_from_slice(&((if rate <= 0xFFFF { rate } else { 0 }) << 16).to_be_bytes());
        entry.extend_from_slice(&atom(b"alac", &cookie));
        if p.channels() > 2 {
            let mut chan = Vec::new();
            chan.extend_from_slice(&0u32.to_be_bytes());
            chan.extend_from_slice(&self.layout_tag.to_be_bytes());
            chan.extend_from_slice(&0u32.to_be_bytes()); // bitmap
            chan.extend_from_slice(&0u32.to_be_bytes()); // no descriptions
            entry.extend_from_slice(&atom(b"chan", &chan));
        }
        let mut stsd = Vec::new();
        stsd.extend_from_slice(&0u32.to_be_bytes());
        stsd.extend_from_slice(&1u32.to_be_bytes());
        stsd.extend_from_slice(&atom(b"alac", &entry));

        // time to sample: every frame is 4096 samples but the last
        let mut stts = Vec::new();
        stts.extend_from_slice(&0u32.to_be_bytes());
        let last = (total - (frames as u64 - 1) * FRAME_SAMPLES as u64) as u32;
        if last == FRAME_SAMPLES as u32 {
            stts.extend_from_slice(&1u32.to_be_bytes());
            stts.extend_from_slice(&frames.to_be_bytes());
            stts.extend_from_slice(&(FRAME_SAMPLES as u32).to_be_bytes());
        } else {
            stts.extend_from_slice(&2u32.to_be_bytes());
            stts.extend_from_slice(&(frames - 1).to_be_bytes());
            stts.extend_from_slice(&(FRAME_SAMPLES as u32).to_be_bytes());
            stts.extend_from_slice(&1u32.to_be_bytes());
            stts.extend_from_slice(&last.to_be_bytes());
        }
        // all frames in one chunk
        let mut stsc = Vec::new();
        stsc.extend_from_slice(&0u32.to_be_bytes());
        stsc.extend_from_slice(&1u32.to_be_bytes());
        stsc.extend_from_slice(&1u32.to_be_bytes());
        stsc.extend_from_slice(&frames.to_be_bytes());
        stsc.extend_from_slice(&1u32.to_be_bytes());
        let mut stsz = Vec::new();
        stsz.extend_from_slice(&0u32.to_be_bytes());
        stsz.extend_from_slice(&0u32.to_be_bytes()); // sizes follow
        stsz.extend_from_slice(&frames.to_be_bytes());
        for s in &self.frame_sizes {
            stsz.extend_from_slice(&s.to_be_bytes());
        }
        let mut chunk_offsets = Vec::new();
        chunk_offsets.extend_from_slice(&0u32.to_be_bytes());
        chunk_offsets.extend_from_slice(&1u32.to_be_bytes());
        let offsets = if data_end > u32::MAX as u64 {
            chunk_offsets.extend_from_slice(&self.data_start.to_be_bytes());
            atom(b"co64", &chunk_offsets)
        } else {
            chunk_offsets.extend_from_slice(&(self.data_start as u32).to_be_bytes());
            atom(b"stco", &chunk_offsets)
        };
        let mut stbl = Vec::new();
        stbl.extend_from_slice(&atom(b"stsd", &stsd));
        stbl.extend_from_slice(&atom(b"stts", &stts));
        stbl.extend_from_slice(&atom(b"stsc", &stsc));
        stbl.extend_from_slice(&atom(b"stsz", &stsz));
        stbl.extend_from_slice(&offsets);

        let mut dref = Vec::new();
        dref.extend_from_slice(&0u32.to_be_bytes());
        dref.extend_from_slice(&1u32.to_be_bytes());
        dref.extend_from_slice(&atom(b"url ", &1u32.to_be_bytes())); // media data in this file
        let dinf = atom(b"dinf", &atom(b"dref", &dref));
        let smhd = atom(b"smhd", &[0u8; 8]);
        let mut minf = Vec::new();
        minf.extend_from_slice(&smhd);
        minf.extend_from_slice(&dinf);
        minf.extend_from_slice(&atom(b"stbl", &stbl));

        let mut mdhd = Vec::new();
        mdhd.extend_from_slice(&[1, 0, 0, 0]); // version 1: 64-bit times
        mdhd.extend_from_slice(&0u64.to_be_bytes());
        mdhd.extend_from_slice(&0u64.to_be_bytes());
        mdhd.extend_from_slice(&rate.to_be_bytes());
        mdhd.extend_from_slice(&total.to_be_bytes());
        mdhd.extend_from_slice(&0x55C4u16.to_be_bytes()); // language: undetermined
        mdhd.extend_from_slice(&0u16.to_be_bytes());
        let mut hdlr = Vec::new();
        hdlr.extend_from_slice(&0u32.to_be_bytes());
        hdlr.extend_from_slice(&0u32.to_be_bytes());
        hdlr.extend_from_slice(b"soun");
        hdlr.extend_from_slice(&[0u8; 12]);
        hdlr.extend_from_slice(b"SoundHandler\0");
        let mut mdia = Vec::new();
        mdia.extend_from_slice(&atom(b"mdhd", &mdhd));
        mdia.extend_from_slice(&atom(b"hdlr", &hdlr));
        mdia.extend_from_slice(&atom(b"minf", &minf));

        let matrix: [u32; 9] = [0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000];
        let mut tkhd = Vec::new();
        tkhd.extend_from_slice(&[1, 0, 0, 3]); // version 1; enabled and in the movie
        tkhd.extend_from_slice(&0u64.to_be_bytes());
        tkhd.extend_from_slice(&0u64.to_be_bytes());
        tkhd.extend_from_slice(&1u32.to_be_bytes()); // track id
        tkhd.extend_from_slice(&0u32.to_be_bytes());
        tkhd.extend_from_slice(&total.to_be_bytes()); // duration in movie time units (= samples)
        tkhd.extend_from_slice(&[0u8; 8]);
        tkhd.extend_from_slice(&0u16.to_be_bytes()); // layer
        tkhd.extend_from_slice(&0u16.to_be_bytes()); // alternate group
        tkhd.extend_from_slice(&0x0100u16.to_be_bytes()); // volume
        tkhd.extend_from_slice(&0u16.to_be_bytes());
        for m in matrix {
            tkhd.extend_from_slice(&m.to_be_bytes());
        }
        tkhd.extend_from_slice(&[0u8; 8]); // width, height
        let mut trak = Vec::new();
        trak.extend_from_slice(&atom(b"tkhd", &tkhd));
        trak.extend_from_slice(&atom(b"mdia", &mdia));

        let mut mvhd = Vec::new();
        mvhd.extend_from_slice(&[1, 0, 0, 0]);
        mvhd.extend_from_slice(&0u64.to_be_bytes());
        mvhd.extend_from_slice(&0u64.to_be_bytes());
        mvhd.extend_from_slice(&rate.to_be_bytes()); // movie time scale = sample rate
        mvhd.extend_from_slice(&total.to_be_bytes());
        mvhd.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // rate 1.0
        mvhd.extend_from_slice(&0x0100u16.to_be_bytes()); // volume
        mvhd.extend_from_slice(&[0u8; 10]);
        for m in matrix {
            mvhd.extend_from_slice(&m.to_be_bytes());
        }
        mvhd.extend_from_slice(&[0u8; 24]);
        mvhd.extend_from_slice(&2u32.to_be_bytes()); // next track id
        let mut moov = Vec::new();
        moov.extend_from_slice(&atom(b"mvhd", &mvhd));
        moov.extend_from_slice(&atom(b"trak", &trak));
        atom(b"moov", &moov)
    }
}

impl BlockWriter for AlacWriter {
    fn write_block(&mut self, planar: &[Vec<f32>], frames: usize) -> std::io::Result<()> {
        let converted = to_integers(planar, &self.params.order, frames, self.bits);
        for (pending, new) in self.pending.iter_mut().zip(converted) {
            pending.extend(new);
        }
        while self.pending[0].len() >= FRAME_SAMPLES {
            let block: Vec<Vec<i32>> = self.pending.iter_mut().map(|p| p.drain(..FRAME_SAMPLES).collect()).collect();
            self.write_frame(&block)?;
        }
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> std::io::Result<u64> {
        if !self.pending[0].is_empty() {
            let block: Vec<Vec<i32>> = self.pending.iter_mut().map(std::mem::take).collect();
            self.write_frame(&block)?;
        }
        let data_bytes: u64 = self.frame_sizes.iter().map(|&s| s as u64).sum();
        let data_end = self.data_start + data_bytes;
        let moov = self.moov(data_end);
        self.out.write_all(&moov)?;
        self.out.flush()?;
        // the real length of the `mdat` box
        self.out.seek(SeekFrom::Start(self.mdat_size_at))?;
        self.out.write_all(&(16 + data_bytes).to_be_bytes())?;
        self.out.flush()?;
        Ok(data_end + moov.len() as u64)
    }
}

/// An MP4 box: size, four-character name, contents.
fn atom(name: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(&((8 + body.len()) as u32).to_be_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_one_goes_in_apples_order() {
        let layout = ChannelLayout::find("5.1").unwrap(); // L R C LFE Ls Rs
        let (order, tag) = alac_order(layout).unwrap();
        assert_eq!(order, vec![2, 0, 1, 4, 5, 3]); // C L R Ls Rs LFE
        assert_eq!(tag, (124 << 16) | 6);
    }

    #[test]
    fn layouts_without_an_apple_arrangement_are_refused() {
        assert!(alac_order(ChannelLayout::find("7.1-side-rear").unwrap()).is_none());
        assert!(alac_order(ChannelLayout::find("5.1.4").unwrap()).is_none());
    }

    #[test]
    fn scalar_codes_match_the_published_examples() {
        // k = 1: the divisor is 1, so the code is the unary quotient and nothing else
        let mut w = BitWriter::default();
        put_scalar(&mut w, 3, 1, 16);
        w.align();
        assert_eq!(w.bytes, vec![0b1110_0000]);
        // k = 3: divisor 7; 9 = 1 * 7 + 2 -> unary 10, then (2 + 1) in 3 bits = 011
        let mut w = BitWriter::default();
        put_scalar(&mut w, 9, 3, 16);
        w.align();
        assert_eq!(w.bytes, vec![0b1001_1000]);
        // a zero remainder is k - 1 zero bits
        let mut w = BitWriter::default();
        put_scalar(&mut w, 7, 3, 16);
        w.align();
        assert_eq!(w.bytes, vec![0b1000_0000]);
    }

    #[test]
    fn predictor_residual_is_what_the_decoder_undoes() {
        // decode the residual with the decoder's rule and compare with the input
        let samples: Vec<i32> = (0..500).map(|i| ((i as f64 * 0.07).sin() * 9000.0) as i32 + (i * 7 % 13) as i32).collect();
        let coefficients = fit_coefficients(&samples, 8).unwrap();
        let residual = predict(&samples, &coefficients, 17);
        let order = 8usize;
        let mut c = coefficients.clone();
        let mut out = vec![0i32; samples.len()];
        out[0] = residual[0];
        for i in 1..=order {
            out[i] = sign_extend(out[i - 1] + residual[i], 17);
        }
        for i in order + 1..samples.len() {
            let top = out[i - order - 1];
            let mut sum = 1i32 << (PREDICTOR_SHIFT - 1);
            for j in 0..order {
                sum += (out[i - 1 - j] - top) * c[j];
            }
            let value = sign_extend((sum >> PREDICTOR_SHIFT) + top + residual[i], 17);
            out[i] = value;
            let err = residual[i];
            if err != 0 {
                let neg = err < 0;
                let mut left = err;
                for index in (0..order).rev() {
                    if (neg && left >= 0) || (!neg && left <= 0) {
                        break;
                    }
                    let d = top - out[i - 1 - index];
                    let mut s = d.signum();
                    if neg {
                        s = -s;
                    }
                    c[index] -= s;
                    left -= ((d * s) >> PREDICTOR_SHIFT) * (order - index) as i32;
                }
            }
        }
        assert_eq!(out, samples);
    }
}
