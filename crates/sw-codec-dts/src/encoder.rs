//! DTS core frame encoder: subband analysis, bit allocation, quantization and bitstream packing.
//!
//! Coding tools used: 32 subbands, mid-tread scalar quantizers with Huffman / block / plain index
//! coding chosen per channel and quantizer, 7-bit linear scale factors, optional LFE channel. Not used:
//! ADPCM, high-frequency VQ, joint intensity coding, transient mode (always "no transient").

use crate::bits::BitWriter;
use crate::qmf::{Analysis, BANDS};
use crate::core_exact::{dequantize, dequantize_lfe, FixedLfe, FixedSynth};
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

static T_SEARCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static T_PACK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static T_EMU: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
    /// quantized indexes [ch][band][sample]
    q: Vec<Vec<Vec<i32>>>,
    /// SEL per channel and abits 1..=10 (== GROUP_SIZE means no Huffman)
    sel: Vec<[u8; 10]>,
    bhuff: Vec<u8>,
    bits: usize,
}

impl Encoder {
    pub fn new(cfg: Config) -> Result<Encoder, Error> {
        if sfreq_code(cfg.sample_rate).is_none() {
            return Err(Error::Unsupported(format!("{} Hz is not a DTS core rate", cfg.sample_rate)));
        }
        if !(1..=MAX_PRIMARY).contains(&cfg.channels) {
            return Err(Error::Unsupported("the DTS core carries 1 to 5 primary channels".into()));
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
            return Err(Error::Unsupported("the XCH extension adds a sixth channel to a 5.x core".into()));
        }
        if frame_bytes < min || frame_bytes > 16384 {
            return Err(Error::Unsupported(format!("frame size of {frame_bytes} bytes is outside what {} channels need (>= {min})", cfg.channels)));
        }
        let lfe_decim = 64usize;
        // symmetric 512-tap decimation filter with unit DC gain
        let sum: f64 = LFE_FIR_64.iter().sum();
        let lfe_fir: Vec<f64> = LFE_FIR_64.iter().map(|v| v / sum).collect();
        Ok(Encoder {
            analysis: (0..cfg.channels + usize::from(cfg.xch)).map(|_| Analysis::new(cfg.perfect)).collect(),
            lfe_hist: vec![0.0; 512],
            lfe_pre: vec![0.0; LFE_ALIGN],
            frame_bytes,
            lfe_decim,
            lfe_fir,
            mask_weight: std::env::var("SW_DTS_BETA").ok().and_then(|v| v.parse().ok()).unwrap_or(0.4),
            exact: None,
            last_offset: 30.0,
            last_core_pcm: Vec::new(),
            keep_residual: false,
            last_residual: Vec::new(),
            last_deq: Vec::new(),
            x96_reserve: 0,
            cfg,
        })
    }

