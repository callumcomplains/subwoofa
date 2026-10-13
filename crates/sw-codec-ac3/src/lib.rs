//! Native AC-3 (Dolby Digital) and E-AC-3 (Dolby Digital Plus) encoder, written from the public ATSC A/52
//! specification.
//!
//! A frame is 1536 samples in six blocks of 256. The encoder works in these steps, one module each:
//!
//! 1. `analysis`: MDCT of every block (long, or two short transforms where the transient detector in `blocksw`
//!    fires), then stereo rematrixing;
//! 2. `coupling_plan`: above a start frequency the channels may share a coupling channel (`coupling`);
//! 3. `frame_plan`: exponents (`exponents`), then the power spectral density and masking curve (`bitalloc`) of
//!    every channel and set of blocks sharing exponents, then a search for the highest SNR offset whose
//!    mantissas (`mantissas`) fit in the frame;
//! 4. `syntax`: the frame in AC-3 or E-AC-3 bitstream syntax, with its CRCs.
//!
//! Bit-allocation parameters are fixed (the usual encoder defaults), so frames carry no delta allocation. Bit
//! rate is constant: every frame is the same size (the 44.1 kHz and E-AC-3 sizes alternate to average out).
//!
//! Many frames can be encoded together: the transform and transient detector run in order, the rest of each
//! frame is independent and runs on all cores (`Encoder::encode_batch`).

mod analysis;
mod bitalloc;
pub mod bits;
pub mod blocksw;
mod coupling;
mod coupling_plan;
mod exponents;
mod frame_plan;
mod layout;
mod mantissas;
pub mod mdct;
pub mod native;
mod plan;
mod syntax;
#[rustfmt::skip]
pub mod tables;

pub use layout::{map_layout, LayoutMap};

use bitalloc::bin_to_band;
use coupling::CplInfo;
use exponents::Strat;
use layout::fbw_channels;
use mdct::{Mdct, M};
use tables::FRAME_SIZE_WORDS;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Unsupported(String),
}

/// Samples per channel in a frame.
pub const SAMPLES_PER_FRAME: usize = 1536;
/// Audio blocks in a frame.
const BLOCKS: usize = 6;
/// Bands of the bit allocation (A/52 table of band starts).
const BANDS: usize = 50;

/// The bit rates AC-3 can signal (kbit/s). E-AC-3 takes any whole number of 16-bit words per frame instead.
const BITRATES: [u32; 19] = [
    32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 576, 640,
];
/// Scale applied to MDCT output (negative: the decoder's synthesis has the opposite sign
/// convention to the textbook MDCT used here). A decoder reconstructs amplitude = 2 x coefficient, and the
/// unscaled MDCT peak of a full-scale sine is about 146, so 1/256 gives unity gain with a
/// full-scale tone at a coefficient of about 0.57 (inside the format's +-1 mantissa range).
const MDCT_SCALE: f64 = -1.0 / 256.0;

