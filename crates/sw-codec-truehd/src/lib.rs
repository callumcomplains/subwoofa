//! Native Dolby TrueHD (MLP) lossless encoder, written from the public bitstream behaviour.
//!
//! Layout of the produced stream
//! * access units of 40/80/160 samples (48k/96k/192k families);
//! * one substream for mono/stereo, otherwise 2 (channels 0-1, then the rest) or 3 substreams
//!   (7.1: channels 0-1, 2-5, 6-7) with no inter-channel matrices;
//! * a major sync + restart header at the start of every restart interval; the first access unit
//!   of an interval carries an 8-sample unfiltered priming block so a decoder can join there;
//! * per channel and interval a static FIR predictor (order <= 8), per block a Huffman codebook
//!   and raw-LSB split chosen to minimise bits.
//!
//! The encoder is bit-exact lossless for up to 24-bit integer samples.

pub mod bits;
pub mod native;
pub mod tables;

use bits::{checksum8, major_sync_checksum, parity, restart_checksum, BitWriter};
use sw_core::{Channel, ChannelLayout};
use tables::{GROUP_CHANNELS, HUFFMAN};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Unsupported(String),
}

pub const MAX_CHANNELS: usize = 8;
const MAX_ORDER: usize = 8;
/// Access units per restart interval (major sync spacing).
pub const DEFAULT_RESTART_INTERVAL: usize = 128;

/// Channel position in TrueHD order: (group index, 0 = left/only, 1 = right).
fn thd_position(c: Channel) -> Option<(usize, usize)> {
    use Channel::*;
    Some(match c {
        L | Lt => (0, 0),
        R | Rt => (0, 1),
        C => (1, 0),
        Lfe => (2, 0),
        Ls | Lss => (3, 0),
        Rs | Rss => (3, 1),
        Lh | Lhs => (4, 0),
        Rh | Rhs => (4, 1),
        Lsr => (6, 0),
        Rsr => (6, 1),
        Cs => (7, 0),
        Oh => (8, 0),
        Lw => (10, 0),
        Rw => (10, 1),
        Ch => (11, 0),
    })
}

/// How a layout maps onto TrueHD channels.
#[derive(Debug, Clone)]
pub struct LayoutMap {
    /// 13-bit channel group map for the full presentation.
    pub chanmap: u16,
    /// Index into `layout.channels` for each coded channel, in TrueHD order.
    pub source_index: Vec<usize>,
}

pub fn map_layout(layout: &ChannelLayout) -> Result<LayoutMap, Error> {
    let mut items: Vec<((usize, usize), usize)> = Vec::new();
    for (i, c) in layout.channels.iter().enumerate() {
        let pos = thd_position(*c).ok_or_else(|| Error::Unsupported(format!("channel {} has no TrueHD position", c.short())))?;
        items.push((pos, i));
    }
    items.sort();
    items.windows(2).try_for_each(|w| {
        if w[0].0 == w[1].0 {
            Err(Error::Unsupported(format!("layout '{}' has two channels in the same TrueHD position", layout.id)))
        } else {
            Ok(())
        }
    })?;
    if items.len() > MAX_CHANNELS {
        return Err(Error::Unsupported("TrueHD carries at most 8 channels here".into()));
    }
    let mut chanmap = 0u16;
    for ((g, _), _) in &items {
        chanmap |= 1 << g;
    }
    // a group present with only one of its two channels is not expressible
    let expected: usize = (0..13).filter(|g| chanmap >> g & 1 == 1).map(|g| GROUP_CHANNELS[g]).sum();
    if expected != items.len() {
        return Err(Error::Unsupported(format!("layout '{}' has an unpaired channel (TrueHD groups channels in L/R pairs)", layout.id)));
    }
    Ok(LayoutMap { chanmap, source_index: items.into_iter().map(|(_, i)| i).collect() })
}

