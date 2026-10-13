//! DTS core frame encoder: subband analysis, bit allocation, quantization and bitstream packing.
//!
//! Coding tools used: 32 subbands, mid-tread scalar quantizers with Huffman / block / plain index
//! coding chosen per channel and quantizer, 7-bit linear scale factors, optional LFE channel. Not used:
//! ADPCM, high-frequency VQ, joint intensity coding, transient mode (always "no transient").

use crate::bits::BitWriter;
use crate::core_exact::{dequantize, dequantize_lfe, FixedLfe, FixedSynth};
use crate::qmf::{Analysis, BANDS};
use crate::tables::*;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Unsupported(String),
}

#[derive(Debug, Clone)]
pub struct Config {
    pub sample_rate: u32,
    /// AMODE code (Table 5-4).
    pub amode: u8,
    /// Primary (full range) channels, 1..=5.
    pub channels: usize,
    pub lfe: bool,
    /// Core bit rate in bits per second.
    pub bitrate: u32,
    /// Dialog normalisation in dB, -31..=0.
    pub dialnorm: i32,
    /// Source resolution flag: 16, 20 or 24.
    pub pcm_bits: u32,
    /// DTS-ES (matrix) flag.
    pub es: bool,
    /// DTS-ES discrete: a sixth full-range channel (the centre surround) travels in the XCH extension at
    /// the end of every frame. `encode_frame` then takes it after the primary channels.
    pub xch: bool,
    /// Perfect-reconstruction filter bank (FILTS flag): needed when a residual extension (XBR) refines
    /// the core.
    pub perfect: bool,
    /// Subband sample blocks per frame: 16 (512 samples) or 32 (1024 samples).
    pub blocks: usize,
}

pub const MAX_PRIMARY: usize = 5;

fn sfreq_code(rate: u32) -> Option<u32> {
    Some(match rate {
        8000 => 1,
        16000 => 2,
        32000 => 3,
        11025 => 6,
        22050 => 7,
        44100 => 8,
        12000 => 11,
        24000 => 12,
        48000 => 13,
        _ => return None,
    })
}

const SEL_BITS: [u32; 10] = [1, 2, 2, 2, 2, 3, 3, 3, 3, 3];
const GROUP_SIZE: [usize; 10] = [1, 3, 3, 3, 3, 7, 7, 7, 7, 7];
/// Bits of one block code word (4 samples) for abits 1..=7.
const BLOCK_BITS: [u32; 7] = [7, 10, 12, 13, 15, 17, 19];

fn step_of(abits: usize) -> f64 {
    STEP_LOSSY[abits] as f64 / 4_194_304.0
}

fn max_q(abits: usize) -> i32 {
    (QUANT_LEVELS[abits] as i32 - 1) / 2
}

/// Bits of a plain (no further coding) index.
fn raw_bits(abits: usize) -> u32 {
    abits as u32 - 3
}

/// Decoder output scale relative to the subband values produced by the analysis bank.
pub const SUBBAND_GAIN: f64 = 1.0 / 181.019_335_983_756_17;
/// Same calibration for the LFE channel (the decoder's LFE path has its own scaling).
const LFE_GAIN: f64 = 1.1541;
/// Samples the LFE path is delayed to line up with the subband path.
const LFE_ALIGN: usize = 32;

/// 512-tap decimation filter (unit DC gain) for the LFE channel. The decoder's interpolation filter already
/// rolls off (-2.5 dB at 80 Hz, -3.9 dB at 100 Hz), so the encoder side is designed as its inverse up to
/// 110 Hz (boost capped at 6 dB), tapering to zero by 260 Hz: the LFE comes back flat instead of doubly
/// attenuated.
fn lfe_decimator() -> Vec<f64> {
    use std::f64::consts::PI;
    let n = 512usize;
    let decoder = |f: f64| -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, h) in LFE_FIR_64.iter().enumerate() {
            let a = -2.0 * PI * f / 48_000.0 * i as f64;
            re += h * a.cos();
            im += h * a.sin();
        }
        (re * re + im * im).sqrt() / 64.0
    };
    let dc = decoder(0.0);
    let want = |f: f64| -> f64 {
        if f <= 110.0 {
            (dc / decoder(f)).min(2.0)
        } else if f < 260.0 {
            let t = (f - 110.0) / 150.0;
            (dc / decoder(110.0)).min(2.0) * 0.5 * (1.0 + (PI * t).cos())
        } else {
            0.0
        }
    };
    // sample the wanted magnitude, linear phase with the filter centre at (n - 1) / 2
    let centre = (n as f64 - 1.0) / 2.0;
    let grid = 4096usize;
    let mut h = vec![0.0f64; n];
    for (i, v) in h.iter_mut().enumerate() {
        let t = i as f64 - centre;
        let mut acc = want(0.0);
        for k in 1..grid / 2 {
            let f = 48_000.0 * k as f64 / grid as f64;
            if f > 300.0 {
                break;
            }
            acc += 2.0 * want(f) * (2.0 * PI * k as f64 / grid as f64 * t).cos();
        }
        let w = 0.5 * (1.0 + (2.0 * PI * (i as f64 + 0.5 - n as f64 / 2.0) / n as f64).cos());
        *v = acc / grid as f64 * w;
    }
    let sum: f64 = h.iter().sum();
    h.iter().map(|v| v / sum).collect()
}

pub struct Encoder {
    cfg: Config,
    analysis: Vec<Analysis>,
    lfe_hist: Vec<f64>,
    lfe_pre: Vec<f64>,
    frame_bytes: usize,
    lfe_decim: usize,
    lfe_fir: Vec<f64>,
    /// 0 = equal noise level in every band (best SNR), 1 = noise follows the masking estimate.
    pub mask_weight: f64,
    last_offset: f64,
    /// Bit-exact decoder emulation (needed by the lossless extension).
    exact: Option<ExactState>,
    /// PCM the reference decoder produces for the last frame (when emulation is on).
    pub last_core_pcm: Vec<Vec<i32>>,
    /// Keep the difference between the input subband samples and the core's reconstruction in
    /// `last_residual` (`[channel][band][block]`, primary channels only) for the XBR extension.
    pub keep_residual: bool,
    pub last_residual: Vec<Vec<Vec<f64>>>,
    /// The core's reconstructed subband samples of the last frame (`[channel][band][block]`), kept with
    /// `keep_residual`.
    pub last_deq: Vec<Vec<Vec<i32>>>,
    /// Bytes at the end of every frame reserved for the X96 extension (96/24 streams).
    pub x96_reserve: usize,
    /// Replaces the core frame's fixed header size in the bit count (XXCH extension encoders).
    header_bits_override: Option<usize>,
    /// The stereo downmix coefficients every frame carries in its auxiliary data.
    aux: Option<AuxDownmix>,
}

