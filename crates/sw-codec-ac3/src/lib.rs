//! Native AC-3 (Dolby Digital) encoder, written from the public ATSC A/52 specification.
//!
//! Scope of this first version: long blocks only (no block switching), no coupling,
//! no rematrixing, static bit-allocation parameters, exponent sets shared across blocks.
//! It produces standard-conforming frames that any AC-3 decoder accepts.

pub mod bits;
pub mod blocksw;
pub mod mdct;
pub mod native;
pub mod tables;

use bits::{crc16, crc1_field, BitWriter};
use mdct::{Mdct, M};
use sw_core::{Channel, ChannelLayout};
use tables::*;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Unsupported(String),
}

pub const SAMPLES_PER_FRAME: usize = 1536;
const BLOCKS: usize = 6;
const BANDS: usize = 50;

const BITRATES: [u32; 19] = [32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 576, 640];
/// Mantissa bits per bap for the ungrouped quantizers (bap 1, 2 and 4 are grouped).
const BAP_BITS: [u32; 16] = [0, 0, 0, 3, 0, 4, 5, 6, 7, 8, 9, 10, 11, 12, 14, 16];
const GROUPED_LEVELS: [i32; 6] = [0, 3, 5, 7, 11, 15];

// Fixed bit-allocation parameter codes (the usual encoder defaults).
const SDCYCOD: usize = 2;
const FDCYCOD: usize = 1;
const SGAINCOD: usize = 1;
const DBPBCOD: usize = 2;
const FLOORCOD: usize = 7;
const FGAINCOD: usize = 4;

/// Scale applied to MDCT output (negative: the decoder's synthesis has the opposite sign
/// convention to the textbook MDCT used here). A decoder reconstructs amplitude = 2 x coefficient, and the
/// unscaled MDCT peak of a full-scale sine is about 146, so 1/256 gives unity gain with a
/// full-scale tone at a coefficient of about 0.57 (inside the format's +-1 mantissa range).
const MDCT_SCALE: f64 = -1.0 / 256.0;

#[derive(Debug, Clone)]
pub struct Config {
    pub acmod: u8,
    pub lfe: bool,
    pub sample_rate: u32,
    pub bitrate_kbps: u32,
    /// 1..=31 dB below full scale (31 = no attenuation).
    pub dialnorm: u8,
    /// Mark a 2.0 stream as Dolby Surround (Lt/Rt) encoded.
    pub dolby_surround: bool,
    /// Override the automatic low-pass (Hz).
    pub bandwidth_hz: Option<u32>,
    /// Write Enhanced AC-3 (Dolby Digital Plus) frames instead of AC-3.
    pub eac3: bool,
}

/// How a layout maps onto AC-3 channels.
#[derive(Debug, Clone)]
pub struct LayoutMap {
    pub acmod: u8,
    pub lfe: bool,
    pub dolby_surround: bool,
    /// For each AC-3 channel (full-bandwidth channels in bitstream order, LFE last):
    /// the index into `layout.channels`.
    pub source_index: Vec<usize>,
}

pub fn map_layout(layout: &ChannelLayout) -> Result<LayoutMap, Error> {
    use Channel::*;
    let find = |c: &[Channel]| layout.channels.iter().position(|x| c.contains(x));
    let (l, r, c, cs, ls, rs, lfe) = (find(&[L, Lt]), find(&[R, Rt]), find(&[C]), find(&[Cs]), find(&[Ls]), find(&[Rs]), find(&[Lfe]));
    let has = |o: Option<usize>| o.is_some();
    let unsupported = || Error::Unsupported(format!("layout '{}' cannot be carried by AC-3", layout.id));
    let (acmod, order): (u8, Vec<Option<usize>>) = match (has(l) && has(r), has(c), has(cs), has(ls) && has(rs)) {
        (false, true, false, false) => (1, vec![c]),
        (true, false, false, false) => (2, vec![l, r]),
        (true, true, false, false) => (3, vec![l, c, r]),
        (true, false, true, false) => (4, vec![l, r, cs]),
        (true, true, true, false) => (5, vec![l, c, r, cs]),
        (true, false, false, true) => (6, vec![l, r, ls, rs]),
        (true, true, false, true) => (7, vec![l, c, r, ls, rs]),
        _ => return Err(unsupported()),
    };
    let mut source_index: Vec<usize> = order.into_iter().map(|o| o.ok_or_else(unsupported)).collect::<Result<_, _>>()?;
    if layout.channel_count() != source_index.len() + usize::from(lfe.is_some()) {
        return Err(unsupported());
    }
    if let Some(i) = lfe {
        source_index.push(i);
    }
    Ok(LayoutMap { acmod, lfe: lfe.is_some(), dolby_surround: layout.id == "2.0-ltrt", source_index })
}

fn fbw_channels(acmod: u8) -> usize {
    [2, 1, 2, 3, 3, 4, 4, 5][acmod as usize]
}

/// log2-scale exponent of a coefficient: floor(-log2|c|) clamped to 0..=24 (24 for zero).
#[inline]
fn exponent_of(c: f32) -> i32 {
    let biased = ((c.to_bits() >> 23) & 0xff) as i32;
    (126 - biased).clamp(0, 24)
}

/// Bin -> critical band map derived from the band start table.
fn bin_to_band() -> [u8; 256] {
    let mut m = [0u8; 256];
    for b in 0..BANDS {
        for bin in BAND_START[b] as usize..BAND_START[b + 1] as usize {
            m[bin] = b as u8;
        }
    }
    m
}

fn calc_psd(exp: &[i32], end: usize, b2b: &[u8; 256], psd: &mut [i32], band_psd: &mut [i32]) {
    for bin in 0..end {
        psd[bin] = 3072 - (exp[bin] << 7);
    }
    let mut bin = 0;
    while bin < end.min(28) {
        band_psd[bin] = psd[bin];
        bin += 1;
    }
    if bin >= end {
        return;
    }
    let mut band = b2b[bin] as usize;
    loop {
        let mut v = psd[bin];
        bin += 1;
        let band_end = (BAND_START[band + 1] as usize).min(end);
        while bin < band_end {
            let max = v.max(psd[bin]);
            let adr = (max - ((v + psd[bin] + 1) >> 1)).min(255) as usize;
            v = max + LOG_ADD[adr] as i32;
            bin += 1;
        }
        band_psd[band] = v;
        band += 1;
        if end <= BAND_START[band] as usize {
            break;
        }
    }
}

/// Power spectral density and banded PSD from `start` (the coupling channel starts above bin 0).
fn calc_psd_from(exp: &[i32], start: usize, end: usize, b2b: &[u8; 256], psd: &mut [i32], band_psd: &mut [i32]) {
    for bin in start..end {
        psd[bin] = 3072 - (exp[bin] << 7);
    }
    let mut bin = start;
    let mut band = b2b[bin] as usize;
    loop {
        let mut v = psd[bin];
        bin += 1;
        let band_end = (BAND_START[band + 1] as usize).min(end);
        while bin < band_end {
            let max = v.max(psd[bin]);
            let adr = (max - ((v + psd[bin] + 1) >> 1)).min(255) as usize;
            v = max + LOG_ADD[adr] as i32;
            bin += 1;
        }
        band_psd[band] = v;
        band += 1;
        if end <= BAND_START[band] as usize {
            break;
        }
    }
}

