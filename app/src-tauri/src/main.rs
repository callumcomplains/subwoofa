#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use sw_backend_ffmpeg as ff;
use sw_core::formats::{bitrate_presets, bitrate_range_kbps, validate, Destination, StreamType};
use sw_core::{autoimport, Channel, ChannelLayout, LAYOUTS};

#[derive(Serialize)]
struct StreamInfo {
    id: &'static str,
    name: &'static str,
    lossless: bool,
    sample_rates: &'static [u32],
    /// Whether any installed backend can encode this stream type today.
    available: bool,
}

#[derive(Serialize)]
struct DestInfo {
    id: &'static str,
    name: &'static str,
    streams: Vec<StreamInfo>,
}

#[derive(Serialize)]
struct Catalog {
    destinations: Vec<DestInfo>,
    /// Every stream type, for unlocked mode where destination rules are bypassed.
    all_streams: Vec<StreamInfo>,
    layouts: &'static [ChannelLayout],
    has_ffmpeg: bool,
}

#[tauri::command]
fn catalog() -> Catalog {
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
                    sample_rates: s.sample_rates(),
                    available: ff::supports(*s),
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
            available: ff::supports(*s),
        })
        .collect();
    Catalog { destinations, all_streams, layouts: LAYOUTS, has_ffmpeg: ff::find_ffmpeg().is_some() }
}

fn parse(dest: &str, stream: &str) -> Result<(Destination, StreamType), String> {
    Ok((
        Destination::from_id(dest).ok_or("unknown destination")?,
        StreamType::from_id(stream).ok_or("unknown stream type")?,
    ))
}

#[derive(Serialize)]
struct Options {
    /// Layout ids valid for this combination.
    layouts: Vec<&'static str>,
    bitrate_min: Option<u32>,
    bitrate_max: Option<u32>,
    presets: Vec<u32>,
    extension: &'static str,
}

/// `unlocked` bypasses the spec rules: every layout is offered and rates are not clipped.
#[tauri::command]
fn options(dest: String, stream: String, layout: String, sample_rate: u32, unlocked: bool) -> Result<Options, String> {
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
    Ok(Options { layouts, bitrate_min, bitrate_max, presets, extension: d.extension(s) })
}

#[tauri::command]
fn auto_import(first: String, layout: String) -> Result<HashMap<String, String>, String> {
    let l = ChannelLayout::find(&layout).ok_or("unknown layout")?;
    Ok(autoimport::auto_import(&PathBuf::from(first), l)
        .into_iter()
        .map(|(c, p)| (c.short().to_string(), p.to_string_lossy().into_owned()))
        .collect())
}

#[derive(Serialize, Default)]
struct Probe {
    channels: Option<u32>,
    sample_rate: Option<u32>,
    bits: Option<u32>,
    duration: Option<f64>,
}

#[tauri::command]
fn probe(path: String) -> Result<Probe, String> {
    let ffmpeg = ff::find_ffmpeg().ok_or("ffmpeg not found")?;
    let ffprobe = ffmpeg.with_file_name(if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" });
    let out = Command::new(ffprobe)
        .args(["-v", "error", "-select_streams", "a:0", "-show_entries",
               "stream=channels,sample_rate,bits_per_raw_sample,bits_per_sample:format=duration", "-of", "json", &path])
        .output()
        .map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let s = &v["streams"][0];
    let num = |x: &serde_json::Value| x.as_str().and_then(|t| t.parse::<u32>().ok()).or_else(|| x.as_u64().map(|n| n as u32));
    let bits = num(&s["bits_per_raw_sample"]).filter(|b| *b > 0).or(num(&s["bits_per_sample"]).filter(|b| *b > 0));
    Ok(Probe {
        channels: num(&s["channels"]),
        sample_rate: num(&s["sample_rate"]),
        bits,
        duration: v["format"]["duration"].as_str().and_then(|d| d.parse().ok()),
    })
}

#[derive(Deserialize)]
struct EncodeRequest {
    dest: String,
    stream: String,
    layout: String,
    sample_rate: u32,
    bitrate: Option<u32>,
    dialnorm: Option<u8>,
    start: Option<f64>,
    duration: Option<f64>,
    inputs: HashMap<String, String>,
    output: String,
    /// Skip spec validation (custom bit rates, sample rates, unusual combinations).
    #[serde(default)]
    unlocked: bool,
}

#[tauri::command]
async fn encode(req: EncodeRequest) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (d, s) = parse(&req.dest, &req.stream)?;
        let layout = ChannelLayout::find(&req.layout).ok_or("unknown layout")?;
        if !req.unlocked {
            validate(d, s, layout, req.sample_rate).map_err(|e| format!("invalid combination: {e:?}"))?;
        }
        let inputs = req
            .inputs
            .iter()
            .filter_map(|(k, v)| Channel::from_id(k).map(|c| (c, PathBuf::from(v))))
            .collect();
        let ffmpeg = ff::find_ffmpeg().ok_or("ffmpeg not found on PATH")?;
        let job = ff::Job {
            stream: s,
            layout,
            inputs,
            output: PathBuf::from(&req.output),
            bitrate_kbps: req.bitrate,
            sample_rate: Some(req.sample_rate),
            dialnorm: req.dialnorm,
            start_secs: req.start,
            duration_secs: req.duration,
            ffmpeg,
        };
        ff::run(&job).map_err(|e| e.to_string())?;
        Ok(format!("wrote {}", req.output))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![catalog, options, auto_import, probe, encode])
        .run(tauri::generate_context!())
        .expect("error while running Subwoofa");
}
