//! Output must not change by accident. Every format is encoded from the same synthetic input and the result is
//! compared with a checksum recorded when the output was last checked (by an independent decoder).
//!
//! When an encoder is changed on purpose, run
//!
//! ```text
//! SW_UPDATE_GOLDEN=1 cargo test -p sw-queue --test golden -- --nocapture
//! ```
//!
//! and paste the printed table over `GOLDEN` after checking the new files decode.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use sw_queue::run::{execute, EncodeSpec};

/// A mono 24-bit WAV of tone, noise and a few clicks and gaps: tonal, noisy, transient and silent stretches, so that
/// predictors, block switching and bit allocation all have something to do. `seed` makes each channel different.
fn write_wav(path: &Path, rate: u32, secs: f64, seed: u32) {
    let n = (rate as f64 * secs) as usize;
    let mut data = Vec::with_capacity(n * 3);
    let mut x = seed.wrapping_mul(2654435761).wrapping_add(1);
    for i in 0..n {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        let t = i as f64 / rate as f64;
        let noise = ((x >> 8) as i32 - (1 << 23)) / 60;
        let tone = (t * 2.0 * std::f64::consts::PI * (180.0 + seed as f64 * 70.0)).sin() * 1_800_000.0;
        let overtone = (t * 2.0 * std::f64::consts::PI * (2_300.0 + seed as f64 * 410.0)).sin() * 300_000.0;
        let click = if i % (rate as usize / 2) < 40 { 3_000_000 } else { 0 };
        let quiet = t > secs * 0.8; // the last fifth is much quieter
        let v = noise as f64 + tone + overtone + click as f64;
        let v = if quiet { v / 200.0 } else { v };
        data.extend_from_slice(&(v as i32).clamp(-8_388_608, 8_388_607).to_le_bytes()[..3]);
    }
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(b"RIFF").unwrap();
    f.write_all(&((36 + data.len()) as u32).to_le_bytes()).unwrap();
    f.write_all(b"WAVEfmt ").unwrap();
    f.write_all(&16u32.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap();
    f.write_all(&rate.to_le_bytes()).unwrap();
    f.write_all(&(rate * 3).to_le_bytes()).unwrap();
    f.write_all(&3u16.to_le_bytes()).unwrap();
    f.write_all(&24u16.to_le_bytes()).unwrap();
    f.write_all(b"data").unwrap();
    f.write_all(&(data.len() as u32).to_le_bytes()).unwrap();
    f.write_all(&data).unwrap();
}

/// The stems for the channels of a layout, written once per sample rate.
fn stems(dir: &Path, rate: u32, channels: &[&str]) -> HashMap<String, String> {
    channels
        .iter()
        .enumerate()
        .map(|(i, ch)| {
            let path = dir.join(format!("g{rate}_{ch}.wav"));
            if !path.exists() {
                write_wav(&path, rate, 3.0, i as u32 + 1);
            }
            (ch.to_string(), path.to_string_lossy().into_owned())
        })
        .collect()
}

const FIVE_ONE: [&str; 6] = ["L", "R", "C", "LFE", "Ls", "Rs"];
const SEVEN_ONE: [&str; 8] = ["L", "R", "C", "LFE", "Lss", "Rss", "Lsr", "Rsr"];
const SIX_ONE: [&str; 7] = ["L", "R", "C", "LFE", "Ls", "Rs", "Cs"];
const THREE_ZERO: [&str; 3] = ["L", "R", "C"];
const STEREO: [&str; 2] = ["L", "R"];

/// One encode: the stream, where it goes, its layout and rate, and the settings that matter for it.
struct Case {
    name: &'static str,
    dest: &'static str,
    stream: &'static str,
    layout: &'static str,
    rate: u32,
    bitrate: Option<u32>,
    pcm_format: &'static str,
    speed: &'static str,
    ext: &'static str,
}

const fn case(name: &'static str, dest: &'static str, stream: &'static str, ext: &'static str) -> Case {
    Case {
        name,
        dest,
        stream,
        layout: "5.1",
        rate: 48_000,
        bitrate: None,
        pcm_format: "",
        speed: "best",
        ext,
    }
}

fn cases() -> Vec<Case> {
    let c = case;
    vec![
        Case {
            bitrate: Some(640),
            ..c("ac3-5.1", "generic", "ac3", "ac3")
        },
        Case {
            layout: "2.0",
            bitrate: Some(192),
            ..c("ac3-2.0", "generic", "ac3", "ac3")
        },
        Case {
            bitrate: Some(192),
            ..c("ac3-5.1-192", "generic", "ac3", "ac3")
        },
        Case {
            layout: "2.0",
            bitrate: Some(256),
            ..c("eac3-2.0", "generic", "eac3", "eac3")
        },
        Case {
            bitrate: Some(640),
            ..c("eac3-5.1", "generic", "eac3", "eac3")
        },
        Case {
            layout: "7.1-side-rear",
            bitrate: Some(768),
            ..c("eac3-7.1", "generic", "eac3", "eac3")
        },
        Case {
            bitrate: Some(1509),
            ..c("dts-core-5.1", "bluray-primary", "dts", "dts")
        },
        Case {
            layout: "3.0",
            bitrate: Some(768),
            ..c("dts-core-3.0", "bluray-primary", "dts", "dts")
        },
        Case {
            bitrate: Some(1509),
            ..c("dts-core-dvd", "dvd", "dts", "cpt")
        },
        Case {
            bitrate: Some(1411),
            rate: 44_100,
            ..c("dts-music-disc", "dts-music-disc", "dts", "wav")
        },
        Case {
            layout: "6.1-matrix",
            bitrate: Some(1509),
            ..c("dts-es-6.1-matrix", "bluray-primary", "dts-es", "dts")
        },
        Case {
            layout: "6.1-discrete",
            bitrate: Some(1509),
            ..c("dts-es-6.1-discrete", "bluray-primary", "dts-es", "dts")
        },
        Case {
            bitrate: Some(1509),
            rate: 96_000,
            ..c("dts-96-24-5.1", "bluray-primary", "dts-96-24", "dts")
        },
        Case {
            bitrate: Some(3018),
            ..c("dts-hra-5.1", "bluray-primary", "dtshd-hra", "dtshd")
        },
        Case {
            bitrate: Some(3018),
            rate: 96_000,
            ..c("dts-hra-96k-5.1", "bluray-primary", "dtshd-hra", "dtshd")
        },
        c("dts-ma-5.1-best", "bluray-primary", "dtshd-ma", "dtshd"),
        Case {
            speed: "balanced",
            ..c("dts-ma-5.1-balanced", "bluray-primary", "dtshd-ma", "dtshd")
        },
        Case {
            speed: "fast",
            ..c("dts-ma-5.1-fast", "bluray-primary", "dtshd-ma", "dtshd")
        },
        Case {
            rate: 96_000,
            ..c("dts-ma-96k-5.1", "bluray-primary", "dtshd-ma", "dtshd")
        },
        Case {
            layout: "7.1-side-rear",
            ..c("dts-ma-7.1", "bluray-primary", "dtshd-ma", "dtshd")
        },
        Case {
            layout: "6.1-discrete",
            ..c("dts-ma-6.1", "bluray-primary", "dtshd-ma", "dtshd")
        },
        Case {
            layout: "7.1-side-rear",
            bitrate: Some(5376),
            ..c("dts-hra-7.1", "bluray-primary", "dtshd-hra", "dtshd")
        },
        Case {
            bitrate: Some(0),
            ..c("dts-ma-5.1-lossless-only", "bluray-primary", "dtshd-ma", "dtshd")
        },
        Case {
            bitrate: Some(256),
            ..c("dts-express-5.1", "bluray-secondary", "dts-express", "dtshd")
        },
        c("truehd-5.1-best", "generic", "truehd", "thd"),
        Case {
            speed: "balanced",
            ..c("truehd-5.1-balanced", "generic", "truehd", "thd")
        },
        Case {
            speed: "fast",
            ..c("truehd-5.1-fast", "generic", "truehd", "thd")
        },
        Case {
            layout: "7.1-side-rear",
            ..c("truehd-7.1", "generic", "truehd", "thd")
        },
        Case {
            layout: "2.0",
            ..c("truehd-2.0", "generic", "truehd", "thd")
        },
        Case {
            rate: 96_000,
            ..c("truehd-5.1-96k", "generic", "truehd", "thd")
        },
        c("mlp-5.1", "generic", "mlp", "thd"),
        c("flac-best", "files", "flac", "flac"),
        Case {
            speed: "balanced",
            ..c("flac-balanced", "files", "flac", "flac")
        },
        Case {
            speed: "fast",
            ..c("flac-fast", "files", "flac", "flac")
        },
        Case {
            layout: "2.0",
            pcm_format: "s16",
            ..c("flac-2.0-16bit", "files", "flac", "flac")
        },
        c("oggflac", "files", "oggflac", "oga"),
        c("alac-best", "files", "alac", "m4a"),
        Case {
            speed: "fast",
            ..c("alac-fast", "files", "alac", "m4a")
        },
        c("als-best", "files", "als", "als"),
        Case {
            speed: "balanced",
            ..c("als-balanced", "files", "als", "als")
        },
        Case {
            speed: "fast",
            ..c("als-fast", "files", "als", "als")
        },
        c("wavpack", "files", "wavpack", "wv"),
        Case {
            layout: "2.0",
            ..c("monkeys-2.0", "files", "ape", "ape")
        },
        c("tta", "files", "tta", "tta"),
        c("wav-s24", "files", "wav", "wav"),
        Case {
            pcm_format: "s16",
            ..c("wav-s16", "files", "wav", "wav")
        },
        Case {
            pcm_format: "f32",
            ..c("wav-f32", "files", "wav", "wav")
        },
        c("aiff", "files", "aiff", "aif"),
        c("caf", "files", "caf", "caf"),
        Case {
            pcm_format: "24",
            ..c("dcp-audio", "files", "dcp-audio", "mxf")
        },
        Case {
            layout: "2.0",
            pcm_format: "dsd64",
            ..c("dsf-2.0", "files", "dsf", "dsf")
        },
    ]
}

fn channels_for(layout: &str) -> &'static [&'static str] {
    match layout {
        "2.0" => &STEREO,
        "3.0" => &THREE_ZERO,
        "6.1-discrete" | "6.1-matrix" => &SIX_ONE,
        "7.1-side-rear" => &SEVEN_ONE,
        _ => &FIVE_ONE,
    }
}