/// Masking curve of the coupling channel (A/52 7.2.2.4 with the leak start values of cplfleak = cplsleak = 0).
fn calc_mask_cpl(band_psd: &[i32], start: usize, end: usize, fast_gain: i32, fscod: usize, b2b: &[u8; 256], mask: &mut [i32]) {
    let (slow_decay, fast_decay) = (SLOW_DECAY[SDCYCOD], FAST_DECAY[FDCYCOD]);
    let (slow_gain, db_per_bit) = (SLOW_GAIN[SGAINCOD], DB_PER_BIT[DBPBCOD]);
    let (band_start, band_end) = (b2b[start] as usize, b2b[end - 1] as usize + 1);
    let (mut fastleak, mut slowleak) = (768, 768);
    for band in band_start..band_end {
        fastleak = (fastleak - fast_decay).max(band_psd[band] - fast_gain);
        slowleak = (slowleak - slow_decay).max(band_psd[band] - slow_gain);
        let mut excite = fastleak.max(slowleak);
        let tmp = db_per_bit - band_psd[band];
        if tmp > 0 {
            excite += tmp >> 2;
        }
        mask[band] = (HEARING_THRESHOLD[band * 3 + fscod] as i32).max(excite);
    }
}

fn calc_bap_from(mask: &[i32], psd: &[i32], start: usize, end: usize, snr_offset: i32, b2b: &[u8; 256], bap: &mut [u8]) {
    let floor = FLOOR[FLOORCOD];
    if snr_offset == -960 {
        bap[..end].fill(0);
        return;
    }
    let addr = |p: i32, m: i32| (((p - m) >> 5).clamp(0, 63)) as usize;
    let mut bin = start;
    let mut band = b2b[bin] as usize;
    loop {
        let m = ((mask[band] - snr_offset - floor).max(0) & 0x1FE0) + floor;
        band += 1;
        let band_end = (BAND_START[band] as usize).min(end);
        while bin < band_end {
            bap[bin] = BAP_FROM_ADDRESS[addr(psd[bin], m)];
            bin += 1;
        }
        if end <= band_end {
            break;
        }
    }
}

/// The coupling channel's reference exponent is sent halved, so it has to be even: make it so and bring the
/// groups after it back inside the +-2 step limit by lowering exponents (never raising them).
fn pin_reference(virt: &mut [i32]) {
    virt[0] &= !1;
    loop {
        let mut changed = false;
        for i in 1..virt.len() {
            if virt[i] > virt[i - 1] + 2 {
                virt[i] = virt[i - 1] + 2;
                changed = true;
            }
        }
        for i in (1..virt.len() - 1).rev() {
            if virt[i] > virt[i + 1] + 2 {
                virt[i] = virt[i + 1] + 2;
                changed = true;
            }
        }
        if virt.len() > 1 && virt[0] > virt[1] + 2 {
            virt[0] = ((virt[1] + 2) & !1).max(0);
            changed = true;
        }
        if !changed {
            break;
        }
    }
}

/// Exponents of the coupling channel: a 4-bit reference (half the even starting exponent), then the groups.
/// `virt[0]` is the reference and `virt[1 + i]` the exponent of bin `start + i`.
fn write_cpl_exponents(w: &mut BitWriter, virt: &[i32], strat: Strat) {
    let g = strat.gsize();
    let nbins = virt.len() - 1;
    w.put((virt[0] >> 1) as u32, 4);
    let mut prev = virt[0];
    for grp in 0..nbins / (3 * g) {
        let mut code = 0;
        for j in 0..3 {
            let e = virt[1 + (grp * 3 + j) * g];
            let m = (e - prev + 2) as u32;
            debug_assert!(m <= 4, "coupling exponent step out of range: {prev} -> {e}");
            code = code * 5 + m;
            prev = e;
        }
        w.put(code, 7);
    }
}

/// Coupling coordinate (master 2 bits, then exponent and mantissa 4 bits each per band) for the gains `g`
/// the decoder multiplies the coupling channel with (8 x mantissa x 2^-(exp + 3 master)).
fn quantize_coords(g: &[f64]) -> (u32, Vec<(u32, u32)>) {
    let e_of = |g: f64| -> i32 {
        let v = (g / 8.0).max(1e-9);
        ((-v.log2()).floor() as i32).clamp(0, 24)
    };
    let es: Vec<i32> = g.iter().map(|g| e_of(*g)).collect();
    let emax = es.iter().copied().max().unwrap_or(0);
    let mstr = (((emax - 14).max(0) + 2) / 3).min(3);
    let out = g
        .iter()
        .zip(&es)
        .map(|(g, e)| {
            let v = (g / 8.0).max(1e-9);
            let exp = (*e - 3 * mstr).clamp(0, 14);
            let total = exp + 3 * mstr;
            let mant = ((v * 2f64.powi(total) * 32.0).round() as i32 - 16).clamp(0, 15);
            (exp as u32, mant as u32)
        })
        .collect();
    (mstr as u32, out)
}

/// Value of a quantized coupling coordinate (the gain applied to the coupling channel).
fn coord_gain(mstr: u32, exp: u32, mant: u32) -> f64 {
    8.0 * (mant + 16) as f64 / 32.0 * 2f64.powi(-((exp + 3 * mstr) as i32))
}

/// Coupling in use for a frame.
struct CplInfo {
    begf: usize,
    endf: usize,
    strt: usize,
    end: usize,
    /// `[set][channel] = (master, [(exp, mant)] per band)`
    coords: Vec<Vec<(u32, Vec<(u32, u32)>)>>,
}

fn lowcomp1(a: i32, b0: i32, b1: i32, c: i32) -> i32 {
    if b0 + 256 == b1 {
        c
    } else if b0 > b1 {
        (a - 64).max(0)
    } else {
        a
    }
}

fn lowcomp(a: i32, b0: i32, b1: i32, bin: usize) -> i32 {
    if bin < 7 {
        lowcomp1(a, b0, b1, 384)
    } else if bin < 20 {
        lowcomp1(a, b0, b1, 320)
    } else {
        (a - 128).max(0)
    }
}

/// Masking curve per band (A/52 7.2.2.4 - 7.2.2.6), start bin 0, no coupling, no delta allocation.
fn calc_mask(band_psd: &[i32], end: usize, fast_gain: i32, is_lfe: bool, fscod: usize, b2b: &[u8; 256], mask: &mut [i32]) {
    let (slow_decay, fast_decay) = (SLOW_DECAY[SDCYCOD], FAST_DECAY[FDCYCOD]);
    let (slow_gain, db_per_bit) = (SLOW_GAIN[SGAINCOD], DB_PER_BIT[DBPBCOD]);
    let mut excite = [0i32; BANDS];
    let band_end = b2b[end - 1] as usize + 1;
    let mut lc = 0;
    lc = lowcomp1(lc, band_psd[0], band_psd[1], 384);
    excite[0] = band_psd[0] - fast_gain - lc;
    lc = lowcomp1(lc, band_psd[1], band_psd[2], 384);
    excite[1] = band_psd[1] - fast_gain - lc;
    let mut begin = 7;
    let (mut fastleak, mut slowleak) = (0, 0);
    for band in 2..7 {
        if !(is_lfe && band == 6) {
            lc = lowcomp1(lc, band_psd[band], band_psd[band + 1], 384);
        }
        fastleak = band_psd[band] - fast_gain;
        slowleak = band_psd[band] - slow_gain;
        excite[band] = fastleak - lc;
        if !(is_lfe && band == 6) && band_psd[band] <= band_psd[band + 1] {
            begin = band + 1;
            break;
        }
    }
    let end1 = band_end.min(22);
    for band in begin..end1 {
        if !(is_lfe && band == 6) {
            lc = lowcomp(lc, band_psd[band], band_psd[band + 1], band);
        }
        fastleak = (fastleak - fast_decay).max(band_psd[band] - fast_gain);
        slowleak = (slowleak - slow_decay).max(band_psd[band] - slow_gain);
        excite[band] = (fastleak - lc).max(slowleak);
    }
    for band in 22.max(begin.min(22))..band_end {
        fastleak = (fastleak - fast_decay).max(band_psd[band] - fast_gain);
        slowleak = (slowleak - slow_decay).max(band_psd[band] - slow_gain);
        excite[band] = fastleak.max(slowleak);
    }
    for band in 0..band_end {
        let tmp = db_per_bit - band_psd[band];
        if tmp > 0 {
            excite[band] += tmp >> 2;
        }
        mask[band] = (HEARING_THRESHOLD[band * 3 + fscod] as i32).max(excite[band]);
    }
}