#[derive(Debug, Clone)]
pub struct Config {
    pub sample_rate: u32,
    pub chanmap: u16,
    pub channels: usize,
    /// Access units per restart interval (8..=128).
    pub restart_interval: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
struct Fir {
    order: u8,
    shift: u8,
    coeff_bits: u8,
    coeff_shift: u8,
    coeff: [i32; MAX_ORDER],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct ChParams {
    fir: Fir,
    codebook: u8,
    lsbs: u8,
}

const DEFAULT_CH: ChParams = ChParams { fir: Fir { order: 0, shift: 0, coeff_bits: 0, coeff_shift: 0, coeff: [0; MAX_ORDER] }, codebook: 0, lsbs: 24 };

/// What the decoder currently believes (mirrors the decoder's state machine).
#[derive(Clone, Debug)]
struct DecState {
    blocksize: usize,
    quant: [u8; MAX_CHANNELS],
    ch: [ChParams; MAX_CHANNELS],
}

impl DecState {
    fn restart() -> DecState {
        DecState { blocksize: 8, quant: [0; MAX_CHANNELS], ch: [DEFAULT_CH; MAX_CHANNELS] }
    }
}

#[derive(Clone, Copy, Debug)]
struct Substream {
    min: usize,
    max: usize,
}

pub struct Encoder {
    cfg: Config,
    au: usize,
    subs: Vec<Substream>,
    rate_code: u32,
    input_timing: u16,
    output_timing: u16,
    /// Running XOR over output samples since the last restart, per substream.
    lossless: Vec<u32>,
    sync_count: usize,
}

fn number_sbits(n: i32) -> u32 {
    // smallest b >= 1 such that -2^(b-1) <= n < 2^(b-1)
    let m = if n < 0 { !n as u32 } else { n as u32 };
    (32 - m.leading_zeros()) + 1
}

impl Encoder {
    pub fn new(cfg: Config) -> Result<Encoder, Error> {
        let (rate_code, au) = match cfg.sample_rate {
            48_000 => (0, 40),
            96_000 => (1, 80),
            192_000 => (2, 160),
            44_100 => (8, 40),
            88_200 => (9, 80),
            176_400 => (10, 160),
            r => return Err(Error::Unsupported(format!("TrueHD supports 44.1/48/88.2/96/176.4/192 kHz, not {r} Hz"))),
        };
        if !(1..=MAX_CHANNELS).contains(&cfg.channels) {
            return Err(Error::Unsupported(format!("{} channels", cfg.channels)));
        }
        if !(8..=128).contains(&cfg.restart_interval) {
            return Err(Error::Unsupported("restart interval must be 8 to 128 access units".into()));
        }
        let n = cfg.channels;
        let subs = if n <= 2 {
            vec![Substream { min: 0, max: n - 1 }]
        } else if n <= 6 {
            vec![Substream { min: 0, max: 1 }, Substream { min: 2, max: n - 1 }]
        } else {
            vec![Substream { min: 0, max: 1 }, Substream { min: 2, max: 5 }, Substream { min: 6, max: n - 1 }]
        };
        let nsub = subs.len();
        Ok(Encoder {
            au,
            subs,
            rate_code,
            input_timing: (0u16).wrapping_sub(au as u16),
            output_timing: 0,
            lossless: vec![0; nsub],
            sync_count: 0,
            cfg,
        })
    }

    pub fn au_samples(&self) -> usize {
        self.au
    }
    pub fn interval_samples(&self) -> usize {
        self.au * self.cfg.restart_interval
    }

    /// Channel map of the first six coded channels (the "6-channel presentation").
    fn chanmap6(&self) -> u16 {
        let mut map = 0u16;
        let mut count = 0;
        for g in 0..13 {
            if self.cfg.chanmap >> g & 1 == 1 {
                if count + GROUP_CHANNELS[g] > 6 {
                    break;
                }
                map |= 1 << g;
                count += GROUP_CHANNELS[g];
            }
        }
        map
    }