/// A digest of the file. A DCP track file carries fresh random identifiers every time, and the DSD modulator's
/// result depends on the processor's floating-point library (it differs between this machine and CI), so for those
/// only the length is compared.
fn checksum(bytes: &[u8], ext: &str) -> String {
    if matches!(ext, "mxf" | "dsf") {
        return format!("{} bytes", bytes.len());
    }
    format!("{:x}", md5::compute(bytes))
}

/// A folder for the stems and outputs. `SW_GOLDEN_DIR` names one to keep (to look at the files afterwards).
fn scratch() -> PathBuf {
    if let Some(dir) = std::env::var_os("SW_GOLDEN_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        return PathBuf::from(dir);
    }
    let dir = std::env::temp_dir().join(format!("sw_golden_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(case: &Case, dir: &Path) -> String {
    let channels = channels_for(case.layout);
    let output = dir.join(format!("{}.{}", case.name, case.ext));
    let spec = EncodeSpec {
        dest: case.dest.into(),
        stream: case.stream.into(),
        layout: case.layout.into(),
        sample_rate: case.rate,
        bitrate: case.bitrate,
        dialnorm: Some(27),
        start: None,
        duration: None,
        inputs: stems(dir, case.rate, channels),
        output: output.to_string_lossy().into_owned(),
        unlocked: false,
        engine: "native".into(),
        downmix: true,
        channel_order: String::new(),
        es_phase_shift: false,
        rear_minus_3db: false,
        source_layout: None,
        core_x96: false,
        pcm_format: case.pcm_format.into(),
        bitstream: Default::default(),
        speed: case.speed.into(),
        threads: 0,
        write_log: true,
    };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    if let Err(e) = execute(&spec, false, &cancel, &mut |_, _| {}) {
        let message = match e {
            sw_queue::run::Failure::Error(m) => m,
            sw_queue::run::Failure::Canceled => "canceled".into(),
        };
        panic!("{}: {message}", case.name);
    }
    checksum(&std::fs::read(&output).unwrap(), case.ext)
}

/// The checksum of every case's output when it was last checked.
#[rustfmt::skip]
const GOLDEN: &[(&str, &str)] = &[
    ("ac3-5.1", "5354a92ac24fc89831c1469a3d819fd5"),
    ("ac3-2.0", "5e7bb29a2d6fbcdd122e86aeb21eb217"),
    ("ac3-5.1-192", "9083c2f4246ef77aa25b270acead08b4"),
    ("eac3-2.0", "8e4898bc59c2fa5e663cd5fb1d4af1ed"),
    ("eac3-5.1", "1d11c3541a47d6183f2321e3b9b6899b"),
    ("eac3-7.1", "716097299fd2bce5ebed60b1024a1fd1"),
    ("dts-core-5.1", "675941fe4e09d4e3d282928b1741ea17"),
    ("dts-core-3.0", "8f2ad5f2fd1ff92bb6de545d22411747"),
    ("dts-core-dvd", "b230e245863bf6503135a6306324d8bd"),
    ("dts-music-disc", "902aacec8c61cccbf00c65048a6a1256"),
    ("dts-es-6.1-matrix", "d2657132ade0d5eee0209bac1da66514"),
    ("dts-es-6.1-discrete", "bed43ab80e0c4e1ba61d5300125c3460"),
    ("dts-96-24-5.1", "30cb866a697d1063482838ffc91ea34e"),
    ("dts-hra-5.1", "bc3d8ac811d972b4b2716057fe48236e"),
    ("dts-hra-96k-5.1", "94fe40ca33f05026d4c2d9fa6b69db9e"),
    ("dts-ma-5.1-best", "ef7cdcc2be694c909dee68a6b0c07309"),
    ("dts-ma-5.1-balanced", "1f746b56aaa1d060f1810379bf5f4e72"),
    ("dts-ma-5.1-fast", "c63299f84d2c3f44821fe8f042af4547"),
    ("dts-ma-96k-5.1", "40ef04e09fa59f23d75584378308f21f"),
    ("dts-ma-7.1", "d5c2feee70738f6a809d2ab03ba76082"),
    ("dts-ma-6.1", "47a7517b768c5f19fcadb700de15c16c"),
    ("dts-hra-7.1", "5f6d20f5ac50464c1c4c3caa2fd219f3"),
    ("dts-ma-5.1-lossless-only", "233de0fac694e43c690998ecf906eecc"),
    ("dts-express-5.1", "c51aa36b075b90fda41d740641c97f28"),
    ("truehd-5.1-best", "ea75ccec9f958448344189f1d2377bb4"),
    ("truehd-5.1-balanced", "ea75ccec9f958448344189f1d2377bb4"),
    ("truehd-5.1-fast", "6ef2871a27b604c881a656c3b5538e28"),
    ("truehd-7.1", "c962da84081f29bbcd91f400b79029c9"),
    ("truehd-2.0", "f7e9297b7213eec40bebe7ad71a1fa30"),
    ("truehd-5.1-96k", "4835e293266e4565563460f57bfa6729"),
    ("mlp-5.1", "cb6861dde060330922815f921307947a"),
    ("flac-best", "ccd07c9e0122698513af94243208031d"),
    ("flac-balanced", "ccd07c9e0122698513af94243208031d"),
    ("flac-fast", "1ca95c86f0e7c146cd9a86d03b243821"),
    ("flac-2.0-16bit", "2a3b49522f9216e83821b1030c9df4a7"),
    ("oggflac", "32fc64ab7f996c094b66e29ce9d87671"),
    ("alac-best", "4ed0281a16a5c6f2b683b9a7c94d7dc3"),
    ("alac-fast", "339163718cb11cc3fe09edc7ed62d2a0"),
    ("als-best", "07e314952fb3dd02ba02d666ac07a957"),
    ("als-balanced", "250299f00176dd9241be5febd550a20d"),
    ("als-fast", "4fe97f21a6c66fa4a829079a88d0a1a9"),
    ("wavpack", "2829ab7a72ae656a4e0cf4b2b026dfb2"),
    ("monkeys-2.0", "9067b73d5a8cf6901657bc6f831266d6"),
    ("tta", "093e4149ecdea34a3f042d9a4b54dee2"),
    ("wav-s24", "ea2db69f4104d9237a1893036d1c6562"),
    ("wav-s16", "9224b2e4b9e36ee8f2a92741a676de5c"),
    ("wav-f32", "bd5872efd28dc17ec7a47df5c34f8b61"),
    ("aiff", "70f93e1222802ce731c393bc2f542b13"),
    ("caf", "a405b3219e09d3a2036cdab64bbca21b"),
    ("dcp-audio", "2610286 bytes"),
    ("dsf-2.0", "2121820 bytes"),
];

#[test]
fn every_format_makes_the_same_bytes_as_before() {
    let dir = scratch();
    let results: Vec<(&str, String)> = cases().iter().map(|c| (c.name, run(c, &dir))).collect();
    if std::env::var_os("SW_UPDATE_GOLDEN").is_some() {
        for (name, sum) in &results {
            println!("    (\"{name}\", \"{sum}\"),");
        }
        return;
    }
    let mut changed = Vec::new();
    for (name, sum) in &results {
        match GOLDEN.iter().find(|(n, _)| n == name) {
            Some((_, expected)) if expected == sum => {}
            Some((_, expected)) => changed.push(format!("{name}: expected {expected}, made {sum}")),
            None => changed.push(format!("{name}: no recorded checksum (made {sum})")),
        }
    }
    if std::env::var_os("SW_GOLDEN_DIR").is_none() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    assert!(changed.is_empty(), "output changed:\n{}", changed.join("\n"));
}
