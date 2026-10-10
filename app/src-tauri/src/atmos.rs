//! Atmos jobs for the app: a master (or channel bed) goes through the Dolby Encoding Engine the user supplies.
//! One job runs at a time; the page polls `atmos_status` for progress, log lines and the verified result.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use sw_backend_dee as dee;

#[derive(Default, Clone, Serialize)]
pub struct Status {
    pub running: bool,
    pub progress: f32,
    pub lines: Vec<String>,
    pub error: Option<String>,
    pub done: Option<Done>,
}

#[derive(Clone, Serialize)]
pub struct Done {
    pub output: String,
    pub seconds: f64,
    pub verify: Option<crate::VerifyOut>,
}

#[derive(Default)]
pub struct State {
    status: Arc<Mutex<Status>>,
    cancel: Mutex<Option<Arc<AtomicBool>>>,
}

#[derive(Deserialize)]
pub struct Req {
    pub input: String,
    pub output: String,
    pub dee: Option<String>,
    /// "truehd" or "ddp"
    pub format: String,
    pub clusters: u8,
    pub drc: String,
    pub legacy: bool,
    pub p8: bool,
    pub p6: bool,
    pub p2: bool,
    pub p2_format: String,
    pub optimize: bool,
    pub bitrate: u32,
    pub dialnorm: Option<i32>,
    pub frame_rate: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub metering: Option<String>,
}

#[derive(Serialize)]
pub struct Inspect {
    pub kind: String,
    pub text: String,
}

fn kind_name(k: dee::InputKind) -> &'static str {
    match k {
        dee::InputKind::Wav => "channel bed (WAV)",
        dee::InputKind::Adm => "ADM BWF master",
        dee::InputKind::Damf => "DAMF master",
        dee::InputKind::AtmosMezz => "Atmos mezzanine",
    }
}

#[tauri::command]
pub fn atmos_locate(configured: Option<String>) -> Option<String> {
    dee::find_dee(configured.as_deref().filter(|s| !s.is_empty()).map(std::path::Path::new)).map(|p| p.display().to_string())
}

#[tauri::command]
pub async fn atmos_inspect(path: String, dee_path: Option<String>) -> Result<Inspect, String> {
    crate::blocking(move || {
        let file = PathBuf::from(&path);
        let kind = dee::InputKind::detect(&file).map_err(|e| e.to_string())?;
        let text = if kind == dee::InputKind::Wav {
            "A plain multichannel WAV: it can make Dolby Digital Plus Atmos (a channel bed up to 7.1.4). TrueHD Atmos needs an Atmos master.".to_string()
        } else {
            let d = dee::find_dee(dee_path.as_deref().filter(|s| !s.is_empty()).map(std::path::Path::new)).ok_or_else(|| dee::Error::NotFound.to_string())?;
            dee::master_info(&d, &file).map_err(|e| e.to_string())?
        };
        Ok(Inspect { kind: kind_name(kind).to_string(), text })
    })
    .await
}

#[tauri::command]
pub fn atmos_status(state: tauri::State<State>) -> Status {
    state.status.lock().unwrap().clone()
}

#[tauri::command]
pub fn atmos_cancel(state: tauri::State<State>) {
    if let Some(c) = state.cancel.lock().unwrap().as_ref() {
        c.store(true, Ordering::Relaxed);
    }
}

#[tauri::command]
pub fn atmos_start(state: tauri::State<State>, req: Req) -> Result<(), String> {
    {
        let s = state.status.lock().unwrap();
        if s.running {
            return Err("an Atmos job is already running".into());
        }
    }
    let d = dee::find_dee(req.dee.as_deref().filter(|s| !s.is_empty()).map(std::path::Path::new)).ok_or_else(|| dee::Error::NotFound.to_string())?;
    let drc: dee::Drc = req.drc.parse()?;
    let target = match req.format.as_str() {
        "truehd" => {
            let pres = |on: bool| on.then_some(dee::Presentation { drc, surround_3db: true });
            dee::Target::TrueHd(dee::TrueHdOptions {
                drc,
                spatial_clusters: req.clusters,
                legacy_authoring: req.legacy,
                presentation_8ch: pres(req.p8),
                presentation_6ch: pres(req.p6),
                presentation_2ch: req.p2.then(|| (drc, true, req.p2_format.clone())),
                optimize_data_rate: req.optimize,
            })
        }
        "ddp" => dee::Target::Ddp(dee::DdpOptions { data_rate_kbps: req.bitrate, drc_line: drc, drc_rf: drc, ..Default::default() }),
        other => return Err(format!("unknown format '{other}'")),
    };
    let mut job = dee::AtmosJob::new(&req.input, &req.output, target);
    job.custom_dialnorm = req.dialnorm.filter(|d| *d != 0);
    job.frame_rate = req.frame_rate.filter(|s| !s.is_empty());
    job.start = req.start.filter(|s| !s.trim().is_empty());
    job.end = req.end.filter(|s| !s.trim().is_empty());
    job.metering = req.metering.filter(|s| !s.is_empty());
    // fail fast on settings DEE would reject
    dee::profile_xml(&job, std::path::Path::new("temp")).map_err(|e| e.to_string())?;

    let cancel = Arc::new(AtomicBool::new(false));
    *state.cancel.lock().unwrap() = Some(cancel.clone());
    *state.status.lock().unwrap() = Status { running: true, ..Default::default() };
    let status = state.status.clone();
    std::thread::spawn(move || {
        let work = std::env::temp_dir().join(format!("subwoofa_atmos_{}", std::process::id()));
        let result = dee::run(
            &d,
            &job,
            &work,
            &mut |p| status.lock().unwrap().progress = p,
            &mut |l| {
                let mut s = status.lock().unwrap();
                s.lines.push(l.to_string());
                if s.lines.len() > 400 {
                    s.lines.remove(0);
                }
            },
            &cancel,
        );
        let _ = std::fs::remove_dir_all(&work);
        let mut s = status.lock().unwrap();
        s.running = false;
        match result {
            Ok(done) => {
                let verify = sw_verify::verify(&done.output, &sw_verify::Options::default()).ok().map(|r| {
                    let text = r.to_text();
                    crate::VerifyOut { report: r, text }
                });
                s.progress = 1.0;
                s.done = Some(Done { output: done.output.display().to_string(), seconds: done.seconds, verify });
            }
            Err(e) => s.error = Some(e.to_string()),
        }
    });
    Ok(())
}
