//! `subwoofa tools`: StreamTools from the command line.

use crate::args::ToolsCmd;
use crate::verify::verify_cmd;
use std::path::{Path, PathBuf};
use sw_tools::Stream;

pub(crate) fn run_tools(op: ToolsCmd) -> Result<(), String> {
    match op {
        ToolsCmd::Info { file } => info(&file),
        ToolsCmd::Pbr { file, window } => pbr(&file, window),
        ToolsCmd::Trim {
            file,
            output,
            start,
            end,
        } => trim(&file, &output, start, end),
        ToolsCmd::Split { file, output, at } => split(&file, &output, &at),
        ToolsCmd::Append { inputs, output } => append(&inputs, &output),
        ToolsCmd::Replace { base, with, at, output } => replace(&base, &with, at, &output),
        ToolsCmd::Insert { base, with, at, output } => insert(&base, &with, at, &output),
        ToolsCmd::Restripe {
            file,
            start,
            fps,
            output,
        } => restripe(&file, &start, &fps, output),
        ToolsCmd::AddSilence {
            file,
            head,
            tail,
            output,
        } => add_silence(&file, head, tail, &output),
        ToolsCmd::Dialnorm { file, set, output } => dialnorm(&file, set, output),
        ToolsCmd::Verify(args) => verify_cmd(args),
    }
}

fn e(err: sw_tools::Error) -> String {
    err.to_string()
}

fn info(file: &Path) -> Result<(), String> {
    let i = Stream::open(file).map_err(e)?.info();
    println!("{} ({})", i.description, i.format);
    println!(
        "{} Hz, {} channels, {} frames, {:.3} s",
        i.sample_rate, i.channels, i.frames, i.duration_secs
    );
    println!(
        "{:.1} MB, average {:.0} kbps, peak {:.0} kbps, {} restart point(s){}",
        i.bytes as f64 / 1e6,
        i.average_kbps,
        i.peak_kbps,
        i.restart_points,
        if i.container { ", .dtshd container" } else { "" }
    );
    Ok(())
}

fn pbr(file: &Path, window: f64) -> Result<(), String> {
    for p in Stream::open(file).map_err(e)?.pbr(window) {
        println!(
            "{:9.2}  mean {:7.0}  peak {:7.0} kbps",
            p.secs, p.mean_kbps, p.peak_kbps
        );
    }
    Ok(())
}

fn trim(file: &Path, output: &Path, start: f64, end: Option<f64>) -> Result<(), String> {
    let (first, n, at) = Stream::open(file).map_err(e)?.trim(output, start, end).map_err(e)?;
    println!(
        "wrote {} frames from frame {first} ({at:.3} s) to {}",
        n,
        output.display()
    );
    Ok(())
}

fn split(file: &Path, output: &Path, at: &[f64]) -> Result<(), String> {
    let ext = file.extension().and_then(|x| x.to_str()).unwrap_or("bin").to_string();
    for p in Stream::open(file).map_err(e)?.split(output, &ext, at).map_err(e)? {
        println!("wrote {}", p.display());
    }
    Ok(())
}

fn append(inputs: &[PathBuf], output: &Path) -> Result<(), String> {
    let refs: Vec<&Path> = inputs.iter().map(|p| p.as_path()).collect();
    sw_tools::append(&refs, output).map_err(e)?;
    println!("wrote {}", output.display());
    Ok(())
}

fn replace(base: &Path, with: &Path, at: f64, output: &Path) -> Result<(), String> {
    let (start, removed) = sw_tools::replace(base, with, at, output).map_err(e)?;
    println!(
        "wrote {} (insert at {:.3} s, {:.3} s of the original replaced)",
        output.display(),
        start,
        removed
    );
    Ok(())
}

fn insert(base: &Path, with: &Path, at: f64, output: &Path) -> Result<(), String> {
    let start = sw_tools::insert(base, with, at, output).map_err(e)?;
    println!("wrote {} (inserted at {:.3} s)", output.display(), start);
    Ok(())
}

fn restripe(file: &Path, start: &str, fps: &str, output: Option<PathBuf>) -> Result<(), String> {
    let code = sw_tools::fps_code(fps).ok_or_else(|| format!("unknown frame rate '{fps}'"))?;
    let tc = sw_tools::parse_timecode(start).ok_or_else(|| format!("start time '{start}' is not hh:mm:ss:ff"))?;
    let out = output.unwrap_or_else(|| file.to_path_buf());
    sw_tools::restripe(file, &out, code, tc).map_err(e)?;
    println!("restriped {} ({} at {} fps code {})", out.display(), start, fps, code);
    Ok(())
}

fn add_silence(file: &Path, head: f64, tail: f64, output: &Path) -> Result<(), String> {
    let (h, t) = sw_tools::add_silence(file, output, head, tail).map_err(e)?;
    println!(
        "wrote {} ({} silent frames at the head, {} at the tail)",
        output.display(),
        h,
        t
    );
    Ok(())
}

/// Shows the dialnorm of an AC-3 / E-AC-3 stream, or writes a copy with another one.
fn dialnorm(file: &Path, set: Option<u8>, output: Option<PathBuf>) -> Result<(), String> {
    let mut data = std::fs::read(file).map_err(|err| format!("{}: {err}", file.display()))?;
    match set {
        None => {
            let found = sw_codec_ac3::edit::inspect(&data)?;
            println!(
                "{}: {} stream, {} frames, dialnorm -{} dBFS",
                file.display(),
                if found.eac3 { "E-AC-3" } else { "AC-3" },
                found.frames,
                found.previous
            );
        }
        Some(level) => {
            let done = sw_codec_ac3::edit::set_dialnorm(&mut data, level)?;
            let out = output.unwrap_or_else(|| file.to_path_buf());
            std::fs::write(&out, data).map_err(|err| format!("{}: {err}", out.display()))?;
            println!(
                "wrote {}: dialnorm -{} dBFS (was -{}), {} of {} frames rewritten",
                out.display(),
                level,
                done.previous,
                done.changed,
                done.frames
            );
        }
    }
    Ok(())
}

/// Seconds as `m:ss` (or `h:mm:ss`).
pub(crate) fn clock(seconds: f32) -> String {
    let total = seconds.round() as u64;
    if total >= 3600 {
        format!("{}:{:02}:{:02}", total / 3600, total / 60 % 60, total % 60)
    } else {
        format!("{}:{:02}", total / 60, total % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::clock;

    #[test]
    fn clock_shows_minutes_and_hours() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(75.4), "1:15");
        assert_eq!(clock(3725.0), "1:02:05");
    }
}
