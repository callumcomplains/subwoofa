//! Deep verification: everything that needs the decoded audio or another program's opinion.

use crate::{Collector, Options, Probe, Status};
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
            c.check("Full decode", Status::Skipped, "FFmpeg was not found, so the audio could not be decoded (set its path in the settings)");
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

// ------------------------------------------------------------------------------------------------
// FLAC's own checksum

/// A FLAC file stores the MD5 of the audio it holds; the decoded audio must hash to the same value.
fn flac_md5_check(path: &Path, ffmpeg: &Path, c: &mut Collector) {
    let stored = c.report.summary.iter().find(|(k, _)| k == "Stored audio MD5").map(|(_, v)| v.clone());
    let bits = c.report.summary.iter().find(|(k, _)| k == "Sample format").and_then(|(_, v)| v.split('-').next().and_then(|n| n.parse::<u32>().ok()));
    let (Some(stored), Some(bits)) = (stored, bits) else { return };
    if stored == "not set" {
        c.check("Audio MD5 (FLAC)", Status::Info, "the file does not store an MD5 of its audio");
        return;
    }
    let codec = if bits <= 16 { "pcm_s16le" } else { "pcm_s24le" };
    let out = Command::new(ffmpeg).args(["-hide_banner", "-v", "error", "-i"]).arg(path).args(["-vn", "-c:a", codec, "-f", "md5", "-"]).stdin(Stdio::null()).output();
    let Ok(out) = out else { return };
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let decoded = text.trim().strip_prefix("MD5=").unwrap_or("").to_lowercase();
    if decoded == stored {
        c.check("Audio MD5 (FLAC)", Status::Pass, "the decoded audio hashes to the MD5 stored in the file");
    } else {
        c.check("Audio MD5 (FLAC)", Status::Fail, format!("decoded audio hashes to {decoded}, the file stores {stored}"));
    }
}

/// A bare `.als` file (the record, then frames) has no container FFmpeg understands.
fn is_raw_als(path: &Path) -> bool {
    let mut head = [0u8; 4];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && &head == b"ALS\0"
}

// ------------------------------------------------------------------------------------------------
// bit rate

fn bitrate_section(c: &mut Collector, p: &Probe) {
    if p.frame_bytes.is_empty() || p.sample_rate == 0 {
        return;
    }
    let n = p.frame_bytes.len();
    let total: u64 = p.frame_bytes.iter().map(|b| *b as u64).sum();
    let total_samples: u64 = p.frame_samples.iter().map(|b| *b as u64).sum();
    let secs = total_samples as f64 / p.sample_rate as f64;
    let (mn, mx) = (p.frame_bytes.iter().min().copied().unwrap_or(0), p.frame_bytes.iter().max().copied().unwrap_or(0));
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
        ("size (min / mean / max)".to_string(), format!("{mn} / {:.0} / {mx} bytes", total as f64 / n as f64)),
        ("average bit rate".to_string(), format!("{:.1} kbps", total as f64 * 8.0 / secs.max(1e-9) / 1000.0)),
        ("peak over one second".to_string(), format!("{peak:.1} kbps (starting at {peak_at:.1} s)")),
        ("constant rate".to_string(), (mn == mx).to_string()),
    ];
    c.section("Bit rate", rows);
}

// ------------------------------------------------------------------------------------------------
// decode

struct Done {
    channels: usize,
}

enum Decoded {
    Missing,
    Done(Done),
}