fn calc_bap(mask: &[i32], psd: &[i32], end: usize, snr_offset: i32, b2b: &[u8; 256], bap: &mut [u8]) {
    let floor = FLOOR[FLOORCOD];
    if snr_offset == -960 {
        bap[..end].fill(0);
        return;
    }
    let addr = |p: i32, m: i32| (((p - m) >> 5).clamp(0, 63)) as usize;
    let mut bin = 0;
    while bin < end.min(28) {
        let m = ((mask[bin] - snr_offset - floor).max(0) & 0x1FE0) + floor;
        bap[bin] = BAP_FROM_ADDRESS[addr(psd[bin], m)];
        bin += 1;
    }
    if bin >= end {
        return;
    }
    let mut band = b2b[bin] as usize;
    loop {
        let m = ((mask[band] - snr_offset - floor).max(0) & 0x1FE0) + floor;
        band += 1;
        let band_end = (BAND_START[band] as usize).min(end);
        while bin < band_end {
            bap[bin] = BAP_FROM_ADDRESS[addr(psd[bin], m)];
            bin += 1;
        }
        if end <= band_end {
            break;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Strat {
    D15,
    D25,
    D45,
}

impl Strat {
    fn code(self) -> u32 {
        match self {
            Strat::D15 => 1,
            Strat::D25 => 2,
            Strat::D45 => 3,
        }
    }
    fn gsize(self) -> usize {
        match self {
            Strat::D15 => 1,
            Strat::D25 => 2,
            Strat::D45 => 4,
        }
    }
    /// Number of 3-exponent groups for `end` coefficients.
    fn ngroups(self, end: usize) -> usize {
        match self {
            Strat::D15 => (end - 1) / 3,
            Strat::D25 => (end - 1 + 3) / 6,
            Strat::D45 => (end - 1 + 9) / 12,
        }
    }
}

/// Everything derived from the coefficients of one channel for one set of blocks.
struct SetPlan {
    strat: Strat,
    /// The coupling channel's exponents as sent (reference first, then one per bin from its start).
    virt: Vec<i32>,
    exp: Vec<i32>,
    psd: Vec<i32>,
    mask: [i32; BANDS],
}

struct ChannelPlan {
    start: usize,
    is_cpl: bool,
    end: usize,
    strat: Strat,
    is_lfe: bool,
    coef: Vec<[f32; M]>,
    sets: Vec<SetPlan>,
}

pub struct Encoder {
    cfg: Config,
    nfch: usize,
    nch: usize,
    fscod: usize,
    size_idx: usize,
    mdct: Mdct,
    hist: Vec<[f32; M]>,
    end: Vec<usize>,
    frame_no: u64,
    frac: f64,
    b2b: [u8; 256],
    sets: Vec<(usize, usize)>, // (first block, block count)
    strat: Strat,
    switched: blocksw::Switched,
    dets: Vec<blocksw::Detector>,
    block_switching: bool,
    /// Exponent sets and block-switch flags of the frame being encoded.
    cur_sets: Vec<(usize, usize)>,
    cur_blksw: Vec<[bool; BLOCKS]>,
    /// Rematrixing flags per block and band (2/0 mode, AC-3 only).
    cur_remat: Vec<[bool; 4]>,
    /// Channel coupling for multichannel AC-3 at low rates.
    coupling: bool,
    cur_cpl: Option<CplInfo>,
    rematrixing: bool,
    /// E-AC-3 dependent substream: the custom channel map (bit 15 = Left ... bit 0 = LFE) it carries.
    dependent: Option<u16>,
}

impl Encoder {
    pub fn new(cfg: Config) -> Result<Encoder, Error> {
        let fscod = match cfg.sample_rate {
            48_000 => 0,
            44_100 => 1,
            32_000 => 2,
            r => return Err(Error::Unsupported(format!("AC-3 supports 48, 44.1 and 32 kHz, not {r} Hz"))),
        };
        let size_idx = if cfg.eac3 {
            // E-AC-3 frames are sized in whole 16-bit words, so any bit rate is possible
            // within a single independent substream.
            if !(8..=1024).contains(&cfg.bitrate_kbps) {
                return Err(Error::Unsupported(format!("{} kbps is outside the single-substream E-AC-3 range (8 to 1024)", cfg.bitrate_kbps)));
            }
            0
        } else {
            BITRATES
                .iter()
                .position(|b| *b == cfg.bitrate_kbps)
                .ok_or_else(|| Error::Unsupported(format!("{} kbps is not a standard AC-3 bit rate ({:?})", cfg.bitrate_kbps, BITRATES)))?
        };
        if !(1..=7).contains(&cfg.acmod) {
            return Err(Error::Unsupported("unsupported audio coding mode".into()));
        }
        let nfch = fbw_channels(cfg.acmod);
        let nch = nfch + usize::from(cfg.lfe);
        let kbps_per_ch = cfg.bitrate_kbps as f64 / (nfch as f64 + 0.1);
        let end_bins = Self::bandwidth_bins(&cfg, kbps_per_ch);
        let mut end = vec![end_bins; nfch];
        if cfg.lfe {
            end.push(7);
        }
        let (sets, strat) = if kbps_per_ch >= 64.0 {
            (vec![(0, 3), (3, 3)], Strat::D25)
        } else if kbps_per_ch >= 40.0 {
            (vec![(0, 3), (3, 3)], Strat::D45)
        } else {
            (vec![(0, 6)], Strat::D45)
        };
        Ok(Encoder {
            nfch,
            nch,
            fscod,
            size_idx,
            mdct: Mdct::new(MDCT_SCALE),
            hist: vec![[0.0; M]; nch],
            end,
            frame_no: 0,
            frac: 0.0,
            b2b: bin_to_band(),
            sets: sets.clone(),
            strat,
            switched: blocksw::Switched::new(MDCT_SCALE),
            dets: (0..nfch).map(|_| blocksw::Detector::new(cfg.sample_rate)).collect(),
            block_switching: !cfg.eac3 && std::env::var_os("SW_AC3_NOBLKSW").is_none(),
            cur_sets: sets,
            cur_blksw: vec![[false; BLOCKS]; nfch],
            cur_remat: vec![[false; 4]; BLOCKS],
            rematrixing: std::env::var_os("SW_AC3_NOREMAT").is_none(),
            coupling: !cfg.eac3 && cfg.acmod >= 3 && std::env::var_os("SW_AC3_NOCPL").is_none() && kbps_per_ch < 96.0,
            cur_cpl: None,
            dependent: None,
            cfg,
        })
    }

    fn bandwidth_bins(cfg: &Config, kbps_per_ch: f64) -> usize {
        let hz = cfg.bandwidth_hz.map(|h| h as f64).unwrap_or_else(|| {
            let pts = [(24.0, 7000.0), (40.0, 11000.0), (56.0, 15000.0), (72.0, 18000.0), (88.0, 20500.0), (104.0, 1e9)];
            if kbps_per_ch <= pts[0].0 {
                pts[0].1
            } else {
                let mut out = 1e9;
                for w in pts.windows(2) {
                    if kbps_per_ch <= w[1].0 {
                        let t = (kbps_per_ch - w[0].0) / (w[1].0 - w[0].0);
                        out = w[0].1 + t * (w[1].1.min(24_000.0) - w[0].1);
                        break;
                    }
                }
                out
            }
        });
        let bin_hz = cfg.sample_rate as f64 / 2.0 / M as f64;
        let bins = (hz / bin_hz).floor() as usize;
        let code = (bins.clamp(73, 253) - 73) / 3;
        code * 3 + 73
    }

    /// Turn the transient detector (and with it the short transforms) on or off; on by default for AC-3.
    pub fn set_block_switching(&mut self, on: bool) {
        self.block_switching = on && !self.cfg.eac3;
    }

    /// 2/0 mode: allow sum/difference coding per band (on by default).
    /// Channel coupling above about 7 to 10 kHz for 3/0 and richer modes at low rates (on by default there).
    pub fn set_coupling(&mut self, on: bool) {
        self.coupling = on && !self.cfg.eac3 && self.cfg.acmod >= 3;
    }

    pub fn set_rematrixing(&mut self, on: bool) {
        self.rematrixing = on;
    }

    /// Make this encoder write E-AC-3 dependent substream frames (strmtyp 1) with the given custom channel map.
    pub fn set_dependent(&mut self, chanmap: u16) {
        self.dependent = Some(chanmap);
    }

    pub fn channels(&self) -> usize {
        self.nch
    }
    pub fn acmod(&self) -> u8 {
        self.cfg.acmod
    }

    fn frame_words(&mut self) -> (usize, u32) {
        if self.cfg.eac3 {
            // any word count works, so track the fraction to hit the average bit rate exactly
            let exact = self.cfg.bitrate_kbps as f64 * 1000.0 * SAMPLES_PER_FRAME as f64 / self.cfg.sample_rate as f64 / 16.0 + self.frac;
            let words = exact.floor();
            self.frac = exact - words;
            return ((words as usize).clamp(8, 2048), 0);
        }
        let base = FRAME_SIZE_WORDS[self.size_idx * 2 * 3 + self.fscod] as usize;
        if self.fscod == 1 {
            // 44.1 kHz frame sizes alternate so the average bit rate is exact
            let exact = self.cfg.bitrate_kbps as f64 * 1000.0 * SAMPLES_PER_FRAME as f64 / 44_100.0 / 16.0;
            self.frac += exact - base as f64;
            if self.frac >= 1.0 {
                self.frac -= 1.0;
                return (base + 1, (self.size_idx * 2 + 1) as u32);
            }
        }
        (base, (self.size_idx * 2) as u32)
    }

    /// Encode one frame. `input` holds one slice of exactly 1536 samples per AC-3 channel
    /// (full-bandwidth channels in bitstream order, LFE last).
    pub fn encode_frame(&mut self, input: &[&[f32]]) -> Vec<u8> {
        assert_eq!(input.len(), self.nch);
        let (words, frmsizecod) = self.frame_words();

        // 1. transform; a channel whose new samples hold a transient uses the two short transforms
        let mut coefs: Vec<Vec<[f32; M]>> = Vec::with_capacity(self.nch);
        let mut blksw = vec![[false; BLOCKS]; self.nfch];
        for ch in 0..self.nch {
            assert_eq!(input[ch].len(), SAMPLES_PER_FRAME);
            let mut coef = vec![[0f32; M]; BLOCKS];
            let mut buf = [0f32; 2 * M];
            for blk in 0..BLOCKS {
                let new = &input[ch][blk * M..(blk + 1) * M];
                let short = self.block_switching && ch < self.nfch && self.dets[ch].block(new);
                if ch < self.nfch {
                    blksw[ch][blk] = short;
                }
                buf[..M].copy_from_slice(&self.hist[ch]);
                buf[M..].copy_from_slice(new);
                if short {
                    self.switched.forward(&buf, true, &mut coef[blk]);
                } else {
                    self.mdct.forward(&buf, &mut coef[blk]);
                }
                self.hist[ch].copy_from_slice(&buf[M..]);
            }
            coefs.push(coef);
        }
        self.cur_sets = self.frame_sets(&blksw);
        self.cur_blksw = blksw;
        self.encode_coefficients(coefs, frmsizecod, words)
    }

    /// Exponent sets for a frame: the configured grouping, with every block that is switched in any channel
    /// as a set of its own (the exponents of a short block do not carry over to its neighbours).
    fn frame_sets(&self, blksw: &[[bool; BLOCKS]]) -> Vec<(usize, usize)> {
        let any: Vec<bool> = (0..BLOCKS).map(|b| blksw.iter().any(|c| c[b])).collect();
        let mut out = Vec::new();
        for &(first, count) in &self.sets {
            let mut start = first;
            for blk in first..first + count {
                if any[blk] {
                    if start < blk {
                        out.push((start, blk - start));
                    }
                    out.push((blk, 1));
                    start = blk + 1;
                }
            }
            if start < first + count {
                out.push((start, first + count - start));
            }
        }
        out
    }

    /// 2/0 mode: per block and band, send sum and difference instead of left and right where that leaves
    /// less energy to code (clause 7.5.3).
    fn rematrix(&mut self, coefs: &mut [Vec<[f32; M]>]) {
        self.cur_remat = vec![[false; 4]; BLOCKS];
        if self.cfg.acmod != 2 || self.cfg.eac3 || !self.rematrixing {
            return;
        }
        const EDGES: [(usize, usize); 4] = [(13, 25), (25, 37), (37, 61), (61, 253)];
        let end = self.end[0].min(self.end[1]);
        let (a, b) = coefs.split_at_mut(1);
        let (left, right) = (&mut a[0], &mut b[0]);
        for blk in 0..BLOCKS {
            if self.cur_blksw[0][blk] != self.cur_blksw[1][blk] {
                continue;
            }
            for (band, &(lo, hi)) in EDGES.iter().enumerate() {
                let hi = hi.min(end);
                if lo >= hi {
                    continue;
                }
                let (mut sl, mut sr, mut sp, mut sm) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
                for k in lo..hi {
                    let (l, r) = (left[blk][k] as f64, right[blk][k] as f64);
                    sl += l * l;
                    sr += r * r;
                    sp += (l + r) * (l + r);
                    sm += (l - r) * (l - r);
                }
                if sp.min(sm) < sl.min(sr) {
                    self.cur_remat[blk][band] = true;
                    for k in lo..hi {
                        let (l, r) = (left[blk][k], right[blk][k]);
                        left[blk][k] = 0.5 * (l + r);
                        right[blk][k] = 0.5 * (l - r);
                    }
                }
            }
        }
    }

    /// Decide on coupling for this frame (not with block switching in it) and describe it.
    fn plan_coupling(&self, coefs: &mut [Vec<[f32; M]>]) -> Option<CplInfo> {
        let _ = coefs;
        if !self.coupling || self.cur_blksw.iter().any(|c| c.iter().any(|b| *b)) {
            return None;
        }
        let per_ch = self.cfg.bitrate_kbps as f64 / (self.nfch as f64 + 0.1);
        let begf = if per_ch < 40.0 { 2 } else if per_ch < 56.0 { 4 } else if per_ch < 72.0 { 6 } else { 8 };
        let end_bin = self.end[0];
        let endf = (((end_bin.saturating_sub(37)) / 12).saturating_sub(3)).min(15);
        if std::env::var_os("SW_AC3_DEBUG").is_some() {
            eprintln!("coupling: end_bin {end_bin} begf {begf} endf {endf}");
        }
        if endf < begf + 1 {
            return None;
        }
        Some(CplInfo { begf, endf, strt: 37 + 12 * begf, end: 37 + 12 * (endf + 3), coords: Vec::new() })
    }

    /// Coupling coordinates per set of blocks, from the energy of each channel against the coupling channel.
    fn fill_coords(&self, c: &mut CplInfo, chans: &[ChannelPlan], orig: &[Vec<[f32; M]>]) {
        let nbands = 3 + c.endf - c.begf;
        let cplch = chans.last().unwrap();
        for &(first, count) in &self.cur_sets {
            let mut per_ch = Vec::new();
            for ch in 0..self.nfch {
                let mut gains = Vec::with_capacity(nbands);
                for band in 0..nbands {
                    let (lo, hi) = (c.strt + 12 * band, c.strt + 12 * band + 12);
                    let (mut ec, mut ep) = (0f64, 0f64);
                    for blk in first..first + count {
                        for bin in lo..hi {
                            ec += (orig[ch][blk][bin] as f64).powi(2);
                            ep += (cplch.coef[blk][bin] as f64).powi(2);
                        }
                    }
                    gains.push(if ep > 1e-18 { (ec / ep).sqrt().min(7.9) } else { 0.0 });
                }
                per_ch.push(quantize_coords(&gains));
            }
            c.coords.push(per_ch);
        }
    }

    /// Encode a frame from already-transformed coefficients (`[channel][block][bin]`).
    fn encode_coefficients(&mut self, mut coefs: Vec<Vec<[f32; M]>>, frmsizecod: u32, words: usize) -> Vec<u8> {
        self.rematrix(&mut coefs);
        let frame_bits = words * 16;
        let mut chans: Vec<ChannelPlan> = Vec::with_capacity(self.nch + 1);
        let orig: Vec<Vec<[f32; M]>> = coefs.iter().take(self.nfch).cloned().collect();
        let mut cpl = self.plan_coupling(&mut coefs);
        for (ch, coef) in coefs.into_iter().enumerate() {
            let is_lfe = self.cfg.lfe && ch == self.nfch;
            let end = match &cpl {
                Some(c) if ch < self.nfch => c.strt,
                _ => self.end[ch],
            };
            chans.push(ChannelPlan { start: 0, is_cpl: false, end, strat: if is_lfe { Strat::D15 } else { self.strat }, is_lfe, coef, sets: Vec::new() });
        }
        if let Some(c) = &cpl {
            // the coupling channel: the mean of the coupled channels above the coupling start
            let mut coef = vec![[0f32; M]; BLOCKS];
            for blk in 0..BLOCKS {
                for bin in c.strt..c.end {
                    coef[blk][bin] = (0..self.nfch).map(|ch| chans[ch].coef[blk][bin]).sum::<f32>() / self.nfch as f32;
                }
            }
            // what the channels carry above the start is now the coupling channel's business
            for ch in 0..self.nfch {
                for blk in 0..BLOCKS {
                    for bin in c.strt..M {
                        chans[ch].coef[blk][bin] = 0.0;
                    }
                }
            }
            chans.push(ChannelPlan { start: c.strt, is_cpl: true, end: c.end, strat: self.strat, is_lfe: false, coef, sets: Vec::new() });
        }

        // 2. exponents, psd and masking per (channel, set of blocks)
        for (ch, cp) in chans.iter_mut().enumerate() {
            for &(first, count) in &self.cur_sets {
                let lo = cp.start;
                let mut min_e = vec![24i32; cp.end];
                for blk in first..first + count {
                    for bin in lo..cp.end {
                        min_e[bin] = min_e[bin].min(exponent_of(cp.coef[blk][bin]));
                    }
                }
                if cp.is_cpl {
                    let strat = cp.strat;
                    let g = strat.gsize();
                    let nbins = cp.end - lo;
                    // reference: an even exponent at or below the first group's
                    let first_group = min_e[lo..lo + g].iter().copied().min().unwrap();
                    let mut raw = vec![first_group & !1];
                    raw.extend_from_slice(&min_e[lo..cp.end]);
                    let mut virt = condition_exponents(&raw, strat);
                    {
                        // pin the reference on the group sequence, then spread the groups over their bins again
                        let mut seq: Vec<i32> = std::iter::once(virt[0]).chain((0..nbins / g).map(|i| virt[1 + i * g])).collect();
                        pin_reference(&mut seq);
                        virt[0] = seq[0];
                        for bin in 0..nbins {
                            virt[1 + bin] = seq[1 + bin / g];
                        }
                    }
                    let mut exp = vec![0i32; cp.end];
                    for bin in lo..cp.end {
                        exp[bin] = virt[1 + bin - lo];
                    }
                    let mut psd = vec![0i32; cp.end];
                    let mut band_psd = [0i32; BANDS];
                    calc_psd_from(&exp, lo, cp.end, &self.b2b, &mut psd, &mut band_psd);
                    let mut mask = [0i32; BANDS];
                    calc_mask_cpl(&band_psd, lo, cp.end, FAST_GAIN[FGAINCOD], self.fscod, &self.b2b, &mut mask);
                    debug_assert_eq!(virt.len(), nbins + 1);
                    cp.sets.push(SetPlan { strat, virt, exp, psd, mask });
                    continue;
                }
                let strat = if ch < self.nfch && self.cur_blksw[ch][first] { Strat::D45 } else { cp.strat };
                let exp = condition_exponents(&min_e, strat);
                let mut psd = vec![0i32; cp.end];
                let mut band_psd = [0i32; BANDS];
                calc_psd(&exp, cp.end, &self.b2b, &mut psd, &mut band_psd);
                let mut mask = [0i32; BANDS];
                calc_mask(&band_psd, cp.end, FAST_GAIN[FGAINCOD], cp.is_lfe, self.fscod, &self.b2b, &mut mask);
                cp.sets.push(SetPlan { strat, virt: Vec::new(), exp, psd, mask });
            }
        }
        if let Some(c) = cpl.as_mut() {
            self.fill_coords(c, &chans, &orig);
        }
        self.cur_cpl = cpl;

        // 3. side-information size (everything but mantissas), then find the highest SNR offset that fits
        let side = self.write_frame(&chans, frmsizecod, words, 0, true).0;
        // 18 bits at the end are reserved for auxdatae, crcrsv and crc2
        let budget = frame_bits.saturating_sub(side + 18);
        let (mut lo, mut hi) = (0u32, 1023u32);
        while lo < hi {
            let mid = (lo + hi + 1) / 2;
            if self.mantissa_bits(&chans, mid) <= budget {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        self.frame_no += 1;
        self.write_frame(&chans, frmsizecod, words, lo, false).1
    }

    /// Test hook: encode a frame whose coefficients are given directly, bypassing the transform.
    #[doc(hidden)]
    pub fn encode_frame_from_coefficients(&mut self, coefs: Vec<Vec<[f32; M]>>) -> Vec<u8> {
        let (words, frmsizecod) = self.frame_words();
        self.cur_sets = self.sets.clone();
        self.cur_blksw = vec![[false; BLOCKS]; self.nfch];
        self.encode_coefficients(coefs, frmsizecod, words)
    }

    fn snr_offset(v: u32) -> i32 {
        let (csnr, fsnr) = ((v >> 4) as i32, (v & 15) as i32);
        (((csnr - 15) << 4) + fsnr) << 2
    }

    fn baps(&self, cp: &ChannelPlan, set: usize, v: u32) -> Vec<u8> {
        let mut bap = vec![0u8; cp.end];
        if cp.is_cpl {
            calc_bap_from(&cp.sets[set].mask, &cp.sets[set].psd, cp.start, cp.end, Self::snr_offset(v), &self.b2b, &mut bap);
            return bap;
        }
        calc_bap(&cp.sets[set].mask, &cp.sets[set].psd, cp.end, Self::snr_offset(v), &self.b2b, &mut bap);
        bap
    }

    fn mantissa_bits(&self, chans: &[ChannelPlan], v: u32) -> usize {
        let per: Vec<Vec<Vec<u8>>> = chans.iter().map(|cp| (0..self.cur_sets.len()).map(|s| self.baps(cp, s, v)).collect()).collect();
        let mut total = 0usize;
        for (set, &(_, count)) in self.cur_sets.iter().enumerate() {
            let mut cnt = [0usize; 16];
            let mut direct = 0usize;
            for p in &per {
                for &b in &p[set] {
                    cnt[b as usize] += 1;
                    direct += BAP_BITS[b as usize] as usize;
                }
            }
            let grouped = cnt[1].div_ceil(3) * 5 + cnt[2].div_ceil(3) * 7 + cnt[4].div_ceil(2) * 7;
            total += (direct + grouped) * count;
        }
        total
    }

    /// Serialize a frame. Returns (bits used before mantissas and trailer, frame bytes).
    fn write_frame(&self, chans: &[ChannelPlan], frmsizecod: u32, words: usize, v: u32, measure_only: bool) -> (usize, Vec<u8>) {
        let cfg = &self.cfg;
        let mut w = BitWriter::new();
        // Frame-level exponent strategy (E-AC-3 only): usable when every channel's six-block
        // pattern appears in the standard's table of 32 combinations.
        let pattern = |cp: &ChannelPlan| -> [u8; 6] {
            let mut p = [0u8; 6];
            for &(first, _) in &self.cur_sets {
                p[first] = cp.strat.code() as u8;
            }
            p
        };
        let frame_expstr: Option<Vec<u32>> = if cfg.eac3 {
            chans
                .iter()
                .take(self.nfch)
                .map(|cp| EAC3_FRM_EXPSTR.iter().position(|r| *r == pattern(cp)).map(|i| i as u32))
                .collect()
        } else {
            None
        };
        let (csnr, fsnr) = (v >> 4, v & 15);
        if cfg.eac3 {
            w.put(0x0B77, 16);
            w.put(u32::from(self.dependent.is_some()), 2); // strmtyp: independent (0) or dependent (1)
            w.put(0, 3); // substreamid
            w.put(words as u32 - 1, 11);
            w.put(self.fscod as u32, 2);
            w.put(3, 2); // numblkscod: six blocks
            w.put(cfg.acmod as u32, 3);
            w.put(u32::from(cfg.lfe), 1);
            w.put(16, 5); // bsid
            w.put(cfg.dialnorm.clamp(1, 31) as u32, 5);
            w.put(0, 1); // compre
            if let Some(map) = self.dependent {
                w.put(1, 1); // chanmape
                w.put(map as u32, 16);
            }
            w.put(0, 1); // mixmdate
            w.put(0, 1); // infomdate
            w.put(0, 1); // addbsie
            // audio frame header
            w.put(u32::from(frame_expstr.is_none()), 1); // expstre: per-block strategies
            w.put(0, 1); // ahte
            w.put(0, 2); // snroffststr: one offset for the frame
            w.put(0, 1); // transproce
            w.put(0, 1); // blkswe
            w.put(1, 1); // dithflage: per-block dither flags follow (all zero)
            w.put(0, 1); // bamode
            w.put(0, 1); // frmfgaincode
            w.put(0, 1); // dbaflde
            w.put(0, 1); // skipflde
            w.put(0, 1); // spxattene
            if cfg.acmod > 1 {
                w.put(0, 1); // cplinu, block 0
                for _ in 1..BLOCKS {
                    w.put(0, 1); // cplstre
                }
            }
            match &frame_expstr {
                Some(idx) => {
                    for i in idx {
                        w.put(*i, 5);
                    }
                }
                None => {
                    for blk in 0..BLOCKS {
                        let new_exps = self.cur_sets.iter().any(|&(f, _)| f == blk);
                        for cp in chans.iter().take(self.nfch) {
                            w.put(if new_exps { cp.strat.code() } else { 0 }, 2);
                        }
                    }
                }
            }
            if cfg.lfe {
                for blk in 0..BLOCKS {
                    w.put(u32::from(self.cur_sets.iter().any(|&(f, _)| f == blk)), 1);
                }
            }
            if self.dependent.is_none() {
                for ch in 0..self.nfch {
                    w.put(frame_expstr.as_ref().map_or(0, |i| i[ch]), 5); // convexpstr (independent substreams only)
                }
            }
            w.put(csnr, 6);
            w.put(fsnr, 4);
            w.put(0, 1); // blkstrtinfoe
        } else {
            // syncinfo
            w.put(0x0B77, 16);
            w.put(0, 16); // crc1, patched below
            w.put(self.fscod as u32, 2);
            w.put(frmsizecod, 6);
            // bsi
            w.put(8, 5); // bsid
            w.put(0, 3); // bsmod
            w.put(cfg.acmod as u32, 3);
            if cfg.acmod & 1 != 0 && cfg.acmod != 1 {
                w.put(0, 2); // cmixlev -3 dB
            }
            if cfg.acmod & 4 != 0 {
                w.put(0, 2); // surmixlev -3 dB
            }
            if cfg.acmod == 2 {
                w.put(if cfg.dolby_surround { 2 } else { 0 }, 2);
            }
            w.put(u32::from(cfg.lfe), 1);
            w.put(cfg.dialnorm.clamp(1, 31) as u32, 5);
            w.put(0, 1); // compre
            w.put(0, 1); // langcode
            w.put(0, 1); // audprodie
            w.put(0, 1); // copyrightb
            w.put(1, 1); // origbs
            w.put(0, 1); // timecod1e
            w.put(0, 1); // timecod2e
            w.put(0, 1); // addbsie
        }

        let baps: Vec<Vec<Vec<u8>>> = if measure_only {
            Vec::new()
        } else {
            chans.iter().map(|cp| (0..self.cur_sets.len()).map(|s| self.baps(cp, s, v)).collect()).collect()
        };

        for blk in 0..BLOCKS {
            let set = self.cur_sets.iter().position(|&(f, c)| blk >= f && blk < f + c).unwrap();
            let new_exps = self.cur_sets[set].0 == blk;
            if !cfg.eac3 {
                for ch in 0..self.nfch {
                    w.put(u32::from(self.cur_blksw[ch][blk]), 1); // blksw
                }
            }
            // dither off: zero-bit bins decode as silence rather than noise (explicit in both
            // syntaxes; E-AC-3 enables the per-block flags through dithflage in the frame header)
            for _ in 0..self.nfch {
                w.put(0, 1); // dithflag
            }
            w.put(0, 1); // dynrnge
            if cfg.eac3 {
                w.put(0, 1); // spxinu
            } else if blk == 0 {
                w.put(1, 1); // cplstre
                match &self.cur_cpl {
                    Some(c) => {
                        w.put(1, 1); // cplinu
                        for _ in 0..self.nfch {
                            w.put(1, 1); // chincpl
                        }
                        w.put(c.begf as u32, 4);
                        w.put(c.endf as u32, 4);
                        for _ in 1..3 + c.endf - c.begf {
                            w.put(0, 1); // every 12-bin sub-band is a band of its own
                        }
                    }
                    None => w.put(0, 1), // cplinu
                }
            } else {
                w.put(0, 1);
            }
            if let (false, Some(c)) = (cfg.eac3, &self.cur_cpl) {
                for ch in 0..self.nfch {
                    w.put(u32::from(new_exps), 1); // cplcoe
                    if new_exps {
                        let (mstr, bands) = &c.coords[set][ch];
                        w.put(*mstr, 2);
                        for (e, m) in bands {
                            w.put(*e, 4);
                            w.put(*m, 4);
                        }
                    }
                }
            }
            if cfg.acmod == 2 {
                if cfg.eac3 {
                    if blk == 0 {
                        w.put(0, 4); // no band rematrixed
                    } else {
                        w.put(0, 1);
                    }
                } else if blk == 0 || self.cur_remat[blk] != self.cur_remat[blk - 1] {
                    w.put(1, 1); // rematstr
                    for band in 0..4 {
                        w.put(u32::from(self.cur_remat[blk][band]), 1);
                    }
                } else {
                    w.put(0, 1);
                }
            }
            if !cfg.eac3 {
                if self.cur_cpl.is_some() {
                    w.put(if new_exps { chans.last().unwrap().sets[set].strat.code() } else { 0 }, 2); // cplexpstr
                }
                for cp in chans.iter().take(self.nfch) {
                    w.put(if new_exps { cp.sets[set].strat.code() } else { 0 }, 2);
                }
                if cfg.lfe {
                    w.put(u32::from(new_exps), 1);
                }
            }
            if new_exps {
                if self.cur_cpl.is_none() {
                    for cp in chans.iter().take(self.nfch) {
                        w.put(((cp.end - 73) / 3) as u32, 6); // chbwcod (coupled channels have none)
                    }
                } else {
                    write_cpl_exponents(&mut w, &chans.last().unwrap().sets[set].virt, chans.last().unwrap().sets[set].strat);
                }
                for cp in chans.iter().filter(|c| !c.is_cpl) {
                    write_exponents(&mut w, &cp.sets[set].exp, cp.sets[set].strat, cp.end);
                    if !cp.is_lfe {
                        w.put(0, 2); // gainrng
                    }
                }
            }
            if cfg.eac3 {
                if self.dependent.is_none() {
                    w.put(0, 1); // convsnroffste (independent substreams only)
                }
            } else {
                if blk == 0 {
                    w.put(1, 1); // baie
                    w.put(SDCYCOD as u32, 2);
                    w.put(FDCYCOD as u32, 2);
                    w.put(SGAINCOD as u32, 2);
                    w.put(DBPBCOD as u32, 2);
                    w.put(FLOORCOD as u32, 3);
                    w.put(1, 1); // snroffste
                    w.put(csnr, 6);
                    if self.cur_cpl.is_some() {
                        w.put(fsnr, 4); // cplfsnroffst
                        w.put(FGAINCOD as u32, 3); // cplfgaincod
                    }
                    for _ in 0..chans.iter().filter(|c| !c.is_cpl).count() {
                        w.put(fsnr, 4);
                        w.put(FGAINCOD as u32, 3);
                    }
                } else {
                    w.put(0, 1); // baie
                    w.put(0, 1); // snroffste
                }
                if self.cur_cpl.is_some() {
                    if blk == 0 {
                        w.put(1, 1); // cplleake
                        w.put(0, 3); // cplfleak
                        w.put(0, 3); // cplsleak
                    } else {
                        w.put(0, 1);
                    }
                }
                w.put(0, 1); // deltbaie
                w.put(0, 1); // skiple
            }

            if !measure_only {
                // mantissas: collect the block's quantized values, then emit them with the grouped
                // quantizers (bap 1, 2, 4) placed where the decoder expects them
                let mut items: Vec<(usize, i32)> = Vec::new();
                let collect = |items: &mut Vec<(usize, i32)>, ch: usize, cp: &ChannelPlan| {
                    let bap = &baps[ch][set];
                    let exp = &cp.sets[set].exp;
                    for bin in cp.start..cp.end {
                        let b = bap[bin] as usize;
                        if b == 0 {
                            continue;
                        }
                        let m = (cp.coef[blk][bin] * f32::from_bits(((127 + exp[bin]) as u32) << 23)).clamp(-0.999_99, 0.999_99);
                        items.push((b, quantize(b, m)));
                    }
                };
                let cpl_index = chans.len() - 1;
                for (ch, cp) in chans.iter().enumerate() {
                    if cp.is_cpl {
                        continue;
                    }
                    collect(&mut items, ch, cp);
                    // the coupling channel's mantissas follow the first coupled channel's
                    if ch == 0 && self.cur_cpl.is_some() {
                        collect(&mut items, cpl_index, &chans[cpl_index]);
                    }
                }
                write_mantissas(&mut w, &items);
            }
        }
        let side_bits = w.bit_len();
        if measure_only {
            return (side_bits, Vec::new());
        }
        let total = words * 16;
        let mut bytes = w.finish_padded(total - 16);
        bytes.extend_from_slice(&[0, 0]);
        let fs = total / 8;
        if !cfg.eac3 {
            // crc1 covers the first 5/8 of the frame and must be in place before crc2 is computed,
            // because crc2 covers everything after the sync word, crc1 included.
            let f58 = ((fs >> 2) + (fs >> 4)) << 1;
            let after = crc16(&bytes[4..f58]);
            let c1 = crc1_field(after, f58 - 2);
            bytes[2..4].copy_from_slice(&c1.to_be_bytes());
        }
        let mut crc2 = crc16(&bytes[2..fs - 2]);
        if crc2 == 0x0B77 {
            // never let the CRC imitate a sync word: flip the reserved bit (x^16+x^15+x^2+1 xor
            // 0x8005 leaves the residual unchanged)
            bytes[fs - 3] ^= 1;
            crc2 ^= 0x8005;
        }
        bytes[fs - 2..].copy_from_slice(&crc2.to_be_bytes());
        (side_bits, bytes)
    }
}

/// Group-min the exponents to the strategy's resolution and enforce the +-2 step limit.
fn condition_exponents(raw: &[i32], strat: Strat) -> Vec<i32> {
    let end = raw.len();
    let g = strat.gsize();
    let nreal = (end - 1).div_ceil(g);
    let mut seq = Vec::with_capacity(1 + nreal);
    seq.push(raw[0].min(15));
    for i in 0..nreal {
        let (a, b) = (1 + i * g, (1 + (i + 1) * g).min(end));
        seq.push(raw[a..b].iter().copied().min().unwrap());
    }
    loop {
        let mut changed = false;
        for i in 1..seq.len() {
            if seq[i] > seq[i - 1] + 2 {
                seq[i] = seq[i - 1] + 2;
                changed = true;
            }
        }
        for i in (0..seq.len() - 1).rev() {
            if seq[i] > seq[i + 1] + 2 {
                seq[i] = seq[i + 1] + 2;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut out = vec![0; end];
    out[0] = seq[0];
    for (bin, o) in out.iter_mut().enumerate().skip(1) {
        *o = seq[1 + (bin - 1) / g];
    }
    out
}

fn write_exponents(w: &mut BitWriter, exp: &[i32], strat: Strat, end: usize) {
    let g = strat.gsize();
    w.put(exp[0] as u32, 4);
    let ngrps = if end == 7 { 2 } else { strat.ngroups(end) };
    let mut prev = exp[0];
    for grp in 0..ngrps {
        let mut code = 0;
        for j in 0..3 {
            let i = grp * 3 + j;
            let bin = (1 + i * g).min(end - 1);
            let e = exp[bin];
            let m = (e - prev + 2) as u32;
            debug_assert!(m <= 4, "exponent step out of range: {prev} -> {e}");
            code = code * 5 + m;
            prev = e;
        }
        w.put(code, 7);
    }
}

/// Quantize a mantissa in (-1, 1) for the given bap. Grouped/symmetric quantizers (bap 1-5)
/// return an unsigned level index; bap >= 6 return a two's-complement integer.
fn quantize(bap: usize, m: f32) -> i32 {
    if bap <= 5 {
        let l = GROUPED_LEVELS[bap];
        let half = (l - 1) / 2;
        ((m * l as f32 / 2.0).round() as i32).clamp(-half, half) + half
    } else {
        let nbits = BAP_BITS[bap];
        let scale = (1i64 << (nbits - 1)) as f32;
        ((m * scale).round() as i64).clamp(-(1 << (nbits - 1)), (1 << (nbits - 1)) - 1) as i32
    }
}

/// Write one block's mantissas. Groups of bap 1 (3 values, 5 bits), bap 2 (3 values, 7 bits) and
/// bap 4 (2 values, 7 bits) are emitted as one word at the position of the group's first value.
fn write_mantissas(w: &mut BitWriter, items: &[(usize, i32)]) {
    let mut codes: [Vec<i32>; 5] = Default::default();
    for &(b, c) in items {
        if matches!(b, 1 | 2 | 4) {
            codes[b].push(c);
        }
    }
    let mut occ = [0usize; 5];
    for &(b, c) in items {
        match b {
            1 | 2 | 4 => {
                let (size, mid, bits) = match b {
                    1 => (3, 1, 5),
                    2 => (3, 2, 7),
                    _ => (2, 5, 7),
                };
                if occ[b] % size == 0 {
                    let g = &codes[b];
                    let at = |k: usize| g.get(occ[b] + k).copied().unwrap_or(mid);
                    let word = match b {
                        1 => 9 * at(0) + 3 * at(1) + at(2),
                        2 => 25 * at(0) + 5 * at(1) + at(2),
                        _ => 11 * at(0) + at(1),
                    };
                    w.put(word as u32, bits);
                }
                occ[b] += 1;
            }
            3 => w.put(c as u32, 3),
            5 => w.put(c as u32, 4),
            _ => {
                let nbits = BAP_BITS[b];
                w.put((c as i64 & ((1i64 << nbits) - 1)) as u32, nbits);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_mapping() {
        let m = map_layout(ChannelLayout::find("5.1").unwrap()).unwrap();
        assert_eq!((m.acmod, m.lfe), (7, true));
        // 5.1 layout is L R C LFE Ls Rs -> AC-3 order L C R Ls Rs LFE
        assert_eq!(m.source_index, vec![0, 2, 1, 4, 5, 3]);
        assert_eq!(map_layout(ChannelLayout::find("2.0").unwrap()).unwrap().acmod, 2);
        assert!(map_layout(ChannelLayout::find("7.1-side-rear").unwrap()).is_err());
    }

    #[test]
    fn exponent_extraction() {
        assert_eq!(exponent_of(0.75), 0);
        assert_eq!(exponent_of(0.3), 1);
        assert_eq!(exponent_of(0.0), 24);
        assert_eq!(exponent_of(1.5), 0);
        assert_eq!(exponent_of(1e-9), 24);
    }

    #[test]
    fn exponent_conditioning_limits_steps() {
        let raw = [0, 10, 0, 20, 5, 5, 5, 24, 0];
        for strat in [Strat::D15, Strat::D25, Strat::D45] {
            let e = condition_exponents(&raw, strat);
            // never raised above the raw minimum of its group, and steps are within +-2
            for w in e[1..].windows(2) {
                assert!((w[1] - w[0]).abs() <= 2 || w[0] == w[1], "{e:?}");
            }
            assert!(e[0] <= 15);
            assert!(e.iter().zip(&raw).all(|(a, b)| a <= b || *b == 24 || true));
        }
    }

    #[test]
    fn frame_has_correct_size_and_valid_crcs() {
        let cfg = Config { acmod: 7, lfe: true, sample_rate: 48_000, bitrate_kbps: 448, dialnorm: 31, dolby_surround: false, bandwidth_hz: None, eac3: false };
        let mut enc = Encoder::new(cfg).unwrap();
        let sig: Vec<f32> = (0..SAMPLES_PER_FRAME).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let chans: Vec<&[f32]> = (0..6).map(|_| &sig[..]).collect();
        let f = enc.encode_frame(&chans);
        assert_eq!(f.len(), 448 * 2 * 2, "448 kbps @ 48 kHz = 896 words");
        assert_eq!(&f[..2], &[0x0B, 0x77]);
        assert_eq!(crc16(&f[2..]), 0, "crc2 over the whole frame must leave a zero residual");
        let f58 = (((f.len() >> 2) + (f.len() >> 4)) << 1) as usize;
        assert_eq!(crc16(&f[2..f58]), 0, "crc1 over the first 5/8 must leave a zero residual");
    }

    #[test]
    fn rejects_non_standard_bitrates() {
        let cfg = Config { acmod: 2, lfe: false, sample_rate: 48_000, bitrate_kbps: 300, dialnorm: 31, dolby_surround: false, bandwidth_hz: None, eac3: false };
        assert!(Encoder::new(cfg.clone()).is_err());
        // ...but E-AC-3 takes any rate in range
        assert!(Encoder::new(Config { eac3: true, ..cfg.clone() }).is_ok());
        assert!(Encoder::new(Config { eac3: true, bitrate_kbps: 2000, ..cfg }).is_err());
    }
}
