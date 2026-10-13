//! DTS Express (LBR): encoder for the low bit rate extension (ETSI TS 102 114 clause 9).
//!
//! The codec works on "time samples": 32 bands of 128 samples per frame (4096 samples at 48 kHz) produced
//! by a hybrid filterbank (`lbr_fb`). Per band and 16-sample slot an amplitude (scale factor) is sent, the
//! normalised samples are quantized with one of five small alphabets chosen by a quantizer profile, and
//! bands that are not coded are replaced by noise of the right level by the decoder. Steady tones are coded
//! separately as tonal components (`lbr_tonal`). The writers below follow the order in which a decoder parses the
//! chunks.

use crate::express::bits::{BitsLe, Tables};
use crate::express::filterbank::{Analysis, HOP};
use crate::express::levels::{Geo, MIN_K};
use crate::express::tables::AVG_G3_FREQS;
use rayon::prelude::*;

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
    pub(crate) cfg: LbrConfig,
    pub(crate) t: Tables,
    pub(crate) ana: Vec<Analysis>,
    /// Source samples per channel not yet fully analysed.
    pub(crate) buf: Vec<Vec<f64>>,
    pub(crate) next_hop: usize,
    /// Samples dropped from the front of `buf` / `lfe_buf`.
    pub(crate) base: usize,
    pub(crate) lfe: Option<crate::express::lfe::LfeEncoder>,
    pub(crate) lfe_buf: Vec<f64>,
    pub(crate) lfe_base: usize,
    pub(crate) frames: usize,
    pub(crate) frame_bytes: usize,
    pub(crate) geo: Geo,
    /// Size of the previous frame's coarsest possible residual: what tones must leave room for.
    pub(crate) min_resid: std::sync::atomic::AtomicUsize,
    pub(crate) groups: Vec<crate::express::tonal::Group>,
    /// Prediction state after the last frame and the plan of the frame being coded, per channel.
    pub(crate) lpc_state: std::sync::RwLock<Vec<LpcState>>,
    pub(crate) lpc_plan: std::sync::RwLock<Vec<LpcPlan>>,
}

/// What the decoder holds of a channel's three predicted bands at the end of a frame: the coefficient set that
/// the next frame's first sixteen samples still use, and the last eight decoded samples.
#[derive(Clone, Default)]
pub(crate) struct LpcState {
    pub(crate) last: [[f64; 8]; 3],
    pub(crate) hist: [[f64; 8]; 3],
}

/// The predictor of one channel in the frame being coded: the reflection codes sent (two sets for bands 0 and
/// 1, one for band 2) and their direct-form coefficients.
#[derive(Clone, Default)]
pub(crate) struct LpcPlan {
    pub(crate) codes: [[[u8; 8]; 2]; 3],
    pub(crate) cur: [[[f64; 8]; 2]; 3],
}

/// From this bit rate on the LFE uses the 6-bit coding (17 bytes a frame more, about 1.6 kbps).
const LFE_WIDE_FROM: u32 = 256_000;

/// How far above the noise floor a spectral line has to stand before it is coded as a tonal component.
const TONE_KAPPA: f64 = 4.0;

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
    pub(crate) fn lookahead(&self) -> usize {
        if true {
            54 * HOP
        } else {
            0
        }
    }

    pub(crate) fn next_frame(&mut self) -> Vec<u8> {
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
    pub(crate) fn choose_tones(&mut self, f: usize) -> Vec<Vec<u8>> {
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

    pub(crate) fn header_len(&self) -> usize {
        5 + 11 + 3
    }

    /// The bit rate the LBR header announces: the payload after the 16-byte LBR header, as the commercial
    /// encoder counts it.
    pub(crate) fn hdr_br(&self) -> u32 {
        (((self.frame_bytes.saturating_sub(16)) as u64 * 8 * self.cfg.sample_rate as u64 / 4096) as u32) & !31
    }

    pub(crate) fn br_scale(&self) -> f64 {
        let per_ch = self.hdr_br() as f64 / self.cfg.channels as f64;
        if per_ch < 14000.0 {
            0.85
        } else if per_ch < 32000.0 {
            (per_ch - 14000.0) / 120000.0 + 0.85
        } else {
            1.0
        }
    }

    pub(crate) fn encode_frame(&self, ts: &[Vec<Vec<f64>>], lfe: Option<Vec<u8>>, tonal: Vec<Vec<u8>>) -> Vec<u8> {
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
    pub(crate) fn plan_prediction(&self, ts: &[Vec<Vec<f64>>]) {
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
    pub(crate) fn commit_prediction(&self, tails: &[(usize, [[f64; 8]; 3], [bool; 3])]) {
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
    pub(crate) fn assemble(&self, chunks: Vec<Vec<u8>>) -> Vec<u8> {
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

    pub(crate) fn g3_avg_only_start(&self) -> usize {
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

    // ---------------------------------------------------------------------------------------------
    // chunk writers (the order is the order in which the decoder parses them)
}

pub(crate) fn chunk(id: u8, w: BitsLe) -> Vec<u8> {
    let data = w.into_bytes();
    let mut out = Vec::with_capacity(data.len() + 3);
    out.push(id | 0x80);
    out.extend_from_slice(&(data.len() as u16).to_be_bytes());
    out.extend_from_slice(&data);
    out
}
