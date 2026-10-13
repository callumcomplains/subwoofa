//! Deep verification: everything that needs the decoded audio or another program's opinion.

mod compare;
mod decode;
mod md5;
mod stats;

use crate::{Collector, Options, Probe, Status};
use compare::{bitrate_section, core_comparison, reference};
use decode::{decode, Decoded};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub(crate) fn run(path: &Path, opt: &Options, c: &mut Collector, probe: Option<&Probe>) {
    if let Some(p) = probe {
        bitrate_section(c, p);
    }
    let ffmpeg = opt.ffmpeg.clone().unwrap_or_else(|| PathBuf::from("ffmpeg"));
    match decode(path, &ffmpeg, probe, c) {
        Decoded::Missing => {
            c.check(
                "Full decode",
                Status::Skipped,
                "FFmpeg was not found, so the audio could not be decoded (set its path in the settings)",
            );
        }
        Decoded::Done(d) => {
            if let Some(p) = probe {
                core_comparison(path, &ffmpeg, p, d.channels, c);
            }
        }
    }
    flac_md5_check(path, &ffmpeg, c);
    reference(path, opt, probe, c);
}

/// A FLAC file stores the MD5 of the audio it holds; the decoded audio must hash to the same value.
fn flac_md5_check(path: &Path, ffmpeg: &Path, c: &mut Collector) {
    let stored = c
        .report
        .summary
        .iter()
        .find(|(k, _)| k == "Stored audio MD5")
        .map(|(_, v)| v.clone());
    let bits = c
        .report
        .summary
        .iter()
        .find(|(k, _)| k == "Sample format")
        .and_then(|(_, v)| v.split('-').next().and_then(|n| n.parse::<u32>().ok()));
    let (Some(stored), Some(bits)) = (stored, bits) else {
        return;
    };
    if stored == "not set" {
        c.check(
            "Audio MD5 (FLAC)",
            Status::Info,
            "the file does not store an MD5 of its audio",
        );
        return;
    }
    let codec = if bits <= 16 { "pcm_s16le" } else { "pcm_s24le" };
    let out = Command::new(ffmpeg)
        .args(["-hide_banner", "-v", "error", "-i"])
        .arg(path)
        .args(["-vn", "-c:a", codec, "-f", "md5", "-"])
        .stdin(Stdio::null())
        .output();
    let Ok(out) = out else { return };
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let decoded = text.trim().strip_prefix("MD5=").unwrap_or("").to_lowercase();
    if decoded == stored {
        c.check(
            "Audio MD5 (FLAC)",
            Status::Pass,
            "the decoded audio hashes to the MD5 stored in the file",
        );
    } else {
        c.check(
            "Audio MD5 (FLAC)",
            Status::Fail,
            format!("decoded audio hashes to {decoded}, the file stores {stored}"),
        );
    }
}

/// A bare `.als` file (the record, then frames) has no container FFmpeg understands.
fn is_raw_als(path: &Path) -> bool {
    let mut head = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok()
        && &head == b"ALS\0"
}
