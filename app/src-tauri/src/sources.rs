//! Input files: folders of per-channel files, auto-import, multichannel files and probing a file's format.

use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use sw_backend_ffmpeg as ff;
use sw_core::{autoimport, ChannelLayout};

#[derive(Serialize)]
pub struct FolderGroup {
    name: String,
    inputs: HashMap<String, String>,
}

/// Programmes in a folder: every complete set of per-channel files for `layout`.
#[tauri::command]
pub fn scan_folder(dir: String, layout: String) -> Result<Vec<FolderGroup>, String> {
    let l = ChannelLayout::find(&layout).ok_or("unknown layout")?;
    Ok(autoimport::scan_folder(std::path::Path::new(&dir), l)
        .into_iter()
        .map(|g| FolderGroup {
            name: g.name,
            inputs: g
                .inputs
                .into_iter()
                .map(|(c, p)| (c.short().to_string(), p.to_string_lossy().into_owned()))
                .collect(),
        })
        .collect())
}

#[tauri::command]
pub fn auto_import(first: String, layout: String) -> Result<HashMap<String, String>, String> {
    let l = ChannelLayout::find(&layout).ok_or("unknown layout")?;
    // one multichannel file for every channel of the layout
    if let Some(all) = sw_pcm::multichannel_inputs(l, std::path::Path::new(&first)) {
        return Ok(all
            .into_iter()
            .map(|(c, p)| (c.short().to_string(), p.to_string_lossy().into_owned()))
            .collect());
    }
    Ok(autoimport::auto_import(&PathBuf::from(first), l)
        .into_iter()
        .map(|(c, p)| (c.short().to_string(), p.to_string_lossy().into_owned()))
        .collect())
}

#[derive(Serialize)]
pub struct WholeFile {
    channels: usize,
    mask: Option<u32>,
    layout: Option<&'static str>,
}

/// Which of the given layouts a multichannel file fits (see `sw_pcm::suggest_layout`).
#[tauri::command]
pub fn multichannel_pick(path: String, candidates: Vec<String>) -> Result<WholeFile, String> {
    let list: Vec<&'static ChannelLayout> = candidates.iter().filter_map(|id| ChannelLayout::find(id)).collect();
    let (channels, mask, pick) =
        sw_pcm::suggest_layout(std::path::Path::new(&path), &list).map_err(|e| e.to_string())?;
    Ok(WholeFile {
        channels,
        mask,
        layout: pick.map(|l| l.id),
    })
}

#[derive(Serialize, Default)]
pub struct Probe {
    channels: Option<u32>,
    sample_rate: Option<u32>,
    bits: Option<u32>,
    duration: Option<f64>,
    /// Broadcast-WAV start time in seconds since midnight, if the file carries one.
    start_secs: Option<f64>,
}

/// Read file info natively (WAV/BWF/RF64/AIFF); fall back to ffprobe for other containers.
#[tauri::command]
pub fn probe(path: String) -> Result<Probe, String> {
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
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=channels,sample_rate,bits_per_raw_sample,bits_per_sample:format=duration",
            "-of",
            "json",
            &path,
        ])
        .output()
        .map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("ffprobe gave no JSON: {e}"))?;
    let s = &v["streams"][0];
    if s.is_null() {
        return Err(format!(
            "no audio stream found in {path}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let num = |x: &serde_json::Value| {
        x.as_str()
            .and_then(|t| t.parse::<u32>().ok())
            .or_else(|| x.as_u64().map(|n| n as u32))
    };
    let bits = num(&s["bits_per_raw_sample"])
        .filter(|b| *b > 0)
        .or(num(&s["bits_per_sample"]).filter(|b| *b > 0));
    Ok(Probe {
        channels: num(&s["channels"]),
        sample_rate: num(&s["sample_rate"]),
        bits,
        duration: v["format"]["duration"].as_str().and_then(|d| d.parse().ok()),
        start_secs: None,
    })
}
