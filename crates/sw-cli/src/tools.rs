//! `subwoofa tools`: StreamTools from the command line.

use crate::args::ToolsCmd;
use crate::verify::verify_cmd;

pub(crate) fn run_tools(op: ToolsCmd) -> Result<(), String> {
    use sw_tools::Stream;
    let e = |e: sw_tools::Error| e.to_string();
    match op {
        ToolsCmd::Info { file } => {
            let i = Stream::open(&file).map_err(e)?.info();
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
        }
        ToolsCmd::Pbr { file, window } => {
            for p in Stream::open(&file).map_err(e)?.pbr(window) {
                println!(
                    "{:9.2}  mean {:7.0}  peak {:7.0} kbps",
                    p.secs, p.mean_kbps, p.peak_kbps
                );
            }
        }
        ToolsCmd::Trim {
            file,
            output,
            start,
            end,
        } => {
            let s = Stream::open(&file).map_err(e)?;
            let (first, n, at) = s.trim(&output, start, end).map_err(e)?;
            println!(
                "wrote {} frames from frame {first} ({at:.3} s) to {}",
                n,
                output.display()
            );
        }
        ToolsCmd::Split { file, output, at } => {
            let s = Stream::open(&file).map_err(e)?;
            let ext = file.extension().and_then(|x| x.to_str()).unwrap_or("bin").to_string();
            for p in s.split(&output, &ext, &at).map_err(e)? {
                println!("wrote {}", p.display());
            }
        }
        ToolsCmd::Append { inputs, output } => {
            let refs: Vec<&std::path::Path> = inputs.iter().map(|p| p.as_path()).collect();
            sw_tools::append(&refs, &output).map_err(e)?;
            println!("wrote {}", output.display());
        }
        ToolsCmd::Replace { base, with, at, output } => {
            let (start, removed) = sw_tools::replace(&base, &with, at, &output).map_err(e)?;
            println!(
                "wrote {} (insert at {:.3} s, {:.3} s of the original replaced)",
                output.display(),
                start,
                removed
            );
        }
        ToolsCmd::Insert { base, with, at, output } => {
            let start = sw_tools::insert(&base, &with, at, &output).map_err(e)?;
            println!("wrote {} (inserted at {:.3} s)", output.display(), start);
        }
        ToolsCmd::Restripe {
            file,
            start,
            fps,
            output,
        } => {
            let code = sw_tools::fps_code(&fps).ok_or_else(|| format!("unknown frame rate '{fps}'"))?;
            let tc =
                sw_tools::parse_timecode(&start).ok_or_else(|| format!("start time '{start}' is not hh:mm:ss:ff"))?;
            let out = output.unwrap_or_else(|| file.clone());
            sw_tools::restripe(&file, &out, code, tc).map_err(e)?;
            println!("restriped {} ({} at {} fps code {})", out.display(), start, fps, code);
        }
        ToolsCmd::AddSilence {
            file,
            head,
            tail,
            output,
        } => {
            let (h, t) = sw_tools::add_silence(&file, &output, head, tail).map_err(e)?;
            println!(
                "wrote {} ({} silent frames at the head, {} at the tail)",
                output.display(),
                h,
                t
            );
        }
        ToolsCmd::Verify { files, deep, ffmpeg } => return verify_cmd(&files, deep, false, ffmpeg),
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
