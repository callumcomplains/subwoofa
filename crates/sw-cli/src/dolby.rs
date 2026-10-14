//! `subwoofa dolby`: jobs for the Dolby Encoding Engine that Subwoofa does not do itself.

use crate::args::DolbyCmd;
use crate::progress::Progress;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use sw_backend_dee::{run_task, work_folder, Error, Source, Task, TaskDone, TaskJob};

/// Runs one job with a progress line on a terminal and prints what the engine warned about.
fn run_job(dee: Option<PathBuf>, source: Source, output: &Path, task: Task) -> Result<TaskDone, String> {
    let engine = sw_backend_dee::find_dee(dee.as_deref()).ok_or_else(|| Error::NotFound.to_string())?;
    let work = work_folder()
        .map_err(|e| e.to_string())?
        .join(format!("cli{}", std::process::id()));
    let job = TaskJob {
        source,
        output: output.to_path_buf(),
        task,
        start: None,
        end: None,
    };
    let mut progress = Progress::new();
    let done = run_task(
        &engine,
        &job,
        &work,
        &mut |p| progress.tick(p),
        &mut |_| {},
        &AtomicBool::new(false),
    )
    .map_err(|e| e.to_string());
    let _ = std::fs::remove_dir_all(&work);
    let done = done?;
    for m in &done.messages {
        eprintln!("engine: {m}");
    }
    println!("wrote {} in {:.1} s", done.output.display(), done.seconds);
    Ok(done)
}

pub(crate) fn run_dolby(op: DolbyCmd) -> Result<(), String> {
    match op {
        DolbyCmd::Decode {
            file,
            output,
            drc,
            atmos,
            dee,
        } => {
            run_job(dee, Source::Stream { path: file, atmos }, &output, Task::Decode { drc })?;
        }
        DolbyCmd::Transcode {
            file,
            output,
            bitrate,
            correct,
            dee,
        } => {
            let atmos = false;
            run_job(
                dee,
                Source::Stream { path: file, atmos },
                &output,
                Task::Transcode {
                    data_rate_kbps: bitrate,
                    correct,
                },
            )?;
        }
        DolbyCmd::Edit {
            files,
            output,
            start,
            end,
            invert,
            dee,
        } => {
            run_job(
                dee,
                Source::Streams { paths: files },
                &output,
                Task::Edit {
                    start,
                    end,
                    invert_trim: invert,
                },
            )?;
        }
        DolbyCmd::Correct {
            file,
            output,
            preset,
            dee,
        } => {
            let done = run_job(dee, Source::Wav { path: file }, &output, Task::Correct { preset })?;
            if let Some(m) = done.measured {
                if let Some(l) = m.integrated {
                    println!("loudness of the result: {l:.1} LUFS");
                }
            }
        }
    }
    Ok(())
}
