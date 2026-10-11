#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use sw_backend_ffmpeg as ff;
use sw_core::formats::{bitrate_presets, bitrate_range_kbps, validate, Destination, StreamType};
use sw_core::downmix::{Matrix, OFF};
use sw_queue::{EncodeSpec, Queue};
mod atmos;
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
    /// Whether the FFmpeg backend can make it.
    ffmpeg: bool,
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
                    sample_rates: sw_core::formats::sample_rates_for(*d, *s),
                    available: ff::supports(*s) || s.is_native(),
                    native: s.is_native(),
                    ffmpeg: ff::supports(*s),
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

#[derive(Serialize)]
struct FolderGroup {
    name: String,
    inputs: HashMap<String, String>,
}

/// Programmes in a folder: every complete set of per-channel files for `layout`.
#[tauri::command]
fn scan_folder(dir: String, layout: String) -> Result<Vec<FolderGroup>, String> {
    let l = ChannelLayout::find(&layout).ok_or("unknown layout")?;
    Ok(autoimport::scan_folder(std::path::Path::new(&dir), l)
        .into_iter()
        .map(|g| FolderGroup {
            name: g.name,
            inputs: g.inputs.into_iter().map(|(c, p)| (c.short().to_string(), p.to_string_lossy().into_owned())).collect(),
        })
        .collect())
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

#[derive(Serialize)]
struct WholeFile {
    channels: usize,
    mask: Option<u32>,
    layout: Option<&'static str>,
}

/// Which of the given layouts a multichannel file fits (see `sw_pcm::suggest_layout`).
#[tauri::command]
fn multichannel_pick(path: String, candidates: Vec<String>) -> Result<WholeFile, String> {
    let list: Vec<&'static ChannelLayout> = candidates.iter().filter_map(|id| ChannelLayout::find(id)).collect();
    let (channels, mask, pick) = sw_pcm::suggest_layout(std::path::Path::new(&path), &list).map_err(|e| e.to_string())?;
    Ok(WholeFile { channels, mask, layout: pick.map(|l| l.id) })
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

/* ---- StreamTools ---- */

pub(crate) async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

/// Opens one of the project's documentation pages in the browser. Only known page names, never free text.
#[tauri::command]
fn open_docs(page: String) -> Result<(), String> {
    const PAGES: [&str; 6] = ["ENGINES.md", "ATMOS.md", "GETTING-STARTED.md", "FORMATS.md", "TROUBLESHOOTING.md", "LEGAL.md"];
    if !PAGES.contains(&page.as_str()) {
        return Err("unknown page".into());
    }
    let url = format!("https://github.com/CallumUncensored/subwoofa/blob/main/docs/{page}");
    std::process::Command::new("cmd").args(["/C", "start", "", &url]).spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
fn app_version() -> String {
    sw_core::version::display()
}

#[tauri::command]
async fn tools_info(path: String) -> Result<sw_tools::Info, String> {
    blocking(move || sw_tools::Stream::open(std::path::Path::new(&path)).map(|s| s.info()).map_err(|e| e.to_string())).await
}

#[tauri::command]
async fn tools_pbr(path: String, window: f64) -> Result<Vec<sw_tools::PbrPoint>, String> {
    blocking(move || sw_tools::Stream::open(std::path::Path::new(&path)).map(|s| s.pbr(window.max(0.1))).map_err(|e| e.to_string())).await
}

#[tauri::command]
async fn tools_trim(path: String, output: String, start: f64, end: Option<f64>) -> Result<String, String> {
    blocking(move || {
        let s = sw_tools::Stream::open(std::path::Path::new(&path)).map_err(|e| e.to_string())?;
        let (first, n, at) = s.trim(std::path::Path::new(&output), start, end).map_err(|e| e.to_string())?;
        Ok(format!("Wrote {n} frames starting at frame {first} ({at:.3} s) to {output}"))
    })
    .await
}

#[tauri::command]
async fn tools_split(path: String, output: String, at: Vec<f64>) -> Result<Vec<String>, String> {
    blocking(move || {
        let s = sw_tools::Stream::open(std::path::Path::new(&path)).map_err(|e| e.to_string())?;
        let ext = std::path::Path::new(&path).extension().and_then(|x| x.to_str()).unwrap_or("bin").to_string();
        // the save dialog returns a file name with an extension: drop it, the parts add their own
        let prefix = std::path::Path::new(&output).with_extension("");
        let paths = s.split(&prefix, &ext, &at).map_err(|e| e.to_string())?;
        Ok(paths.iter().map(|p| p.to_string_lossy().into_owned()).collect())
    })
    .await
}

#[tauri::command]
async fn tools_append(inputs: Vec<String>, output: String) -> Result<String, String> {
    blocking(move || {
        let refs: Vec<&std::path::Path> = inputs.iter().map(|p| std::path::Path::new(p)).collect();
        sw_tools::append(&refs, std::path::Path::new(&output)).map_err(|e| e.to_string())?;
        Ok(format!("Wrote {output}"))
    })
    .await
}

#[tauri::command]
async fn tools_replace(path: String, with: String, at: f64, output: String, insert: bool) -> Result<String, String> {
    blocking(move || {
        let (base, with, out) = (std::path::Path::new(&path), std::path::Path::new(&with), std::path::Path::new(&output));
        if insert {
            let t = sw_tools::insert(base, with, at, out).map_err(|e| e.to_string())?;
            Ok(format!("Wrote {output}\nInserted at {t:.3} s"))
        } else {
            let (t, removed) = sw_tools::replace(base, with, at, out).map_err(|e| e.to_string())?;
            Ok(format!("Wrote {output}\nReplaced {removed:.3} s starting at {t:.3} s"))
        }
    })
    .await
}

#[tauri::command]
async fn tools_stripe(path: String) -> Result<serde_json::Value, String> {
    blocking(move || {
        let s = sw_tools::read_stripe(std::path::Path::new(&path)).map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "fps_code": s.fps_code, "start_samples": s.start_samples, "reference_samples": s.reference_samples }))
    })
    .await
}