/// Downmix coefficients in the auxiliary data of a core frame: the matrix type (1 Lo/Ro, 2 Lt/Rt) and one 9-bit
/// code (sign bit 8, table index) per channel for each of the two outputs, channels in the core's order with the
/// LFE last.
#[derive(Debug, Clone)]
pub struct AuxDownmix {
    pub kind: u8,
    pub rows: Vec<Vec<u16>>,
}

impl AuxDownmix {
    /// Sync word, the data, and the checksum of the data.
    fn block(&self) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.put(0, 1); // no decoding time stamp
        w.put(1, 1); // dynamic downmix present
        w.put(u32::from(self.kind), 3);
        for row in &self.rows {
            for c in row {
                w.put(*c as u32, 9);
            }
        }
        let payload = w.into_bytes();
        let mut out = vec![0x9A, 0x11, 0x05, 0xA0];
        out.extend_from_slice(&payload);
        out.extend_from_slice(&crate::xll::crc16(&payload).to_be_bytes());
        out
    }

    /// Bytes from the sync word to the end of the checksum.
    fn size(&self) -> usize {
        4 + (5 + 9 * self.rows.iter().map(Vec::len).sum::<usize>()).div_ceil(8) + 2
    }
}

/// An XXCH extension (DTS-HD HRA 6.1/7.1): channels beyond the core's five, coded like core channels in the
/// extension substream. All masks use the DTS loudspeaker bit positions (C 0, L 1, R 2, Ls 3, Rs 4, LFE 5, Cs 6,
/// Lsr 7, Rsr 8, Lss 9, Rss 10, then Lc 11, Rc 12, Lh 13, Ch 14, Rh 15, LFE2 16, Lw 17, Rw 18, Oh 19, Lhs 20, Rhs 21).
#[derive(Debug, Clone)]
pub struct XxchParams {
    /// Speakers of the core (with Ls/Rs written as Lss/Rss when the core carries the sides).
    pub core_mask: u32,
    /// Speakers of the channels coded here, in coding order.
    pub spk_mask: u32,
    pub mask_bits: u32,
    /// Embedded downmix: the pre-scaling code (6 bits) applied to every core channel and, per extension
    /// channel, the core speakers it was added to with the 7-bit coefficient code (sign bit 6, magnitude 0..=63).
    pub dmix: Option<(u32, Vec<Vec<(u32, u32)>>)>,
}

struct ExactState {
    synth: Vec<FixedSynth>,
    lfe: FixedLfe,
    x96: bool,
}

/// Everything decided for one frame.
struct Plan {
    abits: Vec<[u8; BANDS]>,
    scale_idx: Vec<[u8; BANDS]>,
    /// quantized indexes [ch][band][sample]; empty until `fill_indexes` (only the plan that is kept needs them)
    q: Vec<Vec<Vec<i32>>>,
    /// SEL per channel and abits 1..=10 (== GROUP_SIZE means no Huffman)
    sel: Vec<[u8; 10]>,
    bhuff: Vec<u8>,
    bits: usize,
}

/// What a frame leaves behind besides its bytes, for the extensions that are built on the core.
#[derive(Default)]
pub struct Leftovers {
    /// The PCM the decoder produces for the frame (only with `enable_exact_decode`).
    pub core_pcm: Vec<Vec<i32>>,
    /// Input subband samples minus the core's reconstruction (only with `keep_residual`).
    pub residual: Vec<Vec<Vec<f64>>>,
    /// The core's reconstructed subband samples (only with `keep_residual`).
    pub deq: Vec<Vec<Vec<i32>>>,
}

/// A frame planned and packed, waiting for the part that has to follow the frames before it.
struct Coded {
    bytes: Vec<u8>,
    plan: Plan,
    offset: f64,
    planes: Option<(Vec<Vec<Vec<f64>>>, Vec<Vec<Vec<i32>>>)>,
}

/// One band coded with one quantiser: the scale factor, the indexes, and what each Huffman book would cost.
struct BandCode {
    scale_idx: u8,
    q: Vec<i32>,
    /// Bits of the indexes with book `s` (only for quantisers 1 to 10, which have books).
    cost: [u32; 10],
}

/// What the search of one frame works out once instead of once per candidate offset: the masking level of every
/// band, the noise each quantiser leaves in it, and the `BandCode` of every (channel, band, quantiser) met.
/// Neighbouring candidate offsets give most bands the same quantiser.
struct BandCache {
    mask: Vec<[f64; BANDS]>,
    /// `noise[ch][band][k]`: the noise power of quantiser `k` (1 to 25) on this band.
    noise: Vec<Vec<[f64; 26]>>,
    codes: Vec<std::sync::OnceLock<BandCode>>,
}

const QUANTISERS: usize = 27;

impl BandCache {
    fn new(power: &[[f64; BANDS]], peak: &[[f64; BANDS]]) -> BandCache {
        let mask = power
            .iter()
            .map(|p| {
                let pdb: Vec<f64> = p.iter().map(|p| 10.0 * (p + 1e-6).log10()).collect();
                let mut mask = [0f64; BANDS];
                for (b, m) in mask.iter_mut().enumerate() {
                    // masking by the neighbourhood: shallow upward spread, steep downward
                    *m = f64::NEG_INFINITY;
                    for (b2, p) in pdb.iter().enumerate() {
                        let d = b as i32 - b2 as i32;
                        let att = if d >= 0 { 12.0 * d as f64 } else { 25.0 * (-d) as f64 };
                        *m = m.max(p - att);
                    }
                }
                mask
            })
            .collect();
        let noise = peak
            .iter()
            .map(|p| {
                p.iter()
                    .map(|peak| {
                        let mut n = [0f64; 26];
                        for (k, n) in n.iter_mut().enumerate().skip(1) {
                            let delta = peak / max_q(k) as f64;
                            *n = delta * delta / 12.0;
                        }
                        n
                    })
                    .collect()
            })
            .collect();
        BandCache {
            mask,
            noise,
            codes: (0..power.len() * BANDS * QUANTISERS)
                .map(|_| std::sync::OnceLock::new())
                .collect(),
        }
    }

    fn get(&self, ch: usize, band: usize, k: usize, make: impl FnOnce() -> BandCode) -> &BandCode {
        self.codes[(ch * BANDS + band) * QUANTISERS + k].get_or_init(make)
    }
}

