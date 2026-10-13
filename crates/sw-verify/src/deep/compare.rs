//! Comparing what a file claims with what its audio does: the bit-rate section, the DTS core against the
//! extension, and a second decoder's opinion.

use crate::deep::decode::{extract_stream, read_pcm};
use crate::{Collector, Options, Probe, Status};
use std::path::Path;
use std::process::Command;

pub(crate) fn bitrate_section(c: &mut Collector, p: &Probe) {
    if p.frame_bytes.is_empty() || p.sample_rate == 0 {
        return;
    }
    let n = p.frame_bytes.len();
    let total: u64 = p.frame_bytes.iter().map(|b| *b as u64).sum();
    let total_samples: u64 = p.frame_samples.iter().map(|b| *b as u64).sum();
    let secs = total_samples as f64 / p.sample_rate as f64;
    let (mn, mx) = (
        p.frame_bytes.iter().min().copied().unwrap_or(0),
        p.frame_bytes.iter().max().copied().unwrap_or(0),
    );
    // peak over one-second windows (the usual measure of a variable rate stream)
    let mut peak = 0f64;
    let mut peak_at = 0f64;
    let (mut acc_bytes, mut acc_samples) = (0u64, 0u64);
    let mut head = 0usize;
    let mut t_head = 0f64;
    for i in 0..n {
        acc_bytes += p.frame_bytes[i] as u64;
        acc_samples += p.frame_samples[i] as u64;
        while acc_samples as f64 > p.sample_rate as f64 && head < i {
            acc_bytes -= p.frame_bytes[head] as u64;
            acc_samples -= p.frame_samples[head] as u64;
            t_head += p.frame_samples[head] as f64 / p.sample_rate as f64;
            head += 1;
        }
        if acc_samples as f64 >= p.sample_rate as f64 * 0.9 || i == n - 1 {
            let kbps = acc_bytes as f64 * 8.0 / (acc_samples as f64 / p.sample_rate as f64) / 1000.0;
            if kbps > peak {
                peak = kbps;
                peak_at = t_head;
            }
        }
    }
    let rows = vec![
        ("access units".to_string(), n.to_string()),
        (
            "size (min / mean / max)".to_string(),
            format!("{mn} / {:.0} / {mx} bytes", total as f64 / n as f64),
        ),
        (
            "average bit rate".to_string(),
            format!("{:.1} kbps", total as f64 * 8.0 / secs.max(1e-9) / 1000.0),
        ),
        (
            "peak over one second".to_string(),
            format!("{peak:.1} kbps (starting at {peak_at:.1} s)"),
        ),
        ("constant rate".to_string(), (mn == mx).to_string()),
    ];
    c.section("Bit rate", rows);
}

pub(crate) fn core_comparison(path: &Path, ffmpeg: &Path, p: &Probe, channels: usize, c: &mut Collector) {
    if !p.dts || !p.has_core || !p.lossless || channels == 0 {
        return;
    }
    if p.core_rate != 0 && p.core_rate != p.sample_rate {
        c.check("Core against lossless extension", Status::Info, format!("the core runs at {} Hz and the lossless audio at {} Hz, so the two decodes cannot be compared sample for sample", p.core_rate, p.sample_rate));
        return;
    }
    let (Some(full), Some(core)) = (read_pcm(path, ffmpeg, false, 30), read_pcm(path, ffmpeg, true, 30)) else {
        return;
    };
    if core.is_empty() {
        return;
    }
    // the core decodes to fewer channels when the stream is wider than the core
    let core_ch = if core.len() * channels == full.len() {
        channels
    } else {
        0
    };
    if core_ch == 0 {
        c.check("Core against lossless extension", Status::Info, "the core has a different channel count or sample rate than the full stream, so the two decodes were not compared");
        return;
    }
    let frames = (full.len() / channels).min(core.len() / channels);
    // find the lag of channel 0 (a window from the middle)
    let win = 20_000.min(frames.saturating_sub(4000));
    if win < 4000 {
        return;
    }
    let mid = frames / 2;
    let at = |v: &Vec<i32>, i: usize, ch: usize| v[i * channels + ch] as f64;
    let mut best = (0i64, f64::MIN);
    for lag in -1200i64..=1200 {
        let mut s = 0f64;
        for i in (mid..mid + win).step_by(3) {
            let j = (i as i64 + lag) as usize;
            s += at(&full, i, 0) * at(&core, j, 0);
        }
        if s > best.1 {
            best = (lag, s);
        }
    }
    let lag = best.0;
    let mut sig = 0f64;
    let mut err = 0f64;
    for i in 2000..frames.saturating_sub(2000) {
        let j = (i as i64 + lag) as usize;
        if j >= frames {
            break;
        }
        for ch in 0..channels {
            let a = at(&full, i, ch);
            let b = at(&core, j, ch);
            sig += a * a;
            err += (a - b) * (a - b);
        }
    }
    let snr = if err > 0.0 {
        10.0 * (sig / err).log10()
    } else {
        f64::INFINITY
    };
    let aligned = lag == 0;
    c.check(
        "Core against lossless extension",
        if aligned && snr > 10.0 { Status::Pass } else { Status::Warn },
        format!("over the first 30 s the core-only decode differs from the lossless decode by {snr:.1} dB SNR at a lag of {lag} samples{}", if aligned { "" } else { " (the core is not time-aligned with the lossless audio)" }),
    );
}

pub(crate) fn reference(path: &Path, opt: &Options, probe: Option<&Probe>, c: &mut Collector) {
    if !probe.is_some_and(|p| p.dts) {
        return;
    }
    let tool = opt.reference_verifier.clone().or_else(crate::mas_verifier);
    let Some(tool) = tool else {
        c.check(
            "DTS reference verifier",
            Status::Skipped,
            "DTSHDVerify is not installed; install the DTS-HD encoder suite to add its verdict",
        );
        return;
    };
    let out = match Command::new(&tool)
        .arg(path)
        .stdin(std::process::Stdio::null())
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            c.check(
                "DTS reference verifier",
                Status::Skipped,
                format!("could not run {}: {e}", tool.display()),
            );
            return;
        }
    };
    let mut text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    // the verifier reads a `.dtshd` file through its extension substream: a core-only stream has none, so it gets
    // the bare stream instead
    if text.contains("number of audio presentations (0)") {
        if let Some(raw) = extract_stream(path) {
            if let Ok(o) = Command::new(&tool)
                .arg(&raw)
                .stdin(std::process::Stdio::null())
                .output()
            {
                text = String::from_utf8_lossy(&o.stdout).to_string() + &String::from_utf8_lossy(&o.stderr);
            }
            let _ = std::fs::remove_file(&raw);
        }
    }
    let summary = text
        .lines()
        .find(|l| l.contains("frames verified"))
        .map(|l| l.trim().to_string());
    let passed = text.contains("passes verification with no errors");
    c.check(
        "DTS reference verifier (DTSHDVerify)",
        if passed { Status::Pass } else { Status::Fail },
        summary.unwrap_or_else(|| {
            text.lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("no output")
                .trim()
                .to_string()
        }),
    );
    if !passed {
        for l in text.lines().filter(|l| l.to_lowercase().contains("error")).take(20) {
            c.finding(Status::Fail, None, format!("DTSHDVerify: {}", l.trim()));
        }
    }
}
