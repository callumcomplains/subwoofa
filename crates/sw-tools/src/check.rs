//! Checking a stream by decoding it with FFmpeg.

use crate::{Result, Stream};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct VerifyReport {
    pub passed: bool,
    /// Decoder complaints (empty when the stream decodes cleanly).
    pub messages: Vec<String>,
    /// MD5 of the decoded 24-bit PCM, so two encodes of the same source can be compared.
    pub pcm_md5: Option<String>,
    pub frames: usize,
    pub duration_secs: f64,
}

/// Structural check plus a full decode with FFmpeg (errors on stderr fail the check).
pub fn verify(path: &Path, ffmpeg: &Path) -> Result<VerifyReport> {
    let s = Stream::open(path)?;
    let out = sw_core::process::command(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(path)
        .args(["-c:a", "pcm_s24le", "-f", "md5", "-"])
        .output()?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    let messages: Vec<String> = stderr
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect();
    let md5 = String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("MD5=").map(|m| m.to_string()));
    Ok(VerifyReport {
        passed: out.status.success() && messages.is_empty(),
        messages,
        pcm_md5: md5,
        frames: s.frames.len(),
        duration_secs: s.duration_secs(),
    })
}
