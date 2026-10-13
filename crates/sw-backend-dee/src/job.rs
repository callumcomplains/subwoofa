//! A job for the engine: the target format with its options, and the files in and out.

use crate::input::InputKind;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Drc {
    FilmStandard,
    FilmLight,
    MusicStandard,
    MusicLight,
    Speech,
    /// Dolby Digital Plus only.
    None,
}

impl Drc {
    pub(crate) fn tag(self) -> &'static str {
        match self {
            Drc::FilmStandard => "film_standard",
            Drc::FilmLight => "film_light",
            Drc::MusicStandard => "music_standard",
            Drc::MusicLight => "music_light",
            Drc::Speech => "speech",
            Drc::None => "none",
        }
    }
}

impl std::str::FromStr for Drc {
    type Err = String;
    fn from_str(s: &str) -> Result<Drc, String> {
        Ok(match s {
            "film_standard" => Drc::FilmStandard,
            "film_light" => Drc::FilmLight,
            "music_standard" => Drc::MusicStandard,
            "music_light" => Drc::MusicLight,
            "speech" => Drc::Speech,
            "none" => Drc::None,
            _ => {
                return Err(format!(
                    "'{s}': use film_standard, film_light, music_standard, music_light, speech or none"
                ))
            }
        })
    }
}

/// One of TrueHD's lower presentations (8, 6 or 2 channels), derived from the Atmos mix.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Presentation {
    pub drc: Drc,
    /// 3 dB attenuation of the surround channels (8 and 6 channel presentations).
    pub surround_3db: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrueHdOptions {
    pub drc: Drc,
    /// Spatial clusters of the Atmos presentation: 12, 14 or 16.
    pub spatial_clusters: u8,
    /// Keep compatibility with older Blu-ray authoring tools (a little higher peak data rate).
    pub legacy_authoring: bool,
    pub presentation_8ch: Option<Presentation>,
    pub presentation_6ch: Option<Presentation>,
    /// Stereo presentation: its DRC and format (`stereo`, `dolby_surround_encoded`, `dolby_headphone_encoded`).
    pub presentation_2ch: Option<(Drc, bool, String)>,
    pub optimize_data_rate: bool,
}

impl Default for TrueHdOptions {
    fn default() -> Self {
        TrueHdOptions {
            drc: Drc::FilmLight,
            spatial_clusters: 12,
            legacy_authoring: true,
            presentation_8ch: None,
            presentation_6ch: None,
            presentation_2ch: None,
            optimize_data_rate: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DdpOptions {
    /// 384, 448, 576, 640, 768 or 1024 kbps.
    pub data_rate_kbps: u32,
    pub drc_line: Drc,
    pub drc_rf: Drc,
    /// Lo/Ro and Lt/Rt centre and surround mix levels in dB (as DEE spells them, e.g. "-3").
    pub loro_center: String,
    pub loro_surround: String,
    pub ltrt_center: String,
    pub ltrt_surround: String,
    /// `loro`, `ltrt`, `ltrt-pl2` or `not_indicated`.
    pub preferred_downmix: String,
}

impl Default for DdpOptions {
    fn default() -> Self {
        DdpOptions {
            data_rate_kbps: 768,
            drc_line: Drc::FilmLight,
            drc_rf: Drc::FilmLight,
            loro_center: "-3".into(),
            loro_surround: "-3".into(),
            ltrt_center: "-3".into(),
            ltrt_surround: "-3".into(),
            preferred_downmix: "loro".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    TrueHd(TrueHdOptions),
    Ddp(DdpOptions),
    /// Convert a master between `adm`, `damf` and `mxf_iab` (no encoding).
    Convert {
        format: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtmosJob {
    pub input: PathBuf,
    /// Detected from the file when `None`.
    pub kind: Option<InputKind>,
    pub output: PathBuf,
    pub target: Target,
    /// Frame rate of the master's timecode (`23.976`, `24`, `25`, `29.97`, `30`, `48`, `50`, `59.94`, `60`).
    pub frame_rate: Option<String>,
    /// Encode only this span (`HH:MM:SS:FF` or `HH:MM:SS.xx`).
    pub start: Option<String>,
    pub end: Option<String>,
    /// Measure loudness with this standard (`1770-4`...) so the dialogue level is set from it.
    pub metering: Option<String>,
    /// Dialogue level override, -31 to 0 (0 keeps the measured one).
    pub custom_dialnorm: Option<i32>,
    pub prepend_silence: Option<String>,
    pub append_silence: Option<String>,
}

impl AtmosJob {
    pub fn new(input: impl Into<PathBuf>, output: impl Into<PathBuf>, target: Target) -> AtmosJob {
        AtmosJob {
            input: input.into(),
            kind: None,
            output: output.into(),
            target,
            frame_rate: None,
            start: None,
            end: None,
            metering: None,
            custom_dialnorm: None,
            prepend_silence: None,
            append_silence: None,
        }
    }
}
