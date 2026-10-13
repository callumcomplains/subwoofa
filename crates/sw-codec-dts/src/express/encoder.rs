//! DTS Express (LBR): encoder for the low bit rate extension (ETSI TS 102 114 clause 9).
//!
//! The codec works on "time samples": 32 bands of 128 samples per frame (4096 samples at 48 kHz) produced
//! by a hybrid filterbank (`lbr_fb`). Per band and 16-sample slot an amplitude (scale factor) is sent, the
//! normalised samples are quantized with one of five small alphabets chosen by a quantizer profile, and
//! bands that are not coded are replaced by noise of the right level by the decoder. Steady tones are coded
//! separately as tonal components (`lbr_tonal`). The writers below follow the order in which a decoder parses the
//! chunks.

use crate::express::filterbank::{Analysis, BANDS, HOP, TS};
use crate::express::tables::*;
use rayon::prelude::*;

/// Bit writer, least significant bit first (the bit order of LBR streams).
#[derive(Default, Clone)]
pub struct BitsLe {
    bytes: Vec<u8>,
    cur: u64,
    n: u32,
}

impl BitsLe {
    pub fn new() -> BitsLe {
        BitsLe::default()
    }

    pub fn put(&mut self, v: u32, bits: u32) {
        debug_assert!(bits <= 32);
        if bits == 0 {
            return;
        }
        let v = if bits == 32 {
            v as u64
        } else {
            (v as u64) & ((1u64 << bits) - 1)
        };
        self.cur |= v << self.n;
        self.n += bits;
        while self.n >= 8 {
            self.bytes.push(self.cur as u8);
            self.cur >>= 8;
            self.n -= 8;
        }
    }

    pub fn bit_len(&self) -> usize {
        self.bytes.len() * 8 + self.n as usize
    }

    pub fn into_bytes(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.bytes.push(self.cur as u8);
        }
        self.bytes
    }
}

/// Canonical code of a table given as (symbol, length) in code order (the first entry gets the all-zero
/// code; every next code is the previous plus one, shifted to the length of the entry). Returns the
/// (code, length) of every symbol (the shortest one when a symbol appears twice).
fn codes_of(table: &[(u8, u8)], symbols: usize) -> Vec<Option<(u32, u8)>> {
    let mut out: Vec<Option<(u32, u8)>> = vec![None; symbols];
    let mut code = 0u32;
    let mut prev_len = table[0].1;
    for (i, &(sym, len)) in table.iter().enumerate() {
        if i > 0 {
            code += 1;
            if len > prev_len {
                code <<= len - prev_len;
            } else {
                code >>= prev_len - len;
            }
        }
        prev_len = len;
        let slot = &mut out[sym as usize];
        if slot.is_none_or(|(_, l)| len < l) {
            *slot = Some((code, len));
        }
    }
    out
}

/// A variable length code table whose decoded value is `symbol - 1` (symbol 0 is the escape).
pub(crate) struct Vlc {
    codes: Vec<Option<(u32, u8)>>,
}

impl Vlc {
    pub(crate) fn new(table: &[(u8, u8)]) -> Vlc {
        let n = table.iter().map(|t| t.0 as usize).max().unwrap() + 1;
        Vlc {
            codes: codes_of(table, n),
        }
    }

    /// Write the value `v` (what the decoder returns, 0 or more).
    pub(crate) fn put(&self, w: &mut BitsLe, v: u32) {
        let sym = v as usize + 1;
        if let Some(Some((code, len))) = self.codes.get(sym) {
            put_code(w, *code, *len);
            return;
        }
        // escape: symbol 0, then a 3-bit length and the raw value
        let (code, len) = self.codes[0].expect("escape code");
        put_code(w, code, len);
        let nbits = (32 - v.leading_zeros()).max(1);
        w.put(nbits - 1, 3);
        w.put(v, nbits);
    }
}

/// Codes are defined most significant bit first; they are written bit by bit.
fn put_code(w: &mut BitsLe, code: u32, len: u8) {
    for i in (0..len).rev() {
        w.put((code >> i) & 1, 1);
    }
}

struct Tables {
    fst_rsd_amp: Vlc,
    rsd_apprx: Vlc,
    rsd_amp: Vlc,
    avg_g3: Vlc,
    st_grid: Vlc,
    grid_3: Vlc,
    grid_2: Vlc,
    rsd: Vec<Option<(u32, u8)>>,
    pack5: Vec<Option<u8>>,
    pack3: Vec<Option<u8>>,
}

impl Tables {
    fn new() -> Tables {
        // reverse maps of the packed sample codes
        let mut pack5 = vec![None; 243];
        for (code, v) in RSD_PACK_5_IN_8.iter().enumerate() {
            let mut idx = 0usize;
            let mut valid = true;
            for j in 0..5 {
                let f = ((*v >> (2 * j)) & 3) as usize;
                if f > 2 {
                    valid = false;
                    break;
                }
                idx += f * 3usize.pow(j as u32);
            }
            if valid && pack5[idx].is_none() {
                pack5[idx] = Some(code as u8);
            }
        }
        let mut pack3 = vec![None; 125];
        for (code, v) in RSD_PACK_3_IN_7.iter().enumerate() {
            if v.iter().all(|x| *x < 5) {
                let idx = v[0] as usize + 5 * v[1] as usize + 25 * v[2] as usize;
                if pack3[idx].is_none() {
                    pack3[idx] = Some(code as u8);
                }
            }
        }
        Tables {
            fst_rsd_amp: Vlc::new(&HUFF_FST_RSD_AMP),
            rsd_apprx: Vlc::new(&HUFF_RSD_APPRX),
            rsd_amp: Vlc::new(&HUFF_RSD_AMP),
            avg_g3: Vlc::new(&HUFF_AVG_G3),
            st_grid: Vlc::new(&HUFF_ST_GRID),
            grid_3: Vlc::new(&HUFF_GRID_3),
            grid_2: Vlc::new(&HUFF_GRID_2),
            rsd: codes_of(&HUFF_RSD, 8),
            pack5,
            pack3,
        }
    }
}

/// Amplitude the decoder applies to a sample pair with scale factor `s` and `step` attenuation steps: the step
/// is subtracted from the scale factor, and a negative result silences the pair.
fn step_gain(s: usize, step: u8) -> f64 {
    if (step as usize) > s {
        0.0
    } else {
        QUANT_AMP[s - step as usize] as f64
    }
}

/// The values the decoder reconstructs, before the scale factor, for quantizer level 1 to 5.
fn alphabet(level: usize) -> Vec<f32> {
    match level {
        1 => RSD_LEVEL_2A.to_vec(),
        2 => RSD_LEVEL_3.to_vec(),
        3 => RSD_LEVEL_5.to_vec(),
        4 => RSD_LEVEL_8.to_vec(),
        _ => RSD_LEVEL_16.to_vec(),
    }
}

