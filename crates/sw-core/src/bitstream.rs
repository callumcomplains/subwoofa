//! Settings of the Bitstream tab: metadata the stream carries that does not change the audio coding.
//!
//! They follow the original DTS-HD Master Audio Suite encoder's Bitstream tab, whose behaviour was measured by
//! driving that encoder (see `docs/ORIGINAL-ENCODER.md`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct BitstreamOptions {
    /// Free text stored in the `.dtshd` file (Latin-1, up to 2000 characters). Empty for none.
    #[serde(default)]
    pub program_info: String,
    /// Seamless branching: where the encoder switches its prediction off so a player can jump between versions.
    #[serde(default)]
    pub seamless: Seamless,
    /// Mixing metadata of Blu-ray secondary audio (DTS Express).
    #[serde(default)]
    pub secondary: SecondaryAudio,
    /// Wide 7.1 (Lw/Rw) streams: add the speaker remapping sets that let a decoder play them on a side-rear
    /// layout or a 5.1 system.
    #[serde(default)]
    pub wide_remap: bool,
    /// Frame rate and start timecode stored in the file (the Audio tab's Timecode section).
    #[serde(default)]
    pub timecode: Timecode,
}

/// `rate` is one of "23.976", "24", "25", "29.97", "29.97 drop", "30", "30 drop" (empty means 24); `start` is
/// `HH:MM:SS:FF` (empty means zero).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Timecode {
    #[serde(default)]
    pub rate: String,
    #[serde(default)]
    pub start: String,
    /// Reference timecode (the position the programme was cut from), `HH:MM:SS:FF`; empty means the start.
    #[serde(default)]
    pub reference: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub enum Seamless {
    #[default]
    Off,
    /// The whole encode is one clip: a branch point at its start.
    SingleClip,
    /// Branch points as timecodes (`HH:MM:SS:FF`, `;` before the frames for drop frame), one per entry.
    Timecodes(Vec<String>),
}

/// What the player does with the primary audio while this secondary stream plays, and where the (mono) secondary
/// audio is panned.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SecondaryAudio {
    /// Attenuation of the primary audio in dB (0 to 60; larger values are limited to 60).
    #[serde(default = "default_attenuation")]
    pub attenuation_db: f64,
    /// Seconds the attenuation takes to reach its value at the start, and to release again at the end (0 to 9).
    #[serde(default)]
    pub fade_down_secs: f64,
    #[serde(default)]
    pub fade_up_secs: f64,
    /// Express Dialog Mode: the Express stream keeps only the lower half of the band (speech).
    #[serde(default)]
    pub dialog_mode: bool,
    /// Level in dB at which a mono secondary is sent to each output speaker; `None` is off (minus infinity).
    #[serde(default = "default_mono_map")]
    pub mono_map: MonoMap,
}

fn default_attenuation() -> f64 {
    6.0
}

fn default_mono_map() -> MonoMap {
    MonoMap { c: Some(0.0), ..MonoMap::default() }
}

impl Default for SecondaryAudio {
    fn default() -> Self {
        SecondaryAudio { attenuation_db: 6.0, fade_down_secs: 0.0, fade_up_secs: 0.0, dialog_mode: false, mono_map: default_mono_map() }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MonoMap {
    pub c: Option<f64>,
    pub l: Option<f64>,
    pub r: Option<f64>,
    pub ls: Option<f64>,
    pub rs: Option<f64>,
    pub lfe: Option<f64>,
}
