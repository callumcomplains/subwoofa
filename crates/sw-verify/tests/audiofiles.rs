//! Verify must accept every audio file Subwoofa writes, and notice when one is cut short.
use std::collections::HashMap;
use sw_codec_pcm::{encode_to_file, Container, Request, SampleFormat};
use sw_core::ChannelLayout;
use sw_verify::{verify, Options};

/// Writes a short stereo 24-bit WAV tone to use as the source.
fn write_source(path: &std::path::Path, rate: u32, seconds: u32, freq: f64) {
    let mut data = Vec::new();
    for i in 0..rate * seconds {
        let v = ((2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64).sin() * 3_000_000.0) as i32;
        data.extend_from_slice(&v.to_le_bytes()[..3]);
    }
    let mut f = Vec::new();
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

fn make(dir: &std::path::Path, container: Container, ext: &str, multiplier: u32) -> std::path::PathBuf {
    let layout = ChannelLayout::find("2.0").unwrap();
    let mut inputs = HashMap::new();
    for (i, ch) in layout.channels.iter().enumerate() {
        let p = dir.join(format!("{}.wav", ch.short()));
        write_source(&p, 48_000, 2, 440.0 + 100.0 * i as f64);
        inputs.insert(*ch, p);
    }
    let mut stems = sw_pcm::StemSet::open(layout, &inputs).unwrap();
    let out = dir.join(format!("out.{ext}"));
    let req = Request { layout, container, format: SampleFormat::Int24, start_secs: 0.0, duration_secs: None, output: &out, dsd_multiplier: multiplier };
    encode_to_file(&mut stems, &req, &mut |_| {}, &std::sync::atomic::AtomicBool::new(false)).unwrap();
    out
}

const CASES: &[(Container, &str, u32)] = &[
    (Container::Wav, "wav", 0),
    (Container::Aiff, "aif", 0),
    (Container::Caf, "caf", 0),
    (Container::Flac, "flac", 0),
    (Container::OggFlac, "oga", 0),
    (Container::Alac, "m4a", 0),
    (Container::Tta, "tta", 0),
    (Container::Dsf, "dsf", 64),
    (Container::Dff, "dff", 64),
];

#[test]
fn every_audio_file_passes_the_structure_check() {
    let dir = std::env::temp_dir().join(format!("sw_verify_files_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for &(container, ext, multiplier) in CASES {
        let path = make(&dir, container, ext, multiplier);
        let report = verify(&path, &Options::default()).unwrap();
        assert!(report.passed, "{ext}: {:?}", report.checks.iter().filter(|c| c.status == sw_verify::Status::Fail).collect::<Vec<_>>());
        assert!(!report.format.starts_with("unknown"), "{ext}: format {}", report.format);
        assert!(report.duration_secs > 1.9 && report.duration_secs < 2.1, "{ext}: duration {}", report.duration_secs);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_cut_short_fails() {
    let dir = std::env::temp_dir().join(format!("sw_verify_cut_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // formats whose headers state a size or a checksum over the audio
    for &(container, ext, multiplier) in CASES.iter().filter(|c| matches!(c.1, "wav" | "aif" | "caf" | "oga" | "m4a" | "tta" | "dsf" | "dff")) {
        let path = make(&dir, container, ext, multiplier);
        let bytes = std::fs::read(&path).unwrap();
        let cut = dir.join(format!("cut.{ext}"));
        // m4a keeps its tables at the end, so cut inside the audio by removing from the middle instead
        let broken = if ext == "m4a" { [&bytes[..bytes.len() / 2], &bytes[bytes.len() - 100..]].concat() } else { bytes[..bytes.len() / 2].to_vec() };
        std::fs::write(&cut, broken).unwrap();
        let report = verify(&cut, &Options::default()).unwrap();
        assert!(!report.passed, "{ext}: a damaged file passed");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