impl Encoder {
    pub fn new(cfg: Config) -> Result<Encoder, Error> {
        if sfreq_code(cfg.sample_rate).is_none() {
            return Err(Error::Unsupported(format!(
                "{} Hz is not a DTS core rate",
                cfg.sample_rate
            )));
        }
        if !(1..=MAX_PRIMARY).contains(&cfg.channels) {
            return Err(Error::Unsupported(
                "the DTS core carries 1 to 5 primary channels".into(),
            ));
        }
        if cfg.blocks != 16 && cfg.blocks != 32 {
            return Err(Error::Unsupported("blocks per frame must be 16 or 32".into()));
        }
        if !(-31..=0).contains(&cfg.dialnorm) {
            return Err(Error::Unsupported("dialnorm must be between -31 and 0".into()));
        }
        let samples = cfg.blocks * 32;
        let frame_bytes = (cfg.bitrate as u64 * samples as u64 / cfg.sample_rate as u64 / 8) as usize;
        let min = 40 + (60 + 4 * 32) * cfg.channels + if cfg.lfe { 12 } else { 0 };
        if cfg.xch && cfg.channels != 5 {
            return Err(Error::Unsupported(
                "the XCH extension adds a sixth channel to a 5.x core".into(),
            ));
        }
        if frame_bytes < min || frame_bytes > 16384 {
            return Err(Error::Unsupported(format!(
                "frame size of {frame_bytes} bytes is outside what {} channels need (>= {min})",
                cfg.channels
            )));
        }
        let lfe_decim = 64usize;
        let lfe_fir = lfe_decimator();
        Ok(Encoder {
            analysis: (0..cfg.channels + usize::from(cfg.xch))
                .map(|_| Analysis::new(cfg.perfect))
                .collect(),
            lfe_hist: vec![0.0; 512],
            lfe_pre: vec![0.0; LFE_ALIGN],
            frame_bytes,
            lfe_decim,
            lfe_fir,
            mask_weight: std::env::var("SW_DTS_BETA")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.4),
            exact: None,
            last_offset: 30.0,
            last_core_pcm: Vec::new(),
            keep_residual: false,
            last_residual: Vec::new(),
            last_deq: Vec::new(),
            x96_reserve: 0,
            header_bits_override: None,
            aux: None,
            cfg,
        })
    }

    /// Track the decoder's exact output (`last_core_pcm`) for every frame encoded from now on.
    /// With `x96` the emulated decoder synthesises at twice the core rate (for 96 kHz lossless streams).
    pub fn enable_exact_decode(&mut self, x96: bool) {
        let n = self.cfg.channels;
        self.exact = Some(ExactState {
            synth: (0..n).map(|_| FixedSynth::new(false, x96)).collect(),
            lfe: FixedLfe::new(),
            x96,
        });
    }

    pub fn frame_samples(&self) -> usize {
        self.cfg.blocks * 32
    }

    /// Channels with subband data: the primaries plus the XCH channel.
    fn nplan(&self) -> usize {
        self.cfg.channels + usize::from(self.cfg.xch)
    }

    pub fn frame_bytes(&self) -> usize {
        self.frame_bytes
    }

    /// Write the stereo downmix coefficients into the auxiliary data of every frame (from now on).
    pub fn set_aux_downmix(&mut self, aux: Option<AuxDownmix>) {
        self.aux = aux;
    }

    /// The latest place the auxiliary data may start: it ends four bytes short of the end of the core part (the
    /// frame less the extension area), on a 32-bit boundary.
    fn aux_sync(&self, aux: &AuxDownmix) -> usize {
        (self.frame_bytes - 4 - self.x96_reserve - (4 + aux.size() - 4).div_ceil(4) * 4) & !3
    }

    /// Samples of delay between input and decoder output.
    pub fn delay(&self) -> usize {
        self.analysis[0].delay()
    }

    /// Encode one frame. `pcm[ch]` holds `frame_samples()` samples in 24-bit units, primary channels in
    /// stream order followed by the LFE channel when present.
    pub fn encode_frame(&mut self, pcm: &[Vec<f64>]) -> Vec<u8> {
        let (bytes, left) = self.run_batch(&[pcm]).pop().unwrap();
        self.adopt(left);
        bytes
    }

    /// Encodes consecutive frames together and returns each one's bytes with what it left behind. The bytes are
    /// the same as `encode_frame` called frame by frame. The filter banks run in order, the plan search and
    /// packing of the frames run on all cores, and the decoder emulation (if on) runs in order again. A caller
    /// that needs the leftovers of a frame (`adopt`) takes them from the result.
    pub fn encode_batch(&mut self, frames: &[&[Vec<f64>]]) -> Vec<(Vec<u8>, Leftovers)> {
        self.run_batch(frames)
    }

    /// Makes the leftovers of a frame from `encode_batch` the ones `last_core_pcm`, `last_residual` and
    /// `last_deq` report.
    pub fn adopt(&mut self, left: Leftovers) {
        self.last_core_pcm = left.core_pcm;
        self.last_residual = left.residual;
        self.last_deq = left.deq;
    }

    fn run_batch(&mut self, frames: &[&[Vec<f64>]]) -> Vec<(Vec<u8>, Leftovers)> {
        use rayon::prelude::*;
        let nch = self.nplan();
        let subbands = self.analyse(frames);
        let lfe: Vec<Option<(Vec<i32>, u8)>> = frames
            .iter()
            .map(|pcm| self.cfg.lfe.then(|| self.lfe_samples(&pcm[nch])))
            .collect();
        // a lone frame searches with all cores; many frames keep a core each
        let spread = frames.len() < 4;
        let hint = self.last_offset;
        let me = &*self;
        let coded: Vec<Coded> = subbands
            .par_iter()
            .zip(lfe.par_iter())
            .map(|(sub, lfe)| me.plan_and_pack(sub, lfe.as_ref(), hint, spread))
            .collect();
        coded
            .into_iter()
            .zip(&lfe)
            .map(|(c, lfe)| self.settle(c, lfe.as_ref()))
            .collect()
    }

    /// The subband samples `[frame][channel][band][block]` of frames in order. Each channel's filter bank runs on
    /// its own thread; it only depends on its own earlier input.
    fn analyse(&mut self, frames: &[&[Vec<f64>]]) -> Vec<Vec<Vec<Vec<f64>>>> {
        use rayon::prelude::*;
        let (nch, blocks) = (self.nplan(), self.cfg.blocks);
        let per_channel: Vec<Vec<Vec<Vec<f64>>>> = self.analysis[..nch]
            .par_iter_mut()
            .enumerate()
            .map(|(ch, an)| {
                frames
                    .iter()
                    .map(|pcm| {
                        let mut out = vec![vec![0.0; blocks]; BANDS];
                        for b in 0..blocks {
                            let mut o = [0f64; BANDS];
                            an.step(&pcm[ch][b * 32..b * 32 + 32], &mut o);
                            for (band, v) in o.iter().enumerate() {
                                out[band][b] = v * SUBBAND_GAIN;
                            }
                        }
                        out
                    })
                    .collect()
            })
            .collect();
        let mut columns: Vec<_> = per_channel.into_iter().map(|c| c.into_iter()).collect();
        (0..frames.len())
            .map(|_| columns.iter_mut().map(|c| c.next().unwrap()).collect())
            .collect()
    }

    /// The plan and bytes of a frame. This needs nothing from the frames around it except the starting point
    /// `hint` of the search, so frames can be done in any order or together.
    fn plan_and_pack(&self, sub: &[Vec<Vec<f64>>], lfe: Option<&(Vec<i32>, u8)>, hint: f64, spread: bool) -> Coded {
        let (offset, plan) = self.plan_for(sub, hint, spread);
        let bytes = self.pack(&plan, lfe);
        let planes = self.keep_residual.then(|| self.planes_of(&plan, sub));
        Coded {
            bytes,
            plan,
            offset,
            planes,
        }
    }

    /// The part of a frame that follows the frames before it: the search's starting point and the decoder
    /// emulation.
    fn settle(&mut self, coded: Coded, lfe: Option<&(Vec<i32>, u8)>) -> (Vec<u8>, Leftovers) {
        self.last_offset = coded.offset;
        let core_pcm = if self.exact.is_some() {
            self.emulate_decoder(&coded.plan, lfe)
        } else {
            Vec::new()
        };
        let (residual, deq) = coded.planes.unwrap_or_default();
        (
            coded.bytes,
            Leftovers {
                core_pcm,
                residual,
                deq,
            },
        )
    }

    /// The core's reconstruction of a frame (`deq`) and what it left over (`residual`), `[channel][band][block]`.
    fn planes_of(&self, best: &Plan, sub: &[Vec<Vec<f64>>]) -> (Vec<Vec<Vec<f64>>>, Vec<Vec<Vec<i32>>>) {
        let blocks = self.cfg.blocks;
        let planes = self.nplan(); // the primaries, and the XCH channel when there is one
        let mut res = Vec::with_capacity(planes);
        let mut all_deq = Vec::with_capacity(planes);
        for ch in 0..planes {
            let mut bands = Vec::with_capacity(BANDS);
            let mut deqs = Vec::with_capacity(BANDS);
            for b in 0..BANDS {
                let k = best.abits[ch][b] as usize;
                let mut deq = vec![0i32; blocks];
                if k > 0 {
                    let scale = SCALE_7[best.scale_idx[ch][b] as usize] as i32;
                    dequantize(&best.q[ch][b], STEP_LOSSY[k] as i32, scale, &mut deq);
                }
                bands.push(sub[ch][b].iter().zip(&deq).map(|(v, d)| v - *d as f64).collect());
                deqs.push(deq);
            }
            res.push(bands);
            all_deq.push(deqs);
        }
        (res, all_deq)
    }

    /// Keeps the core's reconstruction of this frame (`last_deq`) and what it left over (`last_residual`).
    fn keep_planes(&mut self, best: &Plan, sub: &[Vec<Vec<f64>>]) {
        (self.last_residual, self.last_deq) = self.planes_of(best, sub);
    }

    /// Highest quality plan that fits the frame, searched from where the last frame ended.
    fn search_plan(&mut self, sub: &[Vec<Vec<f64>>]) -> Plan {
        let (offset, plan) = self.plan_for(sub, self.last_offset, true);
        self.last_offset = offset;
        plan
    }

    /// Highest quality plan that fits the frame (the budget is the frame minus any X96 reserve), searched around
    /// the offset `hint`: quality offsets vary slowly from frame to frame. `spread` lets the search use many
    /// threads for this one frame.
    fn plan_for(&self, sub: &[Vec<Vec<f64>>], hint: f64, spread: bool) -> (f64, Plan) {
        let blocks = self.cfg.blocks;
        let mut budget = (self.frame_bytes - self.x96_reserve) * 8;
        if let Some(aux) = &self.aux {
            // the audio ends in front of the auxiliary data, with its 6-bit size field
            budget = self.aux_sync(aux) * 8 - 8;
        }
        let power: Vec<[f64; BANDS]> = sub
            .iter()
            .map(|c| {
                let mut p = [0f64; BANDS];
                for (band, s) in c.iter().enumerate() {
                    p[band] = s.iter().map(|v| v * v).sum::<f64>() / blocks as f64;
                }
                p
            })
            .collect();
        let peak: Vec<[f64; BANDS]> = sub
            .iter()
            .map(|c| {
                let mut p = [0f64; BANDS];
                for (band, s) in c.iter().enumerate() {
                    p[band] = s.iter().fold(0f64, |m, v| m.max(v.abs()));
                }
                p
            })
            .collect();
        let cache = BandCache::new(&power, &peak);
        let (offset, mut best) = self.search_offset(sub, &power, &peak, budget, &cache, hint, spread);
        self.fill_indexes(&mut best, sub, &cache, &peak);
        (offset, best)
    }

    /// Encode one frame from ready-made subband samples `sub[ch][band][block]` (bypasses the analysis bank).
    pub fn encode_subbands(&mut self, sub: &[Vec<Vec<f64>>], lfe: Option<(Vec<i32>, u8)>) -> Vec<u8> {
        let coded = self.plan_and_pack(sub, lfe.as_ref(), self.last_offset, true);
        let (bytes, left) = self.settle(coded, lfe.as_ref());
        self.adopt(left);
        bytes
    }

    /// Decimated, quantizable LFE samples for this frame: (samples, scale index).
    fn lfe_samples(&mut self, pcm: &[f64]) -> (Vec<i32>, u8) {
        let mut input = std::mem::take(&mut self.lfe_pre);
        input.extend_from_slice(pcm);
        self.lfe_pre = input.split_off(pcm.len());
        let pcm = &input[..];
        let n_out = pcm.len() / self.lfe_decim;
        let mut vals = Vec::with_capacity(n_out);
        for d in 0..n_out {
            self.lfe_hist.drain(..self.lfe_decim);
            self.lfe_hist
                .extend_from_slice(&pcm[d * self.lfe_decim..(d + 1) * self.lfe_decim]);
            let y: f64 = self.lfe_hist.iter().zip(self.lfe_fir.iter()).map(|(x, h)| x * h).sum();
            vals.push(y * LFE_GAIN);
        }
        let peak = vals.iter().fold(0f64, |m, v| m.max(v.abs()));
        let mut idx = 0usize;
        while idx < 124 && SCALE_7[idx] as f64 * 0.035 * 127.0 < peak {
            idx += 1;
        }
        let step = SCALE_7[idx] as f64 * 0.035;
        let q = vals
            .iter()
            .map(|v| ((v / step).round() as i32).clamp(-128, 127))
            .collect();
        (q, idx as u8)
    }

    /// Highest quality offset (dB) whose plan fits `budget` bits, with that plan. On a multi-core machine the
    /// candidates of each round are evaluated in parallel; resolution is 0.375 dB either way.
    fn search_offset(
        &self,
        sub: &[Vec<Vec<f64>>],
        power: &[[f64; BANDS]],
        peak: &[[f64; BANDS]],
        budget: usize,
        cache: &BandCache,
        hint: f64,
        spread: bool,
    ) -> (f64, Plan) {
        use rayon::prelude::*;
        if spread && rayon::current_num_threads() >= 6 {
            let base = hint;
            let first: Vec<(f64, Plan)> = (-4..=4)
                .into_par_iter()
                .map(|i| {
                    let o = base + 3.0 * i as f64;
                    (o, self.make_plan(sub, power, peak, o, cache))
                })
                .collect();
            let top_fit = first.iter().rposition(|(_, p)| p.bits <= budget);
            if let Some(i) = top_fit {
                if i + 1 < first.len() {
                    // the answer lies between candidate i (fits) and i + 1 (does not): refine in eighths
                    let lo = first[i].0;
                    let mut best = (lo, first.into_iter().nth(i).unwrap().1);
                    let fine: Vec<(f64, Plan)> = (1..8)
                        .into_par_iter()
                        .map(|j| {
                            let o = lo + 3.0 * j as f64 / 8.0;
                            (o, self.make_plan(sub, power, peak, o, cache))
                        })
                        .collect();
                    for (o, p) in fine {
                        if p.bits <= budget {
                            best = (o, p);
                        }
                    }
                    return best;
                }
                // everything fits: continue upwards from the top candidate
                let top = first.into_iter().last().unwrap();
                return self.search_sequential(sub, power, peak, budget, top.0 - 12.0, cache);
            }
            return self.search_sequential(sub, power, peak, budget, base - 12.0 - 24.0, cache);
        }
        self.search_sequential(sub, power, peak, budget, hint - 12.0, cache)
    }

    fn search_sequential(
        &self,
        sub: &[Vec<Vec<f64>>],
        power: &[[f64; BANDS]],
        peak: &[[f64; BANDS]],
        budget: usize,
        start: f64,
        cache: &BandCache,
    ) -> (f64, Plan) {
        let mut lo = start;
        let mut best = self.make_plan(sub, power, peak, lo, cache);
        let mut guard = 0;
        while best.bits > budget && lo > -200.0 {
            lo -= 24.0;
            best = self.make_plan(sub, power, peak, lo, cache);
            guard += 1;
            if guard > 12 {
                break;
            }
        }
        if best.bits > budget {
            lo = -200.0;
            best = self.make_plan(sub, power, peak, lo, cache);
        } else {
            let mut hi = lo + 24.0;
            loop {
                let p = self.make_plan(sub, power, peak, hi, cache);
                if p.bits <= budget && hi < 200.0 {
                    lo = hi;
                    best = p;
                    hi += 24.0;
                } else {
                    break;
                }
            }
            for _ in 0..6 {
                let mid = 0.5 * (lo + hi);
                let p = self.make_plan(sub, power, peak, mid, cache);
                if p.bits <= budget {
                    lo = mid;
                    best = p;
                } else {
                    hi = mid;
                }
            }
        }
        (lo, best)
    }

    fn make_plan(
        &self,
        sub: &[Vec<Vec<f64>>],
        power: &[[f64; BANDS]],
        peak: &[[f64; BANDS]],
        offset_db: f64,
        cache: &BandCache,
    ) -> Plan {
        let nch = self.nplan();
        let blocks = self.cfg.blocks;
        let mut abits = vec![[0u8; BANDS]; nch];
        for ch in 0..nch {
            for b in 0..BANDS {
                let noise_db = self.mask_weight * cache.mask[ch][b] - offset_db;
                let noise = 10f64.powf(noise_db / 10.0);
                if power[ch][b] <= noise || peak[ch][b] < 0.5 {
                    continue;
                }
                // the first quantiser coarse enough to stay within the noise (26 when none is)
                let k = (1..26).find(|&k| cache.noise[ch][b][k] <= noise).unwrap_or(26);
                abits[ch][b] = k as u8;
            }
        }
        self.finish_plan(sub, abits, peak, blocks, cache)
    }

    /// Codes band `b` of channel `ch` with quantiser `k`.
    fn code_band(&self, sub: &[f64], peak: f64, k: usize) -> BandCode {
        let step = step_of(k);
        let mq = max_q(k) as f64;
        let mut idx = 0usize;
        while idx < 124 && (SCALE_7[idx] as f64) * step * mq < peak {
            idx += 1;
        }
        let delta = SCALE_7[idx] as f64 * step;
        let q: Vec<i32> = sub
            .iter()
            .map(|v| ((v / delta).round() as i32).clamp(-(mq as i32), mq as i32))
            .collect();
        let mut cost = [0u32; 10];
        if k <= 10 {
            let off = (QUANT_HUFF[k - 1][0].len() as i32 - 1) / 2;
            for (s, c) in cost.iter_mut().enumerate().take(GROUP_SIZE[k - 1]) {
                let t = QUANT_HUFF[k - 1][s];
                *c = q.iter().map(|v| t[(*v + off) as usize].1 as u32).sum();
            }
        }
        BandCode {
            scale_idx: idx as u8,
            q,
            cost,
        }
    }

    /// Puts the quantised indexes into a plan found by the search.
    fn fill_indexes(&self, plan: &mut Plan, sub: &[Vec<Vec<f64>>], cache: &BandCache, peak: &[[f64; BANDS]]) {
        let nch = self.nplan();
        plan.q = vec![vec![Vec::new(); BANDS]; nch];
        for ch in 0..nch {
            for b in 0..BANDS {
                let k = plan.abits[ch][b] as usize;
                if k > 0 {
                    let code = cache.get(ch, b, k, || self.code_band(&sub[ch][b], peak[ch][b], k));
                    plan.q[ch][b] = code.q.clone();
                }
            }
        }
    }

    fn finish_plan(
        &self,
        sub: &[Vec<Vec<f64>>],
        abits: Vec<[u8; BANDS]>,
        peak: &[[f64; BANDS]],
        blocks: usize,
        cache: &BandCache,
    ) -> Plan {
        let nch = self.nplan();
        let mut scale_idx = vec![[0u8; BANDS]; nch];
        for ch in 0..nch {
            for b in 0..BANDS {
                let k = abits[ch][b] as usize;
                if k > 0 {
                    scale_idx[ch][b] = cache
                        .get(ch, b, k, || self.code_band(&sub[ch][b], peak[ch][b], k))
                        .scale_idx;
                }
            }
        }
        // codebook choices
        let mut sel = vec![[0u8; 10]; nch];
        let mut bhuff = vec![6u8; nch];
        let mut bits = self.header_bits();
        let nssc = blocks / 8;
        let mut xch_bits = 0usize;
        for ch in 0..nch {
            let bits_before = bits;
            // ABITS coding
            let all_huff = abits[ch].iter().all(|a| (1..=12).contains(a));
            let all4 = abits[ch].iter().all(|a| *a <= 15);
            let mut best = (5 * BANDS, 6u8);
            if all4 && 4 * BANDS < best.0 {
                best = (4 * BANDS, 5);
            }
            if all_huff {
                for (j, t) in BITALLOC_HUFF.iter().enumerate() {
                    let c: usize = abits[ch].iter().map(|a| t[*a as usize - 1].1 as usize).sum();
                    if c < best.0 {
                        best = (c, j as u8);
                    }
                }
            }
            bits += best.0;
            bhuff[ch] = best.1;
            // transient mode flags and scale factors
            let active = abits[ch].iter().filter(|a| **a > 0).count();
            if nssc > 1 {
                bits += active * TMODE_HUFF[0][0].1 as usize;
            }
            bits += active * 7;
            // sample coding per quantizer
            for k in 1..=26usize {
                let bands: Vec<usize> = (0..BANDS).filter(|b| abits[ch][*b] as usize == k).collect();
                if bands.is_empty() {
                    if k <= 10 {
                        sel[ch][k - 1] = GROUP_SIZE[k - 1] as u8;
                    }
                    continue;
                }
                let nsamp = bands.len() * blocks;
                if k > 10 {
                    bits += nsamp * raw_bits(k) as usize;
                    continue;
                }
                let g = GROUP_SIZE[k - 1];
                let plain = if k <= 7 {
                    nsamp / 4 * BLOCK_BITS[k - 1] as usize
                } else {
                    nsamp * raw_bits(k) as usize
                };
                let mut best = (plain, g as u8);
                for s in 0..g {
                    let mut c = 2usize; // adjustment index
                    for b in &bands {
                        c += cache
                            .get(ch, *b, k, || self.code_band(&sub[ch][*b], peak[ch][*b], k))
                            .cost[s] as usize;
                    }
                    if c < best.0 {
                        best = (c, s as u8);
                    }
                }
                sel[ch][k - 1] = best.1;
                bits += best.0;
            }
            bits += SEL_BITS.iter().sum::<u32>() as usize;
            for (i, g) in GROUP_SIZE.iter().enumerate() {
                if (sel[ch][i] as usize) < *g {
                    bits += 2;
                }
            }
            if ch >= self.cfg.channels {
                // the extension channel is counted apart
                xch_bits = bits - bits_before;
                bits = bits_before;
            }
        }
        if self.cfg.lfe {
            bits += 8 * (2 * 2 * nssc) + 8;
        }
        bits += 16; // DSYNC
        if self.cfg.xch {
            // the extension starts on a 32-bit boundary; sync, size, arrangement, channel count, the header
            // arrays (21 bits) and PMODE flags (32 bits) of its channel, and its DSYNC
            // decoders look for the extension from the end of the frame and want it at least 96 bytes long
            let x = (xch_bits + 49 + 21 + BANDS + 16).max(96 * 8);
            bits = bits.div_ceil(32) * 32 + x;
            if x.div_ceil(8) > 1000 {
                bits = usize::MAX / 4; // XChFSIZE is 10 bits
            }
        }
        Plan {
            abits,
            scale_idx,
            q: Vec::new(),
            sel,
            bhuff,
            bits,
        }
    }

    /// Frame header + primary audio coding header + fixed parts of the subframe.
    fn header_bits(&self) -> usize {
        if let Some(h) = self.header_bits_override {
            return h;
        }
        let nch = self.cfg.channels;
        104 + 7 + 5 * nch + 5 * nch + 3 * nch + 2 * nch + 3 * nch + 3 * nch + 5 + BANDS * nch
    }

    /// Run the decoder's integer arithmetic over this frame's coded data.
    fn emulate_decoder(&mut self, plan: &Plan, lfe: Option<&(Vec<i32>, u8)>) -> Vec<Vec<i32>> {
        let nch = self.cfg.channels;
        let blocks = self.cfg.blocks;
        let st = self.exact.as_mut().unwrap();
        let mut deq = vec![vec![vec![0i32; blocks]; BANDS]; nch];
        for ch in 0..nch {
            for b in 0..BANDS {
                let k = plan.abits[ch][b] as usize;
                if k == 0 {
                    continue;
                }
                let scale = SCALE_7[plan.scale_idx[ch][b] as usize] as i32;
                dequantize(&plan.q[ch][b], STEP_LOSSY[k] as i32, scale, &mut deq[ch][b]);
            }
        }
        let mut out: Vec<Vec<i32>> = Vec::new();
        for ch in 0..nch {
            let mult = if st.x96 { 2 } else { 1 };
            let mut pcm = vec![0i32; blocks * 32 * mult];
            for blk in 0..blocks {
                let mut input = [0i32; 32];
                for b in 0..BANDS {
                    input[b] = deq[ch][b][blk];
                }
                st.synth[ch].step(&input, &mut pcm[blk * 32 * mult..(blk + 1) * 32 * mult]);
            }
            out.push(pcm);
        }
        if let Some((q, idx)) = lfe {
            let samples = dequantize_lfe(q, *idx as usize);
            out.push(st.lfe.interpolate(&samples, st.x96));
        }
        out
    }

    fn pack(&self, plan: &Plan, lfe: Option<&(Vec<i32>, u8)>) -> Vec<u8> {
        let cfg = &self.cfg;
        let nch = cfg.channels;
        let blocks = cfg.blocks;
        let nssc = blocks / 8;
        let mut w = BitWriter::new();
        // frame header
        w.put(0x7FFE_8001, 32);
        w.put(1, 1);
        w.put(31, 5);
        w.put(0, 1);
        w.put(blocks as u32 - 1, 7);
        w.put(self.frame_bytes as u32 - 1, 14);
        w.put(cfg.amode as u32, 6);
        w.put(sfreq_code(cfg.sample_rate).unwrap(), 4);
        // 1234.8 kbit/s is the 14-bit DTS CD stream, labelled with the disc's 1411.2 kbit/s rate code
        let rate_code = if cfg.bitrate == 1_234_800 {
            22
        } else {
            BIT_RATES.iter().take(25).position(|r| *r >= cfg.bitrate).unwrap_or(29) as u32
        };
        w.put(rate_code, 5);
        w.put(0, 1); // fixed bit
        w.put(0, 1); // dynamic range coefficients
        w.put(0, 1); // time stamp
        w.put(u32::from(self.aux.is_some()), 1); // auxiliary data
        w.put(0, 1); // HDCD
        w.put(if self.x96_reserve > 0 { 2 } else { 0 }, 3); // extension audio id: XCH (0) or X96 (2)
        w.put(u32::from(cfg.xch || self.x96_reserve > 0), 1); // extension audio present
        w.put(0, 1); // DSYNC only per subframe
        w.put(if cfg.lfe { 2 } else { 0 }, 2); // LFE interpolation by 64
        w.put(1, 1); // predictor history valid
        w.put(u32::from(cfg.perfect), 1); // perfect (1) or non-perfect (0) reconstruction bank
        let (vernum, dn) = if cfg.dialnorm >= -15 {
            (7, -cfg.dialnorm)
        } else {
            (6, -cfg.dialnorm - 16)
        };
        w.put(vernum, 4);
        w.put(1, 2); // copy history
        let pcmr = match (cfg.pcm_bits, cfg.es) {
            (16, false) => 0b000,
            (16, true) => 0b001,
            (20, false) => 0b010,
            (20, true) => 0b011,
            (24, false) => 0b110,
            _ => 0b101,
        };
        w.put(pcmr, 3);
        w.put(0, 1); // front sum/difference
        w.put(0, 1); // surround sum/difference
        w.put(dn as u32, 4);
        // primary audio coding header
        w.put(0, 4); // one subframe
        w.put(nch as u32 - 1, 3);
        for _ in 0..nch {
            w.put(30, 5); // 32 active subbands
        }
        for _ in 0..nch {
            w.put(31, 5); // no high frequency VQ
        }
        for _ in 0..nch {
            w.put(0, 3); // no joint intensity
        }
        for _ in 0..nch {
            w.put(0, 2); // transient codebook A4
        }
        for _ in 0..nch {
            w.put(6, 3); // 7-bit linear scale factors
        }
        for ch in 0..nch {
            w.put(plan.bhuff[ch] as u32, 3);
        }
        for (i, nb) in SEL_BITS.iter().enumerate() {
            for ch in 0..nch {
                w.put(plan.sel[ch][i] as u32, *nb);
            }
        }
        for (i, g) in GROUP_SIZE.iter().enumerate() {
            for ch in 0..nch {
                if (plan.sel[ch][i] as usize) < *g {
                    w.put(0, 2); // adjustment 1.0
                }
            }
        }
        // subframe side information
        w.put(nssc as u32 - 1, 2);
        w.put(0, 3);
        for _ in 0..nch {
            for _ in 0..BANDS {
                w.put(0, 1); // no ADPCM
            }
        }
        for ch in 0..nch {
            for b in 0..BANDS {
                let a = plan.abits[ch][b] as u32;
                match plan.bhuff[ch] {
                    6 => w.put(a, 5),
                    5 => w.put(a, 4),
                    j => {
                        let (c, l) = BITALLOC_HUFF[j as usize][a as usize - 1];
                        w.put(c as u32, l as u32);
                    }
                }
            }
        }
        if nssc > 1 {
            for ch in 0..nch {
                for b in 0..BANDS {
                    if plan.abits[ch][b] > 0 {
                        let (c, l) = TMODE_HUFF[0][0];
                        w.put(c as u32, l as u32);
                    }
                }
            }
        }
        for ch in 0..nch {
            for b in 0..BANDS {
                if plan.abits[ch][b] > 0 {
                    w.put(plan.scale_idx[ch][b] as u32, 7);
                }
            }
        }
        // LFE
        if let Some((q, idx)) = lfe {
            for v in q {
                w.put_signed(*v, 8);
            }
            w.put(*idx as u32, 8);
        }
        // audio data
        for ss in 0..nssc {
            for ch in 0..nch {
                for b in 0..BANDS {
                    let k = plan.abits[ch][b] as usize;
                    if k == 0 {
                        continue;
                    }
                    let vals = &plan.q[ch][b][ss * 8..ss * 8 + 8];
                    self.put_samples(&mut w, k, plan.sel[ch][k.min(10) - 1], vals);
                }
            }
        }
        w.put(0xFFFF, 16);
        if let Some(aux) = &self.aux {
            w.put(aux.size() as u32, 6); // bytes of auxiliary data
        }
        let mut bytes = w.into_bytes();
        if let Some(aux) = &self.aux {
            // a decoder looks for the sync word at the next 32-bit boundary after the size field
            let at = bytes.len().div_ceil(4) * 4;
            assert!(
                at <= self.aux_sync(aux),
                "frame overflow before the auxiliary data: {at} > {}",
                self.aux_sync(aux)
            );
            bytes.resize(at, 0);
            bytes.extend_from_slice(&aux.block());
        }
        if cfg.xch {
            // the extension ends the frame and its size field has 10 bits: after a quiet frame's short core data it
            // starts later than the data ends, with zeros between
            let start = bytes.len().div_ceil(4) * 4;
            let start = start.max((self.frame_bytes - 1020) & !3);
            bytes.resize(start, 0);
            bytes.extend_from_slice(&self.pack_xch(plan, self.frame_bytes - start));
        }
        assert!(
            bytes.len() <= self.frame_bytes,
            "frame overflow: {} > {}",
            bytes.len(),
            self.frame_bytes
        );
        bytes.resize(self.frame_bytes, 0);
        bytes
    }

    /// XXCH extension frame for the channels of this encoder (which must be set up like a core encoder for
    /// those channels): analysis, rate search to `total_bytes`, packing. `pcm` holds one slice of
    /// `frame_samples()` per channel.
    pub fn encode_xxch(&mut self, pcm: &[Vec<f64>], total_bytes: usize, p: &XxchParams) -> Vec<u8> {
        let nch = self.cfg.channels;
        assert_eq!(pcm.len(), nch);
        let sub = self.analyse(&[pcm]).pop().unwrap();
        self.frame_bytes = total_bytes;
        self.x96_reserve = 0;
        // fixed parts: frame header, channel set header without the per-channel selector tables, ADPCM flags
        let hdr = Self::xxch_set_header_fixed_bits(p, nch).div_ceil(8) * 8 + 16;
        self.header_bits_override = Some(88 + hdr + BANDS * nch + 8);
        let plan = self.search_plan(&sub);
        if self.keep_residual {
            self.keep_planes(&plan, &sub);
        }
        self.pack_xxch(&plan, p, total_bytes)
    }

    fn xxch_set_header_fixed_bits(p: &XxchParams, nch: usize) -> usize {
        let nb = p.mask_bits as usize;
        let mut bits = 7 + 3 + (nb - 6) + 1;
        if let Some((_, maps)) = &p.dmix {
            bits += 1 + 6 + nb * nch + maps.iter().map(|m| 7 * m.len()).sum::<usize>();
        }
        bits + nch * (5 + 5 + 3 + 2 + 3 + 3)
    }

    /// The XXCH frame: frame header, one channel set (header and subframe), padded to `total` bytes.
    fn pack_xxch(&self, plan: &Plan, p: &XxchParams, total: usize) -> Vec<u8> {
        let nch = self.cfg.channels;
        let nssc = self.cfg.blocks / 8;
        let nb = p.mask_bits;
        // ---- channel set header
        let mut w = BitWriter::new();
        w.put(0, 7); // size, patched below
        w.put(nch as u32 - 1, 3);
        w.put(p.spk_mask >> 6, nb - 6);
        match &p.dmix {
            Some((scale, maps)) => {
                w.put(1, 1);
                w.put(1, 1); // the downmix is already in the core
                w.put(*scale, 6);
                for m in maps {
                    w.put(m.iter().fold(0, |a, (spk, _)| a | (1 << spk)), nb);
                }
                for m in maps {
                    let mut sorted = m.clone();
                    sorted.sort_by_key(|(spk, _)| *spk);
                    for (_, code) in sorted {
                        w.put(code, 7);
                    }
                }
            }
            None => w.put(0, 1),
        }
        for _ in 0..nch {
            w.put(30, 5); // 32 active subbands
        }
        for _ in 0..nch {
            w.put(31, 5); // no high frequency VQ
        }
        for _ in 0..nch {
            w.put(0, 3); // no joint intensity
        }
        for _ in 0..nch {
            w.put(0, 2); // transient codebook A4
        }
        for _ in 0..nch {
            w.put(6, 3); // 7-bit linear scale factors
        }
        for ch in 0..nch {
            w.put(plan.bhuff[ch] as u32, 3);
        }
        for (i, nbits) in SEL_BITS.iter().enumerate() {
            for ch in 0..nch {
                w.put(plan.sel[ch][i] as u32, *nbits);
            }
        }
        for (i, g) in GROUP_SIZE.iter().enumerate() {
            for ch in 0..nch {
                if (plan.sel[ch][i] as usize) < *g {
                    w.put(0, 2);
                }
            }
        }
        w.align(8);
        let size = w.bit_len() / 8 + 2;
        let mut head = w.into_bytes();
        head[0] = (head[0] & 0x01) | (((size - 1) as u8) << 1);
        let crc = crate::xll::crc16(&head);
        head.extend_from_slice(&crc.to_be_bytes());

        // ---- subframe: side information then samples
        let mut w = BitWriter::new();
        for _ in 0..nch {
            for _ in 0..BANDS {
                w.put(0, 1); // no ADPCM
            }
        }
        for ch in 0..nch {
            for b in 0..BANDS {
                let a = plan.abits[ch][b] as u32;
                match plan.bhuff[ch] {
                    6 => w.put(a, 5),
                    5 => w.put(a, 4),
                    j => {
                        let (c, l) = BITALLOC_HUFF[j as usize][a as usize - 1];
                        w.put(c as u32, l as u32);
                    }
                }
            }
        }
        if nssc > 1 {
            for ch in 0..nch {
                for b in 0..BANDS {
                    if plan.abits[ch][b] > 0 {
                        let (c, l) = TMODE_HUFF[0][0];
                        w.put(c as u32, l as u32);
                    }
                }
            }
        }
        for ch in 0..nch {
            for b in 0..BANDS {
                if plan.abits[ch][b] > 0 {
                    w.put(plan.scale_idx[ch][b] as u32, 7);
                }
            }
        }
        for ss in 0..nssc {
            for ch in 0..nch {
                for b in 0..BANDS {
                    let k = plan.abits[ch][b] as usize;
                    if k == 0 {
                        continue;
                    }
                    let vals = &plan.q[ch][b][ss * 8..ss * 8 + 8];
                    self.put_samples(&mut w, k, plan.sel[ch][k.min(10) - 1], vals);
                }
            }
        }
        w.put(0xFFFF, 16);
        let data = w.into_bytes();
        let set_bytes = head.len() + data.len();

        // ---- frame header
        let mut f = BitWriter::new();
        f.put(0x4700_4A03, 32);
        let hs = (32 + 6 + 1 + 5 + 2 + 14 + nb as usize).div_ceil(8) + 2;
        f.put(hs as u32 - 1, 6);
        f.put(1, 1); // channel set headers carry a CRC
        f.put(nb - 1, 5);
        f.put(0, 2); // one channel set
        f.put(set_bytes as u32 - 1, 14);
        f.put(p.core_mask, nb);
        f.align(8);
        let mut out = f.into_bytes();
        let crc = crate::xll::crc16(&out[4..]);
        out.extend_from_slice(&crc.to_be_bytes());
        debug_assert_eq!(out.len(), hs);
        out.extend_from_slice(&head);
        out.extend_from_slice(&data);
        assert!(out.len() <= total, "XXCH overflow: {} > {}", out.len(), total);
        out.resize(total, 0);
        out
    }

    /// The XCH extension: the sixth channel coded like a core channel, ending at the end of the frame.
    fn pack_xch(&self, plan: &Plan, dist: usize) -> Vec<u8> {
        let ch = self.cfg.channels;
        let nssc = self.cfg.blocks / 8;
        let mut w = BitWriter::new();
        w.put(0x5A5A_5A5A, 32);
        w.put(dist as u32 - 1, 10);
        w.put(1, 4); // one extension channel
        w.put(0, 3);
        w.put(30, 5); // 32 active subbands
        w.put(31, 5); // no high frequency VQ
        w.put(0, 3); // no joint intensity
        w.put(0, 2); // transient codebook A4
        w.put(6, 3); // 7-bit linear scale factors
        w.put(plan.bhuff[ch] as u32, 3);
        for (i, nb) in SEL_BITS.iter().enumerate() {
            w.put(plan.sel[ch][i] as u32, *nb);
        }
        for (i, g) in GROUP_SIZE.iter().enumerate() {
            if (plan.sel[ch][i] as usize) < *g {
                w.put(0, 2);
            }
        }
        for _ in 0..BANDS {
            w.put(0, 1); // no ADPCM
        }
        for b in 0..BANDS {
            let a = plan.abits[ch][b] as u32;
            match plan.bhuff[ch] {
                6 => w.put(a, 5),
                5 => w.put(a, 4),
                j => {
                    let (c, l) = BITALLOC_HUFF[j as usize][a as usize - 1];
                    w.put(c as u32, l as u32);
                }
            }
        }
        if nssc > 1 {
            for b in 0..BANDS {
                if plan.abits[ch][b] > 0 {
                    let (c, l) = TMODE_HUFF[0][0];
                    w.put(c as u32, l as u32);
                }
            }
        }
        for b in 0..BANDS {
            if plan.abits[ch][b] > 0 {
                w.put(plan.scale_idx[ch][b] as u32, 7);
            }
        }
        for ss in 0..nssc {
            for b in 0..BANDS {
                let k = plan.abits[ch][b] as usize;
                if k == 0 {
                    continue;
                }
                let vals = &plan.q[ch][b][ss * 8..ss * 8 + 8];
                self.put_samples(&mut w, k, plan.sel[ch][k.min(10) - 1], vals);
            }
        }
        w.put(0xFFFF, 16);
        let mut bytes = w.into_bytes();
        assert!(bytes.len() <= dist, "XCH overflow: {} > {}", bytes.len(), dist);
        bytes.resize(dist, 0);
        bytes
    }

    fn put_samples(&self, w: &mut BitWriter, k: usize, sel: u8, vals: &[i32]) {
        if k > 10 {
            for v in vals {
                w.put_signed(*v, raw_bits(k));
            }
            return;
        }
        if (sel as usize) < GROUP_SIZE[k - 1] {
            let t = QUANT_HUFF[k - 1][sel as usize];
            let off = (t.len() as i32 - 1) / 2;
            for v in vals {
                let (c, l) = t[(*v + off) as usize];
                w.put(c as u32, l as u32);
            }
        } else if k <= 7 {
            let levels = QUANT_LEVELS[k] as i32;
            let off = (levels - 1) / 2;
            for blk in vals.chunks(4) {
                let mut code = 0i32;
                for v in blk.iter().rev() {
                    code = code * levels + (*v + off);
                }
                w.put(code as u32, BLOCK_BITS[k - 1]);
            }
        } else {
            for v in vals {
                w.put_signed(*v, raw_bits(k));
            }
        }
    }
}
