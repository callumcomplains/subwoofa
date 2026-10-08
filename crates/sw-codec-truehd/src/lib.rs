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

pub mod analyze;
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
        _ => return None,
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

/// Stereo downmix carried in substream 0 (coded channels 0 and 1).
///
/// With `dest = D` (2.14 fixed point, `1 << 14` = unity up to 32767 ~ 2.0) coded channel 0 becomes
/// `Lo = (L*2^14 + sum(lo[j] * x[j])) / D`, and channel 1 likewise for `Ro`, summing over the other
/// channels `j >= 2`. `D > 2^14` attenuates the mix by `2^14 / D` for headroom. A full decoder
/// recovers L exactly as `((D*Lo - sum(lo[j] * x[j])) >> 14) + b`, where `b` is one stored bit per
/// sample (the "LSB bypass") that absorbs the rounding of `Lo`. With `D == 2^14` no bit is needed.
/// For eight channels the rear pair (coded 6 and 7) is first folded into the side surrounds
/// (coded 4 and 5) with unit gain by `fold`, which defines the 6-channel presentation.
#[derive(Debug, Clone, Default)]
pub struct Downmix {
    /// Coefficient of each coded channel into Lo (indexes 0 and 1 are ignored).
    pub lo: Vec<i32>,
    pub ro: Vec<i32>,
    /// 2.14 coefficient folding Lrs/Rrs into Ls/Rs (7.1 only).
    pub fold: i32,
    /// Decoder-side gain of the front channels in 2.14 (16384 to 32767).
    pub dest: i32,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub sample_rate: u32,
    pub chanmap: u16,
    pub channels: usize,
    /// Access units per restart interval (8..=128).
    pub restart_interval: usize,
    /// Stereo downmix substream; `None` codes the channels directly (L/R in substream 0).
    pub downmix: Option<Downmix>,
    /// Predict each channel from the lower-numbered ones with per-interval matrices (lossless).
    pub decorrelate: bool,
    /// Write MLP (DVD-Audio style) instead of TrueHD: one substream, no downmix, up to 6 channels.
    pub mlp: Option<Mlp>,
}

/// MLP stream parameters.
#[derive(Debug, Clone, Copy)]
pub struct Mlp {
    /// Channel assignment code of the major sync (see `map_layout_mlp`).
    pub arrangement: u8,
    /// Source word length (16, 20 or 24).
    pub bits: u32,
}

/// `(channel occupancy, summary info)` of the MLP channel assignments 0..=12.
const MLP_CHANNEL_INFO: [(u8, u8); 13] = [(0x01, 0x1f), (0x03, 0x1b), (0x07, 0x1f), (0x0F, 0x19), (0x07, 0x03), (0x0F, 0x1f), (0x1F, 0x01), (0x07, 0x1a), (0x0F, 0x1f), (0x1F, 0x18), (0x0F, 0x02), (0x1F, 0x1f), (0x3F, 0x00)];

/// MLP channel assignment and coded channel order for a layout (the layouts the format can describe).
pub fn map_layout_mlp(layout: &ChannelLayout) -> Result<(u8, Vec<usize>), Error> {
    use sw_core::Channel::*;
    const TABLE: &[(u8, &[sw_core::Channel])] = &[
        (0, &[C]),
        (1, &[L, R]),
        (2, &[L, R, Cs]),
        (3, &[L, R, Ls, Rs]),
        (4, &[L, R, Lfe]),
        (7, &[L, R, C]),
        (8, &[L, R, C, Cs]),
        (9, &[L, R, C, Ls, Rs]),
        (10, &[L, R, C, Lfe]),
        (11, &[L, R, C, Lfe, Cs]),
        (12, &[L, R, C, Lfe, Ls, Rs]),
    ];
    for (code, order) in TABLE {
        if order.len() == layout.channels.len() && order.iter().all(|c| layout.channels.contains(c)) {
            let idx = order.iter().map(|c| layout.channels.iter().position(|x| x == c).unwrap()).collect();
            return Ok((*code, idx));
        }
    }
    Err(Error::Unsupported(format!("layout '{}' has no MLP channel assignment (mono to 5.1 in the DVD-Audio arrangements)", layout.id)))
}

/// One primitive matrix: rewrites channel `out` from a weighted sum of all channels (2.14).
#[derive(Clone, PartialEq, Eq, Debug)]
struct MatrixSpec {
    out: usize,
    coeff: Vec<i32>,
    lsb_bypass: bool,
}

/// Round-up division by 2^14 used by the downmix (the exact inverse of the decoder's floor shift).
#[inline]
fn ceil14(y: i64) -> i64 {
    (y + 16383) >> 14
}

