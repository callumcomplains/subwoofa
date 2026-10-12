//! Settings of the Bitstream tab: metadata the stream carries that does not change the audio coding.
//!
//! They follow the original DTS-HD Master Audio Suite encoder's Bitstream tab, whose behaviour was measured by
//! driving that encoder (see the private research notes).

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
    /// The Downmix tab: how a decoder folds the extra channels into 5.1 and 5.1 into stereo.
    #[serde(default)]
    pub downmix: Downmix,
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
    MonoMap {
        c: Some(0.0),
        ..MonoMap::default()
    }
}

impl Default for SecondaryAudio {
    fn default() -> Self {
        SecondaryAudio {
            attenuation_db: 6.0,
            fade_down_secs: 0.0,
            fade_up_secs: 0.0,
            dialog_mode: false,
            mono_map: default_mono_map(),
        }
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

/// A downmix level in dB below full level; `None` is off (minus infinity, "INF" in the original's Downmix tab).
pub type Level = Option<f64>;

/// The six channels the downmix tables are indexed by.
pub const DOWNMIX_CHANNELS: [&str; 6] = ["C", "L", "R", "Ls", "Rs", "LFE"];

/// The Downmix tab. Channels are in the order `C, L, R, Ls, Rs, LFE` ("Ls"/"Rs" are the side pair of the layout).
///
/// Each source channel is mixed into two outputs, `a` and `b`: for the downmix to 5.1 they are the first and the
/// second extra channel of a 7.x layout (in the lossless extension's speaker order), for the downmix to stereo
/// the left and the right output. The `primary` levels are the gain of the 5.1 channels themselves.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Downmix {
    /// 7.x to 5.1: gain of each 5.1 channel when a decoder plays the 5.1 part alone.
    #[serde(default = "primary_51")]
    pub primary: [Level; 6],
    /// 7.x to 5.1: level at which the first / second extra channel is added to each 5.1 channel.
    #[serde(default = "extra_a")]
    pub extra_a: [Level; 6],
    #[serde(default = "extra_b")]
    pub extra_b: [Level; 6],
    /// 5.1 to stereo: level at which each channel goes to the left / right output.
    #[serde(default = "stereo_a")]
    pub stereo_a: [Level; 6],
    #[serde(default = "stereo_b")]
    pub stereo_b: [Level; 6],
    /// Master Audio: carry a stereo downmix as a channel set of its own, so a decoder can play stereo without
    /// the other channels.
    #[serde(default)]
    pub embed_stereo: bool,
    /// Master Audio: write the stereo downmix coefficients as information (a decoder applies them itself).
    #[serde(default)]
    pub stereo_matrix: bool,
    /// The stereo downmix is matrix encoded (Lt/Rt) rather than plain (Lo/Ro); only labels the matrix.
    #[serde(default)]
    pub lt_rt: bool,
}

fn primary_51() -> [Level; 6] {
    [Some(3.0); 6]
}
fn extra_a() -> [Level; 6] {
    [None, None, None, Some(3.0), None, None]
}
fn extra_b() -> [Level; 6] {
    [None, None, None, None, Some(3.0), None]
}
fn stereo_a() -> [Level; 6] {
    [Some(6.0), Some(3.0), None, Some(6.0), None, None]
}
fn stereo_b() -> [Level; 6] {
    [Some(6.0), None, Some(3.0), None, Some(6.0), None]
}

impl Default for Downmix {
    fn default() -> Self {
        Downmix {
            primary: primary_51(),
            extra_a: extra_a(),
            extra_b: extra_b(),
            stereo_a: stereo_a(),
            stereo_b: stereo_b(),
            embed_stereo: false,
            stereo_matrix: false,
            lt_rt: false,
        }
    }
}

impl Downmix {
    /// Update levels from `L=3,Ls=inf,...` (names from `DOWNMIX_CHANNELS`, case-insensitive; `inf` or `off` for none).
    pub fn parse_levels(table: &mut [Level; 6], text: &str) -> Result<(), String> {
        for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (name, value) = part
                .split_once('=')
                .ok_or_else(|| format!("'{part}': expected CHANNEL=dB"))?;
            let idx = DOWNMIX_CHANNELS
                .iter()
                .position(|c| c.eq_ignore_ascii_case(name.trim()))
                .ok_or_else(|| format!("unknown channel '{}' (use C, L, R, Ls, Rs or LFE)", name.trim()))?;
            let value = value.trim();
            table[idx] = if value.eq_ignore_ascii_case("inf") || value.eq_ignore_ascii_case("off") {
                None
            } else {
                Some(
                    value
                        .parse::<f64>()
                        .map_err(|_| format!("'{value}' is not a number of dB"))?
                        .clamp(0.0, 90.0),
                )
            };
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_sends_downmix_settings_as_json() {
        let json = r#"{"downmix":{"primary":[3,3,3,3,3,3],"extra_a":[null,null,null,3,null,null],"extra_b":[null,null,null,null,3,null],
            "stereo_a":[6,3,null,6,null,null],"stereo_b":[6,null,3,null,6,null],"embed_stereo":true,"stereo_matrix":false,"lt_rt":false}}"#;
        let o: BitstreamOptions = serde_json::from_str(json).unwrap();
        assert_eq!(
            o.downmix,
            Downmix {
                embed_stereo: true,
                ..Downmix::default()
            }
        );
        // leaving it out gives the original encoder's defaults
        assert_eq!(
            serde_json::from_str::<BitstreamOptions>("{}").unwrap().downmix,
            Downmix::default()
        );
    }

    #[test]
    fn level_lists_parse() {
        let mut t = Downmix::default().stereo_a;
        Downmix::parse_levels(&mut t, "C=inf, ls=4.5").unwrap();
        assert_eq!(t, [None, Some(3.0), None, Some(4.5), None, None]);
        assert!(Downmix::parse_levels(&mut t, "X=1").is_err());
    }
}