fn nearest(alpha: &[f32], x: f32) -> usize {
    let mut best = 0;
    let mut bd = f32::MAX;
    for (i, a) in alpha.iter().enumerate() {
        let d = (a - x).abs();
        if d < bd {
            bd = d;
            best = i;
        }
    }
    best
}

/// Quantizer level (1..=5) of every position from the quantizer profile byte (clause 9.6.3.3.2).
fn levels_from_profile(profile: u32, nsubbands: usize, limited_rate: u32) -> Vec<usize> {
    let ol = ((profile >> 3) & 7) as i32;
    let st = (profile >> 6) as i32;
    let max_sb = (profile & 7) as usize;
    let q: Vec<usize> = (0..nsubbands)
        .map(|sb| {
            let f = (sb as i32) * limited_rate as i32 / nsubbands as i32;
            let a = 18000 / (12 * f / 1000 + 100 + 40 * st) + 20 * ol;
            match a {
                i32::MIN..=95 => 1,
                96..=140 => 2,
                141..=180 => 3,
                181..=230 => 4,
                _ => 5,
            }
        })
        .collect();
    let mut out = q.clone();
    for sb in 0..8.min(nsubbands) {
        out[sb] = q[SB_REORDER[max_sb][sb] as usize];
    }
    out
}

pub struct LbrConfig {
    pub sample_rate: u32,
    /// Full-band channels, coded in pairs (L R)(Ls Rs)(C).
    pub channels: usize,
    /// Speaker activity mask of the stream (C = 1, L/R = 2, Ls/Rs = 4, LFE = 8).
    pub speaker_mask: u16,
    /// An LFE channel follows the full-band channels in the input.
    pub lfe: bool,
    /// Bits per second of the whole stream.
    pub bitrate: u32,
    /// Express Dialog Mode: code only the lower half of the band (the original encoder's `BAND_LIMIT_RATIO = 0.5`).
    pub dialog_mode: bool,
    /// Exact bytes of every frame's payload (when the container fixes them); otherwise the bit rate decides.
    pub payload_bytes: Option<usize>,
}

pub struct LbrEncoder {
    cfg: LbrConfig,
    t: Tables,
    ana: Vec<Analysis>,
    /// Source samples per channel not yet fully analysed.
    buf: Vec<Vec<f64>>,
    next_hop: usize,
    /// Samples dropped from the front of `buf` / `lfe_buf`.
    base: usize,
    lfe: Option<crate::express::lfe::LfeEncoder>,
    lfe_buf: Vec<f64>,
    lfe_base: usize,
    frames: usize,
    frame_bytes: usize,
    geo: Geo,
    /// Size of the previous frame's coarsest possible residual: what tones must leave room for.
    min_resid: std::sync::atomic::AtomicUsize,
    groups: Vec<crate::express::tonal::Group>,
    /// Prediction state after the last frame and the plan of the frame being coded, per channel.
    lpc_state: std::sync::RwLock<Vec<LpcState>>,
    lpc_plan: std::sync::RwLock<Vec<LpcPlan>>,
}

const SUBBANDS: usize = BANDS;

/// What the decoder holds of a channel's three predicted bands at the end of a frame: the coefficient set that
/// the next frame's first sixteen samples still use, and the last eight decoded samples.
#[derive(Clone, Default)]
struct LpcState {
    last: [[f64; 8]; 3],
    hist: [[f64; 8]; 3],
}

/// The predictor of one channel in the frame being coded: the reflection codes sent (two sets for bands 0 and
/// 1, one for band 2) and their direct-form coefficients.
#[derive(Clone, Default)]
struct LpcPlan {
    codes: [[[u8; 8]; 2]; 3],
    cur: [[[f64; 8]; 2]; 3],
}

/// Band layout of a stream: the number of coded subbands and the regions derived from it (clause 9.6.2).
#[derive(Clone, Copy, Debug)]
struct Geo {
    /// 0, 1 or 2: the stream is limited to 1/1, 1/2 or 1/4 of the sample rate's bandwidth.
    band_limit: u32,
    nsb: usize,
    limited_rate: u32,
    min_mono: usize,
    max_mono: usize,
    /// Groups of the first grid (bands 2 up to this) and of the second grid.
    ng1: usize,
    ng2: usize,
    /// Bits of a band index in the time-sample chunks.
    idx_bits: u32,
}

impl Geo {
    fn new(sample_rate: u32, band_limit: u32) -> Geo {
        let nsb = SUBBANDS >> band_limit;
        let limited_rate = sample_rate >> band_limit;
        let half = (limited_rate / 2) as usize;
        Geo {
            band_limit,
            nsb,
            limited_rate,
            min_mono: (nsb * 2000 / half).min(nsb),
            max_mono: (nsb * 14000 / half).min(nsb),
            ng1: SCF_TO_GRID_1[nsb - 1] as usize + 1,
            ng2: SCF_TO_GRID_2[nsb - 1] as usize + 1,
            idx_bits: 3 + (2 - band_limit),
        }
    }
}
const AMP_MAX: usize = 56;
/// From this bit rate on the LFE uses the 6-bit coding (17 bytes a frame more, about 1.6 kbps).
const LFE_WIDE_FROM: u32 = 256_000;

/// Narrowest coded bandwidth (in bands) the rate control may fall back to.
const MIN_K: usize = 6;

/// How far above the noise floor a spectral line has to stand before it is coded as a tonal component.
const TONE_KAPPA: f64 = 4.0;

/// Everything derived from the time samples of one channel for one frame and quantizer profile.
struct ChanCoding {
    /// Squared error against the input time samples (noise-filled bands count twice their energy).
    err: f64,
    /// Second grid: per group of bands, 64 temporal attenuation steps (two samples each) shared by its bands.
    g2: Vec<[u8; 64]>,
    /// The last eight samples the decoder will hold in each of the three predicted bands.
    tails: [[f64; 8]; 3],
    /// Which of the three predicted bands use their predictor in this frame.
    lpc_used: [bool; 3],
    /// Quantized symbol indexes `[band][slot][16]`.
    q: Vec<Vec<Vec<i32>>>,
    grids: Grids,
}

struct Grids {
    g1: Vec<[i32; 8]>,
    avg: Vec<i32>,
    g3: Vec<[i32; 8]>,
}

