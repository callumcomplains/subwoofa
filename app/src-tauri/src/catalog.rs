//! What the page may choose from: destinations, stream types, layouts and the options valid for a combination.

use serde::Serialize;
use sw_backend_ffmpeg as ff;
use sw_core::formats::{bitrate_presets, bitrate_range_kbps, validate, Destination, StreamType};
use sw_core::{ChannelLayout, LAYOUTS};

#[derive(Serialize)]
pub struct StreamInfo {
    id: &'static str,
    name: &'static str,
    lossless: bool,
    sample_rates: &'static [u32],
    /// Whether any installed backend can encode this stream type today.
    available: bool,
    /// Whether Subwoofa's own encoder (no FFmpeg needed) can make it.
    native: bool,
    /// Whether the FFmpeg backend can make it.
    ffmpeg: bool,
    /// What the lower speed settings cost in file size, for the formats they change (none for the rest).
    speed_cost: Option<SpeedCost>,
}

#[derive(Serialize)]
pub struct SpeedCost {
    balanced: &'static str,
    fast: &'static str,
}

fn speed_cost(stream: StreamType) -> Option<SpeedCost> {
    use sw_core::Effort;
    Some(SpeedCost {
        balanced: sw_core::effort::size_cost(stream, Effort::Balanced)?,
        fast: sw_core::effort::size_cost(stream, Effort::Fast)?,
    })
}

#[derive(Serialize)]
pub struct DestInfo {
    id: &'static str,
    name: &'static str,
    streams: Vec<StreamInfo>,
}

#[derive(Serialize)]
pub struct Catalog {
    destinations: Vec<DestInfo>,
    /// Every stream type, for unlocked mode where destination rules are bypassed.
    all_streams: Vec<StreamInfo>,
    layouts: &'static [ChannelLayout],
    has_ffmpeg: bool,
}

#[tauri::command]
pub fn catalog() -> Catalog {
    let destinations = Destination::ALL
        .iter()
        .map(|d| DestInfo {
            id: d.id(),
            name: d.display_name(),
            streams: d
                .stream_types()
                .iter()
                .map(|s| StreamInfo {
                    id: s.id(),
                    name: s.display_name(),
                    lossless: s.is_lossless(),
                    sample_rates: sw_core::formats::sample_rates_for(*d, *s),
                    available: ff::supports(*s) || s.is_native(),
                    native: s.is_native(),
                    ffmpeg: ff::supports(*s),
                    speed_cost: speed_cost(*s),
                })
                .collect(),
        })
        .collect();
    let all_streams = StreamType::ALL
        .iter()
        .map(|s| StreamInfo {
            id: s.id(),
            name: s.display_name(),
            lossless: s.is_lossless(),
            sample_rates: s.sample_rates(),
            available: ff::supports(*s) || s.is_native(),
            native: s.is_native(),
            ffmpeg: ff::supports(*s),
            speed_cost: speed_cost(*s),
        })
        .collect();
    Catalog {
        destinations,
        all_streams,
        layouts: LAYOUTS,
        has_ffmpeg: ff::find_ffmpeg().is_some(),
    }
}

fn parse(dest: &str, stream: &str) -> Result<(Destination, StreamType), String> {
    Ok((
        Destination::from_id(dest).ok_or("unknown destination")?,
        StreamType::from_id(stream).ok_or("unknown stream type")?,
    ))
}

#[derive(Serialize)]
pub struct Options {
    /// Layout ids valid for this combination.
    layouts: Vec<&'static str>,
    bitrate_min: Option<u32>,
    bitrate_max: Option<u32>,
    presets: Vec<u32>,
    extension: &'static str,
}

/// `unlocked` bypasses the spec rules: every layout is offered and rates are not clipped.
#[tauri::command]
pub fn options(
    dest: String,
    stream: String,
    layout: String,
    sample_rate: u32,
    unlocked: bool,
) -> Result<Options, String> {
    let (d, s) = parse(&dest, &stream)?;
    let layouts = LAYOUTS
        .iter()
        .filter(|l| unlocked || validate(d, s, l, sample_rate).is_ok())
        .map(|l| l.id)
        .collect();
    let lay = ChannelLayout::find(&layout);
    let (bitrate_min, bitrate_max) = lay
        .and_then(|l| bitrate_range_kbps(d, s, l, sample_rate))
        .map_or((None, None), |(a, b)| (Some(a), Some(b)));
    let presets = lay.map(|l| bitrate_presets(d, s, l, sample_rate)).unwrap_or_default();
    Ok(Options {
        layouts,
        bitrate_min,
        bitrate_max,
        presets,
        extension: d.extension(s),
    })
}