    /// Track the decoder's exact output (`last_core_pcm`) for every frame encoded from now on.
    /// With `x96` the emulated decoder synthesises at twice the core rate (for 96 kHz lossless streams).
    pub fn enable_exact_decode(&mut self, x96: bool) {
        let n = self.cfg.channels;
        self.exact = Some(ExactState { synth: (0..n).map(|_| FixedSynth::new(false, x96)).collect(), lfe: FixedLfe::new(), x96 });
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

    /// Samples of delay between input and decoder output.
    pub fn delay(&self) -> usize {
        self.analysis[0].delay()
    }

    /// Encode one frame. `pcm[ch]` holds `frame_samples()` samples in 24-bit units, primary channels in
    /// stream order followed by the LFE channel when present.
    pub fn encode_frame(&mut self, pcm: &[Vec<f64>]) -> Vec<u8> {
        let nch = self.nplan();
        let blocks = self.cfg.blocks;
        assert_eq!(pcm.len(), nch + usize::from(self.cfg.lfe));
        // 1. subband analysis
        let mut sub: Vec<Vec<Vec<f64>>> = vec![vec![vec![0.0; blocks]; BANDS]; nch];
        {
            use rayon::prelude::*;
            self.analysis.par_iter_mut().zip(sub.par_iter_mut()).zip(pcm.par_iter()).for_each(|((an, out), input)| {
                for b in 0..blocks {
                    let mut o = [0f64; BANDS];
                    an.step(&input[b * 32..b * 32 + 32], &mut o);
                    for (band, v) in o.iter().enumerate() {
                        out[band][b] = v * SUBBAND_GAIN;
                    }
                }
            });
        }
        let lfe = if self.cfg.lfe { Some(self.lfe_samples(&pcm[nch])) } else { None };
        self.encode_subbands(&sub, lfe)
    }

    /// Encode one frame from ready-made subband samples `sub[ch][band][block]` (bypasses the analysis bank).
    pub fn encode_subbands(&mut self, sub: &[Vec<Vec<f64>>], lfe: Option<(Vec<i32>, u8)>) -> Vec<u8> {
        let blocks = self.cfg.blocks;
        // 3. allocation: highest quality that fits the frame
        let budget = (self.frame_bytes - self.x96_reserve) * 8;
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
        // search around the previous frame's operating point: quality offsets vary slowly
        let t_a = std::time::Instant::now();
        let (lo, best) = self.search_offset(sub, &power, &peak, budget);
        T_SEARCH.fetch_add(t_a.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
        self.last_offset = lo;
        if std::env::var_os("SW_DTS_DEBUG").is_some() {
            let used: Vec<f64> = best.abits.iter().map(|a| a.iter().map(|x| *x as f64).sum::<f64>() / 32.0).collect();
            eprintln!("bits {} / {}  lo {:.1}  mean abits {:?}", best.bits, budget, lo, used);
        }
        let t_b = std::time::Instant::now();
        let bytes = self.pack(&best, lfe.as_ref());
        T_PACK.fetch_add(t_b.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
        if self.keep_residual {
            let mut res = Vec::with_capacity(self.cfg.channels);
            let mut all_deq = Vec::with_capacity(self.cfg.channels);
            for ch in 0..self.cfg.channels {
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
            self.last_residual = res;
            self.last_deq = all_deq;
        }
        if self.exact.is_some() {
            let t_c = std::time::Instant::now();
            self.emulate_decoder(&best, lfe.as_ref());
            T_EMU.fetch_add(t_c.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
            if std::env::var_os("SW_TIMING").is_some() {
                eprintln!("core: search {} ms, pack {} ms, emulate {} ms", T_SEARCH.load(std::sync::atomic::Ordering::Relaxed) / 1_000_000, T_PACK.load(std::sync::atomic::Ordering::Relaxed) / 1_000_000, T_EMU.load(std::sync::atomic::Ordering::Relaxed) / 1_000_000);
            }
        }
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
            self.lfe_hist.extend_from_slice(&pcm[d * self.lfe_decim..(d + 1) * self.lfe_decim]);
            let y: f64 = self.lfe_hist.iter().zip(self.lfe_fir.iter()).map(|(x, h)| x * h).sum();
            vals.push(y * LFE_GAIN);
        }
        let peak = vals.iter().fold(0f64, |m, v| m.max(v.abs()));
        let mut idx = 0usize;
        while idx < 124 && SCALE_7[idx] as f64 * 0.035 * 127.0 < peak {
            idx += 1;
        }
        let step = SCALE_7[idx] as f64 * 0.035;
        let q = vals.iter().map(|v| ((v / step).round() as i32).clamp(-128, 127)).collect();
        (q, idx as u8)
    }

    /// Highest quality offset (dB) whose plan fits `budget` bits, with that plan. On a multi-core machine the
    /// candidates of each round are evaluated in parallel; resolution is 0.375 dB either way.
    fn search_offset(&self, sub: &[Vec<Vec<f64>>], power: &[[f64; BANDS]], peak: &[[f64; BANDS]], budget: usize) -> (f64, Plan) {
        use rayon::prelude::*;
        if rayon::current_num_threads() >= 6 {
            let base = self.last_offset;
            let first: Vec<(f64, Plan)> = (-4..=4).into_par_iter().map(|i| { let o = base + 3.0 * i as f64; (o, self.make_plan(sub, power, peak, o)) }).collect();
            let top_fit = first.iter().rposition(|(_, p)| p.bits <= budget);
            if let Some(i) = top_fit {
                if i + 1 < first.len() {
                    // the answer lies between candidate i (fits) and i + 1 (does not): refine in eighths
                    let lo = first[i].0;
                    let mut best = (lo, first.into_iter().nth(i).unwrap().1);
                    let fine: Vec<(f64, Plan)> = (1..8).into_par_iter().map(|j| { let o = lo + 3.0 * j as f64 / 8.0; (o, self.make_plan(sub, power, peak, o)) }).collect();
                    for (o, p) in fine {
                        if p.bits <= budget {
                            best = (o, p);
                        }
                    }
                    return best;
                }
                // everything fits: continue upwards from the top candidate
                let top = first.into_iter().last().unwrap();
                return self.search_sequential(sub, power, peak, budget, top.0 - 12.0);
            }
            return self.search_sequential(sub, power, peak, budget, base - 12.0 - 24.0);
        }
        self.search_sequential(sub, power, peak, budget, self.last_offset - 12.0)
    }

    fn search_sequential(&self, sub: &[Vec<Vec<f64>>], power: &[[f64; BANDS]], peak: &[[f64; BANDS]], budget: usize, start: f64) -> (f64, Plan) {
        let mut lo = start;
        let mut best = self.make_plan(sub, power, peak, lo);
        let mut guard = 0;
        while best.bits > budget && lo > -200.0 {
            lo -= 24.0;
            best = self.make_plan(sub, power, peak, lo);
            guard += 1;
            if guard > 12 {
                break;
            }
        }
        if best.bits > budget {
            lo = -200.0;
            best = self.make_plan(sub, power, peak, lo);
        } else {
            let mut hi = lo + 24.0;
            loop {
                let p = self.make_plan(sub, power, peak, hi);
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
                let p = self.make_plan(sub, power, peak, mid);
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

    fn make_plan(&self, sub: &[Vec<Vec<f64>>], power: &[[f64; BANDS]], peak: &[[f64; BANDS]], offset_db: f64) -> Plan {
        let nch = self.nplan();
        let blocks = self.cfg.blocks;
        let mut abits = vec![[0u8; BANDS]; nch];
        for ch in 0..nch {
            let pdb: Vec<f64> = power[ch].iter().map(|p| 10.0 * (p + 1e-6).log10()).collect();
            for b in 0..BANDS {
                // masking by the neighbourhood: shallow upward spread, steep downward
                let mut mask = f64::NEG_INFINITY;
                for (b2, p) in pdb.iter().enumerate() {
                    let d = b as i32 - b2 as i32;
                    let att = if d >= 0 { 12.0 * d as f64 } else { 25.0 * (-d) as f64 };
                    mask = mask.max(p - att);
                }
                let noise_db = self.mask_weight * mask - offset_db;
                let noise = 10f64.powf(noise_db / 10.0);
                if power[ch][b] <= noise || peak[ch][b] < 0.5 {
                    continue;
                }
                let mut k = 1;
                while k < 26 {
                    let delta = peak[ch][b] / max_q(k) as f64;
                    if delta * delta / 12.0 <= noise {
                        break;
                    }
                    k += 1;
                }
                abits[ch][b] = k as u8;
            }
        }
        self.finish_plan(sub, abits, peak, blocks)
    }

    fn finish_plan(&self, sub: &[Vec<Vec<f64>>], abits: Vec<[u8; BANDS]>, peak: &[[f64; BANDS]], blocks: usize) -> Plan {
        let nch = self.nplan();
        let mut scale_idx = vec![[0u8; BANDS]; nch];
        let mut q = vec![vec![Vec::new(); BANDS]; nch];
        for ch in 0..nch {
            for b in 0..BANDS {
                let k = abits[ch][b] as usize;
                if k == 0 {
                    continue;
                }
                let step = step_of(k);
                let mq = max_q(k) as f64;
                let mut idx = 0usize;
                while idx < 124 && (SCALE_7[idx] as f64) * step * mq < peak[ch][b] {
                    idx += 1;
                }
                scale_idx[ch][b] = idx as u8;
                let delta = SCALE_7[idx] as f64 * step;
                q[ch][b] = sub[ch][b].iter().map(|v| ((v / delta).round() as i32).clamp(-(mq as i32), mq as i32)).collect();
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
                let plain = if k <= 7 { nsamp / 4 * BLOCK_BITS[k - 1] as usize } else { nsamp * raw_bits(k) as usize };
                let off = (QUANT_HUFF[k - 1][0].len() as i32 - 1) / 2;
                let mut best = (plain, g as u8);
                for s in 0..g {
                    let t = QUANT_HUFF[k - 1][s];
                    let mut c = 2usize; // adjustment index
                    for b in &bands {
                        for v in &q[ch][*b] {
                            c += t[(*v + off) as usize].1 as usize;
                        }
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
            let x = xch_bits + 49 + 21 + BANDS + 16;
            bits = bits.div_ceil(32) * 32 + x;
            if x.div_ceil(8) > 1000 {
                bits = usize::MAX / 4; // XChFSIZE is 10 bits
            }
        }
        Plan { abits, scale_idx, q, sel, bhuff, bits }
    }

    /// Frame header + primary audio coding header + fixed parts of the subframe.
    fn header_bits(&self) -> usize {
        let nch = self.cfg.channels;
        104 + 7 + 5 * nch + 5 * nch + 3 * nch + 2 * nch + 3 * nch + 3 * nch + 5 + BANDS * nch
    }

    /// Run the decoder's integer arithmetic over this frame's coded data.
    fn emulate_decoder(&mut self, plan: &Plan, lfe: Option<&(Vec<i32>, u8)>) {
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
        self.last_core_pcm = out;
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
        let rate_code = BIT_RATES.iter().take(25).position(|r| *r >= cfg.bitrate).unwrap_or(29) as u32;
        w.put(rate_code, 5);
        w.put(0, 1); // fixed bit
        w.put(0, 1); // dynamic range coefficients
        w.put(0, 1); // time stamp
        w.put(0, 1); // auxiliary data
        w.put(0, 1); // HDCD
        w.put(if self.x96_reserve > 0 { 2 } else { 0 }, 3); // extension audio id: XCH (0) or X96 (2)
        w.put(u32::from(cfg.xch || self.x96_reserve > 0), 1); // extension audio present
        w.put(0, 1); // DSYNC only per subframe
        w.put(if cfg.lfe { 2 } else { 0 }, 2); // LFE interpolation by 64
        w.put(1, 1); // predictor history valid
        w.put(u32::from(cfg.perfect), 1); // perfect (1) or non-perfect (0) reconstruction bank
        let (vernum, dn) = if cfg.dialnorm >= -15 { (7, -cfg.dialnorm) } else { (6, -cfg.dialnorm - 16) };
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
        let mut bytes = w.into_bytes();
        if cfg.xch {
            let start = bytes.len().div_ceil(4) * 4;
            bytes.resize(start, 0);
            bytes.extend_from_slice(&self.pack_xch(plan, self.frame_bytes - start));
        }
        assert!(bytes.len() <= self.frame_bytes, "frame overflow: {} > {}", bytes.len(), self.frame_bytes);
        bytes.resize(self.frame_bytes, 0);
        bytes
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
