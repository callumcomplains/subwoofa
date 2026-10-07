#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use sw_backend_ffmpeg as ff;
use sw_core::formats::{bitrate_presets, bitrate_range_kbps, validate, Destination, StreamType};
use sw_core::downmix::{Matrix, OFF};
use sw_queue::{EncodeSpec, Queue};
use tauri::{Emitter, Manager};
use sw_core::{autoimport, Channel, ChannelLayout, LAYOUTS};

#[derive(Serialize)]
struct StreamInfo {
    id: &'static str,
    name: &'static str,
    lossless: bool,
    sample_rates: &'static [u32],
    /// Whether any installed backend can encode this stream type today.
    available: bool,
    /// Whether Subwoofa's own encoder (no FFmpeg needed) can make it.
    native: bool,
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
                    available: ff::supports(*s) || matches!(*s, StreamType::Ac3 | StreamType::Eac3 | StreamType::TrueHd),
                    native: matches!(*s, StreamType::Ac3 | StreamType::Eac3 | StreamType::TrueHd),
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
            available: ff::supports(*s) || matches!(*s, StreamType::Ac3 | StreamType::Eac3 | StreamType::TrueHd),
            native: matches!(*s, StreamType::Ac3 | StreamType::Eac3 | StreamType::TrueHd),
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
    // one multichannel file for every channel of the layout
    if let Some(all) = sw_pcm::multichannel_inputs(l, std::path::Path::new(&first)) {
        return Ok(all.into_iter().map(|(c, p)| (c.short().to_string(), p.to_string_lossy().into_owned())).collect());
    }
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
    /// Broadcast-WAV start time in seconds since midnight, if the file carries one.
    start_secs: Option<f64>,
}

