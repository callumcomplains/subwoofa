//! StreamTools and Verify for the page: info, bit rate, cut, join, relabel and the structure check.

use crate::blocking;
use sw_backend_ffmpeg as ff;

#[tauri::command]
pub async fn tools_info(path: String) -> Result<sw_tools::Info, String> {
    blocking(move || {
        sw_tools::Stream::open(std::path::Path::new(&path))
            .map(|s| s.info())
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn tools_pbr(path: String, window: f64) -> Result<Vec<sw_tools::PbrPoint>, String> {
    blocking(move || {
        sw_tools::Stream::open(std::path::Path::new(&path))
            .map(|s| s.pbr(window.max(0.1)))
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn tools_trim(path: String, output: String, start: f64, end: Option<f64>) -> Result<String, String> {
    blocking(move || {
        let s = sw_tools::Stream::open(std::path::Path::new(&path)).map_err(|e| e.to_string())?;
        let (first, n, at) = s
            .trim(std::path::Path::new(&output), start, end)
            .map_err(|e| e.to_string())?;
        Ok(format!(
            "Wrote {n} frames starting at frame {first} ({at:.3} s) to {output}"
        ))
    })
    .await
}

#[tauri::command]
pub async fn tools_split(path: String, output: String, at: Vec<f64>) -> Result<Vec<String>, String> {
    blocking(move || {
        let s = sw_tools::Stream::open(std::path::Path::new(&path)).map_err(|e| e.to_string())?;
        let ext = std::path::Path::new(&path)
            .extension()
            .and_then(|x| x.to_str())
            .unwrap_or("bin")
            .to_string();
        // the save dialog returns a file name with an extension: drop it, the parts add their own
        let prefix = std::path::Path::new(&output).with_extension("");
        let paths = s.split(&prefix, &ext, &at).map_err(|e| e.to_string())?;
        Ok(paths.iter().map(|p| p.to_string_lossy().into_owned()).collect())
    })
    .await
}

#[tauri::command]
pub async fn tools_append(inputs: Vec<String>, output: String) -> Result<String, String> {
    blocking(move || {
        let refs: Vec<&std::path::Path> = inputs.iter().map(std::path::Path::new).collect();
        sw_tools::append(&refs, std::path::Path::new(&output)).map_err(|e| e.to_string())?;
        Ok(format!("Wrote {output}"))
    })
    .await
}

#[tauri::command]
pub async fn tools_replace(
    path: String,
    with: String,
    at: f64,
    output: String,
    insert: bool,
) -> Result<String, String> {
    blocking(move || {
        let (base, with, out) = (
            std::path::Path::new(&path),
            std::path::Path::new(&with),
            std::path::Path::new(&output),
        );
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
pub async fn tools_stripe(path: String) -> Result<serde_json::Value, String> {
    blocking(move || {
        let s = sw_tools::read_stripe(std::path::Path::new(&path)).map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "fps_code": s.fps_code, "start_samples": s.start_samples, "reference_samples": s.reference_samples }))
    })
    .await
}

#[tauri::command]
pub async fn tools_restripe(path: String, output: String, fps: String, start: String) -> Result<String, String> {
    blocking(move || {
        let code = sw_tools::fps_code(&fps).ok_or_else(|| format!("unknown frame rate {fps}"))?;
        let tc = sw_tools::parse_timecode(&start).ok_or_else(|| format!("{start} is not a timecode (hh:mm:ss:ff)"))?;
        sw_tools::restripe(std::path::Path::new(&path), std::path::Path::new(&output), code, tc)
            .map_err(|e| e.to_string())?;
        Ok(format!("Wrote {output}\nStart {start} at {fps} fps"))
    })
    .await
}

#[tauri::command]
pub async fn tools_add_silence(path: String, output: String, head: f64, tail: f64) -> Result<String, String> {
    blocking(move || {
        let (h, t) = sw_tools::add_silence(std::path::Path::new(&path), std::path::Path::new(&output), head, tail)
            .map_err(|e| e.to_string())?;
        Ok(format!(
            "Wrote {output}\n{h} silent frames at the head, {t} at the tail"
        ))
    })
    .await
}

/// What an AC-3 / E-AC-3 stream says about its dialogue level.
#[derive(serde::Serialize)]
pub struct DialnormInfo {
    eac3: bool,
    frames: usize,
    level: u8,
}

#[tauri::command]
pub async fn ac3_dialnorm_info(path: String) -> Result<DialnormInfo, String> {
    blocking(move || {
        let data = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
        let found = sw_codec_ac3::edit::inspect(&data)?;
        Ok(DialnormInfo {
            eac3: found.eac3,
            frames: found.frames,
            level: found.previous,
        })
    })
    .await
}

/// Writes a copy of an AC-3 / E-AC-3 stream with a new dialnorm; the audio is not touched.
#[tauri::command]
pub async fn ac3_set_dialnorm(path: String, output: String, level: u8) -> Result<String, String> {
    blocking(move || {
        let mut data = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
        let done = sw_codec_ac3::edit::set_dialnorm(&mut data, level)?;
        std::fs::write(&output, data).map_err(|e| format!("{output}: {e}"))?;
        Ok(format!(
            "Wrote {output}
Dialnorm -{level} dBFS (was -{}); {} of {} frames rewritten",
            done.previous, done.changed, done.frames
        ))
    })
    .await
}

#[derive(serde::Serialize, Clone)]
pub struct VerifyOut {
    pub(crate) report: sw_verify::Report,
    pub(crate) text: String,
    /// The QC report page.
    pub(crate) html: String,
}

/// What the encode was made from, for the comparison in a deep check.
#[derive(serde::Deserialize)]
pub struct Against {
    /// One of the source files; the others are found from it (or it holds every channel).
    first: String,
    layout: String,
    start: Option<f64>,
    duration: Option<f64>,
}

/// Native verification of a stream file; `deep` adds the decode, statistics and header dumps, and `against` lines
/// the decode up with the source it was made from (which needs the deep check).
#[tauri::command]
pub async fn verify_file(path: String, deep: bool, against: Option<Against>) -> Result<VerifyOut, String> {
    blocking(move || {
        let deep = deep || against.is_some();
        let opt = sw_verify::Options {
            deep,
            // a deep check also measures the audio, for the QC report
            analyse: deep,
            ffmpeg: ff::find_ffmpeg(),
            reference_verifier: None,
        };
        let file = std::path::Path::new(&path);
        let mut report = sw_verify::verify(file, &opt).map_err(|e| e.to_string())?;
        if let Some(a) = against {
            let layout = sw_core::ChannelLayout::find(&a.layout).ok_or("unknown layout")?;
            let inputs = crate::sources::import_inputs(std::path::Path::new(&a.first), layout);
            if inputs.len() < layout.channels.len() {
                return Err(format!(
                    "Only {} of the {} channels of {} were found from that source file.",
                    inputs.len(),
                    layout.channels.len(),
                    layout.id
                ));
            }
            let ffmpeg = opt
                .ffmpeg
                .as_deref()
                .ok_or("The comparison needs FFmpeg to decode the file.")?;
            let source = sw_verify::Source {
                layout,
                inputs: &inputs,
                order: sw_pcm::ChannelOrder::Auto,
                start: a.start,
                duration: a.duration,
                delay_frames: 0,
                gain_db: None,
                order_known: false,
            };
            sw_verify::compare_with_source(&mut report, file, ffmpeg, &source)?;
        }
        let (text, html) = (report.to_text(), report.to_html());
        // the page shows the checks; the spectrograms only go into the saved report
        report.analysis = None;
        Ok(VerifyOut { report, text, html })
    })
    .await
}
