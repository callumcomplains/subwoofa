//! The job as plain data, and what a finished job reports.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use sw_backend_ffmpeg as ff;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncodeSpec {
    pub dest: String,
    pub stream: String,
    pub layout: String,
    pub sample_rate: u32,
    pub bitrate: Option<u32>,
    pub dialnorm: Option<u8>,
    pub start: Option<f64>,
    pub duration: Option<f64>,
    pub inputs: HashMap<String, String>,
    pub output: String,
    /// Skip spec validation (custom bit rates, sample rates, unusual combinations).
    #[serde(default)]
    pub unlocked: bool,
    /// "ffmpeg" (default) or "native" (Subwoofa's own encoder, AC-3 only for now).
    #[serde(default)]
    pub engine: String,
    /// Native TrueHD: include the stereo downmix substream (default on).
    #[serde(default = "yes")]
    pub downmix: bool,
    /// Multichannel-file channel order: "auto" (default), "mask" or "file".
    #[serde(default)]
    pub channel_order: String,
    /// DTS: rotate the surrounds by +/-45 degrees before the centre surround is added (6.x ES).
    #[serde(default)]
    pub es_phase_shift: bool,
    /// DTS: attenuate the rear channels by 3 dB first.
    #[serde(default)]
    pub rear_minus_3db: bool,
    /// Layout of the input files when it is richer than the output layout (e.g. "22.2"): the inputs are
    /// mixed down to `layout` by constant-power panning. Native engine only.
    #[serde(default)]
    pub source_layout: Option<String>,
    /// DTS-HD MA at 96 kHz: the core carries an X96 extension (a 96/24 core).
    #[serde(default)]
    pub core_x96: bool,
    /// Audio files (WAV, AIFF, CAF, FLAC): the sample format, one of "s16", "s24" (default), "s32", "f32", "f64".
    #[serde(default)]
    pub pcm_format: String,
    /// The Bitstream tab: programme information, seamless branching, secondary-audio mixing metadata, wide 7.1 remapping.
    #[serde(default)]
    pub bitstream: sw_core::BitstreamOptions,
    /// How hard lossless formats search: "fast", "balanced" (the default, also for an empty string) or "best".
    #[serde(default)]
    pub speed: String,
    /// How many threads the encode may use; 0 means every core.
    #[serde(default)]
    pub threads: u32,
    /// Apply gain so the integrated loudness of the encoded span lands on this many LUFS (the true peak is kept under
    /// -1 dBTP, which can leave the result quieter). Needs a format Subwoofa encodes itself.
    #[serde(default)]
    pub normalize_lufs: Option<f64>,
    /// Delay the audio by this many milliseconds: positive puts silence in front of it, negative leaves the start out.
    /// Needs a format Subwoofa encodes itself.
    #[serde(default)]
    pub delay_ms: i32,
    /// Leave a log of the run next to the output (the app does; the command line does not).
    #[serde(default = "yes")]
    pub write_log: bool,
}

/// The dialnorm setting (the dialogue level, 1 to 31 for -1 to -31 dBFS) as the attenuation a DTS decoder applies,
/// written as a negative number of dB: -27 dBFS is 4 dB of attenuation, -31 dBFS none.
pub fn dts_dialnorm(level: Option<u8>) -> i32 {
    -(31 - level.unwrap_or(31).clamp(1, 31) as i32)
}

pub(crate) fn yes() -> bool {
    true
}

impl EncodeSpec {
    pub fn display_name(&self) -> String {
        let file = self.output.rsplit(['\\', '/']).next().unwrap_or(&self.output);
        file.to_string()
    }

    /// `<output without extension>_log.txt`
    pub fn log_path(&self) -> String {
        let p = std::path::Path::new(&self.output);
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("encode");
        p.with_file_name(format!("{stem}_log.txt"))
            .to_string_lossy()
            .into_owned()
    }
}

/// What a finished job reports.
#[derive(Default)]
pub struct Outcome {
    pub message: String,
    pub verified: bool,
    /// Size of the output file.
    pub bytes: u64,
    /// Seconds of audio that were encoded, and how long the whole job took.
    pub span_secs: f64,
    pub elapsed_secs: f64,
}

pub enum Failure {
    Canceled,
    Error(String),
}

impl From<String> for Failure {
    fn from(s: String) -> Self {
        Failure::Error(s)
    }
}

impl From<&str> for Failure {
    fn from(s: &str) -> Self {
        Failure::Error(s.to_string())
    }
}

impl From<ff::Error> for Failure {
    fn from(e: ff::Error) -> Self {
        match e {
            ff::Error::Canceled => Failure::Canceled,
            other => Failure::Error(other.to_string()),
        }
    }
}