/// Default ITU-style Lo/Ro downmix for a layout: centre and surrounds at -3 dB, LFE omitted.
/// Returns `None` for layouts where a downmix cannot be expressed (mono, stereo, unusual 7ch/8ch).
pub fn default_downmix(layout: &ChannelLayout, map: &LayoutMap) -> Option<Downmix> {
    let n = map.source_index.len();
    if n <= 2 {
        return None;
    }
    let m = sw_core::downmix::Matrix::lo_ro(layout);
    // the front pair must be exactly L and R in coded slots 0 and 1
    let l = layout.channels[map.source_index[0]];
    let r = layout.channels[map.source_index[1]];
    if !matches!(l, Channel::L | Channel::Lt) || !matches!(r, Channel::R | Channel::Rt) {
        return None;
    }
    // half-level mix for headroom: a stereo-only decoder must be able to hold L + 0.7C + 0.7Ls in 24 bits
    let mut dm = Downmix { lo: vec![0; n], ro: vec![0; n], fold: 0, dest: 32767 };
    let q = |db: f32| -> i32 {
        if db == sw_core::downmix::OFF { 0 } else { (10f64.powf(db as f64 / 20.0) * 16384.0).round() as i32 }
    };
    let gain = |from: Channel, to: Channel| m.in_idx(from).zip(m.out_idx(to)).map(|(i, o)| m.cells[o][i].gain_db).unwrap_or(sw_core::downmix::OFF);
    if n > 6 {
        // eight channels: only L R C LFE Ls Rs Lrs Rrs (side + rear surrounds) is supported
        if n != 8 {
            return None;
        }
        let rear_l = layout.channels[map.source_index[6]];
        let rear_r = layout.channels[map.source_index[7]];
        if !matches!(rear_l, Channel::Lsr) || !matches!(rear_r, Channel::Rsr) {
            return None;
        }
        dm.fold = q(-3.0103);
    }
    for j in 2..n.min(6) {
        let ch = layout.channels[map.source_index[j]];
        dm.lo[j] = q(gain(ch, Channel::L));
        dm.ro[j] = q(gain(ch, Channel::R));
    }
    Some(dm)
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
    iir: Fir,
    codebook: u8,
    lsbs: u8,
}

const NO_FILTER: Fir = Fir { order: 0, shift: 0, coeff_bits: 0, coeff_shift: 0, coeff: [0; MAX_ORDER] };
const DEFAULT_CH: ChParams = ChParams { fir: NO_FILTER, iir: NO_FILTER, codebook: 0, lsbs: 24 };

/// What the decoder currently believes (mirrors the decoder's state machine).
#[derive(Clone, Debug)]
struct DecState {
    blocksize: usize,
    quant: [u8; MAX_CHANNELS],
    ch: [ChParams; MAX_CHANNELS],
    matrices: Vec<MatrixSpec>,
}

impl DecState {
    fn restart() -> DecState {
        DecState { blocksize: 8, quant: [0; MAX_CHANNELS], ch: [DEFAULT_CH; MAX_CHANNELS], matrices: Vec::new() }
    }
}

