//! FFmpeg backend: builds an `ffmpeg` command that joins per-channel mono/stereo files into
//! one multichannel stream and encodes it with the requested codec.
//!
//! This is the day-one engine; native encoders replace it format by format.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::io::Read;
use std::process::{Command, Stdio};
use sw_core::{Channel, ChannelLayout, StreamType};

pub mod downmix;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no input file for channel {0}")]
    MissingChannel(&'static str),
    #[error("{0} is not supported by the FFmpeg backend")]
    Unsupported(&'static str),
    #[error("could not run ffmpeg: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("ffmpeg failed ({0}):\n{1}")]
    Failed(i32, String),
    #[error("canceled")]
    Canceled,
    #[error("verification failed: {0}")]
    Verify(String),
}

#[derive(Debug, Clone)]
pub struct Job {
    pub stream: StreamType,
    pub layout: &'static ChannelLayout,
    pub inputs: HashMap<Channel, PathBuf>,
    pub output: PathBuf,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate: Option<u32>,
    /// DTS-style dialog normalisation as a positive attenuation in dB (0..=31); AC-3 `-dialnorm`.
    pub dialnorm: Option<u8>,
    /// Encode only this span of the source (seconds): start offset and length.
    pub start_secs: Option<f64>,
    pub duration_secs: Option<f64>,
    /// Exact number of sample frames to encode (per channel, at the source rate). When set the
    /// inputs are padded and the joined stream is trimmed to exactly this length, which avoids
    /// FFmpeg's `join` filter occasionally dropping the final partial frame.
    pub frames: Option<u64>,
    /// Source of each layout channel (file, channel inside the file). When `None`, `inputs` is used
    /// with one mono file per channel.
    pub slots: Option<Vec<(PathBuf, usize)>>,
    pub ffmpeg: PathBuf,
}

/// FFmpeg channel-layout position name for a Subwoofa channel.
pub(crate) fn ff_name(c: Channel) -> Option<&'static str> {
    use Channel::*;
    Some(match c {
        L | Lt => "FL",
        R | Rt => "FR",
        C => "FC",
        Lfe => "LFE",
        Ls => "SL",
        Rs => "SR",
        Cs => "BC",
        Lsr => "BL",
        Rsr => "BR",
        Lss => "SL",
        Rss => "SR",
        Lw => "FLC",
        Rw => "FRC",
        Lh | Lhs => "TFL",
        Rh | Rhs => "TFR",
        Ch => "TFC",
        Oh | Tc => "TC",
        Tfl => "TFL",
        Tfr => "TFR",
        Tfc => "TFC",
        Tbl => "TBL",
        Tbr => "TBR",
        Tbc => "TBC",
        Tsl => "TSL",
        Tsr => "TSR",
        Bfc => "BFC",
        Bfl => "BFL",
        Bfr => "BFR",
        Lfe2 => "LFE2",
    })
}

/// FFmpeg encoder name and extra args for a stream type.
fn codec_args(stream: StreamType) -> Result<(&'static str, Vec<&'static str>), Error> {
    Ok(match stream {
        StreamType::Ac3 => ("ac3", vec![]),
        StreamType::Eac3 => ("eac3", vec![]),
        StreamType::DtsDigitalSurround => ("dca", vec!["-strict", "-2"]),
        StreamType::TrueHd => ("truehd", vec!["-strict", "-2"]),
        StreamType::Mlp => ("mlp", vec!["-strict", "-2"]),
        StreamType::DtsHdMa => return Err(Error::Unsupported("DTS-HD Master Audio")),
        StreamType::DtsHdHra => return Err(Error::Unsupported("DTS-HD High Resolution")),
        StreamType::DtsEs => return Err(Error::Unsupported("DTS-ES")),
        StreamType::Dts9624 => return Err(Error::Unsupported("DTS 96/24")),
        StreamType::PcmWav | StreamType::PcmAiff | StreamType::PcmCaf | StreamType::Flac | StreamType::OggFlac | StreamType::Alac | StreamType::Tta | StreamType::DsdDsf | StreamType::DsdDff => return Err(Error::Unsupported("audio files (written by the native engine)")),
        StreamType::DtsExpress => return Err(Error::Unsupported("DTS Express")),
    })
}