fn channels_of(layout: &str) -> Option<usize> {
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

fn probe_channels(path: &Path, ffmpeg: &Path) -> Option<(usize, u32)> {
    let out = Command::new(ffmpeg).args(["-hide_banner", "-i"]).arg(path).args(["-t", "0", "-f", "null", "-"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stderr).to_string();
    for line in text.lines() {
        if let Some(i) = line.find("Audio:") {
            let parts: Vec<&str> = line[i..].split(',').map(|s| s.trim()).collect();
            let rate = parts.iter().find_map(|p| p.strip_suffix(" Hz").and_then(|r| r.parse::<u32>().ok())).unwrap_or(0);
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

fn decode(path: &Path, ffmpeg: &Path, probe: Option<&Probe>, c: &mut Collector) -> Decoded {
    if is_raw_als(path) {
        c.check("Full decode", Status::Skipped, "FFmpeg cannot read a raw .als file; encode to .mp4 (the same stream in an MP4 file) for a full decode check");
        return Decoded::Done(Done { channels: 0 });
    }
    let Some((channels, rate)) = probe_channels(path, ffmpeg) else {
        // either FFmpeg is missing or it cannot read the file at all
        if Command::new(ffmpeg).arg("-version").output().is_err() {
            return Decoded::Missing;
        }
        c.check("Full decode", Status::Fail, "FFmpeg could not open the file as audio");
        return Decoded::Done(Done { channels: 0 });
    };
    let mut child = match Command::new(ffmpeg)
        .args(["-hide_banner", "-v", "warning", "-err_detect", "crccheck+bitstream+buffer", "-i"])
        .arg(path)
        .args(["-vn", "-c:a", "pcm_s24le", "-f", "s24le", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return Decoded::Missing,
    };
    let mut err = child.stderr.take().unwrap();
    let errs = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    });
    let mut out = child.stdout.take().unwrap();
    let mut stats = Stats::new(channels);
    let mut md5 = Md5::new();
    let mut buf = vec![0u8; 3 * channels.max(1) * 4096];
    let mut have = 0usize;
    loop {
        let n = match out.read(&mut buf[have..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        have += n;
        let whole = have / (3 * channels.max(1)) * 3 * channels.max(1);
        if whole > 0 {
            md5.update(&buf[..whole]);
            stats.feed(&buf[..whole]);
            buf.copy_within(whole..have, 0);
            have -= whole;
        }
    }
    let status = child.wait();
    let messages = errs.join().unwrap_or_default();
    let digest = md5.finish();
    let lines: Vec<&str> = messages.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    let bad: Vec<&&str> = lines.iter().filter(|l| !l.starts_with("Last message repeated") && !l.contains("Estimating duration from bitrate")).collect();
    for l in bad.iter().take(60) {
        let low = l.to_lowercase();
        let fatal = low.contains("error") || low.contains("invalid") || low.contains("corrupt") || low.contains("crc") || low.contains("missing") || low.contains("overread") || low.contains("failed") || low.contains("checksum");
        c.finding(if fatal { Status::Fail } else { Status::Warn }, None, format!("decoder: {l}"));
    }
    let any_fatal = bad.iter().any(|l| {
        let low = l.to_lowercase();
        low.contains("error") || low.contains("invalid") || low.contains("corrupt") || low.contains("crc") || low.contains("missing") || low.contains("overread") || low.contains("failed") || low.contains("checksum")
    });
    let ok_exit = status.map(|s| s.success()).unwrap_or(false);
    c.check(
        "Full decode (FFmpeg, strict)",
        if ok_exit && bad.is_empty() { Status::Pass } else if !ok_exit || any_fatal { Status::Fail } else { Status::Warn },
        if bad.is_empty() && ok_exit { format!("{} samples per channel decoded with no complaints", stats.frames) } else if !ok_exit { "the decoder stopped with an error".to_string() } else { format!("the decoder reported {} message(s)", bad.len()) },
    );
    // length
    if let Some(p) = probe {
        if p.sample_rate > 0 && rate > 0 && stats.frames > 0 {
            let expect = (p.duration_secs * rate as f64).round() as i64;
            let diff = stats.frames as i64 - expect;
            let tol = 4096;
            c.check("Decoded length", if diff.abs() <= tol { Status::Pass } else { Status::Warn }, format!("{} samples decoded, the frame count implies {expect} (difference {diff})", stats.frames));
        }
        if channels != 0 && p.channels != 0 && channels as u32 != p.channels {
            c.check("Channel count", Status::Info, format!("the decoder outputs {channels} channels; the headers describe {}", p.channels));
        }
    }
    // results
    c.kv("Decoded PCM (24-bit) MD5", digest.clone());
    if stats.frames > 0 {
        stats.report(c, rate.max(1));
    }
    Decoded::Done(Done { channels })
}

struct Stats {
    channels: usize,
    frames: u64,
    peak: Vec<i32>,
    sum: Vec<i64>,
    sumsq: Vec<f64>,
    clipped: Vec<u64>,
    zero_run: u64,
    longest_zero: u64,
    longest_zero_at: u64,
    cur_zero_start: u64,
    first_sound: Option<u64>,
    last_sound: u64,
    or_bits: Vec<u32>,
    hashes: Vec<u64>,
    silent_all: Vec<bool>,
}

impl Stats {
    fn new(channels: usize) -> Stats {
        Stats { channels, frames: 0, peak: vec![0; channels], sum: vec![0; channels], sumsq: vec![0.0; channels], clipped: vec![0; channels], zero_run: 0, longest_zero: 0, longest_zero_at: 0, cur_zero_start: 0, first_sound: None, last_sound: 0, or_bits: vec![0; channels], hashes: vec![0xcbf29ce484222325; channels], silent_all: vec![true; channels] }
    }
    fn feed(&mut self, d: &[u8]) {
        let ch = self.channels;
        for frame in d.chunks_exact(3 * ch) {
            let mut all_zero = true;
            for k in 0..ch {
                let b = &frame[k * 3..k * 3 + 3];
                let v = ((b[0] as i32) | ((b[1] as i32) << 8) | ((b[2] as i32) << 16)) << 8 >> 8;
                if v != 0 {
                    all_zero = false;
                    self.silent_all[k] = false;
                }
                let a = v.abs();
                if a > self.peak[k] {
                    self.peak[k] = a;
                }
                self.sum[k] += v as i64;
                self.sumsq[k] += (v as f64) * (v as f64);
                if a >= 8_388_607 {
                    self.clipped[k] += 1;
                }
                self.or_bits[k] |= v as u32 & 0xFF_FFFF;
                self.hashes[k] = (self.hashes[k] ^ (v as u32 as u64)).wrapping_mul(0x100000001b3);
            }
            if all_zero {
                if self.zero_run == 0 {
                    self.cur_zero_start = self.frames;
                }
                self.zero_run += 1;
                if self.zero_run > self.longest_zero {
                    self.longest_zero = self.zero_run;
                    self.longest_zero_at = self.cur_zero_start;
                }
            } else {
                self.zero_run = 0;
                if self.first_sound.is_none() {
                    self.first_sound = Some(self.frames);
                }
                self.last_sound = self.frames;
            }
            self.frames += 1;
        }
    }
    fn report(&self, c: &mut Collector, rate: u32) {
        let r = rate as f64;
        let db = |x: f64| if x <= 0.0 { "-inf".to_string() } else { format!("{:.1}", 20.0 * (x / 8_388_608.0).log10()) };
        let mut rows = vec![("samples per channel".to_string(), format!("{} ({:.3} s at {rate} Hz)", self.frames, self.frames as f64 / r))];
        let mut clip_total = 0u64;
        for k in 0..self.channels {
            let rms = (self.sumsq[k] / self.frames.max(1) as f64).sqrt();
            let dc = self.sum[k] as f64 / self.frames.max(1) as f64;
            let wasted = if self.or_bits[k] == 0 { 24 } else { self.or_bits[k].trailing_zeros() };
            clip_total += self.clipped[k];
            rows.push((
                format!("channel {}", k + 1),
                if self.silent_all[k] { "digital silence throughout".to_string() } else { format!("peak {} dBFS, RMS {} dBFS, DC {:.1}, {} unused low bit(s), {} full-scale sample(s)", db(self.peak[k] as f64), db(rms), dc, wasted, self.clipped[k]) },
            ));
        }
        c.section("Decoded audio", rows);
        let silent: Vec<usize> = (0..self.channels).filter(|k| self.silent_all[*k]).map(|k| k + 1).collect();
        c.check("Channels carry audio", if silent.is_empty() || silent.len() == self.channels { if silent.len() == self.channels { Status::Warn } else { Status::Pass } } else { Status::Info }, if silent.is_empty() { "every channel has signal".to_string() } else if silent.len() == self.channels { "the whole programme is digital silence".to_string() } else { format!("channel(s) {silent:?} are digital silence throughout (normal for unused LFE or height channels)") });
        let mut dup = Vec::new();
        for a in 0..self.channels {
            for b in a + 1..self.channels {
                if !self.silent_all[a] && self.hashes[a] == self.hashes[b] {
                    dup.push(format!("{}={}", a + 1, b + 1));
                }
            }
        }
        if !dup.is_empty() {
            c.check("Identical channels", Status::Info, format!("these channels are sample-for-sample identical: {}", dup.join(", ")));
        }
        c.check("Clipping", if clip_total == 0 { Status::Pass } else { Status::Warn }, if clip_total == 0 { "no sample reaches full scale".to_string() } else { format!("{clip_total} full-scale sample(s) in all channels (decoded audio, so this includes any clipping in the source)") });
        if let Some(f) = self.first_sound {
            c.kv("Silence before the first sound", format!("{:.3} s", f as f64 / r));
            c.kv("Silence after the last sound", format!("{:.3} s", (self.frames - 1 - self.last_sound) as f64 / r));
        }
        if self.longest_zero as f64 / r > 1.0 && self.first_sound.map_or(false, |f| self.longest_zero_at > f) && self.longest_zero_at + self.longest_zero < self.last_sound {
            c.check("Dropouts", Status::Warn, format!("{:.2} s of digital silence in all channels at {:.2} s", self.longest_zero as f64 / r, self.longest_zero_at as f64 / r));
        } else {
            c.check("Dropouts", Status::Pass, "no stretch of silence longer than a second inside the programme");
        }
    }
}

// ------------------------------------------------------------------------------------------------
// core against the extension

fn read_pcm(path: &Path, ffmpeg: &Path, core_only: bool, seconds: u32) -> Option<Vec<i32>> {
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-hide_banner", "-v", "error"]);
    if core_only {
        cmd.args(["-core_only", "1"]);
    }
    let out = cmd.arg("-i").arg(path).args(["-t", &seconds.to_string(), "-vn", "-c:a", "pcm_s24le", "-f", "s24le", "-"]).output().ok()?;
    Some(out.stdout.chunks_exact(3).map(|b| (((b[0] as i32) | ((b[1] as i32) << 8) | ((b[2] as i32) << 16)) << 8) >> 8).collect())
}

fn core_comparison(path: &Path, ffmpeg: &Path, p: &Probe, channels: usize, c: &mut Collector) {
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
    let core_ch = if core.len() * channels == full.len() { channels } else { 0 };
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
    let snr = if err > 0.0 { 10.0 * (sig / err).log10() } else { f64::INFINITY };
    let aligned = lag == 0;
    c.check(
        "Core against lossless extension",
        if aligned && snr > 10.0 { Status::Pass } else { Status::Warn },
        format!("over the first 30 s the core-only decode differs from the lossless decode by {snr:.1} dB SNR at a lag of {lag} samples{}", if aligned { "" } else { " (the core is not time-aligned with the lossless audio)" }),
    );
}

// ------------------------------------------------------------------------------------------------
// the commercial verifier

fn reference(path: &Path, opt: &Options, probe: Option<&Probe>, c: &mut Collector) {
    if !probe.map_or(false, |p| p.dts) {
        return;
    }
    let tool = opt.reference_verifier.clone().or_else(crate::mas_verifier);
    let Some(tool) = tool else {
        c.check("DTS reference verifier", Status::Skipped, "DTSHDVerify is not installed; install the DTS-HD encoder suite to add its verdict");
        return;
    };
    let out = match Command::new(&tool).arg(path).stdin(std::process::Stdio::null()).output() {
        Ok(o) => o,
        Err(e) => {
            c.check("DTS reference verifier", Status::Skipped, format!("could not run {}: {e}", tool.display()));
            return;
        }
    };
    let mut text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    // the verifier reads a `.dtshd` file through its extension substream: a core-only stream has none, so it gets
    // the bare stream instead
    if text.contains("number of audio presentations (0)") {
        if let Some(raw) = extract_stream(path) {
            if let Ok(o) = Command::new(&tool).arg(&raw).stdin(std::process::Stdio::null()).output() {
                text = String::from_utf8_lossy(&o.stdout).to_string() + &String::from_utf8_lossy(&o.stderr);
            }
            let _ = std::fs::remove_file(&raw);
        }
    }
    let summary = text.lines().find(|l| l.contains("frames verified")).map(|l| l.trim().to_string());
    let passed = text.contains("passes verification with no errors");
    c.check("DTS reference verifier (DTSHDVerify)", if passed { Status::Pass } else { Status::Fail }, summary.unwrap_or_else(|| text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no output").trim().to_string()));
    if !passed {
        for l in text.lines().filter(|l| l.to_lowercase().contains("error")).take(20) {
            c.finding(Status::Fail, None, format!("DTSHDVerify: {}", l.trim()));
        }
    }
}

// ------------------------------------------------------------------------------------------------
// MD5

struct Md5 {
    s: [u32; 4],
    buf: Vec<u8>,
    len: u64,
}

const K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1, 0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];
const R: [u32; 64] = [7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21];

impl Md5 {
    fn new() -> Md5 {
        Md5 { s: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476], buf: Vec::new(), len: 0 }
    }
    fn block(&mut self, m: &[u8]) {
        let w: Vec<u32> = m.chunks_exact(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        let [mut a, mut b, mut c, mut d] = self.s;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let t = d;
            d = c;
            c = b;
            b = b.wrapping_add(a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(w[g]).rotate_left(R[i]));
            a = t;
        }
        self.s[0] = self.s[0].wrapping_add(a);
        self.s[1] = self.s[1].wrapping_add(b);
        self.s[2] = self.s[2].wrapping_add(c);
        self.s[3] = self.s[3].wrapping_add(d);
    }
    fn update(&mut self, data: &[u8]) {
        self.len += data.len() as u64;
        self.buf.extend_from_slice(data);
        let whole = self.buf.len() / 64 * 64;
        let chunk: Vec<u8> = self.buf.drain(..whole).collect();
        for b in chunk.chunks_exact(64) {
            self.block(b);
        }
    }
    fn finish(mut self) -> String {
        let bits = self.len * 8;
        self.buf.push(0x80);
        while self.buf.len() % 64 != 56 {
            self.buf.push(0);
        }
        self.buf.extend_from_slice(&bits.to_le_bytes());
        let rest = std::mem::take(&mut self.buf);
        for b in rest.chunks_exact(64) {
            self.block(b);
        }
        self.s.iter().flat_map(|v| v.to_le_bytes()).map(|b| format!("{b:02x}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_of_known_strings() {
        let mut m = Md5::new();
        m.update(b"abc");
        assert_eq!(m.finish(), "900150983cd24fb0d6963f7d28e17f72");
        let mut m = Md5::new();
        m.update(b"");
        assert_eq!(m.finish(), "d41d8cd98f00b204e9800998ecf8427e");
    }

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

/// Copies the stream data of a `.dtshd` file (or a bare stream with that name) to a temporary raw `.dts` file.
fn extract_stream(path: &Path) -> Option<PathBuf> {
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
