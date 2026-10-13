//! The Downmix tab: default and safe matrices, and the clipping check.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use sw_backend_ffmpeg as ff;
use sw_core::downmix::{Matrix, OFF};
use sw_core::formats::StreamType;
use sw_core::{Channel, ChannelLayout};

// serde_json turns -inf into null, so "off" is carried explicitly as `None`.
#[derive(Serialize, Deserialize, Clone)]
pub struct CellDto {
    gain_db: Option<f32>,
    invert: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct MatrixDto {
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
            .map(|r| {
                r.iter()
                    .map(|c| CellDto {
                        gain_db: (c.gain_db != OFF).then_some(c.gain_db),
                        invert: c.invert,
                    })
                    .collect()
            })
            .collect(),
    }
}

fn from_dto(d: &MatrixDto) -> Result<Matrix, String> {
    let ch = |v: &Vec<String>| {
        v.iter()
            .map(|s| Channel::from_id(s).ok_or(format!("unknown channel '{s}'")))
            .collect::<Result<Vec<_>, _>>()
    };
    let (inputs, outputs) = (ch(&d.inputs)?, ch(&d.outputs)?);
    if d.cells.len() != outputs.len() || d.cells.iter().any(|r| r.len() != inputs.len()) {
        return Err("matrix shape does not match its channels".into());
    }
    let mut m = Matrix::empty(&inputs, &outputs);
    for (o, row) in d.cells.iter().enumerate() {
        for (i, c) in row.iter().enumerate() {
            m.cells[o][i] = sw_core::downmix::Cell {
                gain_db: c.gain_db.unwrap_or(OFF),
                invert: c.invert,
            };
        }
    }
    Ok(m)
}

#[tauri::command]
pub fn downmix_default(layout: String, to: String) -> Result<MatrixDto, String> {
    let l = ChannelLayout::find(&layout).ok_or("unknown layout")?;
    match to.as_str() {
        "stereo" => Ok(to_dto(&Matrix::lo_ro(l))),
        "5.1" => Ok(to_dto(&Matrix::to_51(l))),
        _ => Err("unknown downmix target".into()),
    }
}

#[derive(Serialize)]
pub struct SafeTrim {
    trim_db: f32,
    worst_case: f32,
    matrix: MatrixDto,
}

/// Trim that keeps full-scale input from clipping, with the matrix already trimmed.
#[tauri::command]
pub fn downmix_safe(matrix: MatrixDto) -> Result<SafeTrim, String> {
    let mut m = from_dto(&matrix)?;
    let trim_db = m.safe_trim_db();
    let worst_case = m.worst_case_gain().into_iter().fold(0.0, f32::max);
    m.trim(trim_db);
    Ok(SafeTrim {
        trim_db,
        worst_case,
        matrix: to_dto(&m),
    })
}

#[derive(Deserialize)]
pub struct CheckRequest {
    layout: String,
    inputs: HashMap<String, String>,
    matrix: MatrixDto,
    start: Option<f64>,
    duration: Option<f64>,
}

#[tauri::command]
pub async fn downmix_check(req: CheckRequest) -> Result<ff::downmix::SatReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let layout = ChannelLayout::find(&req.layout).ok_or("unknown layout")?;
        let m = from_dto(&req.matrix)?;
        let inputs = req
            .inputs
            .iter()
            .filter_map(|(k, v)| Channel::from_id(k).map(|c| (c, PathBuf::from(v))))
            .collect();
        let ffmpeg = ff::find_ffmpeg().ok_or("ffmpeg not found on PATH")?;
        let job = ff::Job {
            stream: StreamType::Ac3,
            layout,
            inputs,
            output: PathBuf::new(),
            bitrate_kbps: None,
            sample_rate: None,
            dialnorm: None,
            start_secs: req.start,
            duration_secs: req.duration,
            frames: None,
            slots: None,
            ffmpeg,
        };
        ff::downmix::saturation_check(&job, &m).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
