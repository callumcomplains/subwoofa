//! Regression sweep over the native DTS encoders: every stream type at a spread of layouts, rates and bit rates is
//! encoded from synthetic stems, then checked with `subwoofa verify --deep` (structure, a strict FFmpeg decode and the
//! DTS reference verifier when it is installed).
//!
//! Slow, so it only runs on request:
//!
//!     cargo test --release -p sw-cli --test sweep -- --ignored --nocapture
use std::path::Path;
use std::process::Command;
use sw_core::ChannelLayout;

struct Case {
    stream: &'static str,
    layout: &'static str,
    rate: u32,
    /// `--bitrate` (0 = no core on MA); `None` leaves it to the default.
    bitrate: Option<u32>,
    extra: &'static [&'static str],
}

const fn case(stream: &'static str, layout: &'static str, rate: u32, bitrate: Option<u32>) -> Case {
    Case { stream, layout, rate, bitrate, extra: &[] }
}

const CASES: &[Case] = &[
    case("dts", "5.1", 48_000, Some(1509)),
    case("dts", "2.0", 48_000, None),
    case("dts-es", "6.1-discrete", 48_000, Some(1509)),
    case("dts-es", "6.1-matrix", 48_000, Some(1509)),
    case("dts-96-24", "5.1", 96_000, Some(1509)),
    case("dtshd-hra", "5.1", 48_000, Some(3018)),
    case("dtshd-hra", "7.1-ls-rs-lsr-rsr", 48_000, Some(3018)),
    case("dtshd-hra", "5.1", 96_000, Some(4224)),
    case("dtshd-hra", "6.1-matrix", 96_000, Some(4224)),
    case("dtshd-hra", "6.1-discrete", 96_000, Some(4224)),
    case("dtshd-hra", "7.1-side-rear", 96_000, Some(5376)),
    case("dtshd-hra", "7.0-side-rear", 96_000, Some(5376)),
    case("dtshd-ma", "2.0", 48_000, Some(768)),
    case("dtshd-ma", "5.1", 48_000, Some(1509)),
    case("dtshd-ma", "5.1", 48_000, Some(0)),
    case("dtshd-ma", "7.1-side-rear", 48_000, Some(1509)),
    case("dtshd-ma", "7.1-side-rear", 96_000, Some(1509)),
    case("dtshd-ma", "5.1", 192_000, Some(1509)),
    case("dtshd-ma", "5.1", 192_000, Some(0)),
    case("dtshd-ma", "7.1.4", 48_000, Some(0)),
    Case { stream: "dtshd-ma", layout: "5.1", rate: 96_000, bitrate: Some(1509), extra: &["--core-96-24"] },
];

fn write_wav(path: &Path, rate: u32, seconds: usize, freq: f64, seed: u32) {
    let n = rate as usize * seconds;
    let mut data = Vec::with_capacity(n * 3);
    let mut s = seed.wrapping_mul(2654435761).wrapping_add(1);
    for k in 0..n {
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        let noise = ((s >> 12) as i32 & 0xFFF) - 2048;
        let t = k as f64 / rate as f64;
        let v = ((2.0 * std::f64::consts::PI * freq * t).sin() * 1_800_000.0 + (2.0 * std::f64::consts::PI * freq * 3.1 * t).sin() * 250_000.0) as i32 + noise * 12;
        data.extend_from_slice(&v.to_le_bytes()[..3]);
    }
    let mut f = Vec::with_capacity(data.len() + 44);
    f.extend_from_slice(b"RIFF");
    f.extend_from_slice(&((36 + data.len()) as u32).to_le_bytes());
    f.extend_from_slice(b"WAVEfmt ");
    f.extend_from_slice(&16u32.to_le_bytes());
    f.extend_from_slice(&1u16.to_le_bytes());
    f.extend_from_slice(&1u16.to_le_bytes());
    f.extend_from_slice(&rate.to_le_bytes());
    f.extend_from_slice(&(rate * 3).to_le_bytes());
    f.extend_from_slice(&3u16.to_le_bytes());
    f.extend_from_slice(&24u16.to_le_bytes());
    f.extend_from_slice(b"data");
    f.extend_from_slice(&(data.len() as u32).to_le_bytes());
    f.extend_from_slice(&data);
    std::fs::write(path, f).unwrap();
}

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
#[ignore = "slow: run with --ignored"]
fn native_dts_sweep() {
    if !have_ffmpeg() {
        eprintln!("FFmpeg not found: sweep skipped");
        return;
    }
    let exe = env!("CARGO_BIN_EXE_subwoofa");
    let dir = std::env::temp_dir().join(format!("sw_sweep_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut failures = Vec::new();
    for (i, c) in CASES.iter().enumerate() {
        let label = format!("{} {} {} Hz{}{}", c.stream, c.layout, c.rate, c.bitrate.map_or(String::new(), |b| format!(" {b} kbps")), if c.extra.is_empty() { String::new() } else { format!(" {}", c.extra.join(" ")) });
        let layout = ChannelLayout::find(c.layout).unwrap();
        let wavs = dir.join(format!("w{}_{}", c.rate, layout.id));
        std::fs::create_dir_all(&wavs).unwrap();
        let mut cmd = Command::new(exe);
        cmd.args(["encode", "--engine", "native", "--stream", c.stream, "--layout", c.layout, "--dest", "bluray-primary", "--sample-rate", &c.rate.to_string(), "--duration", "3"]);
        for (k, ch) in layout.channels.iter().enumerate() {
            let p = wavs.join(format!("{}.wav", ch.short()));
            if !p.exists() {
                write_wav(&p, c.rate, 4, 180.0 + 97.0 * k as f64, k as u32 + 1);
            }
            cmd.arg("--input").arg(format!("{}={}", ch.short(), p.display()));
        }
        if let Some(b) = c.bitrate {
            cmd.args(["--bitrate", &b.to_string()]);
        }
        cmd.args(c.extra);
        // the core-only streams are bare `.dts` streams; the rest are `.dtshd` containers
        let ext = if matches!(c.stream, "dts" | "dts-es" | "dts-96-24") { "dts" } else { "dtshd" };
        let out = dir.join(format!("case{i}.{ext}"));
        cmd.arg("-o").arg(&out);
        let enc = cmd.output().unwrap();
        if !enc.status.success() {
            failures.push(format!("{label}: encode failed: {}{}", String::from_utf8_lossy(&enc.stdout), String::from_utf8_lossy(&enc.stderr)));
            println!("FAIL  {label}  (encode)");
            continue;
        }
        let ver = Command::new(exe).args(["verify", "--deep"]).arg(&out).output().unwrap();
        let text = String::from_utf8_lossy(&ver.stdout).to_string();
        let ok = ver.status.success() && text.contains("PASSED");
        println!("{}  {label}  ({} bytes)", if ok { "ok  " } else { "FAIL" }, std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0));
        if !ok {
            let bad: Vec<&str> = text.lines().filter(|l| l.contains("[FAIL]") || l.contains("[WARN]")).collect();
            failures.push(format!("{label}: verify: {}", bad.join(" | ")));
        }
    }
    if std::env::var_os("SW_SWEEP_KEEP").is_some() {
        println!("files kept in {}", dir.display());
    } else {
        let _ = std::fs::remove_dir_all(&dir);
    }
    assert!(failures.is_empty(), "{} of {} cases failed:\n{}", failures.len(), CASES.len(), failures.join("\n"));
}
