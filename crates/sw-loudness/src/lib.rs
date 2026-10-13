//! Programme loudness as ITU-R BS.1770-4 and EBU R 128 define it: integrated loudness with its two gates, momentary
//! and short-term loudness, loudness range (EBU Tech 3342) and true peak.
//!
//! Feed the audio to a [`Meter`] in blocks, channel by channel, then take the [`Loudness`] it reports. Channels are
//! weighted by where they sit (surrounds count 1.5 dB more, the LFE not at all). The measurement of a whole
//! programme is also the usual starting point for a Dolby dialnorm: see [`Loudness::dialnorm`].

mod filter;
mod meter;
mod peak;

pub use meter::{channel_weight, Meter};
use serde::Serialize;

/// What was measured. Levels are in LUFS (loudness) and dBTP / dBFS (peaks); `None` means there was nothing above
/// the gate (digital silence) or too little audio for the measurement.
#[derive(Debug, Clone, Serialize)]
pub struct Loudness {
    /// Integrated loudness of the whole programme.
    pub integrated: Option<f64>,
    /// The loudest 3-second and 400 ms stretches.
    pub short_term_max: Option<f64>,
    pub momentary_max: Option<f64>,
    /// Loudness range, in LU.
    pub range: Option<f64>,
    /// True peak and sample peak of each channel, with the channel's short name.
    pub channels: Vec<ChannelPeak>,
    /// Length measured, in seconds.
    pub seconds: f64,
}

/// The peaks of one channel.
#[derive(Debug, Clone, Serialize)]
pub struct ChannelPeak {
    pub name: String,
    /// dBTP, or `None` for silence.
    pub true_peak: Option<f64>,
    /// dBFS, or `None` for silence.
    pub sample_peak: Option<f64>,
}

impl Loudness {
    /// The highest true peak of any channel, in dBTP.
    pub fn true_peak(&self) -> Option<f64> {
        self.channels.iter().filter_map(|c| c.true_peak).reduce(f64::max)
    }

    /// The dialnorm that matches the integrated loudness: its distance below full scale, rounded and kept in the
    /// 1 to 31 the formats allow. (Dolby asks for the loudness of the dialogue; the whole programme is what can be
    /// measured without finding the dialogue, and is a sound stand-in when the mix is anchored on dialogue.)
    pub fn dialnorm(&self) -> Option<u8> {
        self.integrated.map(|l| (-l).round().clamp(1.0, 31.0) as u8)
    }
}
