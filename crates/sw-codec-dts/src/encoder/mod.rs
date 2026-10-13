//! DTS core frame encoder: subband analysis, bit allocation, quantization and bitstream packing.
//!
//! Coding tools used: 32 subbands, mid-tread scalar quantizers with Huffman / block / plain index
//! coding chosen per channel and quantizer, 7-bit linear scale factors, optional LFE channel. Not used:
//! ADPCM, high-frequency VQ, joint intensity coding, transient mode (always "no transient").

mod emulation;
mod extension_pack;
mod frames;
mod lfe;
mod pack;
mod search;
mod tables;
mod types;

pub use frames::Leftovers;
pub use tables::SUBBAND_GAIN;
pub use types::{AuxDownmix, Config, Error, XxchParams, MAX_PRIMARY};

use crate::qmf::Analysis;
use emulation::ExactState;
use lfe::{lfe_decimator, LFE_ALIGN};
use pack::sfreq_code;

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
}