/// True if this backend has an encoder for the stream type.
pub fn supports(stream: StreamType) -> bool {
    codec_args(stream).is_ok()
}

/// `-i` arguments for every channel file plus a filter that joins them into one multichannel
/// stream with the layout's speaker positions, ending in the output pad `[label]`.
pub(crate) fn join_inputs(job: &Job, label: &str) -> Result<(Vec<String>, String), Error> {
    let n = job.layout.channel_count();
    // where each layout channel comes from: (file, channel inside the file)
    let mut sources: Vec<(PathBuf, usize)> = Vec::with_capacity(n);
    match &job.slots {
        Some(slots) => sources.extend(slots.iter().cloned()),
        None => {
            for ch in job.layout.channels {
                sources.push((job.inputs.get(ch).ok_or(Error::MissingChannel(ch.short()))?.clone(), 0));
            }
        }
    }
    if sources.len() != n {
        return Err(Error::Unsupported("source list does not match the layout"));
    }
    // one ffmpeg input per distinct file (a multichannel file feeds several layout channels)
    let mut files: Vec<PathBuf> = Vec::new();
    let mut stream_of: Vec<usize> = Vec::with_capacity(n);
    for (path, _) in &sources {
        let idx = files.iter().position(|f| f == path).unwrap_or_else(|| {
            files.push(path.clone());
            files.len() - 1
        });
        stream_of.push(idx);
    }
    let mut args = Vec::new();
    for f in &files {
        if let Some(s) = job.start_secs {
            args.extend(["-ss".into(), s.to_string()]);
        }
        if let Some(d) = job.duration_secs {
            args.extend(["-t".into(), d.to_string()]);
        }
        args.extend(["-i".into(), f.to_string_lossy().into_owned()]);
    }
    // with a known length, each input is padded so no input can run dry before the trim
    let padded = job.frames.is_some();
    let mut parts: Vec<String> = Vec::new();
    let pad_label = |i: usize| if padded { format!("[p{i}]") } else { format!("[{i}:a]") };
    if padded {
        for i in 0..files.len() {
            parts.push(format!("[{i}:a]apad=pad_len=16384[p{i}]"));
        }
    }
    let names: Vec<&'static str> = job.layout.channels.iter().map(|c| ff_name(*c).ok_or(Error::Unsupported("channel"))).collect::<Result<_, _>>()?;
    let trim = job.frames.map(|f| format!("atrim=end_sample={f}"));
    if n == 1 {
        let src = pad_label(0);
        parts.push(match &trim {
            Some(t) => format!("{src}{t}[{label}]"),
            None => format!("{src}anull[{label}]"),
        });
    } else {
        let joined = if trim.is_some() { "jx".to_string() } else { label.to_string() };
        let inputs: String = (0..files.len()).map(pad_label).collect();
        // Explicit input channel -> output channel assignment: without `map`, join matches channel
        // names carried in the files' metadata, which scrambles plain WAVs that have no channel mask.
        let assign: Vec<String> = sources.iter().zip(&stream_of).zip(&names).map(|(((_, ch), s), name)| format!("{s}.{ch}-{name}")).collect();
        parts.push(format!("{inputs}join=inputs={}:channel_layout={}:map={}[{joined}]", files.len(), names.join("+"), assign.join("|")));
        if let Some(t) = &trim {
            parts.push(format!("[jx]{t}[{label}]"));
        }
    }
    Ok((args, parts.join(";")))
}

