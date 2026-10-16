//! Using the Dolby Encoding Engine for what Subwoofa can also do itself: measuring loudness the way Dolby does (finding
//! the dialogue first, which is what a dialnorm should hold) and encoding Dolby Digital and Dolby Digital Plus with
//! Dolby's own encoder, as a reference to compare Subwoofa's against or where a delivery asks for it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use sw_backend_dee::{clock, run_task, work_folder_near, Measured, Source, Task, TaskJob};
use sw_core::{Channel, ChannelLayout};

/// Where the engine is: the path chosen in the app or the command line (`SUBWOOFA_DEE`), else the usual places.
pub fn find_engine() -> Result<PathBuf, String> {
    sw_backend_dee::find_dee(None).ok_or_else(|| sw_backend_dee::Error::NotFound.to_string())
}

/// The engine's name for a channel, which it spells differently from Subwoofa for the surrounds.
fn engine_name(channel: Channel) -> Option<&'static str> {
    Some(match channel {
        Channel::L => "L",
        Channel::R => "R",
        Channel::C => "C",
        Channel::Lfe => "LFE",
        Channel::Ls | Channel::Lss => "LS",
        Channel::Rs | Channel::Rss => "RS",
        Channel::Lsr => "LRS",
        Channel::Rsr => "RRS",
        _ => return None,
    })
}

/// The job's audio as the engine wants it: one multichannel file as it is, or one mono file per channel as a list
/// (the engine takes mono, stereo, 5.1 and 7.1).
pub fn engine_source(layout: &ChannelLayout, inputs: &HashMap<Channel, PathBuf>) -> Result<Source, String> {
    let channels = layout.channels;
    let configuration = match channels.len() {
        1 => "mono",
        2 => "stereo",
        6 => "5.1",
        8 => "7.1",
        n => {
            return Err(format!(
                "the Dolby encoder takes mono, stereo, 5.1 or 7.1, not a layout of {n} channels"
            ))
        }
    };
    let files: Vec<&PathBuf> = channels.iter().filter_map(|c| inputs.get(c)).collect();
    if files.len() != channels.len() {
        return Err("every channel needs a file".into());
    }
    // all channels in one file: the engine reads it whole
    if files.iter().all(|f| *f == files[0]) && channels.len() > 1 {
        return Ok(Source::Wav { path: files[0].clone() });
    }
    let mut list = Vec::new();
    for (channel, file) in channels.iter().zip(&files) {
        let name = engine_name(*channel)
            .ok_or_else(|| format!("the Dolby encoder has no place for the channel {}", channel.short()))?;
        list.push((name.to_string(), (*file).clone()));
    }
    Ok(Source::WavList {
        layout: configuration.into(),
        files: list,
    })
}

/// Measures a span of the programme with the engine's meter. `dialogue` makes it find the speech and measure that,
/// which is the level a dialnorm describes.
pub fn measure(
    layout: &ChannelLayout,
    inputs: &HashMap<Channel, PathBuf>,
    span: (Option<f64>, Option<f64>),
    dialogue: bool,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<Measured, String> {
    let dee = find_engine()?;
    let source = engine_source(layout, inputs)?;
    let work = job_folder("measure", source.first_file())?;
    let start = span.0.filter(|s| *s > 0.0);
    let output = work.with_extension("xml");
    let job = TaskJob {
        source,
        output,
        task: Task::Measure {
            metering: "1770-3".into(),
            dialogue_intelligence: dialogue,
            speech_threshold: 15,
        },
        start: start.map(clock),
        end: span.1.map(|d| clock(start.unwrap_or(0.0) + d)),
    };
    let done = run_task(&dee, &job, &work, progress, &mut |_| {}, cancel).map_err(|e| e.to_string());
    remove_job_folder(&work);
    let done = done?;
    let _ = std::fs::remove_file(&done.output);
    done.measured
        .ok_or_else(|| "the engine gave no measurement".to_string())
}

/// Folders handed out so far by this process, so no two jobs ever get the same one.
static FOLDERS: AtomicU64 = AtomicU64::new(0);

/// A folder of its own for one engine job, named for it, on the drive of `near` (the job's first input) when it can
/// be. Jobs running side by side in the queue each get a different one: the engine's staging empties its folder at
/// the start and end, which would wipe another job's files.
pub fn job_folder(tag: &str, near: Option<&Path>) -> Result<PathBuf, String> {
    let n = FOLDERS.fetch_add(1, Ordering::Relaxed);
    Ok(work_folder_near(near)
        .map_err(|e| e.to_string())?
        .join(format!("{tag}{}_{n}", std::process::id())))
}

/// Removes a job's folder, and the shared folder above it once no job is left in it (it may sit at the root of a
/// drive, made for this job).
pub fn remove_job_folder(work: &Path) {
    let _ = std::fs::remove_dir_all(work);
    if let Some(parent) = work.parent() {
        let _ = std::fs::remove_dir(parent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_differ() {
        let (a, b) = (job_folder("encode", None).unwrap(), job_folder("encode", None).unwrap());
        assert_ne!(a, b);
    }
}
