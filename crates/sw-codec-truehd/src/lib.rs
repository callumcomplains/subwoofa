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
mod coding;
mod downmix;
mod filter;
mod layout;
pub mod native;
mod predict;
mod substream;
mod sync;
#[rustfmt::skip]
pub mod tables;

pub use downmix::{default_downmix, Downmix};
pub use layout::{map_layout, map_layout_mlp, LayoutMap, Mlp};

use bits::parity;
use downmix::{apply_downmix, coarse_quant};
use predict::{analyze_with_prediction, ChannelAnalysis, Tools};
use std::borrow::Cow;
use substream::{substream_layout, write_substream_au, DecState, MatrixSpec, Substream};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Unsupported(String),
}

pub const MAX_CHANNELS: usize = 8;
const MAX_ORDER: usize = 8;
/// Access units per restart interval (major sync spacing).
pub const DEFAULT_RESTART_INTERVAL: usize = 128;

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
    /// How many predictor families the analysis tries.
    pub effort: sw_core::Effort,
    /// Write MLP (DVD-Audio style) instead of TrueHD: one substream, no downmix, up to 6 channels.
    pub mlp: Option<Mlp>,
}

/// One restart interval of input for `Encoder::encode_intervals`: the samples per coded channel (at least as long as
/// the interval), how many of them are real, and whether it ends the stream.
#[derive(Clone, Copy)]
pub struct Interval<'a> {
    pub chans: &'a [Vec<i32>],
    pub valid: usize,
    pub last: bool,
}

