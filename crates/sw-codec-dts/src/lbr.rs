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
struct Vlc {
    codes: Vec<Option<(u32, u8)>>,
}

impl Vlc {
    fn new(table: &[(u8, u8)]) -> Vlc {
        let n = table.iter().map(|t| t.0 as usize).max().unwrap() + 1;
        Vlc { codes: codes_of(table, n) }
    }

    /// Write the value `v` (what the decoder returns, 0 or more).
    fn put(&self, w: &mut BitsLe, v: u32) {
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
    frames: usize,
    frame_bytes: usize,
}

const SUBBANDS: usize = BANDS;
const AMP_MAX: usize = 56;
const MIN_MONO: usize = 2;
const MAX_MONO: usize = 18;

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
        if cfg.channels == 0 || cfg.channels > 5 || cfg.lfe {
            return Err(crate::Error::Unsupported("DTS Express: 1 to 5 full-band channels (LFE is not implemented yet)".into()));
        }
        let frame_bytes = (cfg.bitrate as u64 * 4096 / cfg.sample_rate as u64 / 8) as usize;
        let n = cfg.channels;
        Ok(LbrEncoder { t: Tables::new(), ana: (0..n).map(|_| Analysis::new()).collect(), buf: vec![Vec::new(); n], next_hop: 0, frames: 0, frame_bytes, cfg })
    }

    pub fn frame_samples(&self) -> usize {
        4096
    }

    /// Feed any number of samples per channel; returns the LBR payloads (one per 4096 input samples) that
    /// are ready.
    pub fn push(&mut self, pcm: &[Vec<f32>]) -> Vec<Vec<u8>> {
        for (b, c) in self.buf.iter_mut().zip(pcm) {
            b.extend(c.iter().map(|v| *v as f64));
        }
        let mut out = Vec::new();
        // a frame needs hops up to 32 f + 32, i.e. samples up to 4096 f + 4096 + 256
        while self.buf[0].len() >= (self.frames + 1) * 4096 + 256 + 128 {
            out.push(self.next_frame());
        }
        out
    }

    /// Flush: pad with silence so every pushed sample is covered.
    pub fn finish(&mut self) -> Vec<Vec<u8>> {
        let pushed = self.buf[0].len();
        let frames = pushed.div_ceil(4096);
        let want = (frames + 1) * 4096 + 512;
        for b in self.buf.iter_mut() {
            b.resize(want, 0.0);
        }
        let mut out = Vec::new();
        while self.frames < frames {
            out.push(self.next_frame());
        }
        out
    }

    fn next_frame(&mut self) -> Vec<u8> {
        let f = self.frames;
        while self.next_hop <= 32 * f + 32 {
            let start = self.next_hop * HOP;
            for (c, a) in self.ana.iter_mut().enumerate() {
                let block: Vec<f64> = (0..2 * HOP).map(|i| self.buf[c].get(start + i).copied().unwrap_or(0.0)).collect();
                a.push_hop(&block);
            }
            self.next_hop += 1;
        }
        let ts: Vec<Vec<Vec<f64>>> = self.ana.iter_mut().map(|a| a.take_frame()).collect();
        self.frames += 1;
        self.encode_frame(&ts)
    }

    fn header_len(&self) -> usize {
        5 + 11 + 3
    }

    fn br_scale(&self) -> f64 {
        let per_ch = self.cfg.bitrate as f64 / self.cfg.channels as f64;
        if per_ch < 14000.0 {
            0.85
        } else if per_ch < 32000.0 {
            (per_ch - 14000.0) / 120000.0 + 0.85
        } else {
            1.0
        }
    }

    fn encode_frame(&self, ts: &[Vec<Vec<f64>>]) -> Vec<u8> {
        let nch = self.cfg.channels;
        let npairs = nch.div_ceil(2);
        let budget = self.frame_bytes.saturating_sub(self.header_len() + 8);
        // candidate profiles from the finest quantizers to the coarsest
        let mut profiles: Vec<u32> = Vec::new();
        for ol in (0..8u32).rev() {
            for st in 0..4u32 {
                profiles.push((st << 6) | (ol << 3));
            }
        }
        let mut chosen: Option<Vec<Vec<u8>>> = None;
        'search: for &p in &profiles {
            // the same profile for every pair; shrink the coded bandwidth until it fits
            for k in (18..=SUBBANDS).rev() {
                let mut chunks = Vec::new();
                for pair in 0..npairs {
                    chunks.extend(self.write_pair(pair, ts, p, k));
                }
                let bytes: usize = chunks.iter().map(|c| c.len()).sum();
                if bytes <= budget {
                    chosen = Some(chunks);
                    break 'search;
                }
                // a coarser profile is tried first when truncating would cut deeper than 24 positions
                if k <= 24 {
                    break;
                }
            }
        }
        let chunks = chosen.unwrap_or_else(|| {
            let mut c = Vec::new();
            for pair in 0..npairs {
                c.extend(self.write_pair(pair, ts, 0, 18));
            }
            c
        });
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
        out.push(0x14); // 16-bit, no band limit, no LFE, no downmix
        let br = self.cfg.bitrate;
        out.push((((br >> 16) & 0xF) | (((br >> 16) & 0xF) << 4)) as u8);
        out.extend_from_slice(&(br as u16).to_le_bytes());
        out.extend_from_slice(&(br as u16).to_le_bytes());
        let pad = self.frame_bytes.saturating_sub(out.len() + 3 + body.len());
        let len = body.len() + pad;
        out.push(0x86); // frame chunk without checksum
        out.extend_from_slice(&(len as u16).to_be_bytes());
        out.extend_from_slice(&body);
        out.resize(out.len() + pad, 0);
        out
    }

    fn g3_avg_only_start(&self) -> usize {
        let per = self.cfg.channels as u32 + 2;
        let res_profile = if self.cfg.bitrate >= 44000 * per {
            2
        } else if self.cfg.bitrate >= 25000 * per {
            1
        } else {
            0
        };
        (SUBBANDS * AVG_G3_FREQS[res_profile] as usize / 24000).min(SUBBANDS)
    }

    /// Quantize one channel for a set of levels. Bands at or above `k` are not coded; they get the scale
    /// factor that makes the decoder's noise match their level.
    fn code_channel(&self, x: &[Vec<f64>], levels: &[usize], k: usize) -> ChanCoding {
        let mut q = vec![vec![Vec::new(); 8]; SUBBANDS];
        let mut scf = vec![[0usize; 8]; SUBBANDS];
        let noise_gain = 0.453 * self.br_scale();
        for sb in 0..SUBBANDS {
            let alpha = alphabet(levels[sb]);
            let lmax = alpha.iter().fold(0f32, |m, a| m.max(a.abs()));
            for slot in 0..8 {
                let seg = &x[sb][slot * 16..slot * 16 + 16];
                if sb < k {
                    let (s, qq) = self.pick_scale(seg, &alpha, lmax);
                    scf[sb][slot] = s;
                    q[sb][slot] = qq;
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
        let mut g1 = vec![[0i32; 8]; 12];
        for (g, row) in g1.iter_mut().enumerate().take(11) {
            let sb = (GRID_1_TO_SCF[g] as usize).min(SUBBANDS - 1);
            for i in 0..8 {
                let mut m = scf[sb][i] as i32;
                for sb2 in 0..SUBBANDS {
                    if GRID_1_WEIGHTS[g][sb2] > 64 {
                        m = m.max(scf[sb2][i] as i32);
                    }
                }
                row[i] = m;
            }
        }
        let mut avg = vec![0i32; SUBBANDS];
        let mut g3 = vec![[0i32; 8]; SUBBANDS];
        for sb in 4..SUBBANDS {
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
        let ch1 = pair * 2;
        let ch2 = (ch1 + 1).min(self.cfg.channels - 1);
        let stereo = ch1 != ch2;
        let levels = levels_from_profile(profile, SUBBANDS, 48_000);
        let c1 = self.code_channel(&ts[ch1], &levels, k);
        let c2 = if stereo { Some(self.code_channel(&ts[ch2], &levels, k)) } else { None };
        let pid = pair as u8;
        let mut chunks = Vec::new();

        // ---- grid 1 chunk
        let mut w = BitsLe::new();
        for sb in 2..11 {
            self.put_scale_factors(&mut w, &c1.grids.g1[sb]);
        }
        for sb in 0..SUBBANDS - 4 {
            self.t.avg_g3.put(&mut w, (c1.grids.avg[sb] + 16) as u32);
        }
        if stereo {
            w.put(0, 4); // min_v of both channels
            w.put(0, 4);
            for _ in 0..(SUBBANDS - MIN_MONO).div_ceil(4) * 2 * 4 {
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
        self.put_grid2(&mut w, 1);
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
        self.put_grid2(&mut w, 2);
        for sb in 6..MAX_MONO.min(k) {
            w.put(sb as u32, 5);
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
            for sb in 2..11 {
                self.put_scale_factors(&mut w, &c2.grids.g1[sb]);
            }
            for sb in 0..SUBBANDS - 4 {
                self.t.avg_g3.put(&mut w, (c2.grids.avg[sb] + 16) as u32);
            }
            self.put_grid2(&mut w, 3);
            // second pass over the bands: the secondary channel of the mono region, both channels above it
            for sb in MIN_MONO..k {
                let in_mono = sb < MAX_MONO;
                if sb >= 6 && !in_mono {
                    w.put(sb as u32, 5);
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
            for sb in MAX_MONO..k {
                w.put(sb as u32, 5);
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

fn chunk(id: u8, w: BitsLe) -> Vec<u8> {
    let data = w.into_bytes();
    let mut out = Vec::with_capacity(data.len() + 3);
    out.push(id | 0x80);
    out.extend_from_slice(&(data.len() as u16).to_be_bytes());
    out.extend_from_slice(&data);
    out
}