impl LbrEncoder {
    pub fn new(cfg: LbrConfig) -> Result<LbrEncoder, crate::Error> {
        if cfg.sample_rate != 48_000 {
            return Err(crate::Error::Unsupported("DTS Express is implemented at 48 kHz".into()));
        }
        if cfg.channels == 0 || cfg.channels > 5 {
            return Err(crate::Error::Unsupported(
                "DTS Express: 1 to 5 full-band channels".into(),
            ));
        }
        let floor = Self::min_bitrate(cfg.channels, cfg.lfe);
        if cfg.bitrate < floor {
            return Err(crate::Error::Unsupported(format!(
                "this layout needs at least {} kbps in DTS Express (the encoder cannot code it any smaller yet); {} kbps was asked for",
                floor / 1000 + 1,
                cfg.bitrate / 1000
            )));
        }
        // whole 16-bit words: the verifier rejects odd frame sizes
        let frame_bytes = cfg
            .payload_bytes
            .unwrap_or((cfg.bitrate as u64 * 4096 / cfg.sample_rate as u64 / 16) as usize * 2);
        let n = cfg.channels;
        let per_channel = cfg.bitrate as f64 / 1000.0 / cfg.channels as f64;
        // how much of the band is coded: all of it from 128 kbps per channel, half from 56, a quarter below
        let band_limit = if cfg.dialog_mode {
            1
        } else if per_channel >= 128.0 {
            0
        } else if per_channel >= 56.0 {
            1
        } else {
            2
        };
        let geo = Geo::new(cfg.sample_rate, band_limit);
        // the decoder scales a band-limited stream's filter bank output by sqrt(2^(2 - limited range)), so the
        // coefficients are scaled down by the same factor here
        let gain = if band_limit > 0 {
            1.0 / ((1u32 << band_limit) as f64).sqrt()
        } else {
            1.0
        };
        let ana: Vec<Analysis> = (0..n)
            .map(|_| {
                let mut a = Analysis::new();
                a.set_gain(gain);
                a
            })
            .collect();
        Ok(LbrEncoder {
            t: Tables::new(),
            ana,
            buf: vec![Vec::new(); n],
            next_hop: 0,
            base: 0,
            lfe: cfg
                .lfe
                .then(|| crate::express::lfe::LfeEncoder::new(cfg.sample_rate, cfg.bitrate >= LFE_WIDE_FROM)),
            lfe_buf: Vec::new(),
            lfe_base: 0,
            frames: 0,
            frame_bytes,
            geo,
            min_resid: std::sync::atomic::AtomicUsize::new(0),
            groups: (0..5).map(crate::express::tonal::Group::new).collect(),
            lpc_state: std::sync::RwLock::new(vec![LpcState::default(); n]),
            lpc_plan: std::sync::RwLock::new(vec![LpcPlan::default(); n]),
            cfg,
        })
    }

    /// Smallest bit rate (bits per second) the encoder can honour: about 55 kbps per channel pair at its
    /// narrowest bandwidth (6 kHz), about 30 kbps for a lone channel, a few kbps for the LFE.
    pub fn min_bitrate(channels: usize, lfe: bool) -> u32 {
        let pairs = channels / 2;
        let lone = channels % 2;
        pairs as u32 * 56_000 + lone as u32 * 30_000 + if lfe { 3_000 } else { 0 }
    }

    pub fn frame_samples(&self) -> usize {
        4096
    }

    /// Feed any number of samples per channel; returns the LBR payloads (one per 4096 input samples) that
    /// are ready.
    pub fn push(&mut self, pcm: &[Vec<f32>]) -> Vec<Vec<u8>> {
        let n = self.buf.len();
        for (b, c) in self.buf.iter_mut().zip(pcm) {
            b.extend(c.iter().map(|v| *v as f64));
        }
        if self.lfe.is_some() {
            if let Some(c) = pcm.get(n) {
                self.lfe_buf.extend(c.iter().map(|v| *v as f64));
            }
        }
        let mut out = Vec::new();
        // a frame needs hops up to 32 f + 32, i.e. samples up to 4096 f + 4096 + 256
        while self.base + self.buf[0].len() >= (self.frames + 1) * 4096 + 256 + 128 + self.lookahead() {
            out.push(self.next_frame());
        }
        out
    }

    /// Flush: pad with silence so every pushed sample is covered.
    pub fn finish(&mut self) -> Vec<Vec<u8>> {
        let pushed = self.base + self.buf[0].len();
        let frames = pushed.div_ceil(4096);
        let want = (frames + 1) * 4096 + 512 + self.lookahead();
        for b in self.buf.iter_mut() {
            b.resize(want - self.base, 0.0);
        }
        if self.lfe.is_some() {
            self.lfe_buf.resize(want - self.lfe_base, 0.0);
        }
        let mut out = Vec::new();
        while self.frames < frames {
            out.push(self.next_frame());
        }
        out
    }

    /// Samples ahead of a frame's own hops that the tone sets of the frame need (their span reaches 54 steps
    /// beyond the frame's last step).
    fn lookahead(&self) -> usize {
        if true {
            54 * HOP
        } else {
            0
        }
    }

    fn next_frame(&mut self) -> Vec<u8> {
        let f = self.frames;
        let need = 32 * f + 32 + 54;
        while self.next_hop <= need {
            let start = self.next_hop * HOP - self.base;
            for (c, a) in self.ana.iter_mut().enumerate() {
                let block: Vec<f64> = (0..2 * HOP)
                    .map(|i| self.buf[c].get(start + i).copied().unwrap_or(0.0))
                    .collect();
                a.compute_hop(&block);
            }
            self.next_hop += 1;
        }
        let tonal_chunks = self.choose_tones(f);
        for a in self.ana.iter_mut() {
            a.commit_upto(32 * f + 32);
        }
        let ts: Vec<Vec<Vec<f64>>> = self.ana.iter_mut().map(|a| a.take_frame()).collect();
        // drop what no later hop needs
        let keep_from = self.next_hop * HOP;
        if keep_from > self.base {
            let drop = (keep_from - self.base).min(self.buf[0].len());
            for b in self.buf.iter_mut() {
                b.drain(..drop);
            }
            self.base += drop;
        }
        let lfe = self.lfe.as_mut().map(|l| {
            let origin = f * 4096;
            let (buf, base) = (&self.lfe_buf, self.lfe_base);
            let body = l.code_frame(&|i| {
                let p = origin as isize + i - base as isize;
                if p < 0 {
                    0.0
                } else {
                    buf.get(p as usize).copied().unwrap_or(0.0)
                }
            });
            chunk(0x0A, body)
        });
        if self.lfe.is_some() {
            let keep = ((f + 1) * 4096).saturating_sub(self.lfe.as_ref().map_or(0, |l| l.reach()));
            if keep > self.lfe_base {
                let d = (keep - self.lfe_base).min(self.lfe_buf.len());
                self.lfe_buf.drain(..d);
                self.lfe_base += d;
            }
        }
        self.frames += 1;
        self.encode_frame(&ts, lfe, tonal_chunks)
    }