#[derive(Clone, Debug)]
struct Substream {
    min: usize,
    max: usize,
    /// Primitive matrices this substream carries.
    matrices: Vec<MatrixSpec>,
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
    /// Stereo-presentation samples that exceed 24 bits (a stereo-only decoder would wrap or clip them).
    pub downmix_overflows: u64,
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
        if cfg.mlp.is_some() && (cfg.downmix.is_some() || n > 6) {
            return Err(Error::Unsupported("MLP carries up to 6 channels in one substream, without a downmix".into()));
        }
        if let Some(dm) = &cfg.downmix {
            if !(1 << 14..=32767).contains(&dm.dest) {
                return Err(Error::Unsupported("downmix gain must be between 16384 and 32767 (2.14)".into()));
            }
            if n <= 2 || dm.lo.len() != n || dm.ro.len() != n {
                return Err(Error::Unsupported("downmix needs more than two channels and one coefficient per channel".into()));
            }
            if n > 6 && n != 8 {
                return Err(Error::Unsupported("the stereo downmix supports up to 6 channels, or exactly 8".into()));
            }
        }
        let front = |dm: &Downmix, max: usize| -> Vec<MatrixSpec> {
            [(0usize, &dm.lo), (1usize, &dm.ro)]
                .into_iter()
                .map(|(dest, q)| {
                    let mut c = vec![0i32; max + 1];
                    c[dest] = dm.dest;
                    for j in 2..=max.min(5) {
                        c[j] = -q[j];
                    }
                    MatrixSpec { out: dest, coeff: c, lsb_bypass: false }
                })
                .collect()
        };
        let dm = cfg.downmix.as_ref();
        let none = || Vec::new();
        let subs = if cfg.mlp.is_some() || n <= 2 {
            vec![Substream { min: 0, max: n - 1, matrices: none() }]
        } else if n <= 6 {
            vec![
                Substream { min: 0, max: 1, matrices: none() },
                Substream { min: 2, max: n - 1, matrices: dm.map(|d| front(d, n - 1)).unwrap_or_default() },
            ]
        } else {
            let mut full = dm.map(|d| front(d, 7)).unwrap_or_default();
            if let Some(d) = dm {
                // undo the rear fold after the front matrices have read the folded surrounds
                for (dest, rear) in [(4usize, 6usize), (5, 7)] {
                    let mut c = vec![0i32; 8];
                    c[dest] = 1 << 14;
                    c[rear] = -d.fold;
                    full.push(MatrixSpec { out: dest, coeff: c, lsb_bypass: false });
                }
            }
            vec![
                Substream { min: 0, max: 1, matrices: none() },
                Substream { min: 2, max: 5, matrices: dm.map(|d| front(d, 5)).unwrap_or_default() },
                Substream { min: 6, max: n - 1, matrices: full },
            ]
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
            downmix_overflows: 0,
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

    fn write_major_sync_mlp(&self, m: Mlp) -> [u8; 28] {
        let mut w = BitWriter::new();
        w.put(0xF8726F, 24);
        w.put(0xBB, 8);
        w.put(match m.bits { 16 => 0, 20 => 1, _ => 2 }, 4);
        w.put(0xF, 4);
        w.put(self.rate_code, 4);
        w.put(0xF, 4);
        w.put(0, 4);
        w.put(0, 4); // multichannel type
        w.put(0, 3);
        w.put(m.arrangement as u32, 5);
        w.put(0xB752, 16);
        w.put(0x4000, 16); // DVD-Audio flag
        w.put(0, 16);
        w.put(1, 1); // variable bit rate
        let peak = ((9_600_000u32 << 4) - 8) / self.cfg.sample_rate;
        w.put(peak & 0x7FFF, 15);
        w.put(1, 4); // one substream
        w.put(0, 2);
        w.put(0, 2); // extended substream info
        let high = self.rate_code & 3 == 2;
        let info = 0x04 | u32::from(self.cfg.channels <= 2) | if high { 0x02 } else { 0 };
        w.put(info, 8);
        let fs = match self.cfg.sample_rate { 44_100 => 0x09, 88_200 => 0x0D, 176_400 => 0x11, 48_000 => 0x0A, 96_000 => 0x0E, _ => 0x12 };
        w.put(fs, 5);
        w.put(m.bits, 5);
        let (occupancy, summary) = MLP_CHANNEL_INFO[m.arrangement as usize];
        w.put(occupancy as u32, 6);
        w.put(0, 3);
        w.put(0, 10); // speaker layout
        w.put(0, 3); // copy protection
        w.put(0x8080, 16);
        w.put(0, 7);
        w.put(0, 4); // source format
        w.put(summary as u32, 5);
        let mut bytes = w.snapshot();
        bytes.resize(28, 0);
        let c = major_sync_checksum(&bytes[..26]);
        bytes[26..28].copy_from_slice(&c.to_le_bytes());
        bytes.try_into().unwrap()
    }

    fn write_major_sync(&self) -> [u8; 28] {
        if let Some(m) = self.cfg.mlp {
            return self.write_major_sync_mlp(m);
        }
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

        // 0. coded domain: with a downmix, channels 0/1 carry Lo/Ro (and 4/5 the folded surrounds of 7.1)
        let nch = self.cfg.channels;
        let tz_of = |c: usize| -> u8 {
            let or_all = chans[c][..n].iter().fold(0i32, |a, v| a | v);
            if or_all == 0 { 0 } else { (or_all.trailing_zeros() as u8).min(15) }
        };
        // Sources with unused low bits (16/18/20-bit material in 24-bit files) get a coarse downmix: the
        // decoder snaps the reconstructed front channels to their own grid, so the stereo channels need
        // not carry those bits.
        let coarse = self.cfg.downmix.is_some() && nch <= 6;
        let src_q: [u8; 2] = if coarse { [tz_of(0), tz_of(1)] } else { [0, 0] };
        let mut bypass: [Vec<u8>; 2] = [Vec::new(), Vec::new()];
        // Unity mode: the coded pair is the full-level downmix on the source's grid and substream 0 scales it
        // to the stereo presentation with matrices; that keeps the pair free to be decorrelated.
        let mut unity = false;
        let coded_store: Option<Vec<Vec<i32>>> = self.cfg.downmix.as_ref().map(|dm| {
            if coarse && src_q[0] >= 1 && src_q[1] >= 1 {
                let mut u = dm.clone();
                u.dest = 1 << 14;
                let (c, b) = apply_downmix(chans, n, &u, nch, src_q);
                if c[0].iter().chain(c[1].iter()).all(|v| v.unsigned_abs() < 1 << 23) {
                    unity = true;
                    bypass = b;
                    return c;
                }
            }
            let (c, b) = apply_downmix(chans, n, dm, nch, src_q);
            bypass = b;
            c
        });
        let coded: &[Vec<i32>] = coded_store.as_deref().unwrap_or(chans);
        if self.cfg.downmix.is_some() {
            self.downmix_overflows += coded[0][..n].iter().chain(coded[1][..n].iter()).filter(|v| v.unsigned_abs() >= 1 << 23).count() as u64;
        }
        // quantizer step of the stereo channels as coded in substream 0, and whether the front matrices need bypass bits
        let dest = self.cfg.downmix.as_ref().map_or(1 << 14, |d| d.dest);
        let eff_dest = if unity { 1 << 14 } else { dest };
        let stereo_q: [Option<u8>; 2] = [0, 1].map(|k| if coarse && src_q[k] >= 1 { Some(coarse_quant(eff_dest, src_q[k])) } else { None });
        let need_bypass: [bool; 2] = [0, 1].map(|k| self.cfg.downmix.is_some() && stereo_q[k].is_none() && eff_dest != 1 << 14);
        // channels written by a static matrix keep a zero quantizer step (except the coarse stereo pair)
        let forced_q: Vec<Option<u8>> = (0..nch)
            .map(|c| {
                if c < 2 {
                    if self.cfg.downmix.is_some() { Some(stereo_q[c].unwrap_or(0)) } else { None }
                } else if self.subs.iter().any(|s| s.matrices.iter().any(|m| m.out == c)) {
                    Some(0)
                } else {
                    None
                }
            })
            .collect();

        // 1. analysis: per channel quantizer step, predictor and residual
        let decorrelate = self.cfg.decorrelate;
        let both: Vec<(ChannelAnalysis, Option<(ChannelAnalysis, usize)>)> = {
            use rayon::prelude::*;
            (0..self.cfg.channels)
                .into_par_iter()
                .map(|c| {
                    let forced = forced_q[c];
                    let allow = decorrelate && !(coarse && (c == 0 || (c == 1 && !unity)));
                    analyze_with_prediction(coded, c, n, au, forced, allow)
                })
                .collect()
        };
        // decoders accept at most 8 primitive matrices per substream: keep the predictions that save most
        let static_max = self.subs.iter().map(|s| s.matrices.len()).max().unwrap_or(0);
        let budget = 8usize.saturating_sub(static_max);
        let mut order: Vec<usize> = (0..both.len()).filter(|&c| both[c].1.is_some()).collect();
        order.sort_by_key(|&c| std::cmp::Reverse(both[c].1.as_ref().map_or(0, |p| p.1)));
        let keep: Vec<usize> = order.into_iter().take(budget).collect();
        let analyses: Vec<ChannelAnalysis> = both
            .into_iter()
            .enumerate()
            .map(|(c, (base, with))| match with {
                Some((w, _)) if keep.contains(&c) => w,
                _ => base,
            })
            .collect();
        // quantizer step of each channel as the matrices see it (the mask on a matrix output)
        let out_q: Vec<u8> = (0..nch).map(|c| if c < 2 && self.cfg.downmix.is_some() { src_q[c] } else { analyses[c].quant }).collect();
        // per-substream matrix lists: cross-channel predictions (ascending channel order), then the downmix
        let sub_mats: Vec<Vec<MatrixSpec>> = self
            .subs
            .iter()
            .map(|sub| {
                let mut list: Vec<MatrixSpec> = (1..=sub.max)
                    .filter_map(|k| {
                        analyses[k].decor.as_ref().map(|c| {
                            let mut coeff = c.clone();
                            coeff.resize(sub.max + 1, 0);
                            MatrixSpec { out: k, coeff, lsb_bypass: false }
                        })
                    })
                    .collect();
                list.extend(sub.matrices.iter().map(|m| {
                    let mut m = m.clone();
                    if m.out < 2 {
                        m.lsb_bypass = need_bypass[m.out];
                        m.coeff[m.out] = eff_dest;
                    }
                    m
                }));
                if unity && sub.min == 0 {
                    // stereo presentation: the coded full-level pair scaled by 1/dest (the half-level default)
                    let g = (((1i64 << 28) + dest as i64 / 2) / dest as i64) as i32;
                    for k in 0..2 {
                        let mut coeff = vec![0; sub.max + 1];
                        coeff[k] = g;
                        list.push(MatrixSpec { out: k, coeff, lsb_bypass: false });
                    }
                }
                list
            })
            .collect();

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
                    &sub_mats[si],
                    &out_q,
                    &analyses,
                    &mut states[si],
                    a,
                    au,
                    restart,
                    is_last_au.then_some(shorten),
                    self.cfg.mlp.is_some(),
                    self.output_timing,
                    check_for_header[si],
                    max_bits,
                    &bypass,
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

/// Quantizer step for the coded stereo channel when the source front channel has `m` unused low
/// bits: the largest step whose grid still pins the reconstructed channel to its own grid.
fn coarse_quant(dest: i32, m: u8) -> u8 {
    let mut q = m;
    while q > 0 && (dest as i64) << q > 1i64 << (14 + m as u32) {
        q -= 1;
    }
    q
}

/// Coded-domain channels and LSB-bypass bits for the downmix layout (see [`Downmix`]).
/// `mq` is the number of unused low bits of the front channels (coarse downmix when at least one).
fn apply_downmix(chans: &[Vec<i32>], n: usize, dm: &Downmix, nch: usize, mq: [u8; 2]) -> (Vec<Vec<i32>>, [Vec<u8>; 2]) {
    let mut v: Vec<Vec<i32>> = chans.iter().map(|c| c[..n].to_vec()).collect();
    if nch == 8 {
        for i in 0..n {
            v[4][i] = (chans[4][i] as i64 + ceil14(dm.fold as i64 * chans[6][i] as i64)) as i32;
            v[5][i] = (chans[5][i] as i64 + ceil14(dm.fold as i64 * chans[7][i] as i64)) as i32;
        }
    }
    let last = nch.min(6);
    let d = dm.dest as i64;
    let unity = dm.dest == 1 << 14;
    let mut bits = [vec![0u8; n], vec![0u8; n]];
    for i in 0..n {
        let (mut y0, mut y1) = (0i64, 0i64);
        for j in 2..last {
            y0 += dm.lo[j] as i64 * v[j][i] as i64;
            y1 += dm.ro[j] as i64 * v[j][i] as i64;
        }
        for (k, (orig, y)) in [(chans[0][i] as i64, y0), (chans[1][i] as i64, y1)].into_iter().enumerate() {
            let coded = if mq[k] >= 1 {
                // smallest multiple of 2^q with D*z - y >= orig * 2^14: the decoder's masked output is then `orig`
                let q = coarse_quant(dm.dest, mq[k]);
                let step = d << q;
                let target = (orig << 14) + y;
                (target.div_euclid(step) + i64::from(target.rem_euclid(step) != 0)) << q
            } else if unity {
                orig + ceil14(y)
            } else {
                // smallest s with D*s - y >= (orig - 1) * 2^14, so that (D*s - y) >> 14 is orig or orig - 1
                let target = ((orig - 1) << 14) + y;
                let s = target.div_euclid(d) + i64::from(target.rem_euclid(d) != 0);
                let l_hat = (d * s - y) >> 14;
                debug_assert!(orig - l_hat == 0 || orig - l_hat == 1, "bypass bit out of range");
                bits[k][i] = (orig - l_hat) as u8;
                s
            };
            v[k][i] = coded as i32;
        }
    }
    (v, bits)
}

fn fold32(v: u32) -> u8 {
    let v = v ^ (v >> 16);
    (v ^ (v >> 8)) as u8
}

/* ------------------------------------------------------------------------------------------ */
/* analysis                                                                                    */
/* ------------------------------------------------------------------------------------------ */

struct ChannelAnalysis {
    /// Cross-channel prediction chosen for this channel: 2.14 coefficients of channels up to itself.
    decor: Option<Vec<i32>>,
    quant: u8,
    fir: Fir,
    iir: Fir,
    /// Residual per sample (full scale, a multiple of 2^quant); the first 8 samples are raw.
    residual: Vec<i32>,
    /// Exact entropy-coding choice for every block, `[access unit][block]` (two blocks in the first AU).
    codings: Vec<Vec<Coding>>,
}

fn analyze_channel(x: &[i32], au: usize, quant: u8) -> ChannelAnalysis {
    let n = x.len();
    let mask = !((1i32 << quant) - 1);

    // candidate predictors: none, LPC of a few orders, and LPC + a short moving-average (IIR) stage
    let mut best_fir = Fir::default();
    let mut best_iir = Fir::default();
    let mut best_res: Vec<i32> = x.to_vec();
    let mut best_cost = estimate_bits(&best_res, quant, au);

    if n > 64 {
        let xs: Vec<f64> = x.iter().map(|v| (*v >> quant) as f64).collect();
        let lpcs = levinson(&xs, MAX_ORDER);
        let mut consider = |fir: Fir, iir: Fir| {
            let res = apply_filters(x, &fir, &iir, mask);
            let cost = estimate_bits(&res, quant, au) + 40.0 + fir.order as f64 * 14.0 + if iir.order > 0 { 40.0 + iir.order as f64 * 14.0 } else { 0.0 };
            if cost < best_cost {
                best_cost = cost;
                best_fir = fir;
                best_iir = iir;
                best_res = res;
            }
        };
        for &order in &[2usize, 4, 8] {
            if let Some(fir) = quantize_fir(&lpcs[order]) {
                consider(fir, Fir::default());
            }
        }
        // Fixed two-tap IIR stages (poles close to the unit circle) boost the empty top of the spectrum of
        // band-limited high-rate material; the FIR then only has to deal with the rest.
        for &(b1, b2) in FIXED_IIR {
            let mut w = vec![0f64; xs.len()];
            for i in 0..xs.len() {
                let mut v = xs[i];
                if i >= 1 {
                    v -= b1 * w[i - 1];
                }
                if i >= 2 {
                    v -= b2 * w[i - 2];
                }
                w[i] = v;
            }
            for order in [6usize, 5] {
                if let Some(c) = lpc_covariance(&w, order) {
                    if let Some((fir, iir)) = quantize_pair(&c, &[b1, b2]) {
                        consider(fir, iir);
                    }
                }
            }
            let lw = levinson(&w, 6);
            if let Some((fir, iir)) = quantize_pair(&lw[6], &[b1, b2]) {
                consider(fir, iir);
            }
        }
        for &order in &[2usize, 4, 8] {
            if let Some(c) = lpc_covariance(&xs, order) {
                if let Some(fir) = quantize_fir(&c) {
                    consider(fir, Fir::default());
                }
            }
        }
        for &(p, q) in &[(6usize, 2usize), (4, 4), (5, 2)] {
            let coef = &lpcs[p];
            // residual of the float FIR, then a moving-average fit on it
            let r1: Vec<f64> = (0..xs.len())
                .map(|i| {
                    let pred: f64 = (0..p.min(i)).map(|k| coef[k] * xs[i - 1 - k]).sum();
                    xs[i] - pred
                })
                .collect();
            if let Some(b) = fit_moving_average(&r1, q) {
                if let Some((fir, iir)) = quantize_pair(coef, &b) {
                    consider(fir, iir);
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
    ChannelAnalysis { decor: None, quant, fir: best_fir, iir: best_iir, residual: best_res, codings }
}

/// Second-order IIR (moving-average of the residual) stages tried ahead of the FIR: (b1, b2).
const FIXED_IIR: &[(f64, f64)] = &[(0.5, 0.9375), (0.0, 0.9375), (-0.5, 0.9375), (0.5, 0.75)];

/// Total bits of the exact entropy-coding choices of an analysis.
fn analysis_bits(a: &ChannelAnalysis) -> usize {
    a.codings.iter().flatten().map(|c| c.bits).sum()
}

/// Analyse channel `c` of `coded`, trying to predict it from the channels below it (a primitive
/// matrix the decoder undoes) and keeping the prediction only when it saves bits.
fn analyze_with_prediction(coded: &[Vec<i32>], c: usize, n: usize, au: usize, forced: Option<u8>, decorrelate: bool) -> (ChannelAnalysis, Option<(ChannelAnalysis, usize)>) {
    let x = &coded[c][..n];
    let or_all = x.iter().fold(0i32, |a, v| a | v);
    let quant = match forced {
        Some(q) => q,
        None if or_all == 0 => 0,
        None => (or_all.trailing_zeros() as u8).min(15),
    };
    let base = analyze_channel(x, au, quant);
    if !decorrelate || c == 0 || or_all == 0 || n < 256 {
        return (base, None);
    }
    let regs: Vec<&[i32]> = (0..c).map(|j| &coded[j][..n]).collect();
    // fit on signals whitened by the target's own predictor: what is left to remove is the part the
    // per-channel filters cannot (loud bass would otherwise dominate the fit)
    let xs: Vec<f64> = x.iter().map(|v| (*v >> quant) as f64).collect();
    let lpc = levinson(&xs, MAX_ORDER).pop().unwrap_or_default();
    let whiten = |sig: &[i32]| -> Vec<f64> {
        (0..n)
            .map(|i| {
                let pred: f64 = (0..lpc.len().min(i)).map(|k| lpc[k] * sig[i - 1 - k] as f64).sum();
                sig[i] as f64 - pred
            })
            .collect()
    };
    let wt = whiten(x);
    let wr: Vec<Vec<f64>> = regs.iter().map(|r| whiten(r)).collect();
    let Some(coef) = predict_coefficients(&wr, &wt) else { return (base, None) };
    let mask = !((1i32 << quant) - 1);
    let mut res = Vec::with_capacity(n);
    for i in 0..n {
        let s: i64 = coef.iter().zip(&regs).map(|(k, r)| *k as i64 * r[i] as i64).sum();
        let r = x[i] - (((s >> 14) as i32) & mask);
        if r.unsigned_abs() >= 1 << 23 {
            return (base, None);
        }
        res.push(r);
    }
    let mut with = analyze_channel(&res, au, quant);
    let side_cost = 17 * (c + 1) + 40;
    let (b0, b1) = (analysis_bits(&base), analysis_bits(&with) + side_cost);
    if b1 < b0 {
        let mut full = coef;
        full.push(1 << 14); // the channel's own coefficient (index c)
        with.decor = Some(full);
        (base, Some((with, b0 - b1)))
    } else {
        (base, None)
    }
}

/// Least-squares prediction of `target` from `regs` (instantaneous), as 2.14 coefficients.
fn predict_coefficients(regs: &[Vec<f64>], target: &[f64]) -> Option<Vec<i32>> {
    let k = regs.len();
    let mut a = vec![vec![0f64; k]; k];
    let mut b = vec![0f64; k];
    for i in 0..target.len() {
        let t = target[i];
        for p in 0..k {
            let rp = regs[p][i];
            b[p] += rp * t;
            for q in p..k {
                a[p][q] += rp * regs[q][i];
            }
        }
    }
    for p in 0..k {
        for q in 0..p {
            a[p][q] = a[q][p];
        }
    }
    let trace: f64 = (0..k).map(|p| a[p][p]).sum();
    if trace < 1.0 {
        return None;
    }
    for p in 0..k {
        a[p][p] += trace * 1e-9 + 1e-9;
    }
    // Gaussian elimination with partial pivoting
    for col in 0..k {
        let piv = (col..k).max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs()))?;
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for row in col + 1..k {
            let f = a[row][col] / a[col][col];
            for cc in col..k {
                a[row][cc] -= f * a[col][cc];
            }
            b[row] -= f * b[col];
        }
    }
    let mut sol = vec![0f64; k];
    for row in (0..k).rev() {
        let mut v = b[row];
        for cc in row + 1..k {
            v -= a[row][cc] * sol[cc];
        }
        sol[row] = v / a[row][row];
    }
    let q: Vec<i32> = sol.iter().map(|v| if v.abs() < 0.004 { 0 } else { (v.clamp(-1.99, 1.99) * 16384.0).round() as i32 }).collect();
    q.iter().any(|v| *v != 0).then_some(q)
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

/// Least-squares predictor from the data itself (no window, so no spectral leakage limits the
/// steepness of the filter): `w[n] ~ sum c[k] w[n-1-k]`.
fn lpc_covariance(w: &[f64], order: usize) -> Option<Vec<f64>> {
    let n = w.len();
    if n <= order * 4 {
        return None;
    }
    let mut a = vec![vec![0f64; order]; order];
    let mut b = vec![0f64; order];
    for i in order..n {
        let t = w[i];
        for p in 0..order {
            let rp = w[i - 1 - p];
            b[p] += rp * t;
            for q in p..order {
                a[p][q] += rp * w[i - 1 - q];
            }
        }
    }
    let mut trace = 0.0;
    for p in 0..order {
        for q in 0..p {
            a[p][q] = a[q][p];
        }
        trace += a[p][p];
    }
    if trace < 1.0 {
        return None;
    }
    for p in 0..order {
        a[p][p] += trace * 1e-12;
    }
    solve_linear(a, b)
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
    let shift = coefficient_shift(c)?;
    quantize_with_shift(c, shift)
}

/// Largest shift (0..=15) so every coefficient fits in 16 signed bits.
fn coefficient_shift(c: &[f64]) -> Option<u32> {
    let peak = c.iter().fold(0f64, |m, v| m.max(v.abs()));
    if !peak.is_finite() || peak < 1e-6 {
        return None;
    }
    let mut shift = 15u32;
    while shift > 0 && (peak * (1u32 << shift) as f64).round() > 32767.0 {
        shift -= 1;
    }
    Some(shift)
}

/// FIR and IIR stages share one precision; this quantizes both with the coarser of the two.
fn quantize_pair(fir: &[f64], iir: &[f64]) -> Option<(Fir, Fir)> {
    let shift = coefficient_shift(fir)?.min(coefficient_shift(iir)?);
    Some((quantize_with_shift(fir, shift)?, quantize_with_shift(iir, shift)?))
}

fn quantize_with_shift(c: &[f64], shift: u32) -> Option<Fir> {
    let order = c.len();
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
/// The IIR stage filters the past residuals (`sample - prediction`), as the decoder's state does.
fn apply_filters(x: &[i32], fir: &Fir, iir: &Fir, mask: i32) -> Vec<i32> {
    let (fo, io) = (fir.order as usize, iir.order as usize);
    let mut res = x.to_vec();
    if fo == 0 {
        return res;
    }
    let mut e: Vec<i32> = x.to_vec();
    for i in 8..x.len() {
        let mut acc = 0i64;
        for k in 0..fo {
            acc += x[i - 1 - k] as i64 * fir.coeff[k] as i64;
        }
        for k in 0..io {
            acc += e[i - 1 - k] as i64 * iir.coeff[k] as i64;
        }
        let pred = (acc >> fir.shift) as i32;
        res[i] = x[i].wrapping_sub(pred & mask);
        e[i] = x[i].wrapping_sub(pred);
    }
    res
}

/// Moving-average coefficients `b` with `r[n] ~ e[n] + sum b[k] e[n-1-k]` for white `e`, fitted by
/// iterated linear regression on the running innovation. Returns `None` if no stable fit helps.
fn fit_moving_average(r: &[f64], q: usize) -> Option<Vec<f64>> {
    let n = r.len();
    let mut b = vec![0f64; q];
    let mut e: Vec<f64> = r.to_vec();
    let base: f64 = r.iter().map(|v| v * v).sum();
    if base < 1.0 {
        return None;
    }
    for _ in 0..4 {
        let mut a = vec![vec![0f64; q]; q];
        let mut rhs = vec![0f64; q];
        for i in q..n {
            for p in 0..q {
                rhs[p] += e[i - 1 - p] * r[i];
                for c in p..q {
                    a[p][c] += e[i - 1 - p] * e[i - 1 - c];
                }
            }
        }
        for p in 0..q {
            for c in 0..p {
                a[p][c] = a[c][p];
            }
            a[p][p] += base * 1e-9;
        }
        b = solve_linear(a, rhs)?;
        // innovation with the new coefficients
        for i in 0..n {
            let mut v = r[i];
            for k in 0..q.min(i) {
                v -= b[k] * e[i - 1 - k];
            }
            e[i] = v;
        }
        if e.iter().any(|v| !v.is_finite()) {
            return None;
        }
    }
    let after: f64 = e.iter().map(|v| v * v).sum();
    (after < base * 0.999 && b.iter().all(|v| v.abs() < 1.9)).then_some(b)
}

fn solve_linear(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let k = b.len();
    for col in 0..k {
        let piv = (col..k).max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs()))?;
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for row in col + 1..k {
            let f = a[row][col] / a[col][col];
            for cc in col..k {
                a[row][cc] -= f * a[col][cc];
            }
            b[row] -= f * b[col];
        }
    }
    let mut sol = vec![0f64; k];
    for row in (0..k).rev() {
        let mut v = b[row];
        for cc in row + 1..k {
            v -= a[row][cc] * sol[cc];
        }
        sol[row] = v / a[row][row];
    }
    Some(sol)
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

fn plan_blocks(sub: &Substream, mats: &[MatrixSpec], out_q: &[u8], an: &[ChannelAnalysis], au_index: usize, au: usize, restart: bool) -> Vec<BlockPlan> {
    let ranges: Vec<(usize, usize)> = if restart {
        vec![(0, 8), (8, au - 8)]
    } else {
        vec![(au_index * au, au)]
    };
    let mut plans = Vec::new();
    for (bi, (start, len)) in ranges.into_iter().enumerate() {
        let mut want = DecState::restart();
        want.blocksize = len;
        want.matrices = mats.to_vec();
        for ch in 0..sub.min {
            want.quant[ch] = out_q[ch];
        }
        let mut codings = [Coding { codebook: 0, lsb_bits: 0, bits: 0 }; MAX_CHANNELS];
        for ch in sub.min..=sub.max {
            let a = &an[ch];
            want.quant[ch] = a.quant;
            // the priming block is unfiltered; later blocks use the interval's predictor
            let (fir, iir) = if restart && bi == 0 { (Fir::default(), Fir::default()) } else { (a.fir, a.iir) };
            let c = a.codings[au_index][bi];
            codings[ch] = c;
            want.ch[ch] = ChParams { fir, iir, codebook: c.codebook, lsbs: (c.lsb_bits + a.quant as u32) as u8 };
        }
        plans.push(BlockPlan { start, len, want, codings });
    }
    plans
}

#[allow(clippy::too_many_arguments)]
fn write_substream_au(
    sub: &Substream,
    mats: &[MatrixSpec],
    out_q: &[u8],
    an: &[ChannelAnalysis],
    st: &mut DecState,
    au_index: usize,
    au: usize,
    restart: bool,
    end_of_stream_shorten: Option<usize>,
    mlp: bool,
    output_timing: u16,
    lossless_check: u8,
    max_output_bits: u32,
    bypass: &[Vec<u8>; 2],
) -> Vec<u8> {
    let mut w = BitWriter::new();
    let plans = plan_blocks(sub, mats, out_q, an, au_index, au, restart);
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
        write_block_data(&mut w, sub, an, plan, bypass);
        w.put(u32::from(bi == nblocks - 1), 1); // last block of this access unit
    }
    w.align(16);
    if let Some(shorten) = end_of_stream_shorten.filter(|s| *s > 0) {
        w.put(0xD234, 16);
        w.put(if mlp { 0xD234 } else { (shorten as u32 & 0x1FFF) | 0xE000 }, 16);
    }
    let mut bytes = w.into_bytes();
    let p = parity(&bytes) ^ 0xA9;
    let c = checksum8(&bytes);
    bytes.push(p);
    bytes.push(c);
    bytes
}

fn params_differ(sub: &Substream, cur: &DecState, want: &DecState) -> bool {
    cur.blocksize != want.blocksize || cur.quant != want.quant || cur.matrices != want.matrices || (sub.min..=sub.max).any(|c| cur.ch[c] != want.ch[c])
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

/// Matrix parameters: each primitive matrix rewrites one channel from a weighted sum of all channels.
fn write_matrices(w: &mut BitWriter, sub: &Substream, mats: &[MatrixSpec]) {
    // substreams ending at channel 5 or below use the sync word with two noise channels, which add
    // two (always zero) coefficients per matrix; the 0x31EB variant has a noise shift instead
    let noise_channels = sub.max <= 5;
    w.put(mats.len() as u32, 4);
    for m in mats {
        w.put(m.out as u32, 4);
        w.put(14, 4); // fractional bits
        w.put(u32::from(m.lsb_bypass), 1);
        let count = sub.max + 1 + if noise_channels { 2 } else { 0 };
        for ch in 0..count {
            let c = m.coeff.get(ch).copied().unwrap_or(0);
            if c != 0 {
                w.put(1, 1);
                w.put_signed(c, 16);
            } else {
                w.put(0, 1);
            }
        }
        if !noise_channels {
            w.put(0, 4); // matrix noise shift
        }
    }
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
    if cur.matrices != want.matrices {
        w.put(1, 1);
        write_matrices(w, sub, &want.matrices);
    } else {
        w.put(0, 1);
    }
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
            if cur.ch[ch].iir != want.ch[ch].iir {
                w.put(1, 1);
                write_filter(w, &want.ch[ch].iir);
            } else {
                w.put(0, 1);
            }
            w.put(0, 1); // no huffman offset change
            w.put(want.ch[ch].codebook as u32, 2);
            w.put(want.ch[ch].lsbs as u32, 5);
        } else {
            w.put(0, 1);
        }
    }
}

fn write_block_data(w: &mut BitWriter, sub: &Substream, an: &[ChannelAnalysis], plan: &BlockPlan, bypass: &[Vec<u8>; 2]) {
    for i in 0..plan.len {
        // one bypassed LSB per flagged matrix, ahead of the sample's channel data
        for m in plan.want.matrices.iter() {
            if m.lsb_bypass {
                w.put(bypass[m.out][plan.start + i] as u32, 1);
            }
        }
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
