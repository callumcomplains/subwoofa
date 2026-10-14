#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! The desktop app: a Tauri window around the page in `app/ui`. This file is the bridge: every function with
//! `#[tauri::command]` is something the page can call (the catalogue of formats, probing input files, the queue,
//! StreamTools, Verify, the tool paths). The encoding itself is in the crates; nothing here does audio work.

mod atmos;
mod catalog;
mod downmix;
mod jobs;
mod loudness;
mod preview;
mod sources;
mod streamtools;
mod toolpaths;

use sw_queue::Queue;
use tauri::{Emitter, Manager};

// the system allocator on Windows takes a lock for every allocation, which stops the encoders scaling over cores
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

pub(crate) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

/// Opens one of the project's documentation pages in the browser. Only known page names, never free text.
#[tauri::command]
fn open_docs(page: String) -> Result<(), String> {
    const PAGES: [&str; 6] = [
        "ENGINES.md",
        "ATMOS.md",
        "GETTING-STARTED.md",
        "FORMATS.md",
        "TROUBLESHOOTING.md",
        "LEGAL.md",
    ];
    if !PAGES.contains(&page.as_str()) {
        return Err("unknown page".into());
    }
    let url = format!("https://github.com/callumcomplains/subwoofa/blob/main/docs/{page}");
    std::process::Command::new("cmd")
        .args(["/C", "start", "", &url])
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn app_version() -> String {
    sw_core::version::display()
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let default_jobs =
                std::thread::available_parallelism().map_or(2, |n| n.get().saturating_sub(1).clamp(1, 4));
            let queue = Queue::new(
                default_jobs,
                std::sync::Arc::new(move |snap| {
                    let _ = handle.emit("queue", snap);
                }),
            );
            app.manage(queue);
            app.manage(atmos::State::default());
            app.manage(preview::State::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            catalog::catalog,
            catalog::options,
            sources::auto_import,
            sources::multichannel_pick,
            sources::scan_folder,
            sources::probe,
            downmix::downmix_default,
            downmix::downmix_safe,
            downmix::downmix_check,
            jobs::queue_add,
            jobs::queue_list,
            jobs::queue_cancel,
            jobs::queue_remove,
            jobs::queue_clear,
            jobs::queue_set_concurrency,
            jobs::read_log,
            loudness::measure_loudness,
            preview::make_preview,
            preview::preview_cancel,
            preview::read_preview_file,
            jobs::write_text,
            app_version,
            open_docs,
            toolpaths::set_tool_paths,
            toolpaths::tool_status,
            toolpaths::fetch_ffmpeg,
            streamtools::tools_info,
            streamtools::tools_pbr,
            streamtools::tools_trim,
            streamtools::tools_split,
            streamtools::tools_append,
            streamtools::tools_replace,
            streamtools::tools_stripe,
            streamtools::tools_restripe,
            streamtools::tools_add_silence,
            streamtools::ac3_dialnorm_info,
            streamtools::ac3_set_dialnorm,
            streamtools::verify_file,
            atmos::atmos_locate,
            atmos::atmos_inspect,
            atmos::atmos_status,
            atmos::atmos_cancel,
            atmos::atmos_start
        ])
        .run(tauri::generate_context!())
        .expect("error while running Subwoofa");
}