    /// Choose the tone sets of frame `f` (all five groups) from the pending coefficients, subtract them there
    /// and return the tonal chunks.
    fn choose_tones(&mut self, f: usize) -> Vec<Vec<u8>> {
        use crate::express::tonal as tn;
        let nch = self.cfg.channels;
        let lr = 2 - self.geo.band_limit as i32;
        let search = tn::Search {
            max_line: (4 * self.geo.nsb).saturating_sub(6),
            kappa: TONE_KAPPA,
            max_tones: 12,
        };
        let mut all: Vec<Vec<Vec<tn::Tone>>> = Vec::new();
        // at most a quarter of the frame goes to tones, and never so much that the coarsest residual of the
        // previous frame (plus headers) no longer fits
        let room = self.frame_bytes as i64
            - (self.header_len() as i64
                + 8
                + (self.min_resid.load(std::sync::atomic::Ordering::Relaxed) as f64 * 1.15) as i64
                + if self.cfg.lfe { 60 } else { 0 });
        let mut bits_left: i64 = ((self.frame_bytes as i64 * 8) / 4).min(room.max(0) * 8 / 2);
        for g in 0..5usize {
            let mut subs: Vec<Vec<tn::Tone>> = Vec::new();
            for j in 0..(1usize << g) {
                let grp = &self.groups[g];
                let first = 32 * f + tn::TONE_DELAY + j * grp.steps;
                let mut targets: Vec<Vec<[f64; HOP]>> = (0..nch)
                    .map(|ch| {
                        (0..2 * grp.steps)
                            .map(|r| self.ana[ch].coefs_mut(first + r).map(|c| *c).unwrap_or([0.0; HOP]))
                            .collect()
                    })
                    .collect();
                let tones = tn::pick(grp, &mut targets, &search, &mut bits_left);
                if !tones.is_empty() {
                    for ch in 0..nch {
                        for r in 0..2 * grp.steps {
                            if let Some(c) = self.ana[ch].coefs_mut(first + r) {
                                *c = targets[ch][r];
                            }
                        }
                    }
                }
                subs.push(tones);
            }
            all.push(subs);
        }
        let any = all.iter().flatten().any(|t| !t.is_empty());
        if !any {
            return Vec::new();
        }
        let scf = tn::scale_factors(&all, lr);
        let mut out = vec![chunk(0x0E, tn::write_scf(&scf))];
        for (g, subs) in all.iter().enumerate() {
            if subs.iter().any(|t| !t.is_empty()) {
                out.push(chunk(0x1B - g as u8, tn::write_group(g as u32, subs, nch, &scf, lr)));
            }
        }
        out
    }

    fn header_len(&self) -> usize {
        5 + 11 + 3
    }

    /// The bit rate the LBR header announces: the payload after the 16-byte LBR header, as the commercial
    /// encoder counts it.
    fn hdr_br(&self) -> u32 {
        (((self.frame_bytes.saturating_sub(16)) as u64 * 8 * self.cfg.sample_rate as u64 / 4096) as u32) & !31
    }

    fn br_scale(&self) -> f64 {
        let per_ch = self.hdr_br() as f64 / self.cfg.channels as f64;
        if per_ch < 14000.0 {
            0.85
        } else if per_ch < 32000.0 {
            (per_ch - 14000.0) / 120000.0 + 0.85
        } else {
            1.0
        }
    }

