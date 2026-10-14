//! Job files: the settings of an encode as JSON, so a job can be saved once and run again, by hand or from a script.
//!
//! A job file holds one job, a list of jobs, or an object with a `"jobs"` list. Each job is an `EncodeSpec`, the same
//! thing the app builds; `subwoofa encode --save-job FILE` writes one.

use crate::args::RunArgs;
use crate::progress::Progress;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use sw_backend_ffmpeg as ff;
use sw_queue::run::{execute, EncodeSpec, Failure, Outcome};

/// Writes `spec` as a job file.
pub(crate) fn save_job(path: &Path, spec: &EncodeSpec) -> Result<(), String> {
    let text = serde_json::to_string_pretty(spec).map_err(|e| e.to_string())?;
    std::fs::write(path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))
}

/// Reads the jobs in a job file.
pub(crate) fn load_jobs(path: &Path) -> Result<Vec<EncodeSpec>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let list = match value {
        serde_json::Value::Array(items) => items,
        serde_json::Value::Object(mut map) if map.contains_key("jobs") => match map.remove("jobs") {
            Some(serde_json::Value::Array(items)) => items,
            _ => return Err(format!("{}: \"jobs\" must be a list", path.display())),
        },
        single @ serde_json::Value::Object(_) => vec![single],
        _ => {
            return Err(format!(
                "{}: expected a job, a list of jobs or {{\"jobs\": [...]}}",
                path.display()
            ))
        }
    };
    if list.is_empty() {
        return Err(format!("{}: no jobs in the file", path.display()));
    }
    list.into_iter()
        .enumerate()
        .map(|(i, v)| serde_json::from_value(v).map_err(|e| format!("{}: job {}: {e}", path.display(), i + 1)))
        .collect()
}

/// Runs one job, with a progress line on a terminal.
pub(crate) fn run_one(spec: &EncodeSpec, verify: bool) -> Result<Outcome, String> {
    let mut progress = Progress::new();
    let cancel = AtomicBool::new(false);
    execute(spec, verify, &cancel, &mut |p, _| progress.tick(p)).map_err(|e| match e {
        Failure::Canceled => "canceled".to_string(),
        Failure::Error(message) => message,
    })
}

/// One line saying what a finished job made.
pub(crate) fn describe(output: &str, out: &Outcome) -> String {
    format!(
        "wrote {} ({:.2} MB) in {:.1} s = {:.0}x real time{}",
        output,
        out.bytes as f64 / 1e6,
        out.elapsed_secs,
        out.span_secs / out.elapsed_secs.max(1e-3),
        if out.verified { ", verified" } else { "" }
    )
}

pub(crate) fn run_jobs(args: RunArgs) -> Result<(), String> {
    if let Some(path) = args.ffmpeg.clone() {
        ff::set_ffmpeg_override(Some(path));
    }
    let jobs = load_jobs(&args.jobs)?;
    let mut failed = 0;
    for (i, spec) in jobs.iter().enumerate() {
        println!("[{}/{}] {}", i + 1, jobs.len(), spec.output);
        match run_one(spec, args.verify) {
            Ok(out) => println!("  {}", describe(&spec.output, &out)),
            Err(message) => {
                failed += 1;
                eprintln!("  FAILED: {message}");
                if !args.keep_going {
                    return Err(format!("job {} failed (use --keep-going to run the rest)", i + 1));
                }
            }
        }
    }
    if failed > 0 {
        Err(format!("{failed} of {} jobs failed", jobs.len()))
    } else {
        println!("{} job{} done", jobs.len(), if jobs.len() == 1 { "" } else { "s" });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_json() -> String {
        r#"{"dest":"generic","stream":"ac3","layout":"2.0","sample_rate":48000,"bitrate":192,"dialnorm":null,
            "start":null,"duration":null,"inputs":{"L":"l.wav","R":"r.wav"},"output":"out.ac3"}"#
            .to_string()
    }

    fn write(name: &str, text: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("sw_jobs_{}_{name}", std::process::id()));
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn a_job_file_may_hold_one_job_a_list_or_a_jobs_object() {
        let one = write("one.json", &spec_json());
        let list = write("list.json", &format!("[{0},{0}]", spec_json()));
        let object = write("object.json", &format!("{{\"jobs\":[{0},{0},{0}]}}", spec_json()));
        assert_eq!(load_jobs(&one).unwrap().len(), 1);
        assert_eq!(load_jobs(&list).unwrap().len(), 2);
        assert_eq!(load_jobs(&object).unwrap().len(), 3);
        let first = &load_jobs(&one).unwrap()[0];
        assert_eq!(first.stream, "ac3");
        assert_eq!(first.bitrate, Some(192));
    }

    #[test]
    fn a_bad_job_says_which_one() {
        let bad = write("bad.json", &format!("[{},{{\"stream\":\"ac3\"}}]", spec_json()));
        let message = load_jobs(&bad).unwrap_err();
        assert!(message.contains("job 2"), "{message}");
        assert!(load_jobs(&write("empty.json", "[]")).is_err());
        assert!(load_jobs(&write("text.json", "\"hello\"")).is_err());
    }

    #[test]
    fn a_saved_job_loads_again_unchanged() {
        let spec = load_jobs(&write("round.json", &spec_json())).unwrap().remove(0);
        let path = std::env::temp_dir().join(format!("sw_jobs_{}_saved.json", std::process::id()));
        save_job(&path, &spec).unwrap();
        let back = load_jobs(&path).unwrap().remove(0);
        assert_eq!(
            serde_json::to_string(&spec).unwrap(),
            serde_json::to_string(&back).unwrap()
        );
    }
}