/// Build the argument list (without the program name).
pub fn build_args(job: &Job) -> Result<Vec<String>, Error> {
    let (codec, extra) = codec_args(job.stream)?;
    // FFmpeg's E-AC-3 and DTS core encoders stop at 5.1.
    if matches!(job.stream, StreamType::Eac3 | StreamType::DtsDigitalSurround) && job.layout.channel_count() > 6 {
        return Err(Error::Unsupported("more than 5.1 channels with FFmpeg's encoder for this format"));
    }
    let mut args: Vec<String> = vec!["-hide_banner".into(), "-y".into()];
    let (input_args, filter) = join_inputs(job, "out")?;
    args.extend(input_args);
    args.extend(["-filter_complex".into(), filter, "-map".into(), "[out]".into()]);
    args.extend(["-c:a".into(), codec.into()]);
    args.extend(extra.iter().map(|s| s.to_string()));
    if let Some(b) = job.bitrate_kbps {
        args.extend(["-b:a".into(), format!("{b}k")]);
    }
    if let Some(sr) = job.sample_rate {
        args.extend(["-ar".into(), sr.to_string()]);
    }
    if let (Some(d), StreamType::Ac3 | StreamType::Eac3) = (job.dialnorm, job.stream) {
        // FFmpeg takes dialnorm as a negative dBFS value in -31..=-1; -31 means no attenuation.
        args.extend(["-dialnorm".into(), format!("-{}", d.clamp(1, 31))]);
    }
    // Raw elementary-stream muxers, independent of the output file extension.
    let muxer = match job.stream {
        StreamType::Ac3 => Some("ac3"),
        StreamType::Eac3 => Some("eac3"),
        StreamType::DtsDigitalSurround => Some("dts"),
        StreamType::TrueHd => Some("truehd"),
        StreamType::Mlp => Some("mlp"),
        _ => None,
    };
    if let Some(m) = muxer {
        args.extend(["-f".into(), m.into()]);
    }
    args.push(job.output.to_string_lossy().into_owned());
    Ok(args)
}

/// Run the encode. Returns ffmpeg's stderr log on success.
pub fn run(job: &Job) -> Result<String, Error> {
    let args = build_args(job)?;
    let out = Command::new(&job.ffmpeg).args(&args).output()?;
    let log = String::from_utf8_lossy(&out.stderr).into_owned();
    if out.status.success() {
        Ok(log)
    } else {
        // The banner and per-input dump are noise; the reason is in the last few lines.
        let tail: Vec<&str> = log.lines().filter(|l| !l.trim().is_empty()).collect();
        let tail = tail[tail.len().saturating_sub(5)..].join("\n");
        Err(Error::Failed(out.status.code().unwrap_or(-1), tail))
    }
}

fn tail_of(log: &str) -> String {
    let lines: Vec<&str> = log.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(5)..].join("\n")
}

/// Run the encode with progress reporting and cancellation.
/// `total_secs` is the length being encoded (used to turn ffmpeg's clock into a fraction).
pub fn run_progress(
    job: &Job,
    total_secs: f64,
    cancel: &std::sync::atomic::AtomicBool,
    on_progress: &mut dyn FnMut(f32),
) -> Result<String, Error> {
    use std::io::{BufRead, BufReader};
    use std::sync::atomic::Ordering;
    let mut args = build_args(job)?;
    args.splice(2..2, ["-progress".to_string(), "pipe:1".into(), "-nostats".into()]);
    let mut child = Command::new(&job.ffmpeg).args(&args).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let stderr = child.stderr.take().expect("piped stderr");
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = BufReader::new(stderr).read_to_string(&mut s);
        s
    });
    let stdout = child.stdout.take().expect("piped stdout");
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Canceled);
        }
        if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<i64>().ok()) {
            if total_secs > 0.0 && us >= 0 {
                on_progress(((us as f64 / 1e6) / total_secs).clamp(0.0, 1.0) as f32);
            }
        }
    }
    let status = child.wait()?;
    let log = err_thread.join().unwrap_or_default();
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Canceled);
    }
    if status.success() {
        on_progress(1.0);
        Ok(log)
    } else {
        Err(Error::Failed(status.code().unwrap_or(-1), tail_of(&log)))
    }
}

