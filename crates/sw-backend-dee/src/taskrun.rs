//! Running a `TaskJob`: the files go through a space-free staging folder (the engine splits paths on spaces), the engine
//! runs, and the result is moved to where the job asked for it.

use crate::alias::has_space;
use crate::run::run_profile;
use crate::task::{manifest_name, profile, writes_manifest, Measured, Source, Task, TaskJob};
use crate::Error;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// What a finished task leaves behind.
#[derive(Debug, Clone, Serialize)]
pub struct TaskDone {
    pub output: PathBuf,
    pub seconds: f64,
    /// Warnings the engine gave.
    pub messages: Vec<String>,
    /// The numbers of a `Measure` job.
    pub measured: Option<Measured>,
}

/// A hard link to `from` as `to` (no copy, no extra disk), or a copy where links are not possible.
fn stage_file(from: &Path, to: &Path) -> Result<(), Error> {
    if !from.is_file() {
        return Err(Error::Invalid(format!("{} does not exist", from.display())));
    }
    let _ = std::fs::remove_file(to);
    if std::fs::hard_link(from, to).is_err() {
        std::fs::copy(from, to)?;
    }
    Ok(())
}

fn extension_of(path: &Path, fallback: &str) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| fallback.to_string())
}

/// The job with its inputs staged in `dir` under plain names.
fn stage_inputs(job: &TaskJob, dir: &Path) -> Result<TaskJob, Error> {
    let mut staged = job.clone();
    staged.source = match &job.source {
        Source::Wav { path } => {
            let to = dir.join("source.wav");
            stage_file(path, &to)?;
            Source::Wav { path: to }
        }
        Source::WavList { layout, files } => {
            let mut out = Vec::new();
            for (channel, path) in files {
                let to = dir.join(format!("{channel}.wav"));
                stage_file(path, &to)?;
                out.push((channel.clone(), to));
            }
            Source::WavList {
                layout: layout.clone(),
                files: out,
            }
        }
        Source::Stream { path, atmos } => {
            let to = dir.join(format!("source.{}", extension_of(path, "ec3")));
            stage_file(path, &to)?;
            Source::Stream {
                path: to,
                atmos: *atmos,
            }
        }
        Source::Streams { paths } => {
            let mut out = Vec::new();
            for (i, path) in paths.iter().enumerate() {
                let to = dir.join(format!("part{i}.{}", extension_of(path, "ec3")));
                stage_file(path, &to)?;
                out.push(to);
            }
            Source::Streams { paths: out }
        }
    };
    Ok(staged)
}

/// Moves a finished file to its place (a copy when the folders are on different drives).
fn move_file(from: &Path, to: &Path) -> Result<(), Error> {
    if let Some(dir) = to.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(to);
    if std::fs::rename(from, to).is_err() {
        std::fs::copy(from, to)?;
        std::fs::remove_file(from)?;
    }
    Ok(())
}

/// Runs the job with `dee`. `work_dir` must be a folder with no spaces in its path; it holds the staged files and is
/// emptied afterwards.
pub fn run_task(
    dee: &Path,
    job: &TaskJob,
    work_dir: &Path,
    progress: &mut dyn FnMut(f32),
    line: &mut dyn FnMut(&str),
    cancel: &AtomicBool,
) -> Result<TaskDone, Error> {
    let work = std::path::absolute(work_dir)?;
    if has_space(&work) {
        return Err(Error::Invalid(format!(
            "the engine cannot work in a folder with spaces in its path ({}); choose another work folder",
            work.display()
        )));
    }
    let (input_dir, output_dir, temp) = (work.join("in"), work.join("out"), work.join("temp"));
    for d in [&input_dir, &output_dir, &temp] {
        let _ = std::fs::remove_dir_all(d);
        std::fs::create_dir_all(d)?;
    }
    let final_output = std::path::absolute(&job.output)?;
    let result = (|| {
        let mut staged = stage_inputs(job, &input_dir)?;
        staged.output = output_dir.join(format!("result.{}", extension_of(&final_output, "out")));
        let profile_path = work.join("job.xml");
        std::fs::write(&profile_path, profile(&staged, &temp)?)?;
        let (messages, seconds) = run_profile(dee, &profile_path, &work.join("dee.log"), progress, line, cancel)?;
        if !staged.output.is_file() {
            return Err(Error::Failed {
                code: 0,
                messages: "the engine reported success but wrote no output file".into(),
            });
        }
        let measured = match job.task {
            Task::Measure { .. } => Some(Measured::from_manifest(&std::fs::read_to_string(&staged.output)?)?),
            // a decode or a correction measures as it goes: keep the numbers, drop the file
            _ if writes_manifest(&job.task) => std::fs::read_to_string(output_dir.join(manifest_name(&staged.output)))
                .ok()
                .and_then(|x| Measured::from_manifest(&x).ok()),
            _ => None,
        };
        move_file(&staged.output, &final_output)?;
        Ok(TaskDone {
            output: final_output.clone(),
            seconds,
            messages,
            measured,
        })
    })();
    for d in [&input_dir, &output_dir, &temp] {
        let _ = std::fs::remove_dir_all(d);
    }
    result
}

/// A work folder with no spaces in its path: the system temporary folder if that qualifies, else a shared folder
/// that does.
pub fn work_folder() -> Result<PathBuf, Error> {
    let mut candidates = vec![std::env::temp_dir()];
    for var in ["PUBLIC", "ProgramData"] {
        if let Some(p) = std::env::var_os(var) {
            candidates.push(PathBuf::from(p));
        }
    }
    candidates
        .into_iter()
        .map(|d| d.join("subwoofa-dee"))
        .find(|d| !has_space(d))
        .ok_or_else(|| {
            Error::Invalid("no folder without spaces in its path was found for the engine to work in".into())
        })
}
