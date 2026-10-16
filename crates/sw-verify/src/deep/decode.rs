//! Decoding a file with FFmpeg and reading the samples back.

use crate::analysis::AudioTap;
use crate::deep::is_raw_als;
use crate::deep::md5::Md5;
use crate::deep::stats::Stats;
use crate::{Collector, Probe, Status};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use sw_core::process::command;

pub(crate) struct Done {
    pub(crate) channels: usize,
}

pub(crate) enum Decoded {
    Missing,
    Done(Done),
}

pub(crate) fn channels_of(layout: &str) -> Option<usize> {
    let l = layout.trim();
    match l {
        "mono" => return Some(1),
        "stereo" => return Some(2),
        "quad" | "quad(side)" => return Some(4),
        "hexagonal" | "hexadecagonal" => return Some(if l == "hexagonal" { 6 } else { 16 }),
        "octagonal" => return Some(8),
        _ => {}
    }
    if let Some(n) = l.strip_suffix(" channels") {
        return n.trim().parse().ok();
    }
    // 5.1(side), 7.1, 22.2, 3.0, 4.0 ...
    let base = l.split('(').next().unwrap_or(l);
    base.split('.').map(|p| p.parse::<usize>().ok()).sum::<Option<usize>>()
}

pub(crate) fn probe_channels(path: &Path, ffmpeg: &Path) -> Option<(usize, u32)> {
    let out = command(ffmpeg)
        .args(["-hide_banner", "-i"])
        .arg(path)
        .args(["-t", "0", "-f", "null", "-"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stderr).to_string();
    for line in text.lines() {
        if let Some(i) = line.find("Audio:") {
            let parts: Vec<&str> = line[i..].split(',').map(|s| s.trim()).collect();
            let rate = parts
                .iter()
                .find_map(|p| p.strip_suffix(" Hz").and_then(|r| r.parse::<u32>().ok()))
                .unwrap_or(0);
            for p in &parts {
                if let Some(n) = channels_of(p) {
                    if !p.ends_with("Hz") && !p.starts_with("Audio") {
                        return Some((n, rate));
                    }
                }
            }
        }
    }
    None
}

pub(crate) fn decode(path: &Path, ffmpeg: &Path, probe: Option<&Probe>, c: &mut Collector, analyse: bool) -> Decoded {
    if is_raw_als(path) {
        c.check("Full decode", Status::Skipped, "FFmpeg cannot read a raw .als file; encode to .mp4 (the same stream in an MP4 file) for a full decode check");
        return Decoded::Done(Done { channels: 0 });
    }
    let Some((channels, rate)) = probe_channels(path, ffmpeg) else {
        // either FFmpeg is missing or it cannot read the file at all
        if command(ffmpeg).arg("-version").output().is_err() {
            return Decoded::Missing;
        }
        c.check("Full decode", Status::Fail, "FFmpeg could not open the file as audio");
        return Decoded::Done(Done { channels: 0 });
    };
    let expected = probe.map_or(0, |p| (p.duration_secs * rate as f64) as u64);
    let mut tap = (analyse && rate > 0).then(|| AudioTap::new(rate, channels, expected));
    let Some(run) = run_decoder(path, ffmpeg, channels, tap.as_mut()) else {
        return Decoded::Missing;
    };
    if let Some(tap) = tap {
        c.report.analysis = Some(tap.finish(probe));
    }
    let bad: Vec<&str> = run
        .messages
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .filter(|l| !l.starts_with("Last message repeated") && !l.contains("Estimating duration from bitrate"))
        .collect();
    for l in bad.iter().take(60) {
        c.finding(
            if is_fatal(l) { Status::Fail } else { Status::Warn },
            None,
            format!("decoder: {l}"),
        );
    }
    let any_fatal = bad.iter().any(|l| is_fatal(l));
    let clean = run.exit_ok && bad.is_empty();
    c.check(
        "Full decode (FFmpeg, strict)",
        if clean {
            Status::Pass
        } else if !run.exit_ok || any_fatal {
            Status::Fail
        } else {
            Status::Warn
        },
        if clean {
            format!("{} samples per channel decoded with no complaints", run.stats.frames)
        } else if !run.exit_ok {
            "the decoder stopped with an error".to_string()
        } else {
            format!("the decoder reported {} message(s)", bad.len())
        },
    );
    if let Some(p) = probe {
        report_length(c, p, &run.stats, channels, rate);
    }
    c.kv("Decoded PCM (24-bit) MD5", run.digest.clone());
    if run.stats.frames > 0 {
        run.stats.report(c, rate.max(1));
    }
    Decoded::Done(Done { channels })
}

/// Messages from the decoder that mean the file is damaged; the rest are warnings.
fn is_fatal(line: &str) -> bool {
    let low = line.to_lowercase();
    [
        "error", "invalid", "corrupt", "crc", "missing", "overread", "failed", "checksum",
    ]
    .iter()
    .any(|word| low.contains(word))
}

/// Does the decoded length and channel count match what the headers say?
fn report_length(c: &mut Collector, p: &Probe, stats: &Stats, channels: usize, rate: u32) {
    if p.sample_rate > 0 && rate > 0 && stats.frames > 0 {
        let expect = (p.duration_secs * rate as f64).round() as i64;
        let diff = stats.frames as i64 - expect;
        let tol = 4096;
        c.check(
            "Decoded length",
            if diff.abs() <= tol { Status::Pass } else { Status::Warn },
            format!(
                "{} samples decoded, the frame count implies {expect} (difference {diff})",
                stats.frames
            ),
        );
    }
    if channels != 0 && p.channels != 0 && channels as u32 != p.channels {
        c.check(
            "Channel count",
            Status::Info,
            format!(
                "the decoder outputs {channels} channels; the headers describe {}",
                p.channels
            ),
        );
    }
}

/// What came out of one run of the decoder.
struct DecodeRun {
    stats: Stats,
    /// MD5 of the decoded 24-bit PCM.
    digest: String,
    /// Everything the decoder wrote to its error output.
    messages: String,
    exit_ok: bool,
}

/// Decodes the whole file to 24-bit PCM, gathering statistics and a checksum as the samples arrive.
fn run_decoder(path: &Path, ffmpeg: &Path, channels: usize, mut tap: Option<&mut AudioTap>) -> Option<DecodeRun> {
    let mut child = command(ffmpeg)
        .args([
            "-hide_banner",
            "-v",
            "warning",
            "-err_detect",
            "crccheck+bitstream+buffer",
            "-i",
        ])
        .arg(path)
        .args(["-vn", "-c:a", "pcm_s24le", "-f", "s24le", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let mut err = child.stderr.take().unwrap();
    let errs = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    });
    let mut out = child.stdout.take().unwrap();
    let mut stats = Stats::new(channels);
    let mut md5 = Md5::new();
    let frame_bytes = 3 * channels.max(1);
    let mut buf = vec![0u8; frame_bytes * 4096];
    let mut have = 0usize;
    loop {
        let n = match out.read(&mut buf[have..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        have += n;
        let whole = have / frame_bytes * frame_bytes;
        if whole > 0 {
            md5.update(&buf[..whole]);
            stats.feed(&buf[..whole]);
            if let Some(tap) = tap.as_mut() {
                tap.feed(&buf[..whole]);
            }
            buf.copy_within(whole..have, 0);
            have -= whole;
        }
    }
    let status = child.wait();
    Some(DecodeRun {
        stats,
        digest: md5.finish(),
        messages: errs.join().unwrap_or_default(),
        exit_ok: status.map(|s| s.success()).unwrap_or(false),
    })
}

pub(crate) fn read_pcm(path: &Path, ffmpeg: &Path, core_only: bool, seconds: u32) -> Option<Vec<i32>> {
    let mut cmd = command(ffmpeg);
    cmd.args(["-hide_banner", "-v", "error"]);
    if core_only {
        cmd.args(["-core_only", "1"]);
    }
    let out = cmd
        .arg("-i")
        .arg(path)
        .args([
            "-t",
            &seconds.to_string(),
            "-vn",
            "-c:a",
            "pcm_s24le",
            "-f",
            "s24le",
            "-",
        ])
        .output()
        .ok()?;
    Some(
        out.stdout
            .chunks_exact(3)
            .map(|b| (((b[0] as i32) | ((b[1] as i32) << 8) | ((b[2] as i32) << 16)) << 8) >> 8)
            .collect(),
    )
}

/// Copies the stream data of a `.dtshd` file (or a bare stream with that name) to a temporary raw `.dts` file.
pub(crate) fn extract_stream(path: &Path) -> Option<PathBuf> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut pos = 0u64;
    let mut magic = [0u8; 8];
    f.read_exact(&mut magic).ok()?;
    if &magic != b"DTSHDHDR" {
        // a bare stream under a `.dtshd` name: the verifier only needs it to have another extension
        let out = std::env::temp_dir().join(format!("sw_verify_core_{}.dts", std::process::id()));
        std::fs::copy(path, &out).ok()?;
        return Some(out);
    }
    while pos + 16 <= len {
        f.seek(SeekFrom::Start(pos)).ok()?;
        let mut h = [0u8; 16];
        f.read_exact(&mut h).ok()?;
        let size = u64::from_be_bytes(h[8..16].try_into().ok()?);
        if &h[..8] == b"STRMDATA" {
            let out = std::env::temp_dir().join(format!("sw_verify_core_{}.dts", std::process::id()));
            let mut w = std::io::BufWriter::new(std::fs::File::create(&out).ok()?);
            let mut left = size.min(len - pos - 16);
            let mut buf = vec![0u8; 1 << 20];
            while left > 0 {
                let k = left.min(buf.len() as u64) as usize;
                f.read_exact(&mut buf[..k]).ok()?;
                w.write_all(&buf[..k]).ok()?;
                left -= k as u64;
            }
            w.flush().ok()?;
            return Some(out);
        }
        pos += 16 + size;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_names_give_channel_counts() {
        assert_eq!(channels_of("5.1(side)"), Some(6));
        assert_eq!(channels_of("stereo"), Some(2));
        assert_eq!(channels_of("22.2"), Some(24));
        assert_eq!(channels_of("7.1.4"), Some(12));
        assert_eq!(channels_of("9.1.6"), Some(16));
        assert_eq!(channels_of("7 channels"), Some(7));
    }
}