    fn encode_frame(&self, ts: &[Vec<Vec<f64>>], lfe: Option<Vec<u8>>, tonal: Vec<Vec<u8>>) -> Vec<u8> {
        let nch = self.cfg.channels;
        let npairs = nch.div_ceil(2);
        let tonal_bytes: usize = tonal.iter().map(|c| c.len()).sum();
        let budget = self
            .frame_bytes
            .saturating_sub(self.header_len() + 8 + lfe.as_ref().map_or(0, |c| c.len()) + tonal_bytes);
        let nsb = self.geo.nsb;
        self.plan_prediction(ts);
        // profiles from the finest quantizers to the coarsest (steepness 0 only: the others never won)
        let profiles: Vec<u32> = (0..8u32).rev().map(|ol| ol << 3).collect();
        // what the tones must leave room for next time: the coarsest coding of this frame
        {
            let mut n = 0usize;
            for pair in 0..npairs {
                n += self
                    .write_pair(pair, ts, 0, MIN_K)
                    .iter()
                    .map(|c| c.len())
                    .sum::<usize>();
            }
            self.min_resid.store(n, std::sync::atomic::Ordering::Relaxed);
        }
        // For every profile the widest coding that fits the frame; the one with the least error wins.
        // each profile is searched on its own, so they run side by side
        let fits: Vec<Option<(f64, Vec<Vec<u8>>, Vec<(usize, [[f64; 8]; 3], [bool; 3])>)>> = profiles
            .par_iter()
            .map(|&p| {
                let eval = |k: usize| {
                    let mut chunks = Vec::new();
                    let mut err = 0.0;
                    let mut tails = Vec::new();
                    for pair in 0..npairs {
                        let (c, e, t) = self.write_pair_ex(pair, ts, p, k, false);
                        chunks.extend(c);
                        err += e;
                        tails.extend(t);
                    }
                    let bytes: usize = chunks.iter().map(|c| c.len()).sum();
                    (chunks, err, tails, bytes)
                };
                let (mut lo, mut hi) = (MIN_K, nsb + 1);
                let mut fit = None;
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    let (c, e, t, b) = eval(mid);
                    if b <= budget {
                        fit = Some((e, c, t));
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                fit
            })
            .collect();
        // the least error wins; on a tie the finer profile (earlier in the list)
        let mut best = None;
        for fit in fits.into_iter().flatten() {
            if best
                .as_ref()
                .is_none_or(|(be, _, _): &(f64, Vec<Vec<u8>>, Vec<(usize, [[f64; 8]; 3], [bool; 3])>)| fit.0 < *be)
            {
                best = Some(fit);
            }
        }
        let (chunks, tails) = match best {
            Some((_, c, t)) => (c, t),
            None => {
                let mut c = Vec::new();
                let mut t = Vec::new();
                for pair in 0..npairs {
                    let (cc, _, tt) = self.write_pair_ex(pair, ts, 0, MIN_K, true);
                    c.extend(cc);
                    t.extend(tt);
                }
                (c, t)
            }
        };
        self.commit_prediction(&tails);
        let mut chunks = chunks;
        chunks.extend(lfe);
        let used: usize = chunks.iter().map(|c| c.len()).sum();
        if used + tonal_bytes + self.header_len() + 8 <= self.frame_bytes {
            chunks.extend(tonal);
        }
        self.assemble(chunks)
    }

    /// Predictors of the three lowest bands for the frame about to be coded (the reflection codes that are sent
    /// and their direct-form coefficients).
    fn plan_prediction(&self, ts: &[Vec<Vec<f64>>]) {
        let mut plans = vec![LpcPlan::default(); ts.len()];
        for (ch, plan) in plans.iter_mut().enumerate() {
            for sb in 0..3 {
                let y = &ts[ch][sb];
                let sets: [&[f64]; 2] = if sb < 2 {
                    [&y[16..80], &y[80..128]]
                } else {
                    [&y[16..128], &y[16..128]]
                };
                for (set, seg) in sets.iter().enumerate() {
                    let codes = crate::express::lpc::analyse(seg);
                    plan.codes[sb][set] = codes;
                    plan.cur[sb][set] = crate::express::lpc::direct(&codes);
                }
            }
        }
        *self.lpc_plan.write().unwrap() = plans;
    }

    /// What the decoder holds after the frame that was just chosen: the sets its next frame's first samples use
    /// and the last decoded samples of the predicted bands.
    fn commit_prediction(&self, tails: &[(usize, [[f64; 8]; 3], [bool; 3])]) {
        let plans = self.lpc_plan.read().unwrap();
        let mut state = self.lpc_state.write().unwrap();
        for &(ch, tail, on) in tails {
            let st = &mut state[ch];
            let pick = |sb: usize, set: usize| if on[sb] { plans[ch].cur[sb][set] } else { [0f64; 8] };
            st.last = [pick(0, 1), pick(1, 1), pick(2, 0)];
            st.hist = tail;
        }
    }

    /// Wrap the chunks into the frame chunk and add the header; pad to the frame size.
    fn assemble(&self, chunks: Vec<Vec<u8>>) -> Vec<u8> {
        let mut body: Vec<u8> = Vec::new();
        for c in &chunks {
            body.extend_from_slice(c);
        }
        let mut out = Vec::with_capacity(self.frame_bytes);
        out.extend_from_slice(&[0x0A, 0x80, 0x19, 0x21]);
        out.push(2); // decoder initialisation follows
        out.push(12); // 48 kHz
        out.extend_from_slice(&self.cfg.speaker_mask.to_le_bytes());
        out.extend_from_slice(&0x0800u16.to_le_bytes());
        out.push(
            match self.geo.band_limit {
                0 => 0x14,
                1 => 0x08,
                _ => 0x10,
            } | 0x01
                | if self.cfg.lfe { 0x02 } else { 0 },
        ); // band limit, LFE; bit 0 is set in every commercial stream seen
        let br = self.hdr_br();
        out.push((((br >> 16) & 0xF) | (((br >> 16) & 0xF) << 4)) as u8);
        out.extend_from_slice(&(br as u16).to_le_bytes());
        out.extend_from_slice(&(br as u16).to_le_bytes());
        // frame chunk with its 16-bit checksum: the id byte, both length bytes and every byte after the checksum
        let pad = self.frame_bytes.saturating_sub(out.len() + 3 + 2 + body.len() + 2);
        let len = 2 + body.len() + pad;
        let mut csum: u32 = 0x84 + (len >> 8) as u32 + (len & 0xFF) as u32;
        csum += body.iter().map(|b| *b as u32).sum::<u32>();
        out.push(0x84);
        out.extend_from_slice(&(len as u16).to_be_bytes());
        out.extend_from_slice(&(csum as u16).to_be_bytes());
        out.extend_from_slice(&body);
        out.resize(out.len() + pad, 0);
        out.resize(self.frame_bytes, 0); // the commercial encoder leaves two zero bytes after the chunk
        out
    }

    fn g3_avg_only_start(&self) -> usize {
        let per = self.cfg.channels as u32 + 2;
        let res_profile = if self.hdr_br() >= 44000 * per {
            2
        } else if self.hdr_br() >= 25000 * per {
            1
        } else {
            0
        };
        (self.geo.nsb * AVG_G3_FREQS[res_profile] as usize / (self.geo.limited_rate / 2) as usize).min(self.geo.nsb)
    }

    /// Quantize one channel for a set of levels. Bands at or above `k` are not coded; they get the scale
    /// factor that makes the decoder's noise match their level.
    /// Bands below `k` carry data; bands from `k` up to `kw` are written as zero symbols (a channel pair has to
    /// describe the whole mono region); above `kw` the decoder fills noise at the signalled level.
    fn code_channel(
        &self,
        ch: usize,
        x: &[Vec<f64>],
        levels: &[usize],
        pos: &[usize],
        k: usize,
        kw: usize,
        g2: Vec<[u8; 64]>,
    ) -> ChanCoding {
        let nsb = self.geo.nsb;
        let mut q = vec![vec![Vec::new(); 8]; nsb];
        let mut scf = vec![[0usize; 8]; nsb];
        let noise_gain = 0.453 * self.br_scale();
        let mut err = 0.0;
        let mut tails = [[0f64; 8]; 3];
        let mut lpc_used = [false; 3];
        for sb in 0..nsb {
            // the quantizer level belongs to the position a band is coded at (which need not be its own index)
            let alpha = alphabet(levels[pos[sb].min(nsb - 1)]);
            let lmax = alpha.iter().fold(0f32, |m, a| m.max(a.abs()));
            let group = SCF_TO_GRID_2[sb] as usize;
            if sb < 3 && pos[sb] < k {
                // the lowest bands are predicted: a closed loop over what the decoder will have produced
                // with the predictor of the frame, or without it (the first samples still follow the last frame's)
                let off = self.lpc_band(ch, sb, &x[sb], &alpha, lmax, false);
                let on = if levels[sb] >= 3 {
                    Some(self.lpc_band(ch, sb, &x[sb], &alpha, lmax, true))
                } else {
                    None
                };
                let ((s8, q8, e, tail), used) = match on {
                    Some(o) if o.2 < off.2 => (o, true),
                    _ => (off, false),
                };
                lpc_used[sb] = used;
                err += e;
                tails[sb] = tail;
                for slot in 0..8 {
                    scf[sb][slot] = s8[slot];
                    q[sb][slot] = q8[slot].clone();
                }
                continue;
            }
            for slot in 0..8 {
                let seg = &x[sb][slot * 16..slot * 16 + 16];
                // attenuation steps of the eight sample pairs of the slot (none for the lowest bands)
                let steps: [u8; 8] = if sb >= 4 && group < g2.len() {
                    g2[group][slot * 8..slot * 8 + 8].try_into().unwrap()
                } else {
                    [0; 8]
                };
                if pos[sb] < k {
                    let (s, qq, e) = self.pick_scale(seg, &alpha, lmax, &steps);
                    err += e;
                    scf[sb][slot] = s;
                    q[sb][slot] = qq;
                } else if pos[sb] < kw {
                    scf[sb][slot] = 0;
                    err += seg.iter().map(|v| v * v).sum::<f64>();
                    let zero = nearest(&alpha, 0.0) as i32;
                    q[sb][slot] = vec![zero; 16];
                } else {
                    // decoder noise: uniform, rms 0.453 * br scale * amplitude
                    let energy = seg.iter().map(|v| v * v).sum::<f64>();
                    err += 2.0 * energy;
                    let rms = (energy / 16.0).sqrt();
                    let want = rms / noise_gain;
                    let mut s = 0usize;
                    // the noise is shaped by the attenuation steps as well
                    let shaped = |s: usize| (steps.iter().map(|g| step_gain(s, *g).powi(2)).sum::<f64>() / 8.0).sqrt();
                    while s < AMP_MAX - 1 && shaped(s) < want {
                        s += 1;
                    }
                    scf[sb][slot] = if want < 1e-6 { 0 } else { s };
                    q[sb][slot] = Vec::new();
                }
            }
        }
        ChanCoding {
            err,
            q,
            grids: self.grids(&scf),
            g2,
            tails,
            lpc_used,
        }
    }

    /// Closed-loop quantization of one predicted band: scale factors, symbols, squared error and the decoder's last
    /// eight samples. The coefficient set follows the position in the frame as the decoder's synthesis does.
    fn lpc_band(
        &self,
        ch: usize,
        sb: usize,
        y: &[f64],
        alpha: &[f32],
        lmax: f32,
        on: bool,
    ) -> ([usize; 8], Vec<Vec<i32>>, f64, [f64; 8]) {
        let plan_all = self.lpc_plan.read().unwrap();
        let state_all = self.lpc_state.read().unwrap();
        let (plan, st) = (&plan_all[ch], &state_all[ch]);
        let none = [0f64; 8];
        let coef = |n: usize| -> &[f64; 8] {
            if !on && n >= 16 {
                &none
            } else if sb < 2 {
                if n < 16 {
                    &st.last[sb]
                } else if n < 80 {
                    &plan.cur[sb][0]
                } else {
                    &plan.cur[sb][1]
                }
            } else if n < 16 {
                &st.last[2]
            } else {
                &plan.cur[2][0]
            }
        };
        let mut h: Vec<f64> = st.hist[sb].to_vec();
        let mut scf = [0usize; 8];
        let mut q: Vec<Vec<i32>> = vec![Vec::new(); 8];
        let mut err = 0.0;
        let zero = nearest(alpha, 0.0) as i32;
        for slot in 0..8 {
            let base = h.len() - 8;
            // peak of the open-loop residual picks the scale factors to try
            let mut peak = 0f64;
            {
                let mut ctx: Vec<f64> = h[base..].to_vec();
                for i in 0..16 {
                    let n = slot * 16 + i;
                    let c = coef(n);
                    let pred: f64 = -(0..8).map(|j| c[j] * ctx[ctx.len() - 1 - j]).sum::<f64>();
                    peak = peak.max((y[n] - pred).abs());
                    ctx.push(y[n]);
                }
            }
            let mut best: Option<(f64, usize, Vec<i32>, Vec<f64>)> = None;
            if peak < 1e-9 {
                // silence: zero symbols, the decoder's prediction rings on
                let mut dec = h[base..].to_vec();
                let mut e = 0.0;
                for i in 0..16 {
                    let n = slot * 16 + i;
                    let c = coef(n);
                    let pred: f64 = -(0..8).map(|j| c[j] * dec[dec.len() - 1 - j]).sum::<f64>();
                    e += (y[n] - pred).powi(2);
                    dec.push(pred);
                }
                best = Some((e, 0, vec![zero; 16], dec[8..].to_vec()));
            } else {
                let mut s_peak = 0usize;
                while s_peak < AMP_MAX - 1 && (QUANT_AMP[s_peak] * lmax) as f64 + 1e-12 < peak {
                    s_peak += 1;
                }
                for s in s_peak.saturating_sub(4)..=(s_peak + 2).min(AMP_MAX - 1) {
                    let a = QUANT_AMP[s] as f64;
                    let mut dec = h[base..].to_vec();
                    let mut syms = Vec::with_capacity(16);
                    let mut e = 0.0;
                    for i in 0..16 {
                        let n = slot * 16 + i;
                        let c = coef(n);
                        let pred: f64 = -(0..8).map(|j| c[j] * dec[dec.len() - 1 - j]).sum::<f64>();
                        let qi = nearest(alpha, ((y[n] - pred) / a) as f32);
                        let out = pred + a * alpha[qi] as f64;
                        e += (y[n] - out).powi(2);
                        syms.push(qi as i32);
                        dec.push(out);
                    }
                    if best.as_ref().is_none_or(|b| e < b.0) {
                        best = Some((e, s, syms, dec[8..].to_vec()));
                    }
                }
            }
            let (e, s, syms, dec) = best.unwrap();
            err += e;
            scf[slot] = s;
            q[slot] = syms;
            h.extend(dec);
        }
        let mut tail = [0f64; 8];
        tail.copy_from_slice(&h[h.len() - 8..]);
        (scf, q, err, tail)
    }

    /// Pick the scale factor index and the quantized symbols of one 16-sample slot.
    fn pick_scale(&self, seg: &[f64], alpha: &[f32], lmax: f32, steps: &[u8; 8]) -> (usize, Vec<i32>, f64) {
        let peak = seg.iter().fold(0f64, |m, v| m.max(v.abs()));
        if peak < 1e-9 {
            let zero = nearest(alpha, 0.0);
            return (0, vec![zero as i32; 16], 0.0);
        }
        // the smallest scale factor at which every sample fits the alphabet with its own attenuation
        let mut s_peak = 0usize;
        for (i, v) in seg.iter().enumerate() {
            let mut s = steps[i / 2] as usize;
            while s < AMP_MAX - 1 && step_gain(s, steps[i / 2]) * lmax as f64 + 1e-12 < v.abs() {
                s += 1;
            }
            s_peak = s_peak.max(s);
        }
        let mut best = (f64::MAX, s_peak, Vec::new());
        for s in s_peak.saturating_sub(5)..=(s_peak + 1).min(AMP_MAX - 1) {
            let mut err = 0.0;
            let q: Vec<i32> = seg
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let a = step_gain(s, steps[i / 2]);
                    let qi = if a > 0.0 {
                        nearest(alpha, (*v / a) as f32)
                    } else {
                        nearest(alpha, 0.0)
                    };
                    err += (v - a * alpha[qi] as f64).powi(2);
                    qi as i32
                })
                .collect();
            if err < best.0 {
                best = (err, s, q);
            }
        }
        (best.1, best.2, best.0)
    }

    /// The temporal attenuation of every group of bands from the signal's own envelope: the loudest sample pair
    /// of a slot keeps its scale factor, quieter ones are turned down in steps of 3 dB (the decoder subtracts
    /// the steps from the scale factor of the slot, and a negative result silences the pair).
    /// Grid 1 values (11 groups x 8 slots), the third grid averages and per-band offsets.
    fn grids(&self, scf: &[[usize; 8]]) -> Grids {
        let nsb = self.geo.nsb;
        let mut g1 = vec![[0i32; 8]; 12];
        for (g, row) in g1.iter_mut().enumerate().take(self.geo.ng1) {
            let sb = (GRID_1_TO_SCF[g] as usize).min(nsb - 1);
            for i in 0..8 {
                let mut m = scf[sb][i] as i32;
                for sb2 in 0..nsb {
                    if GRID_1_WEIGHTS[g][sb2] > 64 {
                        m = m.max(scf[sb2][i] as i32);
                    }
                }
                row[i] = m;
            }
        }
        let mut avg = vec![0i32; nsb];
        let mut g3 = vec![[0i32; 8]; nsb];
        for sb in 4..nsb {
            let a = SCF_TO_GRID_1[sb] as usize;
            let (w1, w2) = (GRID_1_WEIGHTS[a][sb] as i32, GRID_1_WEIGHTS[a + 1][sb] as i32);
            let mut diffs = [0i32; 8];
            for i in 0..8 {
                let interp = (w1 * g1[a][i] + w2 * g1[a + 1][i]) >> 7;
                diffs[i] = interp - scf[sb][i] as i32;
            }
            let mean = (diffs.iter().sum::<i32>() as f64 / 8.0).round() as i32;
            avg[sb - 4] = mean;
            if sb < self.g3_avg_only_start() {
                for i in 0..8 {
                    g3[sb][i] = (diffs[i] - mean).clamp(-6, 5);
                }
            }
        }
        Grids { g1, avg, g3 }
    }

    // ---------------------------------------------------------------------------------------------
    // chunk writers (the order is the order in which the decoder parses them)

    /// The four chunks of one channel pair (or of a lone channel).
    fn write_pair(&self, pair: usize, ts: &[Vec<Vec<f64>>], profile: u32, k: usize) -> Vec<Vec<u8>> {
        self.write_pair_ex(pair, ts, profile, k, false).0
    }

    /// `muted`: emergency mode, every coded sample becomes the zero symbol (keeps the stream legal when even
    /// the cheapest setting does not fit the frame).
    fn write_pair_ex(
        &self,
        pair: usize,
        ts: &[Vec<Vec<f64>>],
        profile: u32,
        k: usize,
        muted: bool,
    ) -> (Vec<Vec<u8>>, f64, Vec<(usize, [[f64; 8]; 3], [bool; 3])>) {
        let ch1 = pair * 2;
        let ch2 = (ch1 + 1).min(self.cfg.channels - 1);
        let stereo = ch1 != ch2;
        let geo = self.geo;
        let nsb = geo.nsb;
        let k = k.min(nsb);
        // the strongest of the first eight bands gets the finest quantizer (the low bits of the profile name it)
        let energy = |b: usize| -> f64 { (ch1..=ch2).map(|c| ts[c][b].iter().map(|v| v * v).sum::<f64>()).sum() };
        let loudest = (0..8.min(nsb))
            .max_by(|&a, &b| energy(a).partial_cmp(&energy(b)).unwrap())
            .unwrap_or(0);
        let profile = (profile & !7) | loudest as u32;
        let levels = levels_from_profile(profile, nsb, geo.limited_rate);
        let kw = if stereo { k.max(geo.max_mono) } else { k };
        // a lone channel codes its bands in order of energy (the quantizer levels fall with the position, so the
        // strongest bands get the finest ones); a pair keeps them in order
        let ord = if stereo {
            (0..nsb).collect::<Vec<usize>>()
        } else {
            self.band_order(&ts[ch1])
        };
        let mut pos = vec![0usize; nsb];
        for (p, &b) in ord.iter().enumerate() {
            pos[b] = p;
        }
        // the second grid stays flat: its attenuation steps did not pay for their bits
        let g2_for = |_ch: usize| -> Vec<[u8; 64]> { vec![[0u8; 64]; geo.ng2] };
        let mut c1 = self.code_channel(ch1, &ts[ch1], &levels, &pos, k, kw, g2_for(ch1));
        let mut c2 = if stereo {
            Some(self.code_channel(ch2, &ts[ch2], &levels, &pos, k, kw, g2_for(ch2)))
        } else {
            None
        };
        let tails: Vec<(usize, [[f64; 8]; 3], [bool; 3])> = std::iter::once((ch1, c1.tails, c1.lpc_used))
            .chain(c2.as_ref().map(|c| (ch2, c.tails, c.lpc_used)))
            .collect();
        let used = [c1.lpc_used, c2.as_ref().map_or([false; 3], |c| c.lpc_used)];
        let lpc_used = |ch: usize| used[ch - ch1];
        let err = c1.err + c2.as_ref().map_or(0.0, |c| c.err);
        if muted {
            for c in std::iter::once(&mut c1).chain(c2.as_mut()) {
                for sb in 0..nsb {
                    let zero = nearest(&alphabet(levels[pos[sb]]), 0.0) as i32;
                    for slot in c.q[sb].iter_mut() {
                        for v in slot.iter_mut() {
                            *v = zero;
                        }
                    }
                }
            }
        }
        let pid = pair as u8;
        let mut chunks = Vec::new();

        // ---- grid 1 chunk
        let mut w = BitsLe::new();
        for sb in 2..geo.ng1 {
            self.put_scale_factors(&mut w, &c1.grids.g1[sb]);
        }
        for sb in 0..nsb - 4 {
            self.t.avg_g3.put(&mut w, (c1.grids.avg[sb] + 16) as u32);
        }
        if stereo {
            w.put(0, 4); // min_v of both channels
            w.put(0, 4);
            for _ in 0..(nsb - geo.min_mono).div_ceil(4) * 2 * 4 {
                self.t.st_grid.put(&mut w, 0); // neutral stereo image
            }
        }
        w.put(0, 24);
        chunks.push(chunk(0x30 + pid, w));

        // ---- high resolution grid chunk
        let mut w = BitsLe::new();
        w.put(profile, 8);
        {
            let plan = self.lpc_plan.read().unwrap();
            for sb in 0..2 {
                for ch in ch1..=ch2 {
                    for set in 0..2 {
                        for code in if lpc_used(ch)[sb] {
                            plan[ch].codes[sb][set]
                        } else {
                            [8u8; 8]
                        } {
                            w.put(code as u32, 4);
                        }
                    }
                }
            }
        }
        for sb in 0..2 {
            if stereo {
                w.put(0, 8); // secondary channel flags: no mid/side
            }
            self.put_ts(&mut w, &c1.q[sb], levels[sb]);
            if let Some(c2) = &c2 {
                self.put_ts(&mut w, &c2.q[sb], levels[sb]);
            }
        }
        for sb in 0..2 {
            self.put_scale_factors(&mut w, &c1.grids.g1[sb]);
            if let Some(c2) = &c2 {
                self.put_scale_factors(&mut w, &c2.grids.g1[sb]);
            }
        }
        w.put(0, 24);
        chunks.push(chunk(0x40 + pid, w));

        // ---- time samples 1
        let mut w = BitsLe::new();
        {
            let plan = self.lpc_plan.read().unwrap();
            for ch in ch1..=ch2 {
                for code in if lpc_used(ch)[2] {
                    plan[ch].codes[2][0]
                } else {
                    [8u8; 8]
                } {
                    w.put(code as u32, 4);
                }
            }
        }
        for sb in 2..4 {
            if stereo {
                w.put(0, 8);
            }
            self.put_ts(&mut w, &c1.q[sb], levels[sb]);
        }
        self.put_grid2(&mut w, &c1.g2, 0, 1.min(geo.ng2));
        for sb in 4..6 {
            self.put_g3(&mut w, &c1.grids, sb);
            if stereo {
                w.put(0, 8);
            }
            self.put_ts(&mut w, &c1.q[sb], levels[sb]);
        }
        w.put(0, 24);
        chunks.push(chunk(0x50 + pid, w));

        // ---- time samples 2
        let mut w = BitsLe::new();
        self.put_grid2(&mut w, &c1.g2, 1, 3.min(geo.ng2));
        // third grid scale factors are sent the first time a band shows up (bands 4 and 5 came with the chunk before),
        // and all that are still missing at position 12
        let mut pres = vec![false; nsb];
        for b in 4..6.min(nsb) {
            pres[b] = true;
        }
        for p in 6..geo.max_mono.min(kw) {
            let sb = ord[p];
            w.put(sb as u32, geo.idx_bits);
            if p == 12 {
                for sg in 0..self.g3_avg_only_start().saturating_sub(4) {
                    if !pres[sg + 4] {
                        self.put_g3(&mut w, &c1.grids, sg + 4);
                        pres[sg + 4] = true;
                    }
                }
            } else if p < 12 && sb >= 4 && !pres[sb] {
                self.put_g3(&mut w, &c1.grids, sb);
                pres[sb] = true;
            }
            if stereo {
                w.put(0, 8);
            }
            self.put_ts(&mut w, &c1.q[sb], levels[p]);
        }
        if let Some(c2) = &c2 {
            // the secondary channel: first grid scale factors and third grid averages
            for sb in 2..geo.ng1 {
                self.put_scale_factors(&mut w, &c2.grids.g1[sb]);
            }
            for sb in 0..nsb - 4 {
                self.t.avg_g3.put(&mut w, (c2.grids.avg[sb] + 16) as u32);
            }
            self.put_grid2(&mut w, &c2.g2, 0, 3.min(geo.ng2));
            // second pass over the bands: the secondary channel of the mono region, both channels above it
            for sb in geo.min_mono..kw {
                let in_mono = sb < geo.max_mono;
                if sb >= 6 && !in_mono {
                    w.put(sb as u32, geo.idx_bits);
                }
                if sb == 12 {
                    for sg in 0..self.g3_avg_only_start() - 4 {
                        if sg + 4 > 11 {
                            self.put_g3(&mut w, &c2.grids, sg + 4);
                        }
                    }
                } else if (4..12).contains(&sb) {
                    self.put_g3(&mut w, &c2.grids, sb);
                }
                if !in_mono {
                    w.put(0, 8); // sbms
                }
                w.put(0, 8); // lrms
                if in_mono {
                    self.put_ts(&mut w, &c2.q[sb], levels[sb]);
                } else {
                    self.put_ts(&mut w, &c1.q[sb], levels[sb]);
                    self.put_ts(&mut w, &c2.q[sb], levels[sb]);
                }
            }
        } else {
            for p in geo.max_mono..kw {
                let sb = ord[p];
                w.put(sb as u32, geo.idx_bits);
                self.put_ts(&mut w, &c1.q[sb], levels[p]);
            }
        }
        w.put(0, 24);
        chunks.push(chunk(0x60 + pid, w));
        (chunks, err, tails)
    }

    /// Bands in the order they are coded: the first six in place, the others by falling energy.
    fn band_order(&self, x: &[Vec<f64>]) -> Vec<usize> {
        let nsb = self.geo.nsb;
        let mut rest: Vec<(f64, usize)> = (6..nsb).map(|b| (x[b].iter().map(|v| v * v).sum::<f64>(), b)).collect();
        rest.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
        (0..6.min(nsb)).chain(rest.into_iter().map(|r| r.1)).collect()
    }

    fn put_scale_factors(&self, w: &mut BitsLe, vals: &[i32; 8]) {
        // an explicit value for every slot: distance 1 steps
        self.t.fst_rsd_amp.put(w, vals[0].clamp(0, 55) as u32);
        let mut prev = vals[0].clamp(0, 55);
        for sf in 0..7 {
            self.t.rsd_apprx.put(w, 0); // distance 1
            let next = vals[sf + 1].clamp(0, 55);
            let d = next - prev;
            let code = if d > 0 { 2 * d - 1 } else { -2 * d };
            self.t.rsd_amp.put(w, code as u32);
            prev = next;
        }
    }

    /// Second grid, groups `from..to`: a flag for each run of eight steps, then the steps themselves.
    fn put_grid2(&self, w: &mut BitsLe, g2: &[[u8; 64]], from: usize, to: usize) {
        for g in from..to.min(g2.len()) {
            for run in g2[g].chunks(8) {
                if run.iter().any(|s| *s != 0) {
                    w.put(1, 1);
                    for s in run {
                        self.t.grid_2.put(w, *s as u32);
                    }
                } else {
                    w.put(0, 1); // whole group zero
                }
            }
        }
    }

    fn put_g3(&self, w: &mut BitsLe, g: &Grids, sb: usize) {
        for i in 0..8 {
            self.t.grid_3.put(w, (g.g3[sb][i] + 16) as u32);
        }
    }

    /// Time samples of one band: coding method bit, then the samples in the level's packing.
    fn put_ts(&self, w: &mut BitsLe, slots: &[Vec<i32>], level: usize) {
        let samples: Vec<i32> = slots.iter().flatten().copied().collect();
        debug_assert_eq!(samples.len(), TS);
        // three-level bands that are mostly zero are cheaper as a flag per sample (and a sign for the others):
        // 128 + nonzero bits against 205 bits for five samples in eight
        if level == 2 {
            let nonzero = samples.iter().filter(|s| **s != 1).count();
            if 128 + nonzero < 205 {
                w.put(1, 1);
                for s in &samples {
                    if *s == 1 {
                        w.put(0, 1);
                    } else {
                        w.put(1, 1);
                        w.put(u32::from(*s == 2), 1); // the level-three alphabet is -, 0, +
                    }
                }
                return;
            }
        }
        w.put(0, 1); // coding method
        match level {
            1 => {
                for blk in samples.chunks(8) {
                    let mut code = 0u32;
                    for (j, s) in blk.iter().enumerate() {
                        code |= (*s as u32 & 1) << j;
                    }
                    w.put(code, 8);
                }
            }
            2 => {
                for blk in samples.chunks(5) {
                    let mut idx = 0usize;
                    for (j, s) in blk.iter().enumerate() {
                        idx += *s as usize * 3usize.pow(j as u32);
                    }
                    for j in blk.len()..5 {
                        idx += 3usize.pow(j as u32); // pad with the zero level
                    }
                    w.put(self.t.pack5[idx].expect("5-in-8 code") as u32, 8);
                }
            }
            3 => {
                for blk in samples.chunks(3) {
                    let mut v = [2usize; 3];
                    for (j, s) in blk.iter().enumerate() {
                        v[j] = *s as usize;
                    }
                    w.put(
                        self.t.pack3[v[0] + 5 * v[1] + 25 * v[2]].expect("3-in-7 code") as u32,
                        7,
                    );
                }
            }
            4 => {
                for s in &samples {
                    let (code, len) = self.t.rsd[*s as usize].expect("rsd code");
                    put_code(w, code, len);
                }
            }
            _ => {
                for s in &samples {
                    w.put(*s as u32, 4);
                }
            }
        }
    }
}

pub(crate) fn chunk(id: u8, w: BitsLe) -> Vec<u8> {
    let data = w.into_bytes();
    let mut out = Vec::with_capacity(data.len() + 3);
    out.push(id | 0x80);
    out.extend_from_slice(&(data.len() as u16).to_be_bytes());
    out.extend_from_slice(&data);
    out
}