    fn write_major_sync(&self) -> [u8; 28] {
        let mut w = BitWriter::new();
        w.put(0xF8726F, 24);
        w.put(0xBA, 8);
        w.put(self.rate_code, 4);
        w.put(0, 1); // multichannel type 6ch
        w.put(0, 1); // multichannel type 8ch
        w.put(0, 2);
        let (mod2, mod6, mod8) = match self.cfg.channels {
            1 => (3, 3, 3),
            2 => (1, 1, 1),
            _ => (0, 0, 0),
        };
        w.put(mod2, 2);
        w.put(mod6, 2);
        let arr6 = if self.cfg.channels <= 6 { self.cfg.chanmap } else { self.chanmap6() };
        w.put(arr6 as u32 & 0x1F, 5);
        w.put(mod8, 2);
        w.put(self.cfg.chanmap as u32, 13);
        w.put(0xB752, 16);
        w.put(0, 16); // flags
        w.put(0, 16);
        w.put(1, 1); // variable bit rate
        let peak = ((9_600_000u32 << 4) - 8) / self.cfg.sample_rate;
        w.put(peak & 0x7FFF, 15);
        w.put(self.subs.len() as u32, 4);
        w.put(0, 2);
        w.put(0, 2); // extended substream info
        w.put(if self.cfg.channels <= 2 { 0x14 } else { 0x3C }, 8);
        w.put(0, 6);
        w.put(0, 4); // control flags
        w.put(0, 7); // drc start-up gain
        w.put(0, 6);
        w.put(0, 6);
        w.put(0, 5);
        w.put(0, 6);
        w.put(0, 5);
        w.put(0, 5);
        w.put(0, 6);
        w.put(0, 6);
        w.put(0, 1);
        w.put(0, 1);
        let mut bytes = w.snapshot();
        bytes.resize(28, 0);
        let c = major_sync_checksum(&bytes[..26]);
        bytes[26..28].copy_from_slice(&c.to_le_bytes());
        bytes.try_into().unwrap()
    }