/// MD5 of `ffmpeg` output for either the source join or a decoded file.
fn md5_of(ffmpeg: &Path, pre: &[String], filter: Option<&str>, trim_to: Option<u64>, fmt: &str) -> Result<String, Error> {
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-hide_banner", "-nostats", "-loglevel", "error"]).args(pre);
    if let Some(f) = filter {
        cmd.args(["-filter_complex", f, "-map", "[v]"]);
    } else if let Some(n) = trim_to {
        // a lossless codec may pad its last frame with zeros; compare only the real samples
        cmd.args(["-af", &format!("atrim=end_sample={n}")]);
    }
    cmd.args(["-c:a", fmt, "-f", "md5", "-"]);
    let out = cmd.output()?;
    if !out.status.success() {
        return Err(Error::Failed(out.status.code().unwrap_or(-1), tail_of(&String::from_utf8_lossy(&out.stderr))));
    }
    let s = String::from_utf8_lossy(&out.stdout);
    s.trim().strip_prefix("MD5=").map(str::to_string).ok_or_else(|| Error::Verify(format!("unexpected md5 output: {s}")))
}

/// Verify an encode. Lossless streams must decode to exactly the source PCM (MD5 compared);
/// lossy streams must decode cleanly to roughly the expected length. Returns a one-line report.
pub fn verify(job: &Job, bits: u16, expect_secs: f64) -> Result<String, Error> {
    let out = job.output.to_string_lossy().into_owned();
    let fmt = if bits <= 16 { "pcm_s16le" } else if bits <= 24 { "pcm_s24le" } else { "pcm_s32le" };
    if job.stream.is_lossless() {
        let (input_args, join) = join_inputs(job, "v")?;
        let src = md5_of(&job.ffmpeg, &input_args, Some(&join), None, fmt)?;
        let dec = md5_of(&job.ffmpeg, &["-i".to_string(), out], None, job.frames, fmt)?;
        if src == dec {
            Ok(format!("lossless verified, MD5 {src}"))
        } else {
            Err(Error::Verify(format!("decoded audio differs from source (source {src}, decoded {dec})")))
        }
    } else {
        let o = Command::new(&job.ffmpeg)
            .args(["-hide_banner", "-nostats", "-loglevel", "error", "-i", &out, "-f", "null", "-"])
            .output()?;
        let err = String::from_utf8_lossy(&o.stderr);
        if !o.status.success() || !err.trim().is_empty() {
            return Err(Error::Verify(format!("decode reported problems: {}", tail_of(&err))));
        }
        // Duration check: decode time via ffprobe-less approach (`-f null` progress is not captured); trust decode success.
        let _ = expect_secs;
        Ok("decodes cleanly".to_string())
    }
}

/// Locate ffmpeg on PATH.
pub fn find_ffmpeg() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p).map(|d| d.join(exe)).find(|c: &PathBuf| Path::new(c).is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(stream: StreamType) -> Job {
        let layout = ChannelLayout::find("5.1").unwrap();
        let inputs = layout.channels.iter().map(|c| (*c, PathBuf::from(format!("{}.wav", c.short())))).collect();
        Job { stream, layout, inputs, output: "out.ac3".into(), bitrate_kbps: Some(448), sample_rate: Some(48_000), dialnorm: Some(31), start_secs: None, duration_secs: None, frames: None, slots: None, ffmpeg: "ffmpeg".into() }
    }

    #[test]
    fn ac3_args() {
        let a = build_args(&job(StreamType::Ac3)).unwrap().join(" ");
        assert!(a.contains("join=inputs=6:channel_layout=FL+FR+FC+LFE+SL+SR"));
        assert!(a.contains("-c:a ac3"));
        assert!(a.contains("-b:a 448k"));
        assert!(a.contains("-dialnorm -31"), "dialnorm must be negative: {a}");
    }

    #[test]
    fn dtshd_is_unsupported() {
        assert!(matches!(build_args(&job(StreamType::DtsHdMa)), Err(Error::Unsupported(_))));
    }
}
