//! Running the engine and reading its progress.

use crate::alias::{has_space, make_alias, needs_alias};
use crate::input::InputKind;
use crate::job::AtmosJob;
use crate::profile::profile_xml;
use crate::Error;
use serde::Serialize;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

/// What a finished job leaves behind.
#[derive(Debug, Clone, Serialize)]
pub struct Finished {
    pub output: PathBuf,
    /// DEE's own report next to the output (`<output>.log`), if it wrote one.
    pub report: Option<PathBuf>,
    pub seconds: f64,
    /// Everything DEE said that is worth showing (warnings and errors).
    pub messages: Vec<String>,
}

/// Run the job. `progress` receives 0.0 to 1.0, `line` every log line worth showing; `cancel` stops the engine.
pub fn run(
    dee: &Path,
    job: &AtmosJob,
    work_dir: &Path,
    progress: &mut dyn FnMut(f32),
    line: &mut dyn FnMut(&str),
    cancel: &AtomicBool,
) -> Result<Finished, Error> {
    if !job.input.is_file() {
        return Err(Error::Invalid(format!("{} does not exist", job.input.display())));
    }
    // DEE runs from its own folder, so every path it is given must be absolute
    let mut job = job.clone();
    job.input = std::path::absolute(&job.input)?;
    job.output = std::path::absolute(&job.output)?;
    let work_dir = &std::path::absolute(work_dir)?;
    std::fs::create_dir_all(work_dir)?;
    let kind = match job.kind {
        Some(k) => k,
        None => InputKind::detect(&job.input)?,
    };
    let alias = if kind != InputKind::Wav && needs_alias(&job.input) {
        Some(make_alias(&job.input, &job.output, work_dir)?)
    } else {
        None
    };
    if let Some(a) = &alias {
        job.input = a.input.clone();
        job.kind = Some(kind);
    }
    // the output's folder is split on spaces as well: write into a staging folder and move the result afterwards
    let final_output = job.output.clone();
    let staged = has_space(&job.output);
    if staged {
        if has_space(work_dir) {
            return Err(Error::Invalid(format!(
                "DEE cannot write to a path with spaces ({}), and the work folder has spaces too",
                final_output.display()
            )));
        }
        let name: String = final_output
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("out")
            .chars()
            .map(|c| if c.is_whitespace() { '_' } else { c })
            .collect();
        let dir = work_dir.join("staging");
        std::fs::create_dir_all(&dir)?;
        job.output = dir.join(name);
    }
    let job = &job;
    let result = run_inner(dee, job, work_dir, progress, line, cancel);
    if let Some(a) = &alias {
        a.remove();
    }
    let mut done = result?;
    if staged {
        // the output and DEE's reports beside it (<name>.log, <name>.mll...) go to the real folder under the real name
        let staged_name = job
            .output
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or_default()
            .to_string();
        let final_name = final_output
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or_default()
            .to_string();
        let final_dir = final_output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(final_dir)?;
        for entry in std::fs::read_dir(job.output.parent().unwrap())? {
            let entry = entry?;
            let n = entry.file_name().to_string_lossy().to_string();
            if let Some(rest) = n.strip_prefix(&staged_name) {
                let dst = final_dir.join(format!("{final_name}{rest}"));
                if std::fs::rename(entry.path(), &dst).is_err() {
                    std::fs::copy(entry.path(), &dst)?;
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        done.report = {
            let mut name = final_output.as_os_str().to_os_string();
            name.push(".log");
            let p = PathBuf::from(name);
            p.is_file().then_some(p)
        };
        done.output = final_output;
    }
    Ok(done)
}

pub(crate) fn run_inner(
    dee: &Path,
    job: &AtmosJob,
    work_dir: &Path,
    progress: &mut dyn FnMut(f32),
    line: &mut dyn FnMut(&str),
    cancel: &AtomicBool,
) -> Result<Finished, Error> {
    let temp = work_dir.join("temp");
    std::fs::create_dir_all(&temp)?;
    if let Some(d) = job.output.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(d)?;
    }
    let xml = profile_xml(job, &temp)?;
    let profile = work_dir.join("job.xml");
    std::fs::write(&profile, xml)?;
    let log = work_dir.join("dee.log");
    let mut cmd = Command::new(dee);
    cmd.current_dir(dee.parent().unwrap_or(Path::new(".")))
        .arg("--xml")
        .arg(&profile)
        .arg("--progress")
        .arg("--log-file")
        .arg(&log)
        .arg("--stdout")
        .arg("--verbose")
        .arg("info")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // no console window
    }
    let started = std::time::Instant::now();
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        for l in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(l).is_err() {
                break;
            }
        }
    });
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let mut messages: Vec<String> = Vec::new();
    let mut last = 0.0f32;
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Canceled);
        }
        match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(l) => {
                if let Some(p) = parse_progress(&l) {
                    if p > last {
                        last = p;
                        progress(p / 100.0);
                    }
                }
                if let Some(m) = relevant(&l) {
                    if l.contains("ERROR") || l.contains("WARNING") {
                        messages.push(m.clone());
                    }
                    line(&m);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Ok(Some(_)) = child.try_wait() {
                    // drain what is left
                    while let Ok(l) = rx.try_recv() {
                        if let Some(m) = relevant(&l) {
                            if l.contains("ERROR") || l.contains("WARNING") {
                                messages.push(m.clone());
                            }
                            line(&m);
                        }
                    }
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child.wait()?;
    let _ = reader.join();
    let err = err_thread.join().unwrap_or_default();
    if !err.trim().is_empty() {
        messages.push(err.trim().to_string());
    }
    if !status.success() {
        let errors: Vec<String> = messages.iter().filter(|m| m.contains("ERROR")).cloned().collect();
        return Err(Error::Failed {
            code: status.code().unwrap_or(-1),
            messages: if errors.is_empty() {
                messages.join("\n")
            } else {
                errors.join("\n")
            },
        });
    }
    progress(1.0);
    if !job.output.is_file() {
        return Err(Error::Failed {
            code: 0,
            messages: "DEE reported success but wrote no output file".into(),
        });
    }
    let report = {
        let mut name = job.output.as_os_str().to_os_string();
        name.push(".log");
        let p = PathBuf::from(name);
        p.is_file().then_some(p)
    };
    Ok(Finished {
        output: job.output.clone(),
        report,
        seconds: started.elapsed().as_secs_f64(),
        messages,
    })
}

/// "Overall progress: 37.5." in a framework line.
pub(crate) fn parse_progress(l: &str) -> Option<f32> {
    let i = l.find("Overall progress:")?;
    l[i + "Overall progress:".len()..]
        .trim()
        .trim_end_matches('.')
        .parse()
        .ok()
}

/// The lines worth showing: not the component loading chatter, not resource sampling, not the licence's
/// feature list (it names the licence, which is nobody else's business).
pub(crate) fn relevant(l: &str) -> Option<String> {
    let t = l.trim();
    if t.is_empty()
        || t.contains("INTERNAL_INFO")
        || t.contains("CPU:")
        || t.contains("Features enabled by the license")
        || t.contains("Max MEM")
        || t.contains("Average CPU")
        || t.contains("Average MEM")
    {
        return None;
    }
    Some(t.to_string())
}
