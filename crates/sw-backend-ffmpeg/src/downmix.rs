//! Downmix rendering and clip analysis through FFmpeg.

use crate::{ff_name, join_inputs, Error, Job};
use std::io::Read;
use std::process::Stdio;
use sw_core::downmix::{Matrix, SatEvent, SaturationCheck, OFF};
use sw_core::process::command;
use sw_core::Channel;

/// FFmpeg `pan` filter that applies `m` to a stream whose channels are named by the layout.
pub fn pan_filter(m: &Matrix) -> Result<String, Error> {
    use Channel::*;
    let layout = match m.outputs.as_slice() {
        [L, R] => "stereo",
        [L, R, C, Lfe, Ls, Rs] => "5.1(side)",
        _ => return Err(Error::Unsupported("downmix target other than stereo or 5.1")),
    };
    let mut rows = Vec::new();
    for (o, out) in m.outputs.iter().enumerate() {
        let mut terms = Vec::new();
        for (i, inp) in m.inputs.iter().enumerate() {
            let c = &m.cells[o][i];
            if c.gain_db == OFF {
                continue;
            }
            let name = ff_name(*inp).ok_or(Error::Unsupported("channel"))?;
            terms.push(format!("{:.6}*{name}", c.linear()));
        }
        if terms.is_empty() {
            let first = ff_name(m.inputs[0]).ok_or(Error::Unsupported("channel"))?;
            terms.push(format!("0*{first}"));
        }
        let oname = ff_name(*out).ok_or(Error::Unsupported("channel"))?;
        rows.push(format!("{oname}={}", terms.join("+")));
    }
    Ok(format!("pan={layout}|{}", rows.join("|")))
}

#[derive(Debug, serde::Serialize)]
pub struct SatReport {
    pub channels: Vec<&'static str>,
    pub frames: u64,
    pub peak_db: Vec<f32>,
    pub event_count: u64,
    pub events: Vec<SatEvent>,
}

/// Stream the downmix through FFmpeg as 32-bit float and look for clipping (three or more
/// consecutive full-scale samples on a channel). Reports peaks and the first events.
pub fn saturation_check(job: &Job, m: &Matrix) -> Result<SatReport, Error> {
    let (input_args, join) = join_inputs(job, "j")?;
    let filter = format!("{join};[j]{}[out]", pan_filter(m)?);
    let mut cmd = command(&job.ffmpeg);
    cmd.args(["-hide_banner", "-nostats", "-loglevel", "error"])
        .args(&input_args);
    cmd.args([
        "-filter_complex",
        &filter,
        "-map",
        "[out]",
        "-c:a",
        "pcm_f32le",
        "-f",
        "f32le",
        "-",
    ]);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let n = m.outputs.len();
    let mut sat = SaturationCheck::new(n);
    let mut buf = vec![0u8; 1 << 18];
    let mut pending: Vec<u8> = Vec::new();
    let mut floats: Vec<f32> = Vec::new();
    loop {
        let got = stdout.read(&mut buf)?;
        if got == 0 {
            break;
        }
        pending.extend_from_slice(&buf[..got]);
        let whole = pending.len() / (4 * n) * (4 * n);
        floats.clear();
        floats.extend(
            pending[..whole]
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        );
        sat.feed(&floats);
        pending.drain(..whole);
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        let log = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = log.lines().filter(|l| !l.trim().is_empty()).collect();
        return Err(Error::Failed(
            out.status.code().unwrap_or(-1),
            tail[tail.len().saturating_sub(5)..].join("\n"),
        ));
    }
    Ok(SatReport {
        channels: m.outputs.iter().map(|c| c.short()).collect(),
        frames: sat.frames(),
        peak_db: sat.peak_db(),
        event_count: sat.event_count,
        events: sat.events.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sw_core::ChannelLayout;

    #[test]
    fn pan_for_51_to_stereo() {
        let m = Matrix::lo_ro(ChannelLayout::find("5.1").unwrap());
        let p = pan_filter(&m).unwrap();
        assert!(p.starts_with("pan=stereo|FL=1.000000*FL+0.707"), "{p}");
        assert!(p.contains("|FR=") && !p.contains("LFE"), "{p}");
    }
}