/// Read file info natively (WAV/BWF/RF64/AIFF); fall back to ffprobe for other containers.
#[tauri::command]
fn probe(path: String) -> Result<Probe, String> {
    match sw_pcm::PcmReader::open(&path) {
        Ok(r) => {
            let i = r.info();
            return Ok(Probe {
                channels: Some(i.channels as u32),
                sample_rate: Some(i.sample_rate),
                bits: Some(i.bits as u32),
                duration: Some(i.duration_secs()),
                start_secs: i.start_secs(),
            });
        }
        Err(sw_pcm::Error::NotPcm(_)) => {}
        Err(e) => return Err(e.to_string()),
    }
    let ffmpeg = ff::find_ffmpeg().ok_or("not a WAV/AIFF file, and ffmpeg is not available to read it")?;
    let ffprobe = ffmpeg.with_file_name(if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" });
    let out = Command::new(&ffprobe)
        .args(["-v", "error", "-select_streams", "a:0", "-show_entries",
               "stream=channels,sample_rate,bits_per_raw_sample,bits_per_sample:format=duration", "-of", "json", &path])
        .output()
        .map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("ffprobe gave no JSON: {e}"))?;
    let s = &v["streams"][0];
    if s.is_null() {
        return Err(format!("no audio stream found in {path}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let num = |x: &serde_json::Value| x.as_str().and_then(|t| t.parse::<u32>().ok()).or_else(|| x.as_u64().map(|n| n as u32));
    let bits = num(&s["bits_per_raw_sample"]).filter(|b| *b > 0).or(num(&s["bits_per_sample"]).filter(|b| *b > 0));
    Ok(Probe {
        channels: num(&s["channels"]),
        sample_rate: num(&s["sample_rate"]),
        bits,
        duration: v["format"]["duration"].as_str().and_then(|d| d.parse().ok()),
        start_secs: None,
    })
}

/* ---- queue ---- */

#[tauri::command]
fn queue_add(queue: tauri::State<Queue>, spec: EncodeSpec, verify: bool) -> Result<u64, String> {
    queue.enqueue(spec, verify)
}
#[tauri::command]
fn queue_list(queue: tauri::State<Queue>) -> Vec<sw_queue::JobView> {
    queue.snapshot()
}
#[tauri::command]
fn queue_cancel(queue: tauri::State<Queue>, id: u64) {
    queue.cancel(id)
}
#[tauri::command]
fn queue_remove(queue: tauri::State<Queue>, id: u64) {
    queue.remove(id)
}
#[tauri::command]
fn queue_clear(queue: tauri::State<Queue>) {
    queue.clear_finished()
}
#[tauri::command]
fn queue_set_concurrency(queue: tauri::State<Queue>, n: usize) {
    queue.set_concurrency(n)
}
#[tauri::command]
fn read_log(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))
}

/* ---- downmix ---- */

// serde_json turns -inf into null, so "off" is carried explicitly as `None`.
#[derive(Serialize, Deserialize, Clone)]
struct CellDto {
    gain_db: Option<f32>,
    invert: bool,
}

#[derive(Serialize, Deserialize, Clone)]
struct MatrixDto {
    inputs: Vec<String>,
    outputs: Vec<String>,
    cells: Vec<Vec<CellDto>>,
}

fn to_dto(m: &Matrix) -> MatrixDto {
    MatrixDto {
        inputs: m.inputs.iter().map(|c| c.short().to_string()).collect(),
        outputs: m.outputs.iter().map(|c| c.short().to_string()).collect(),
        cells: m
            .cells
            .iter()
            .map(|r| r.iter().map(|c| CellDto { gain_db: (c.gain_db != OFF).then_some(c.gain_db), invert: c.invert }).collect())
            .collect(),
    }
}

fn from_dto(d: &MatrixDto) -> Result<Matrix, String> {
    let ch = |v: &Vec<String>| v.iter().map(|s| Channel::from_id(s).ok_or(format!("unknown channel '{s}'"))).collect::<Result<Vec<_>, _>>();
    let (inputs, outputs) = (ch(&d.inputs)?, ch(&d.outputs)?);
    if d.cells.len() != outputs.len() || d.cells.iter().any(|r| r.len() != inputs.len()) {
        return Err("matrix shape does not match its channels".into());
    }
    let mut m = Matrix::empty(&inputs, &outputs);
    for (o, row) in d.cells.iter().enumerate() {
        for (i, c) in row.iter().enumerate() {
            m.cells[o][i] = sw_core::downmix::Cell { gain_db: c.gain_db.unwrap_or(OFF), invert: c.invert };
        }
    }
    Ok(m)
}

#[tauri::command]
fn downmix_default(layout: String, to: String) -> Result<MatrixDto, String> {
    let l = ChannelLayout::find(&layout).ok_or("unknown layout")?;
    match to.as_str() {
        "stereo" => Ok(to_dto(&Matrix::lo_ro(l))),
        "5.1" => Ok(to_dto(&Matrix::to_51(l))),
        _ => Err("unknown downmix target".into()),
    }
}

#[derive(Serialize)]
struct SafeTrim {
    trim_db: f32,
    worst_case: f32,
    matrix: MatrixDto,
}

/// Trim that keeps full-scale input from clipping, with the matrix already trimmed.
#[tauri::command]
fn downmix_safe(matrix: MatrixDto) -> Result<SafeTrim, String> {
    let mut m = from_dto(&matrix)?;
    let trim_db = m.safe_trim_db();
    let worst_case = m.worst_case_gain().into_iter().fold(0.0, f32::max);
    m.trim(trim_db);
    Ok(SafeTrim { trim_db, worst_case, matrix: to_dto(&m) })
}

#[derive(Deserialize)]
struct CheckRequest {
    layout: String,
    inputs: HashMap<String, String>,
    matrix: MatrixDto,
    start: Option<f64>,
    duration: Option<f64>,
}

#[tauri::command]
async fn downmix_check(req: CheckRequest) -> Result<ff::downmix::SatReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let layout = ChannelLayout::find(&req.layout).ok_or("unknown layout")?;
        let m = from_dto(&req.matrix)?;
        let inputs = req.inputs.iter().filter_map(|(k, v)| Channel::from_id(k).map(|c| (c, PathBuf::from(v)))).collect();
        let ffmpeg = ff::find_ffmpeg().ok_or("ffmpeg not found on PATH")?;
        let job = ff::Job {
            stream: StreamType::Ac3, layout, inputs, output: PathBuf::new(), bitrate_kbps: None, sample_rate: None,
            dialnorm: None, start_secs: req.start, duration_secs: req.duration, frames: None, slots: None, ffmpeg,
        };
        ff::downmix::saturation_check(&job, &m).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let default_jobs = std::thread::available_parallelism().map_or(2, |n| n.get().saturating_sub(1).clamp(1, 4));
            let queue = Queue::new(default_jobs, std::sync::Arc::new(move |snap| {
                let _ = handle.emit("queue", snap);
            }));
            app.manage(queue);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            catalog, options, auto_import, probe, downmix_default, downmix_safe, downmix_check,
            queue_add, queue_list, queue_cancel, queue_remove, queue_clear, queue_set_concurrency, read_log
        ])
        .run(tauri::generate_context!())
        .expect("error while running Subwoofa");
}