    /// Encode one restart interval. `chans` holds one vector per coded channel (TrueHD order),
    /// each at least `valid.div_ceil(au) * au` samples long (zero padded). `valid` is the number of
    /// real samples; if `last` the final access unit is trimmed back to it on decode.
    pub fn encode_interval(&mut self, chans: &[Vec<i32>], valid: usize, last: bool) -> Vec<u8> {
        assert_eq!(chans.len(), self.cfg.channels);
        let au = self.au;
        let n_au = valid.div_ceil(au).max(1);
        let n = n_au * au;
        for c in chans {
            assert!(c.len() >= n, "channel buffer shorter than the interval");
        }
        let shorten = if last { n - valid } else { 0 };

        // 1. analysis: per channel quantizer step, predictor and residual
        let analyses: Vec<ChannelAnalysis> = std::thread::scope(|sc| {
            let handles: Vec<_> = (0..self.cfg.channels).map(|c| sc.spawn(move || analyze_channel(&chans[c][..n], au))).collect();
            handles.into_iter().map(|h| h.join().expect("analysis thread panicked")).collect()
        });

        // 2. emit every access unit
        let mut out = Vec::new();
        let mut states: Vec<DecState> = vec![DecState::restart(); self.subs.len()];
        // restart header values, taken before this interval's samples accumulate
        let check_for_header: Vec<u8> = self.lossless.iter().map(|v| fold32(*v)).collect();
        for l in &mut self.lossless {
            *l = 0;
        }
        let max_bits: u32 = chans.iter().flat_map(|c| c[..n].iter()).map(|v| number_sbits(*v)).max().unwrap_or(1).min(24);

        for a in 0..n_au {
            let restart = a == 0;
            let mut sub_data: Vec<Vec<u8>> = Vec::new();
            for (si, sub) in self.subs.iter().enumerate() {
                let is_last_au = last && a == n_au - 1;
                let data = write_substream_au(
                    sub,
                    &analyses,
                    &mut states[si],
                    a,
                    au,
                    restart,
                    is_last_au.then_some(shorten),
                    self.output_timing,
                    check_for_header[si],
                    max_bits,
                    &chans.iter().map(|c| &c[..n]).collect::<Vec<_>>(),
                );
                sub_data.push(data);
                // accumulate the lossless check over this AU
                for ch in 0..=sub.max {
                    for i in a * au..(a + 1) * au {
                        self.lossless[si] ^= ((chans[ch][i] as u32) & 0xFF_FFFF) << ch;
                    }
                }
            }

            // access unit assembly
            let mut body = Vec::new();
            if restart {
                body.extend_from_slice(&self.write_major_sync());
            }
            let mut end = 0usize;
            let mut sub_headers = Vec::new();
            for d in &sub_data {
                end += d.len();
                let hdr: u16 = ((!restart as u16) << 14) | (1 << 13) | (((end / 2) as u16) & 0x0FFF);
                sub_headers.extend_from_slice(&hdr.to_be_bytes());
            }
            let total_len = 4 + body.len() + sub_headers.len() + end;
            let words = (total_len / 2) as u16;
            let timing = self.input_timing;
            let mut head = [0u8; 4];
            head[0] = (words >> 8) as u8 & 0x0F;
            head[1] = words as u8;
            head[2..4].copy_from_slice(&timing.to_be_bytes());
            // parity nibble: the XOR of all nibbles of the header and substream headers must be 0xF
            let mut x = parity(&head) ^ parity(&sub_headers);
            x = (x >> 4) ^ (x & 0xF);
            head[0] |= ((x ^ 0xF) as u8) << 4;
            out.extend_from_slice(&head);
            out.extend_from_slice(&body);
            out.extend_from_slice(&sub_headers);
            for d in sub_data {
                out.extend_from_slice(&d);
            }

            self.input_timing = self.input_timing.wrapping_add(au as u16);
            self.output_timing = self.output_timing.wrapping_add(au as u16);
        }
        self.sync_count += 1;
        out
    }
}

fn fold32(v: u32) -> u8 {
    let v = v ^ (v >> 16);
    (v ^ (v >> 8)) as u8
}

/* ------------------------------------------------------------------------------------------ */
/* analysis                                                                                    */
/* ------------------------------------------------------------------------------------------ */

struct ChannelAnalysis {
    quant: u8,
    fir: Fir,
    /// Residual per sample (full scale, a multiple of 2^quant); the first 8 samples are raw.
    residual: Vec<i32>,
    /// Exact entropy-coding choice for every block, `[access unit][block]` (two blocks in the first AU).
    codings: Vec<Vec<Coding>>,
}

fn analyze_channel(x: &[i32], au: usize) -> ChannelAnalysis {
    let n = x.len();
    let or_all = x.iter().fold(0i32, |a, v| a | v);
    let quant = if or_all == 0 { 0 } else { (or_all.trailing_zeros() as u8).min(15) };
    let mask = !((1i32 << quant) - 1);

    // candidate predictors: none, and LPC of a few orders; judged with a cheap bit estimate
    let mut best_fir = Fir::default();
    let mut best_res: Vec<i32> = x.to_vec();
    let mut best_cost = estimate_bits(&best_res, quant, au);

    if n > 64 {
        let xs: Vec<f64> = x.iter().map(|v| (*v >> quant) as f64).collect();
        let lpcs = levinson(&xs, MAX_ORDER);
        for &order in &[2usize, 4, 8] {
            if let Some(fir) = quantize_fir(&lpcs[order]) {
                let res = apply_fir(x, &fir, mask);
                let cost = estimate_bits(&res, quant, au) + 40.0 + order as f64 * 14.0;
                if cost < best_cost {
                    best_cost = cost;
                    best_fir = fir;
                    best_res = res;
                }
            }
        }
    }

    // exact coding choice for the real block layout (first AU = 8 priming samples + the rest)
    let n_au = n / au;
    let mut codings = Vec::with_capacity(n_au);
    for a in 0..n_au {
        let ranges: &[(usize, usize)] = if a == 0 { &[(0, 8), (8, 0)] } else { &[(0, 0)] };
        let mut per_block = Vec::new();
        if a == 0 {
            for (start, len) in [(0usize, 8usize), (8, au - 8)] {
                let v: Vec<i32> = best_res[start..start + len].iter().map(|r| r >> quant).collect();
                per_block.push(best_coding(&v, quant));
            }
        } else {
            let v: Vec<i32> = best_res[a * au..(a + 1) * au].iter().map(|r| r >> quant).collect();
            per_block.push(best_coding(&v, quant));
        }
        let _ = ranges;
        codings.push(per_block);
    }
    ChannelAnalysis { quant, fir: best_fir, residual: best_res, codings }
}

/// Cheap estimate of the bits needed to code `res` (about log2 of the mean magnitude per sample).
fn estimate_bits(res: &[i32], quant: u8, au: usize) -> f64 {
    let mut total = 0.0;
    for chunk in res.chunks(au) {
        let sum: u64 = chunk.iter().map(|r| ((r >> quant).unsigned_abs()) as u64).sum();
        let mean = sum as f64 / chunk.len() as f64;
        total += chunk.len() as f64 * ((mean + 1.0).log2() + 1.3);
    }
    total
}

/// Levinson-Durbin: returns predictor coefficients for every order 0..=max (x[n] ~ sum c[k] x[n-1-k]).
fn levinson(x: &[f64], max: usize) -> Vec<Vec<f64>> {
    let mut r = vec![0f64; max + 1];
    for lag in 0..=max {
        r[lag] = x.iter().zip(&x[lag..]).map(|(a, b)| a * b).sum();
    }
    r[0] = r[0] * (1.0 + 1e-9) + 1e-9;
    let mut out: Vec<Vec<f64>> = vec![Vec::new()];
    let mut a: Vec<f64> = Vec::new();
    let mut err = r[0];
    for i in 0..max {
        let mut acc = r[i + 1];
        for (j, aj) in a.iter().enumerate() {
            acc -= aj * r[i - j];
        }
        let k = if err.abs() < 1e-12 { 0.0 } else { acc / err };
        let mut na = vec![0f64; i + 1];
        for j in 0..i {
            na[j] = a[j] - k * a[i - 1 - j];
        }
        na[i] = k;
        err *= 1.0 - k * k;
        a = na;
        out.push(a.clone());
    }
    out
}

fn quantize_fir(c: &[f64]) -> Option<Fir> {
    let order = c.len();
    if order == 0 || order > MAX_ORDER {
        return None;
    }
    let peak = c.iter().fold(0f64, |m, v| m.max(v.abs()));
    if !peak.is_finite() || peak < 1e-6 {
        return None;
    }
    // largest shift (0..=15) so every coefficient fits in 16 signed bits
    let mut shift = 15u32;
    while shift > 0 && (peak * (1u32 << shift) as f64).round() > 32767.0 {
        shift -= 1;
    }
    let mut q = [0i32; MAX_ORDER];
    for (i, v) in c.iter().enumerate() {
        q[i] = (v * (1u32 << shift) as f64).round() as i32;
    }
    if q[..order].iter().all(|v| *v == 0) {
        return None;
    }
    let mut bits = 1;
    let mut or_all = 0i32;
    for v in &q[..order] {
        bits = bits.max(number_sbits(*v));
        or_all |= v;
    }
    let tz = if or_all == 0 { 0 } else { or_all.trailing_zeros().min(7) };
    let coeff_bits = bits.saturating_sub(tz).max(1);
    let coeff_shift = tz.min(16 - coeff_bits);
    // coefficients must be multiples of 2^coeff_shift
    for v in &mut q[..order] {
        *v = (*v >> coeff_shift) << coeff_shift;
    }
    Some(Fir { order: order as u8, shift: shift as u8, coeff_bits: coeff_bits as u8, coeff_shift: coeff_shift as u8, coeff: q })
}

/// Residual exactly as a decoder will undo it. The first 8 samples are left unfiltered.
fn apply_fir(x: &[i32], fir: &Fir, mask: i32) -> Vec<i32> {
    let order = fir.order as usize;
    let mut res = x.to_vec();
    if order == 0 {
        return res;
    }
    for i in 8..x.len() {
        let mut acc = 0i64;
        for k in 0..order {
            acc += x[i - 1 - k] as i64 * fir.coeff[k] as i64;
        }
        let pred = (acc >> fir.shift) as i32;
        res[i] = x[i].wrapping_sub(pred & mask);
    }
    res
}

/* ------------------------------------------------------------------------------------------ */
/* entropy coding of one channel block                                                         */
/* ------------------------------------------------------------------------------------------ */

#[derive(Clone, Copy, Debug)]
struct Coding {
    codebook: u8,
    /// raw bits per sample
    lsb_bits: u32,
    bits: usize,
}

/// Value of `huff_offset`-corrected symbol input for sample `v` under `(codebook, lsb_bits)`.
/// Returns the non-negative number to split into (symbol, raw bits).
#[inline]
fn shifted(v: i32, codebook: u8, l: u32) -> i64 {
    let sign_shift: i32 = l as i32 + if codebook != 0 { 2 - codebook as i32 } else { -1 };
    let mut off = 0i64;
    if codebook > 0 {
        off -= 7i64 << l;
    }
    if sign_shift >= 0 {
        off -= 1i64 << sign_shift;
    }
    v as i64 - off
}

fn cost_of(v: &[i32], codebook: u8, l: u32) -> Option<usize> {
    let mut bits = 0usize;
    for &s in v {
        let y = shifted(s, codebook, l);
        if y < 0 {
            return None;
        }
        if codebook == 0 {
            if (y >> l) != 0 {
                return None;
            }
            bits += l as usize;
        } else {
            let sym = (y >> l) as usize;
            if sym >= 18 {
                return None;
            }
            let len = HUFFMAN[codebook as usize - 1][sym].1;
            if len == 0 {
                return None;
            }
            bits += len as usize + l as usize;
        }
    }
    Some(bits)
}

/// Pick the cheapest (codebook, lsb_bits) for a block of already-shifted residual values.
fn best_coding(v: &[i32], quant: u8) -> Coding {
    let max_l = 24u32.saturating_sub(quant as u32).min(24);
    let peak = v.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    // raw coding: smallest l that holds every value
    let mut best: Option<Coding> = None;
    if peak == 0 {
        return Coding { codebook: 0, lsb_bits: 0, bits: 0 };
    }
    let need = 32 - peak.leading_zeros(); // bits for the magnitude
    for l in need..=(need + 1).min(31) {
        if let Some(b) = cost_of(v, 0, l) {
            best = Some(Coding { codebook: 0, lsb_bits: l, bits: b });
            break;
        }
    }
    let mean = v.iter().map(|s| s.unsigned_abs() as u64).sum::<u64>() / v.len().max(1) as u64;
    let l0 = (64 - mean.leading_zeros()) as i32; // about log2(mean)+1
    for l in (l0 - 3).max(0)..=(l0 + 1) {
        let l = l as u32;
        if l > max_l {
            continue;
        }
        for cb in 1..=3u8 {
            if let Some(b) = cost_of(v, cb, l) {
                if best.map_or(true, |c| b < c.bits) {
                    best = Some(Coding { codebook: cb, lsb_bits: l, bits: b });
                }
            }
        }
    }
    best.unwrap_or(Coding { codebook: 0, lsb_bits: (need + 1).min(31), bits: v.len() * (need as usize + 1) })
}

/* ------------------------------------------------------------------------------------------ */
/* substream writing                                                                           */
/* ------------------------------------------------------------------------------------------ */

struct BlockPlan {
    start: usize,
    len: usize,
    /// desired decoder state after the block's parameter update
    want: DecState,
    codings: [Coding; MAX_CHANNELS],
}

fn plan_blocks(sub: &Substream, an: &[ChannelAnalysis], au_index: usize, au: usize, restart: bool) -> Vec<BlockPlan> {
    let ranges: Vec<(usize, usize)> = if restart {
        vec![(0, 8), (8, au - 8)]
    } else {
        vec![(au_index * au, au)]
    };
    let mut plans = Vec::new();
    for (bi, (start, len)) in ranges.into_iter().enumerate() {
        let mut want = DecState::restart();
        want.blocksize = len;
        let mut codings = [Coding { codebook: 0, lsb_bits: 0, bits: 0 }; MAX_CHANNELS];
        for ch in sub.min..=sub.max {
            let a = &an[ch];
            want.quant[ch] = a.quant;
            // the priming block is unfiltered; later blocks use the interval's predictor
            let fir = if restart && bi == 0 { Fir::default() } else { a.fir };
            let c = a.codings[au_index][bi];
            codings[ch] = c;
            want.ch[ch] = ChParams { fir, codebook: c.codebook, lsbs: (c.lsb_bits + a.quant as u32) as u8 };
        }
        plans.push(BlockPlan { start, len, want, codings });
    }
    plans
}

#[allow(clippy::too_many_arguments)]
fn write_substream_au(
    sub: &Substream,
    an: &[ChannelAnalysis],
    st: &mut DecState,
    au_index: usize,
    au: usize,
    restart: bool,
    end_of_stream_shorten: Option<usize>,
    output_timing: u16,
    lossless_check: u8,
    max_output_bits: u32,
    _raw: &[&[i32]],
) -> Vec<u8> {
    let mut w = BitWriter::new();
    let plans = plan_blocks(sub, an, au_index, au, restart);
    let nblocks = plans.len();
    if restart {
        *st = DecState::restart();
    }
    for (bi, plan) in plans.iter().enumerate() {
        let first_with_restart = restart && bi == 0;
        let diff = params_differ(sub, st, &plan.want);
        if first_with_restart || diff {
            w.put(1, 1); // parameter block present
            if first_with_restart {
                w.put(1, 1); // restart header present
                write_restart_header(&mut w, sub, output_timing, lossless_check, max_output_bits, plans.iter().map(|p| &p.want).fold(0u32, |m, s| (sub.min..=sub.max).map(|c| s.ch[c].lsbs as u32).fold(m, u32::max)));
            } else {
                w.put(0, 1);
            }
            write_params(&mut w, sub, st, &plan.want);
            *st = plan.want.clone();
        } else {
            w.put(0, 1);
        }
        write_block_data(&mut w, sub, an, plan);
        w.put(u32::from(bi == nblocks - 1), 1); // last block of this access unit
    }
    w.align(16);
    if let Some(shorten) = end_of_stream_shorten.filter(|s| *s > 0) {
        w.put(0xD234, 16);
        w.put((shorten as u32 & 0x1FFF) | 0xE000, 16);
    }
    let mut bytes = w.into_bytes();
    let p = parity(&bytes) ^ 0xA9;
    let c = checksum8(&bytes);
    bytes.push(p);
    bytes.push(c);
    bytes
}

fn params_differ(sub: &Substream, cur: &DecState, want: &DecState) -> bool {
    cur.blocksize != want.blocksize || cur.quant != want.quant || (sub.min..=sub.max).any(|c| cur.ch[c] != want.ch[c])
}

fn write_restart_header(w: &mut BitWriter, sub: &Substream, output_timing: u16, lossless_check: u8, max_output_bits: u32, max_huff_lsbs: u32) {
    let start = w.bit_len();
    // matrix channels above 5 need the "noise type" variant of the restart sync word
    w.put(if sub.max > 5 { 0x31EB } else { 0x31EA }, 14);
    w.put(output_timing as u32, 16);
    w.put(sub.min as u32, 4);
    w.put(sub.max as u32, 4);
    w.put(sub.max as u32, 4); // max matrix channel: no matrices, so the last coded channel
    w.put(0, 4); // noise shift
    w.put(0, 23); // noise generator seed
    w.put(0, 4); // max shift
    w.put(max_huff_lsbs.min(31), 5);
    w.put(max_output_bits, 5);
    w.put(max_output_bits, 5);
    w.put(0, 1); // no per-block length check
    w.put(lossless_check as u32, 8);
    w.put(0, 16);
    for ch in 0..=sub.max {
        w.put(ch as u32, 6); // output channel assignment: identity
    }
    let bits = w.bit_len() - start;
    // the checksum covers the header bits but not the two flag bits before it
    let buf = {
        let mut b = w.snapshot();
        b.push(0);
        b
    };
    let flag_bits_offset = start - 2;
    debug_assert!(flag_bits_offset == 0, "restart header must start the substream");
    let ck = restart_checksum(&buf, bits);
    w.put(ck as u32, 8);
}

fn write_filter(w: &mut BitWriter, f: &Fir) {
    w.put(f.order as u32, 4);
    if f.order > 0 {
        w.put(f.shift as u32, 4);
        w.put(f.coeff_bits as u32, 5);
        w.put(f.coeff_shift as u32, 3);
        for i in 0..f.order as usize {
            w.put_signed(f.coeff[i] >> f.coeff_shift, f.coeff_bits as u32);
        }
        w.put(0, 1); // no state data
    }
}

/// Write the decoding parameter block that moves the decoder from `cur` to `want`.
fn write_params(w: &mut BitWriter, sub: &Substream, cur: &DecState, want: &DecState) {
    w.put(0, 1); // parameter presence flags unchanged
    // blocksize
    if cur.blocksize != want.blocksize {
        w.put(1, 1);
        w.put(want.blocksize as u32, 9);
    } else {
        w.put(0, 1);
    }
    w.put(0, 1); // matrices unchanged
    w.put(0, 1); // output shift unchanged
    // quantizer steps cover channels 0..=max
    if cur.quant != want.quant {
        w.put(1, 1);
        for ch in 0..=sub.max {
            w.put(want.quant[ch] as u32, 4);
        }
    } else {
        w.put(0, 1);
    }
    for ch in sub.min..=sub.max {
        if cur.ch[ch] != want.ch[ch] {
            w.put(1, 1);
            if cur.ch[ch].fir != want.ch[ch].fir {
                w.put(1, 1);
                write_filter(w, &want.ch[ch].fir);
            } else {
                w.put(0, 1);
            }
            w.put(0, 1); // no IIR change
            w.put(0, 1); // no huffman offset change
            w.put(want.ch[ch].codebook as u32, 2);
            w.put(want.ch[ch].lsbs as u32, 5);
        } else {
            w.put(0, 1);
        }
    }
}

fn write_block_data(w: &mut BitWriter, sub: &Substream, an: &[ChannelAnalysis], plan: &BlockPlan) {
    for i in 0..plan.len {
        for ch in sub.min..=sub.max {
            let a = &an[ch];
            let v = a.residual[plan.start + i] >> a.quant;
            let c = plan.codings[ch];
            let y = shifted(v, c.codebook, c.lsb_bits);
            debug_assert!(y >= 0, "negative symbol input");
            if c.codebook > 0 {
                let sym = (y >> c.lsb_bits) as usize;
                let (code, len) = HUFFMAN[c.codebook as usize - 1][sym];
                w.put(code as u32, len as u32);
            }
            if c.lsb_bits > 0 {
                w.put((y & ((1i64 << c.lsb_bits) - 1)) as u32, c.lsb_bits);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_one_layout_order() {
        let m = map_layout(ChannelLayout::find("5.1").unwrap()).unwrap();
        // L R C LFE Ls Rs is already TrueHD order
        assert_eq!(m.source_index, vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(m.chanmap, 0b1111);
    }

    #[test]
    fn seven_one_has_rear_group() {
        let m = map_layout(ChannelLayout::find("7.1-side-rear").unwrap()).unwrap();
        assert_eq!(m.source_index.len(), 8);
        assert_eq!(m.chanmap, 0b100_1111);
    }

    #[test]
    fn unpaired_channels_are_rejected() {
        // 4.1 L C R S LFE has a single surround (Cs is fine) but "3.0-lrs" style has no centre issue:
        // a lone Ls without Rs cannot be expressed
        assert!(map_layout(ChannelLayout::find("4.0-lcrs").unwrap()).is_ok());
    }

    #[test]
    fn sbits() {
        assert_eq!(number_sbits(0), 1);
        assert_eq!(number_sbits(1), 2);
        assert_eq!(number_sbits(-1), 1);
        assert_eq!(number_sbits(127), 8);
        assert_eq!(number_sbits(-128), 8);
    }

    #[test]
    fn coding_choices_roundtrip_through_the_cost_model() {
        // a block of small values should prefer a Huffman codebook over raw coding
        let v: Vec<i32> = (0..40).map(|i| ((i * 7) % 5) as i32 - 2).collect();
        let c = best_coding(&v, 0);
        assert!(c.bits < 40 * 4, "{c:?}");
        // digital silence costs nothing
        assert_eq!(best_coding(&[0; 40], 0).bits, 0);
    }

    #[test]
    fn quantized_filter_fits_the_format() {
        let f = quantize_fir(&[1.6, -0.7]).unwrap();
        assert!(f.coeff_bits as u32 + f.coeff_shift as u32 <= 16);
        assert!(f.shift <= 15);
    }
}
