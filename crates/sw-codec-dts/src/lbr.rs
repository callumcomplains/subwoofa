//! DTS Express (LBR): encoder for the low bit rate extension (ETSI TS 102 114 clause 9).
//!
//! The codec works on "time samples": 32 bands of 128 samples per frame (4096 samples at 48 kHz) produced
//! by a hybrid filterbank (`lbr_fb`). Per band and 16-sample slot an amplitude (scale factor) is sent, the
//! normalised samples are quantized with one of five small alphabets chosen by a quantizer profile, and
//! bands that are not coded are replaced by noise of the right level by the decoder. This encoder has no
//! tonal component coding. The writers below follow the order in which a decoder parses the chunks.

use crate::lbr_fb::{Analysis, BANDS, HOP, TS};
use crate::lbr_tables::*;

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
        let v = if bits == 32 { v as u64 } else { (v as u64) & ((1u64 << bits) - 1) };
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
        if slot.map_or(true, |(_, l)| len < l) {
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
        Vlc { codes: codes_of(table, n) }
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
            rsd: codes_of(&HUFF_RSD, 8),
            pack5,
            pack3,
        }
    }
}

/// Quantizer alphabets by level (1..=5): the values the decoder reconstructs before the scale factor.
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
    lfe: Option<crate::lbr_lfe::LfeEncoder>,
    lfe_buf: Vec<f64>,
    lfe_base: usize,
    frames: usize,
    frame_bytes: usize,
    geo: Geo,
    /// Size of the previous frame's coarsest possible residual: what tones must leave room for.
    min_resid: std::cell::Cell<usize>,
    /// Tonal components on (Express streams of the decoder's tone model).
    tonal: bool,
    groups: Vec<crate::lbr_tonal::Group>,
}

const SUBBANDS: usize = BANDS;

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
/// Narrowest coded bandwidth (in bands) the rate control may fall back to.
const MIN_K: usize = 6;