#[tauri::command]
async fn tools_restripe(path: String, output: String, fps: String, start: String) -> Result<String, String> {
    blocking(move || {
        let code = sw_tools::fps_code(&fps).ok_or_else(|| format!("unknown frame rate {fps}"))?;
        let tc = sw_tools::parse_timecode(&start).ok_or_else(|| format!("{start} is not a timecode (hh:mm:ss:ff)"))?;
        sw_tools::restripe(std::path::Path::new(&path), std::path::Path::new(&output), code, tc).map_err(|e| e.to_string())?;
        Ok(format!("Wrote {output}\nStart {start} at {fps} fps"))
    })
    .await
}

#[tauri::command]
async fn tools_add_silence(path: String, output: String, head: f64, tail: f64) -> Result<String, String> {
    blocking(move || {
        let (h, t) = sw_tools::add_silence(std::path::Path::new(&path), std::path::Path::new(&output), head, tail).map_err(|e| e.to_string())?;
        Ok(format!("Wrote {output}\n{h} silent frames at the head, {t} at the tail"))
    })
    .await
}

#[derive(serde::Serialize, Clone)]
pub(crate) struct VerifyOut {
    pub(crate) report: sw_verify::Report,
    pub(crate) text: String,
}

/// Native verification of a stream file; `deep` adds the decode, statistics and header dumps.
#[tauri::command]
async fn verify_file(path: String, deep: bool) -> Result<VerifyOut, String> {
    blocking(move || {
        let opt = sw_verify::Options { deep, ffmpeg: ff::find_ffmpeg(), reference_verifier: None };
        let report = sw_verify::verify(std::path::Path::new(&path), &opt).map_err(|e| e.to_string())?;
        let text = report.to_text();
        Ok(VerifyOut { report, text })
    })
    .await
}

/* ---- tools the user can point at ---- */

#[derive(serde::Serialize)]
struct ToolStatus {
    ffmpeg: Option<String>,
    ffmpeg_version: Option<String>,
    mas_dir: Option<String>,
    mas_files: Vec<(String, bool)>,
}

/// Use the ffmpeg and the DTS-HD Master Audio Suite folder the user chose (empty means look in the usual places).
#[tauri::command]
fn set_tool_paths(ffmpeg: String, mas_dir: String) {
    let opt = |s: String| (!s.trim().is_empty()).then(|| std::path::PathBuf::from(s.trim()));
    ff::set_ffmpeg_override(opt(ffmpeg));
    sw_verify::set_mas_dir(opt(mas_dir));
}

/// What the app finds with the current paths, for the Settings page.
#[tauri::command]
async fn tool_status() -> ToolStatus {
    blocking(move || {
        let ffmpeg = ff::find_ffmpeg();
        let version = ffmpeg.as_ref().and_then(|p| {
            let mut c = std::process::Command::new(p);
            c.arg("-version").stdin(std::process::Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                c.creation_flags(0x0800_0000);
            }
            c.output().ok().and_then(|o| String::from_utf8_lossy(&o.stdout).lines().next().map(str::to_string))
        });
        let dir = sw_verify::mas_dir();
        let files = ["DTSHDVerify.exe", "DtsJobQueue.exe", "dtshd.exe"].iter().map(|n| (n.to_string(), dir.as_ref().map_or(false, |d| d.join(n).is_file()))).collect();
        Ok(ToolStatus { ffmpeg: ffmpeg.map(|p| p.display().to_string()), ffmpeg_version: version, mas_dir: dir.map(|d| d.display().to_string()), mas_files: files })
    })
    .await
    .unwrap_or(ToolStatus { ffmpeg: None, ffmpeg_version: None, mas_dir: None, mas_files: vec![] })
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
fn write_text(path: String, text: String) -> Result<(), String> {
    std::fs::write(&path, text).map_err(|e| format!("{path}: {e}"))
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
            app.manage(atmos::State::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            catalog, options, auto_import, multichannel_pick, scan_folder, probe, downmix_default, downmix_safe, downmix_check,
            queue_add, queue_list, queue_cancel, queue_remove, queue_clear, queue_set_concurrency, read_log, write_text,
            app_version, open_docs, set_tool_paths, tool_status, tools_info, tools_pbr, tools_trim, tools_split, tools_append, tools_replace, tools_stripe, tools_restripe, tools_add_silence, verify_file,
            atmos::atmos_locate, atmos::atmos_inspect, atmos::atmos_status, atmos::atmos_cancel, atmos::atmos_start
        ])
        .run(tauri::generate_context!())
        .expect("error while running Subwoofa");
}
