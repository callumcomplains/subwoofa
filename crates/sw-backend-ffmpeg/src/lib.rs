//! FFmpeg backend: builds an `ffmpeg` command that joins per-channel mono/stereo files into
//! one multichannel stream and encodes it with the requested codec.
//!
//! This is the day-one engine; native encoders replace it format by format.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
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
        Oh => "TC",
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
    let mut args = Vec::new();
    let mut map = String::new();
    let mut names = Vec::new();
    for (i, ch) in job.layout.channels.iter().enumerate() {
        let path = job.inputs.get(ch).ok_or(Error::MissingChannel(ch.short()))?;
        if let Some(s) = job.start_secs {
            args.extend(["-ss".into(), s.to_string()]);
        }
        if let Some(d) = job.duration_secs {
            args.extend(["-t".into(), d.to_string()]);
        }
        args.extend(["-i".into(), path.to_string_lossy().into_owned()]);
        map.push_str(&format!("[{i}:a]"));
        names.push(ff_name(*ch).ok_or(Error::Unsupported("channel"))?);
    }
    let n = job.layout.channel_count();
    let filter = if n == 1 {
        format!("[0:a]anull[{label}]")
    } else {
        format!("{map}join=inputs={n}:channel_layout={}[{label}]", names.join("+"))
    };
    Ok((args, filter))
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
        Job { stream, layout, inputs, output: "out.ac3".into(), bitrate_kbps: Some(448), sample_rate: Some(48_000), dialnorm: Some(31), start_secs: None, duration_secs: None, ffmpeg: "ffmpeg".into() }
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
