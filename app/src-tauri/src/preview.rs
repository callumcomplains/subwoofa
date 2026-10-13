//! The listening preview: encodes a short excerpt of the current job and hands the page the audio to play.

use crate::blocking;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use sw_backend_ffmpeg as ff;
use sw_queue::preview::{self, Preview};
use sw_queue::run::Failure;
use sw_queue::EncodeSpec;
use tauri::Emitter;

/// Whether the preview in progress should stop.
#[derive(Default)]
pub struct State {
    cancel: Arc<AtomicBool>,
}

/// Where the preview files go; a fresh folder for each preview.
fn folder() -> PathBuf {
    std::env::temp_dir().join("subwoofa-preview")
}

/// Encodes `seconds` of the job described by `spec` and returns the files to play. Progress goes out as `preview`
/// events (a fraction from 0 to 1).
#[tauri::command]
pub async fn make_preview(
    app: tauri::AppHandle,
    state: tauri::State<'_, State>,
    spec: EncodeSpec,
    seconds: f64,
) -> Result<Preview, String> {
    let ffmpeg =
        ff::find_ffmpeg().ok_or("A preview needs FFmpeg to decode the encode again. Set its path in Settings.")?;
    state.cancel.store(false, Ordering::Relaxed);
    let handle = app.clone();
    let cancel = state.cancel.clone();
    blocking(move || {
        let dir = folder();
        let _ = std::fs::remove_dir_all(&dir);
        preview::build(&spec, &ffmpeg, seconds, &dir, &cancel, &mut |p| {
            let _ = handle.emit("preview", p);
        })
        .map_err(|f| match f {
            Failure::Canceled => "canceled".to_string(),
            Failure::Error(m) => m,
        })
    })
    .await
}

#[tauri::command]
pub fn preview_cancel(state: tauri::State<State>) {
    state.cancel.store(true, Ordering::Relaxed);
}

/// The bytes of one of the preview's files (nothing else can be read this way).
#[tauri::command]
pub fn read_preview_file(path: String) -> Result<tauri::ipc::Response, String> {
    let path = PathBuf::from(path);
    if !path.starts_with(folder()) || path.components().any(|c| c.as_os_str() == "..") {
        return Err("not a preview file".into());
    }
    std::fs::read(&path)
        .map(tauri::ipc::Response::new)
        .map_err(|e| format!("{}: {e}", path.display()))
}