/// Everything derived from the time samples of one channel for one frame and quantizer profile.
struct ChanCoding {
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
            return Err(crate::Error::Unsupported("DTS Express: 1 to 5 full-band channels".into()));
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
        let frame_bytes = (cfg.bitrate as u64 * 4096 / cfg.sample_rate as u64 / 16) as usize * 2;
        let n = cfg.channels;
        let per_channel = cfg.bitrate as f64 / 1000.0 / cfg.channels as f64;
        let band_limit = std::env::var("SW_LBR_BL").ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(if per_channel >= 128.0 { 0 } else if per_channel >= 56.0 { 1 } else { 2 });
        let geo = Geo::new(cfg.sample_rate, band_limit);
        // the decoder scales a band-limited stream's filter bank output by sqrt(2^(2 - limited range)), so the
        // coefficients are scaled down by the same factor here
        let gain = if band_limit > 0 { 1.0 / ((1u32 << band_limit) as f64).sqrt() } else { 1.0 };
        let ana: Vec<Analysis> = (0..n).map(|_| { let mut a = Analysis::new(); a.set_gain(gain); a }).collect();
        Ok(LbrEncoder { t: Tables::new(), ana, buf: vec![Vec::new(); n], next_hop: 0, base: 0, lfe: cfg.lfe.then(|| crate::lbr_lfe::LfeEncoder::new(cfg.sample_rate)), lfe_buf: Vec::new(), lfe_base: 0, frames: 0, frame_bytes, geo, min_resid: std::cell::Cell::new(0), tonal: std::env::var_os("SW_LBR_NOTONES").is_none(), groups: (0..5).map(crate::lbr_tonal::Group::new).collect(), cfg })
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
        if self.tonal { 54 * HOP } else { 0 }
    }

    fn next_frame(&mut self) -> Vec<u8> {
        let f = self.frames;
        let need = 32 * f + 32 + if self.tonal { 54 } else { 0 };
        while self.next_hop <= need {
            let start = self.next_hop * HOP - self.base;
            for (c, a) in self.ana.iter_mut().enumerate() {
                let block: Vec<f64> = (0..2 * HOP).map(|i| self.buf[c].get(start + i).copied().unwrap_or(0.0)).collect();
                a.compute_hop(&block);
            }
            self.next_hop += 1;
        }
        let tonal_chunks = if self.tonal { self.choose_tones(f) } else { Vec::new() };
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
                if p < 0 { 0.0 } else { buf.get(p as usize).copied().unwrap_or(0.0) }
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
        use crate::lbr_tonal as tn;
        let nch = self.cfg.channels;
        let lr = 2 - self.geo.band_limit as i32;
        let kappa = std::env::var("SW_LBR_KAPPA").ok().and_then(|v| v.parse().ok()).unwrap_or(4.0);
        let max_g = std::env::var("SW_LBR_GROUPS").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(5);
        let search = tn::Search { max_line: (4 * self.geo.nsb).saturating_sub(6), kappa, max_tones: 12, limited_range: lr };
        let mut all: Vec<Vec<Vec<tn::Tone>>> = Vec::new();
        // at most a quarter of the frame goes to tones, and never so much that the coarsest residual of the
        // previous frame (plus headers) no longer fits
        let room = self.frame_bytes as i64 - (self.header_len() as i64 + 8 + (self.min_resid.get() as f64 * 1.15) as i64 + if self.cfg.lfe { 60 } else { 0 });
        let mut bits_left: i64 = ((self.frame_bytes as i64 * 8) / 4).min(room.max(0) * 8 / 2);
        for g in 0..5usize {
            let mut subs: Vec<Vec<tn::Tone>> = Vec::new();
            for j in 0..(1usize << g) {
                if g >= max_g {
                    subs.push(Vec::new());
                    continue;
                }
                let grp = &self.groups[g];
                let first = 32 * f + tn::TONE_DELAY + j * grp.steps;
                let mut targets: Vec<Vec<[f64; HOP]>> = (0..nch)
                    .map(|ch| (0..2 * grp.steps).map(|r| self.ana[ch].coefs_mut(first + r).map(|c| *c).unwrap_or([0.0; HOP])).collect())
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
        let budget = self.frame_bytes.saturating_sub(self.header_len() + 8 + lfe.as_ref().map_or(0, |c| c.len()) + tonal_bytes);
        // candidate profiles from the finest quantizers to the coarsest
        let mut profiles: Vec<u32> = Vec::new();
        for ol in (0..8u32).rev() {
            for st in 0..4u32 {
                profiles.push((st << 6) | (ol << 3));
            }
        }
        // candidates from the finest to the coarsest; the size is close to monotone along the list, so a
        // binary search finds the first one that fits
        let mut cands: Vec<(u32, usize)> = Vec::new();
        for &p in &profiles {
            for k in (MIN_K..=self.geo.nsb).rev() {
                cands.push((p, k));
            }
        }
        if let Some(&(p, k)) = cands.last() {
            let mut n = 0usize;
            for pair in 0..npairs {
                n += self.write_pair(pair, ts, p, k).iter().map(|c| c.len()).sum::<usize>();
            }
            self.min_resid.set(n);
        }
        let try_one = |&(p, k): &(u32, usize)| -> Option<Vec<Vec<u8>>> {
            let mut chunks = Vec::new();
            for pair in 0..npairs {
                chunks.extend(self.write_pair(pair, ts, p, k));
            }
            let bytes: usize = chunks.iter().map(|c| c.len()).sum();
            (bytes <= budget).then_some(chunks)
        };
        if let Ok(v) = std::env::var("SW_LBR_FIXED") {
            let mut it = v.split(',').map(|s| s.parse::<usize>().unwrap());
            let (p, k) = (it.next().unwrap() as u32, it.next().unwrap());
            let mut chunks = Vec::new();
            for pair in 0..npairs {
                chunks.extend(self.write_pair(pair, ts, p, k));
            }
            if std::env::var_os("SW_LBR_DEBUG").is_some() {
                eprintln!("fixed bytes {} chunks {:?}", chunks.iter().map(|c| c.len()).sum::<usize>(), chunks.iter().map(|c| c.len()).collect::<Vec<_>>());
            }
            chunks.extend(tonal);
            return self.assemble(chunks);
        }
        let (mut lo, mut hi) = (0usize, cands.len());
        let mut chosen: Option<Vec<Vec<u8>>> = None;
        while lo < hi {
            let mid = (lo + hi) / 2;
            match try_one(&cands[mid]) {
                Some(c) => {
                    chosen = Some(c);
                    hi = mid;
                }
                None => lo = mid + 1,
            }
        }
        let chunks = chosen.unwrap_or_else(|| {
            let mut c = Vec::new();
            for pair in 0..npairs {
                c.extend(self.write_pair_ex(pair, ts, 0, MIN_K, true));
            }
            c
        });
        let mut chunks = chunks;
        chunks.extend(lfe);
        let used: usize = chunks.iter().map(|c| c.len()).sum();
        if used + tonal_bytes + self.header_len() + 8 <= self.frame_bytes {
            chunks.extend(tonal);
        }
        self.assemble(chunks)
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
        out.push(match self.geo.band_limit { 0 => 0x14, 1 => 0x08, _ => 0x10 } | 0x01 | if self.cfg.lfe { 0x02 } else { 0 }); // band limit, LFE; bit 0 is set in every commercial stream seen
        let br = self.hdr_br();
        out.push((((br >> 16) & 0xF) | (((br >> 16) & 0xF) << 4)) as u8);
        out.extend_from_slice(&(br as u16).to_le_bytes());
        out.extend_from_slice(&(br as u16).to_le_bytes());
        // frame chunk with its 16-bit checksum: the id byte, both length bytes and every byte after the checksum
        if std::env::var_os("SW_LBR_DEBUG").is_some() && out.len() + 5 + body.len() > self.frame_bytes {
            eprintln!("frame {} overshoots: {} bytes against {}", self.frames, out.len() + 5 + body.len(), self.frame_bytes);
        }
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
    fn code_channel(&self, x: &[Vec<f64>], levels: &[usize], k: usize, kw: usize) -> ChanCoding {
        let nsb = self.geo.nsb;
        let mut q = vec![vec![Vec::new(); 8]; nsb];
        let mut scf = vec![[0usize; 8]; nsb];
        let noise_gain = 0.453 * self.br_scale();
        for sb in 0..nsb {
            let alpha = alphabet(levels[sb]);
            let lmax = alpha.iter().fold(0f32, |m, a| m.max(a.abs()));
            for slot in 0..8 {
                let seg = &x[sb][slot * 16..slot * 16 + 16];
                if sb < k {
                    let (s, qq) = self.pick_scale(seg, &alpha, lmax);
                    scf[sb][slot] = s;
                    q[sb][slot] = qq;
                } else if sb < kw {
                    scf[sb][slot] = 0;
                    let zero = nearest(&alpha, 0.0) as i32;
                    q[sb][slot] = vec![zero; 16];
                } else {
                    // decoder noise: uniform, rms 0.453 * br scale * amplitude
                    let rms = (seg.iter().map(|v| v * v).sum::<f64>() / 16.0).sqrt();
                    let want = rms / noise_gain;
                    let mut s = 0usize;
                    while s < AMP_MAX - 1 && (QUANT_AMP[s] as f64) < want {
                        s += 1;
                    }
                    scf[sb][slot] = if want < 1e-6 { 0 } else { s };
                    q[sb][slot] = Vec::new();
                }
            }
        }
        ChanCoding { q, grids: self.grids(&scf) }
    }

    /// Pick the scale factor index and the quantized symbols of one 16-sample slot.
    fn pick_scale(&self, seg: &[f64], alpha: &[f32], lmax: f32) -> (usize, Vec<i32>) {
        let peak = seg.iter().fold(0f64, |m, v| m.max(v.abs()));
        if peak < 1e-9 {
            let zero = nearest(alpha, 0.0);
            return (0, vec![zero as i32; 16]);
        }
        let mut s_peak = 0usize;
        while s_peak < AMP_MAX - 1 && (QUANT_AMP[s_peak] * lmax) as f64 + 1e-12 < peak {
            s_peak += 1;
        }
        let mut best = (f64::MAX, s_peak, Vec::new());
        for s in s_peak.saturating_sub(5)..=(s_peak + 1).min(AMP_MAX - 1) {
            let a = QUANT_AMP[s] as f64;
            let q: Vec<i32> = seg.iter().map(|v| nearest(alpha, (*v / a) as f32) as i32).collect();
            let err: f64 = seg.iter().zip(&q).map(|(v, qi)| (v - a * alpha[*qi as usize] as f64).powi(2)).sum();
            if err < best.0 {
                best = (err, s, q);
            }
        }
        (best.1, best.2)
    }

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
        self.write_pair_ex(pair, ts, profile, k, false)
    }

    /// `muted`: emergency mode, every coded sample becomes the zero symbol (keeps the stream legal when even
    /// the cheapest setting does not fit the frame).
    fn write_pair_ex(&self, pair: usize, ts: &[Vec<Vec<f64>>], profile: u32, k: usize, muted: bool) -> Vec<Vec<u8>> {
        let ch1 = pair * 2;
        let ch2 = (ch1 + 1).min(self.cfg.channels - 1);
        let stereo = ch1 != ch2;
        let geo = self.geo;
        let nsb = geo.nsb;
        let k = k.min(nsb);
        let levels = levels_from_profile(profile, nsb, geo.limited_rate);
        let kw = if stereo { k.max(geo.max_mono) } else { k };
        let mut c1 = self.code_channel(&ts[ch1], &levels, k, kw);
        let mut c2 = if stereo { Some(self.code_channel(&ts[ch2], &levels, k, kw)) } else { None };
        if muted {
            for c in std::iter::once(&mut c1).chain(c2.as_mut()) {
                for sb in 0..nsb {
                    let zero = nearest(&alphabet(levels[sb]), 0.0) as i32;
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
        for _ in 0..2 {
            for _ in 0..(if stereo { 2 } else { 1 }) {
                for _ in 0..16 {
                    w.put(8, 4); // no prediction
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
        for _ in 0..(if stereo { 2 } else { 1 }) {
            for _ in 0..8 {
                w.put(8, 4);
            }
        }
        for sb in 2..4 {
            if stereo {
                w.put(0, 8);
            }
            self.put_ts(&mut w, &c1.q[sb], levels[sb]);
        }
        self.put_grid2(&mut w, 1.min(geo.ng2));
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
        self.put_grid2(&mut w, 3.min(geo.ng2).saturating_sub(1));
        for sb in 6..geo.max_mono.min(kw) {
            w.put(sb as u32, geo.idx_bits);
            if sb == 12 {
                for sg in 0..self.g3_avg_only_start() - 4 {
                    if sg + 4 > 11 {
                        self.put_g3(&mut w, &c1.grids, sg + 4);
                    }
                }
            } else if sb < 12 {
                self.put_g3(&mut w, &c1.grids, sb);
            }
            if stereo {
                w.put(0, 8);
            }
            self.put_ts(&mut w, &c1.q[sb], levels[sb]);
        }
        if let Some(c2) = &c2 {
            // the secondary channel: first grid scale factors and third grid averages
            for sb in 2..geo.ng1 {
                self.put_scale_factors(&mut w, &c2.grids.g1[sb]);
            }
            for sb in 0..nsb - 4 {
                self.t.avg_g3.put(&mut w, (c2.grids.avg[sb] + 16) as u32);
            }
            self.put_grid2(&mut w, 3.min(geo.ng2));
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
                } else if sb < 12 && sb >= 4 {
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
            for sb in geo.max_mono..kw {
                w.put(sb as u32, geo.idx_bits);
                self.put_ts(&mut w, &c1.q[sb], levels[sb]);
            }
        }
        w.put(0, 24);
        chunks.push(chunk(0x60 + pid, w));
        chunks
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

    fn put_grid2(&self, w: &mut BitsLe, groups: usize) {
        for _ in 0..groups {
            for _ in 0..8 {
                w.put(0, 1); // whole group zero
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
                    w.put(self.t.pack3[v[0] + 5 * v[1] + 25 * v[2]].expect("3-in-7 code") as u32, 7);
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
