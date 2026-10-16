//! The Dolby Encoding Engine's everyday jobs: Dolby's own meter (it finds the dialogue), and decode, transcode,
//! trim and join, and loudness correction for Dolby Digital and Digital Plus.

use crate::blocking;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use sw_core::{Channel, ChannelLayout};
use tauri::Emitter;

/// Whether the job in progress should stop.
#[derive(Default)]
pub struct State {
    cancel: Arc<AtomicBool>,
}

/// Remembers the engine the user chose (empty goes back to the usual places).
#[tauri::command]
pub fn set_dee_path(path: String) {
    sw_backend_dee::set_dee_override((!path.trim().is_empty()).then(|| PathBuf::from(path.trim())));
}

/// Measures the files the page has chosen with Dolby's meter, over the span if one is set.
#[tauri::command]
pub async fn measure_loudness_dolby(
    layout: String,
    inputs: HashMap<String, String>,
    start: Option<f64>,
    duration: Option<f64>,
    dialogue: bool,
) -> Result<sw_backend_dee::Measured, String> {
    blocking(move || {
        let layout = ChannelLayout::find(&layout).ok_or("unknown layout")?;
        let files: HashMap<Channel, PathBuf> = inputs
            .iter()
            .filter_map(|(k, v)| Channel::from_id(k).map(|c| (c, PathBuf::from(v))))
            .collect();
        sw_queue::dolby::measure(
            layout,
            &files,
            (start, duration),
            dialogue,
            &AtomicBool::new(false),
            &mut |_| {},
        )
    })
    .await
}

/// Runs one engine job. Progress goes out as `dolby` events (a fraction from 0 to 1).
#[tauri::command]
pub async fn dolby_run(
    app: tauri::AppHandle,
    state: tauri::State<'_, State>,
    job: sw_backend_dee::TaskJob,
) -> Result<sw_backend_dee::TaskDone, String> {
    state.cancel.store(false, Ordering::Relaxed);
    let cancel = state.cancel.clone();
    blocking(move || {
        let engine = sw_queue::dolby::find_engine()?;
        let work = sw_queue::dolby::job_folder("app", job.source.first_file())?;
        let result = sw_backend_dee::run_task(
            &engine,
            &job,
            &work,
            &mut |p| {
                let _ = app.emit("dolby", p);
            },
            &mut |_| {},
            &cancel,
        );
        sw_queue::dolby::remove_job_folder(&work);
        result.map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub fn dolby_cancel(state: tauri::State<State>) {
    state.cancel.store(true, Ordering::Relaxed);
}