pub struct Encoder {
    pub(crate) cfg: Config,
    au: usize,
    pub(crate) subs: Vec<Substream>,
    pub(crate) rate_code: u32,
    /// Access units written so far; the timing fields of the next one follow from it.
    aus_done: usize,
    /// Running XOR over output samples of the last interval written, per substream.
    lossless: Vec<u32>,
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
            r => {
                return Err(Error::Unsupported(format!(
                    "TrueHD supports 44.1/48/88.2/96/176.4/192 kHz, not {r} Hz"
                )))
            }
        };
        if !(1..=MAX_CHANNELS).contains(&cfg.channels) {
            return Err(Error::Unsupported(format!("{} channels", cfg.channels)));
        }
        if !(8..=128).contains(&cfg.restart_interval) {
            return Err(Error::Unsupported(
                "restart interval must be 8 to 128 access units".into(),
            ));
        }
        let n = cfg.channels;
        if cfg.mlp.is_some() && (cfg.downmix.is_some() || n > 6) {
            return Err(Error::Unsupported(
                "MLP carries up to 6 channels in one substream, without a downmix".into(),
            ));
        }
        if let Some(dm) = &cfg.downmix {
            if !(1 << 14..=32767).contains(&dm.dest) {
                return Err(Error::Unsupported(
                    "downmix gain must be between 16384 and 32767 (2.14)".into(),
                ));
            }
            if n <= 2 || dm.lo.len() != n || dm.ro.len() != n {
                return Err(Error::Unsupported(
                    "downmix needs more than two channels and one coefficient per channel".into(),
                ));
            }
            if n > 6 && n != 8 {
                return Err(Error::Unsupported(
                    "the stereo downmix supports up to 6 channels, or exactly 8".into(),
                ));
            }
        }
        let subs = substream_layout(&cfg);
        let nsub = subs.len();
        Ok(Encoder {
            au,
            subs,
            rate_code,
            aus_done: 0,
            lossless: vec![0; nsub],
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

    /// Encode one restart interval. `chans` holds one vector per coded channel (TrueHD order),
    /// each at least `valid.div_ceil(au) * au` samples long (zero padded). `valid` is the number of
    /// real samples; if `last` the final access unit is trimmed back to it on decode.
    pub fn encode_interval(&mut self, chans: &[Vec<i32>], valid: usize, last: bool) -> Vec<u8> {
        let interval = Interval { chans, valid, last };
        self.encode_intervals(std::slice::from_ref(&interval)).pop().unwrap()
    }

    /// Encodes consecutive restart intervals together. Each one starts the decoder afresh, so what it needs from
    /// the ones before it is only where its access units sit in time and a checksum of the previous interval's
    /// samples; both are known from the input. The intervals are coded on all cores, and the bytes are the same
    /// as encoding them one at a time.
    pub fn encode_intervals(&mut self, intervals: &[Interval]) -> Vec<Vec<u8>> {
        use rayon::prelude::*;
        let au = self.au;
        let mut starts = Vec::with_capacity(intervals.len());
        let mut checks = Vec::with_capacity(intervals.len());
        let (mut aus, mut check) = (self.aus_done, self.lossless.clone());
        for iv in intervals {
            starts.push(aus);
            checks.push(check.clone());
            check = self.samples_check(iv.chans, iv.valid.div_ceil(au).max(1) * au);
            aus += iv.valid.div_ceil(au).max(1);
        }
        let me = &*self;
        let done: Vec<(Vec<u8>, u64)> = intervals
            .par_iter()
            .zip(starts.par_iter().zip(checks.par_iter()))
            .map(|(iv, (&first_au, check))| me.interval(iv, first_au, check))
            .collect();
        self.aus_done = aus;
        self.lossless = check;
        done.into_iter()
            .map(|(bytes, overflows)| {
                self.downmix_overflows += overflows;
                bytes
            })
            .collect()
    }

    /// The running XOR of the output samples of an interval, per substream (what the next restart header repeats).
    fn samples_check(&self, chans: &[Vec<i32>], n: usize) -> Vec<u32> {
        self.subs
            .iter()
            .map(|sub| {
                let mut x = 0u32;
                for ch in 0..=sub.max {
                    for v in &chans[ch][..n] {
                        x ^= ((*v as u32) & 0xFF_FFFF) << ch;
                    }
                }
                x
            })
            .collect()
    }

    /// One restart interval as access units, and how many of its stereo-presentation samples overflow 24 bits.
    /// `first_au` counts the access units before it; `check` is `samples_check` of the interval before.
    fn interval(&self, iv: &Interval, first_au: usize, check: &[u32]) -> (Vec<u8>, u64) {
        let Interval { chans, valid, last } = *iv;
        assert_eq!(chans.len(), self.cfg.channels);
        let n_au = valid.div_ceil(self.au).max(1);
        let n = n_au * self.au;
        for c in chans {
            assert!(c.len() >= n, "channel buffer shorter than the interval");
        }
        let domain = self.coded_domain(chans, n);
        let analyses = self.analyse(&domain, n);
        let plan = IntervalPlan {
            out_q: self.matrix_steps(&domain, &analyses),
            sub_mats: self.matrices(&domain, &analyses),
            analyses,
        };
        let place = Placement {
            n_au,
            first_au,
            check,
            shorten: last.then_some(n - valid),
        };
        let bytes = self.write_access_units(chans, &domain, &plan, &place);
        (bytes, domain.overflows)
    }

    /// The channels as they are coded. With a downmix, coded channels 0 and 1 carry Lo/Ro (and 4/5 the folded
    /// surrounds of 7.1); without one they are the source.
    fn coded_domain<'a>(&self, chans: &'a [Vec<i32>], n: usize) -> Domain<'a> {
        let nch = self.cfg.channels;
        let downmix = self.cfg.downmix.as_ref();
        let unused_low_bits = |c: usize| -> u8 {
            let or_all = chans[c][..n].iter().fold(0i32, |a, v| a | v);
            if or_all == 0 {
                0
            } else {
                (or_all.trailing_zeros() as u8).min(15)
            }
        };
        // Sources with unused low bits (16/18/20-bit material in 24-bit files) get a coarse downmix: the
        // decoder snaps the reconstructed front channels to their own grid, so the stereo channels need
        // not carry those bits.
        let coarse = downmix.is_some() && nch <= 6;
        let src_q: [u8; 2] = if coarse {
            [unused_low_bits(0), unused_low_bits(1)]
        } else {
            [0, 0]
        };
        let mut bypass: [Vec<u8>; 2] = [Vec::new(), Vec::new()];
        // Unity mode: the coded pair is the full-level downmix on the source's grid and substream 0 scales it
        // to the stereo presentation with matrices; that keeps the pair free to be decorrelated.
        let mut unity = false;
        let coded_store: Option<Vec<Vec<i32>>> = downmix.map(|dm| {
            if coarse && src_q[0] >= 1 && src_q[1] >= 1 {
                let mut full_level = dm.clone();
                full_level.dest = 1 << 14;
                let (c, b) = apply_downmix(chans, n, &full_level, nch, src_q);
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
        let coded = match coded_store {
            Some(c) => Cow::Owned(c),
            None => Cow::Borrowed(chans),
        };
        let overflows = if downmix.is_some() {
            coded[0][..n]
                .iter()
                .chain(coded[1][..n].iter())
                .filter(|v| v.unsigned_abs() >= 1 << 23)
                .count() as u64
        } else {
            0
        };
        // quantizer step of the stereo channels as coded in substream 0, and whether the front matrices need bypass bits
        let dest = downmix.map_or(1 << 14, |d| d.dest);
        let eff_dest = if unity { 1 << 14 } else { dest };
        let stereo_q: [Option<u8>; 2] = [0, 1].map(|k| {
            if coarse && src_q[k] >= 1 {
                Some(coarse_quant(eff_dest, src_q[k]))
            } else {
                None
            }
        });
        let need_bypass: [bool; 2] = [0, 1].map(|k| downmix.is_some() && stereo_q[k].is_none() && eff_dest != 1 << 14);
        // channels written by a static matrix keep a zero quantizer step (except the coarse stereo pair)
        let forced_q: Vec<Option<u8>> = (0..nch)
            .map(|c| {
                if c < 2 {
                    downmix.map(|_| stereo_q[c].unwrap_or(0))
                } else if self.subs.iter().any(|s| s.matrices.iter().any(|m| m.out == c)) {
                    Some(0)
                } else {
                    None
                }
            })
            .collect();
        Domain {
            coded,
            src_q,
            unity,
            dest,
            eff_dest,
            bypass,
            need_bypass,
            forced_q,
            overflows,
        }
    }

    /// Per channel: the quantizer step, the predictor and the residual, with cross-channel prediction where it pays.
    fn analyse(&self, domain: &Domain, n: usize) -> Vec<ChannelAnalysis> {
        let decorrelate = self.cfg.decorrelate && self.cfg.effort != sw_core::Effort::Fast;
        let tools = Tools::for_effort(self.cfg.effort);
        let both: Vec<(ChannelAnalysis, Option<(ChannelAnalysis, usize)>)> = {
            use rayon::prelude::*;
            (0..self.cfg.channels)
                .into_par_iter()
                .map(|c| {
                    let forced = domain.forced_q[c];
                    // every channel may be predicted from those below it, the stereo pair of a downmix included (channel 0 has
                    // nothing below it, and the analysis knows)
                    let allow = decorrelate;
                    analyze_with_prediction(&domain.coded, c, n, self.au, forced, allow, tools)
                })
                .collect()
        };
        // decoders accept at most 8 primitive matrices per substream: keep the predictions that save most
        let static_max = self.subs.iter().map(|s| s.matrices.len()).max().unwrap_or(0);
        let budget = 8usize.saturating_sub(static_max);
        let mut order: Vec<usize> = (0..both.len()).filter(|&c| both[c].1.is_some()).collect();
        order.sort_by_key(|&c| std::cmp::Reverse(both[c].1.as_ref().map_or(0, |p| p.1)));
        let keep: Vec<usize> = order.into_iter().take(budget).collect();
        both.into_iter()
            .enumerate()
            .map(|(c, (base, with))| match with {
                Some((w, _)) if keep.contains(&c) => w,
                _ => base,
            })
            .collect()
    }

    /// The quantizer step of each channel as the matrices see it (the mask on a matrix output).
    fn matrix_steps(&self, domain: &Domain, analyses: &[ChannelAnalysis]) -> Vec<u8> {
        (0..self.cfg.channels)
            .map(|c| {
                if c < 2 && self.cfg.downmix.is_some() {
                    domain.src_q[c]
                } else {
                    analyses[c].quant
                }
            })
            .collect()
    }

    /// The primitive matrices of each substream: cross-channel predictions (ascending channel order), then the
    /// downmix.
    fn matrices(&self, domain: &Domain, analyses: &[ChannelAnalysis]) -> Vec<Vec<MatrixSpec>> {
        self.subs
            .iter()
            .map(|sub| {
                let mut list: Vec<MatrixSpec> = (1..=sub.max)
                    .filter_map(|k| {
                        analyses[k].decor.as_ref().map(|c| {
                            let mut coeff = c.clone();
                            coeff.resize(sub.max + 1, 0);
                            MatrixSpec {
                                out: k,
                                coeff,
                                lsb_bypass: false,
                            }
                        })
                    })
                    .collect();
                list.extend(sub.matrices.iter().map(|m| {
                    let mut m = m.clone();
                    if m.out < 2 {
                        m.lsb_bypass = domain.need_bypass[m.out];
                        m.coeff[m.out] = domain.eff_dest;
                    }
                    m
                }));
                if domain.unity && sub.min == 0 {
                    // stereo presentation: the coded full-level pair scaled by 1/dest (the half-level default)
                    let g = (((1i64 << 28) + domain.dest as i64 / 2) / domain.dest as i64) as i32;
                    for k in 0..2 {
                        let mut coeff = vec![0; sub.max + 1];
                        coeff[k] = g;
                        list.push(MatrixSpec {
                            out: k,
                            coeff,
                            lsb_bypass: false,
                        });
                    }
                }
                list
            })
            .collect()
    }

    /// Every access unit of the interval: each substream's data, then the header that ties them together.
    fn write_access_units(
        &self,
        chans: &[Vec<i32>],
        domain: &Domain,
        plan: &IntervalPlan,
        place: &Placement,
    ) -> Vec<u8> {
        let Placement {
            n_au,
            first_au,
            check,
            shorten,
        } = *place;
        let au = self.au;
        let mut out = Vec::new();
        let mut states: Vec<DecState> = vec![DecState::restart(); self.subs.len()];
        // restart header values, taken before this interval's samples accumulate
        let check_for_header: Vec<u8> = check.iter().map(|v| fold32(*v)).collect();
        let au_timing = |k: usize| (k * au) as u16;
        let max_bits: u32 = chans
            .iter()
            .flat_map(|c| c[..n_au * au].iter())
            .map(|v| number_sbits(*v))
            .max()
            .unwrap_or(1)
            .min(24);

        for a in 0..n_au {
            let restart = a == 0;
            let mut sub_data: Vec<Vec<u8>> = Vec::new();
            for (si, sub) in self.subs.iter().enumerate() {
                let shorten_here = shorten.filter(|_| a == n_au - 1);
                let data = write_substream_au(
                    sub,
                    &plan.sub_mats[si],
                    &plan.out_q,
                    &plan.analyses,
                    &mut states[si],
                    a,
                    au,
                    restart,
                    shorten_here,
                    self.cfg.mlp.is_some(),
                    au_timing(first_au + a),
                    check_for_header[si],
                    max_bits,
                    &domain.bypass,
                );
                sub_data.push(data);
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
            let timing = au_timing(first_au + a).wrapping_sub(au as u16);
            let mut head = [0u8; 4];
            head[0] = (words >> 8) as u8 & 0x0F;
            head[1] = words as u8;
            head[2..4].copy_from_slice(&timing.to_be_bytes());
            // parity nibble: the XOR of all nibbles of the header and substream headers must be 0xF
            let mut x = parity(&head) ^ parity(&sub_headers);
            x = (x >> 4) ^ (x & 0xF);
            head[0] |= (x ^ 0xF) << 4;
            out.extend_from_slice(&head);
            out.extend_from_slice(&body);
            out.extend_from_slice(&sub_headers);
            for d in sub_data {
                out.extend_from_slice(&d);
            }
        }
        out
    }
}

/// How the stereo presentation shapes the coding of one interval.
struct Domain<'a> {
    /// The channels as coded: the source, or with Lo/Ro in channels 0 and 1.
    coded: Cow<'a, [Vec<i32>]>,
    /// Unused low bits of the two front source channels (0 without a coarse downmix).
    src_q: [u8; 2],
    /// The coded pair is the full-level downmix and substream 0 scales it with matrices.
    unity: bool,
    /// Gain of the front channels in the decoder, as configured and as used in this interval.
    dest: i32,
    eff_dest: i32,
    /// The stored bits that absorb the rounding of Lo/Ro, per front channel.
    bypass: [Vec<u8>; 2],
    need_bypass: [bool; 2],
    /// Channels whose quantizer step is dictated by a matrix.
    forced_q: Vec<Option<u8>>,
    /// Stereo-presentation samples that exceed 24 bits.
    overflows: u64,
}

/// Where an interval sits in the stream.
#[derive(Clone, Copy)]
struct Placement<'a> {
    /// Access units in the interval.
    n_au: usize,
    /// Access units before it.
    first_au: usize,
    /// `Encoder::samples_check` of the interval before.
    check: &'a [u32],
    /// How many samples the last access unit of the stream drops on decode (`None` unless this is the last interval).
    shorten: Option<usize>,
}

/// What the analysis decided for an interval: how every channel is predicted and coded, and the matrices.
struct IntervalPlan {
    analyses: Vec<ChannelAnalysis>,
    out_q: Vec<u8>,
    sub_mats: Vec<Vec<MatrixSpec>>,
}

fn fold32(v: u32) -> u8 {
    let v = v ^ (v >> 16);
    (v ^ (v >> 8)) as u8
}

/* ------------------------------------------------------------------------------------------ */
/* analysis                                                                                    */
/* ------------------------------------------------------------------------------------------ */

/* ------------------------------------------------------------------------------------------ */
/* entropy coding of one channel block                                                         */
/* ------------------------------------------------------------------------------------------ */

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sbits() {
        assert_eq!(number_sbits(0), 1);
        assert_eq!(number_sbits(1), 2);
        assert_eq!(number_sbits(-1), 1);
        assert_eq!(number_sbits(127), 8);
        assert_eq!(number_sbits(-128), 8);
    }
}