#[derive(Debug, Clone)]
pub struct Config {
    /// Audio coding mode (A/52 table 5.8): 1 = 1/0 (mono) ... 7 = 3/2 (L C R Ls Rs).
    pub acmod: u8,
    /// A low-frequency effects channel follows the full-bandwidth ones.
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

/// The encoder. Frames go in as planar `f32` and come out as AC-3 or E-AC-3 frames; see the crate documentation for
/// the steps. The transform state (overlap and transient detectors) carries over from frame to frame; the fields
/// named `cur_*` describe the frame being planned.
#[derive(Clone)]
pub struct Encoder {
    cfg: Config,
    /// Full-bandwidth channels, and all channels (those plus the LFE).
    nfch: usize,
    nch: usize,
    /// Sample rate code (0 = 48 kHz, 1 = 44.1, 2 = 32) and the index into the bit rate table (AC-3).
    fscod: usize,
    size_idx: usize,
    /// The MDCT and the short-block transform. Shared when a batch of frames is planned on several threads.
    mdct: std::sync::Arc<Mdct>,
    switched: std::sync::Arc<blocksw::Switched>,
    /// Per channel: the 256 samples before the block being transformed (the MDCT's overlap).
    hist: Vec<[f32; M]>,
    /// Per channel: the bin the channel is coded up to (the low-pass).
    end: Vec<usize>,
    /// Bits left over from rounding the frame size, carried to the next frame (E-AC-3 and 44.1 kHz sizes alternate).
    frac: f64,
    band_of_bin: [u8; 256],
    /// The blocks that share exponents by default: `(first block, block count)`.
    sets: Vec<(usize, usize)>,
    /// The exponent strategy used for ordinary channels.
    strat: Strat,
    /// One transient detector per full-bandwidth channel.
    dets: Vec<blocksw::Detector>,
    block_switching: bool,
    /// Exponent sets and block-switch flags of the frame being encoded.
    cur_sets: Vec<(usize, usize)>,
    cur_blksw: Vec<[bool; BLOCKS]>,
    /// Rematrixing flags per block and band (2/0 mode, AC-3 only).
    cur_remat: Vec<[bool; 4]>,
    /// Channel coupling for multichannel AC-3 at low rates.
    coupling: bool,
    cur_coupling: Option<CplInfo>,
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
            r => {
                return Err(Error::Unsupported(format!(
                    "AC-3 supports 48, 44.1 and 32 kHz, not {r} Hz"
                )))
            }
        };
        let size_idx = if cfg.eac3 {
            // E-AC-3 frames are sized in whole 16-bit words, so any bit rate is possible
            // within a single independent substream.
            if !(8..=1024).contains(&cfg.bitrate_kbps) {
                return Err(Error::Unsupported(format!(
                    "{} kbps is outside the single-substream E-AC-3 range (8 to 1024)",
                    cfg.bitrate_kbps
                )));
            }
            0
        } else {
            BITRATES.iter().position(|b| *b == cfg.bitrate_kbps).ok_or_else(|| {
                Error::Unsupported(format!(
                    "{} kbps is not a standard AC-3 bit rate ({:?})",
                    cfg.bitrate_kbps, BITRATES
                ))
            })?
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
            mdct: std::sync::Arc::new(Mdct::new(MDCT_SCALE)),
            hist: vec![[0.0; M]; nch],
            end,
            frac: 0.0,
            band_of_bin: bin_to_band(),
            sets: sets.clone(),
            strat,
            switched: std::sync::Arc::new(blocksw::Switched::new(MDCT_SCALE)),
            dets: (0..nfch).map(|_| blocksw::Detector::new(cfg.sample_rate)).collect(),
            block_switching: !cfg.eac3,
            cur_sets: sets,
            cur_blksw: vec![[false; BLOCKS]; nfch],
            cur_remat: vec![[false; 4]; BLOCKS],
            rematrixing: true,
            coupling: cfg.acmod >= 2 && kbps_per_ch < 96.0,
            cur_coupling: None,
            dependent: None,
            cfg,
        })
    }

    fn bandwidth_bins(cfg: &Config, kbps_per_ch: f64) -> usize {
        let hz = cfg.bandwidth_hz.map(|h| h as f64).unwrap_or_else(|| {
            let pts = [
                (24.0, 7000.0),
                (40.0, 11000.0),
                (56.0, 15000.0),
                (72.0, 18000.0),
                (88.0, 20500.0),
                (104.0, 1e9),
            ];
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
        self.coupling = on && self.cfg.acmod >= 2;
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
            let exact =
                self.cfg.bitrate_kbps as f64 * 1000.0 * SAMPLES_PER_FRAME as f64 / self.cfg.sample_rate as f64 / 16.0
                    + self.frac;
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
        let a = self.analyse(input);
        self.cur_sets = self.frame_sets(&a.blksw);
        self.cur_blksw = a.blksw;
        self.encode_coefficients(a.coefs, a.frmsizecod, a.words)
    }

    /// Encodes many frames at once, `frames[n][channel]` being 1536 samples each, and returns them in order.
    /// The transform and transient detection carry state from frame to frame, so they run in order; what
    /// follows (coupling, bit allocation, writing) needs only the frame itself and runs on all cores. The bytes
    /// are the same as `encode_frame` called one frame at a time.
    pub fn encode_batch(&mut self, frames: &[Vec<Vec<f32>>]) -> Vec<Vec<u8>> {
        use rayon::prelude::*;
        let analysed = self.analyse_batch(frames);
        let worker = self.clone();
        analysed
            .into_par_iter()
            .map_init(
                || worker.clone(),
                |enc, a| {
                    enc.cur_sets = enc.frame_sets(&a.blksw);
                    enc.cur_blksw = a.blksw;
                    enc.encode_coefficients(a.coefs, a.frmsizecod, a.words)
                },
            )
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::crc16;

    #[test]
    fn frame_has_correct_size_and_valid_crcs() {
        let cfg = Config {
            acmod: 7,
            lfe: true,
            sample_rate: 48_000,
            bitrate_kbps: 448,
            dialnorm: 31,
            dolby_surround: false,
            bandwidth_hz: None,
            eac3: false,
        };
        let mut enc = Encoder::new(cfg).unwrap();
        let sig: Vec<f32> = (0..SAMPLES_PER_FRAME).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let chans: Vec<&[f32]> = (0..6).map(|_| &sig[..]).collect();
        let f = enc.encode_frame(&chans);
        assert_eq!(f.len(), 448 * 2 * 2, "448 kbps @ 48 kHz = 896 words");
        assert_eq!(&f[..2], &[0x0B, 0x77]);
        assert_eq!(
            crc16(&f[2..]),
            0,
            "crc2 over the whole frame must leave a zero residual"
        );
        let f58 = ((f.len() >> 2) + (f.len() >> 4)) << 1;
        assert_eq!(
            crc16(&f[2..f58]),
            0,
            "crc1 over the first 5/8 must leave a zero residual"
        );
    }

    #[test]
    fn rejects_non_standard_bitrates() {
        let cfg = Config {
            acmod: 2,
            lfe: false,
            sample_rate: 48_000,
            bitrate_kbps: 300,
            dialnorm: 31,
            dolby_surround: false,
            bandwidth_hz: None,
            eac3: false,
        };
        assert!(Encoder::new(cfg.clone()).is_err());
        // ...but E-AC-3 takes any rate in range
        assert!(Encoder::new(Config {
            eac3: true,
            ..cfg.clone()
        })
        .is_ok());
        assert!(Encoder::new(Config {
            eac3: true,
            bitrate_kbps: 2000,
            ..cfg
        })
        .is_err());
    }
}
