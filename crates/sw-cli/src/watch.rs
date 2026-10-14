//! `subwoofa watch`: a hot folder. Every complete set of channel files that lands in the folder is encoded with the
//! settings of a job file, into another folder, and left alone once its encode exists.
//!
//! A set is only touched once its files have stopped changing (a copy still in progress would be read half-written),
//! the encode goes to a hidden temporary name and is renamed when it is whole (so a stopped run never leaves a half
//! file that looks finished), and a set that fails is not retried until the watch is started again.

use crate::args::WatchArgs;
use crate::jobs::{describe, load_jobs, run_one};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use sw_backend_ffmpeg as ff;
use sw_core::{autoimport, Channel, ChannelLayout};
use sw_queue::run::EncodeSpec;

/// A complete set of inputs for one programme.
struct Set {
    name: String,
    inputs: HashMap<Channel, PathBuf>,
}

/// Size and modification time of every file of a set: equal twice in a row means the copy has finished.
type Fingerprint = Vec<(PathBuf, u64, Option<SystemTime>)>;

pub(crate) fn run_watch(args: WatchArgs) -> Result<(), String> {
    if let Some(path) = args.ffmpeg.clone() {
        ff::set_ffmpeg_override(Some(path));
    }
    let template = load_jobs(&args.job)?.remove(0);
    let ext = Path::new(&template.output)
        .extension()
        .and_then(|e| e.to_str())
        .ok_or("the job's output needs a file extension (it says which format to write)")?
        .to_string();
    let layout_id = template
        .source_layout
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&template.layout);
    let layout = ChannelLayout::find(layout_id).ok_or(format!("unknown layout '{layout_id}'"))?;
    if !args.folder.is_dir() {
        return Err(format!("{} is not a folder", args.folder.display()));
    }
    std::fs::create_dir_all(&args.out).map_err(|e| format!("{}: {e}", args.out.display()))?;
    remove_leftovers(&args.out);
    println!(
        "watching {} for {} sets ({}), writing .{ext} files to {}{}",
        args.folder.display(),
        layout.id,
        template.stream,
        args.out.display(),
        if args.once { "" } else { "; stop with Ctrl+C" }
    );

    let mut seen: HashMap<String, Fingerprint> = HashMap::new();
    let mut failed: HashSet<String> = HashSet::new();
    loop {
        let mut worked = false;
        for set in find_sets(&args.folder, layout) {
            let target = args.out.join(format!("{}.{ext}", set.name));
            if target.exists() || failed.contains(&set.name) {
                continue;
            }
            let now = fingerprint(&set);
            let steady = seen.get(&set.name) == Some(&now);
            seen.insert(set.name.clone(), now);
            // a --once run has no second look: it takes what has not changed for a few seconds instead
            if !steady && !(args.once && settled(&set)) {
                continue;
            }
            worked = true;
            match encode_set(&template, &set, &target, &ext, args.verify) {
                Ok(line) => {
                    println!("{}: {line}", set.name);
                    if args.move_done {
                        move_set(&set, &args.folder.join("done"));
                    }
                }
                Err(message) => {
                    eprintln!("{}: FAILED: {message}", set.name);
                    failed.insert(set.name.clone());
                    if args.move_done {
                        move_set(&set, &args.folder.join("failed"));
                    }
                }
            }
        }
        if args.once && !worked {
            return if failed.is_empty() {
                Ok(())
            } else {
                Err(format!("{} set(s) failed", failed.len()))
            };
        }
        if !args.once {
            std::thread::sleep(Duration::from_secs(args.interval.max(1)));
        }
    }
}

/// Every complete set in the folder: per-channel files grouped by name, and single files that hold every channel.
fn find_sets(folder: &Path, layout: &'static ChannelLayout) -> Vec<Set> {
    let mut sets: Vec<Set> = autoimport::scan_folder(folder, layout)
        .into_iter()
        .map(|g| Set {
            name: g.name,
            inputs: g.inputs,
        })
        .collect();
    let taken: HashSet<PathBuf> = sets.iter().flat_map(|s| s.inputs.values().cloned()).collect();
    if let Ok(entries) = std::fs::read_dir(folder) {
        let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
        files.sort();
        for file in files {
            if taken.contains(&file) {
                continue;
            }
            if let Some(inputs) = sw_pcm::multichannel_inputs(layout, &file) {
                let name = file
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                sets.push(Set { name, inputs });
            }
        }
    }
    sets
}

fn fingerprint(set: &Set) -> Fingerprint {
    let mut files: Vec<PathBuf> = set.inputs.values().cloned().collect();
    files.sort();
    files.dedup();
    files
        .into_iter()
        .map(|p| {
            let meta = std::fs::metadata(&p).ok();
            (
                p,
                meta.as_ref().map_or(0, |m| m.len()),
                meta.and_then(|m| m.modified().ok()),
            )
        })
        .collect()
}

/// No file of the set changed in the last few seconds.
fn settled(set: &Set) -> bool {
    fingerprint(set).iter().all(|(_, _, modified)| {
        modified
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(3))
    })
}

/// Encodes one set with the template's settings, to a temporary name that becomes `target` when it is whole.
fn encode_set(template: &EncodeSpec, set: &Set, target: &Path, ext: &str, verify: bool) -> Result<String, String> {
    let temp = target.with_file_name(format!(".{}.partial.{ext}", set.name));
    let mut spec = template.clone();
    spec.inputs = set
        .inputs
        .iter()
        .map(|(c, p)| (c.short().to_string(), p.to_string_lossy().into_owned()))
        .collect();
    spec.output = temp.to_string_lossy().into_owned();
    spec.write_log = false;
    match run_one(&spec, verify) {
        Ok(out) => {
            std::fs::rename(&temp, target).map_err(|e| format!("{}: {e}", target.display()))?;
            Ok(describe(&target.to_string_lossy(), &out))
        }
        Err(message) => {
            let _ = std::fs::remove_file(&temp);
            Err(message)
        }
    }
}

/// Moves a set's files into `dir` (made if needed). A file that cannot be moved stays where it is.
fn move_set(set: &Set, dir: &Path) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let mut files: Vec<&PathBuf> = set.inputs.values().collect();
    files.sort();
    files.dedup();
    for file in files {
        if let Some(name) = file.file_name() {
            let _ = std::fs::rename(file, dir.join(name));
        }
    }
}

/// Half-finished files of an earlier run that was stopped.
fn remove_leftovers(out: &Path) {
    if let Ok(entries) = std::fs::read_dir(out) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') && name.contains(".partial.") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}
