//! A finished encode set against the audio it was made from.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use sw_core::{Channel, ChannelLayout};
use sw_queue::run::{execute, EncodeSpec};

/// A mono 24-bit WAV of two tones and a little noise, different for each `seed`.
fn write_wav(path: &Path, seed: u32) {
    let rate = 48_000u32;
    let mut data = Vec::new();
    let mut x = seed.wrapping_mul(2654435761).wrapping_add(1);
    for i in 0..rate as usize * 6 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        let t = i as f64 / rate as f64;
        let v = (t * 440.0 * (1.0 + seed as f64) * std::f64::consts::TAU).sin() * 2_000_000.0
            + (t * 3_100.0 * std::f64::consts::TAU).sin() * 400_000.0
            + ((x >> 8) as i32 - (1 << 23)) as f64 / 90.0;
        data.extend_from_slice(&(v as i32).to_le_bytes()[..3]);
    }
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(b"RIFF").unwrap();
    f.write_all(&((36 + data.len()) as u32).to_le_bytes()).unwrap();
    f.write_all(b"WAVEfmt ").unwrap();
    for v in [16u32, 0x0001_0001, rate, rate * 3, 0x0018_0003] {
        f.write_all(&v.to_le_bytes()).unwrap();
    }
    f.write_all(b"data").unwrap();
    f.write_all(&(data.len() as u32).to_le_bytes()).unwrap();
    f.write_all(&data).unwrap();
}

fn encode(
    dir: &Path,
    inputs: &HashMap<String, String>,
    stream: &str,
    dest: &str,
    bitrate: Option<u32>,
    ext: &str,
) -> PathBuf {
    let output = dir.join(format!("out_{stream}.{ext}"));
    let spec = EncodeSpec {
        dest: dest.into(),
        stream: stream.into(),
        layout: "2.0".into(),
        sample_rate: 48_000,
        bitrate,
        dialnorm: Some(27),
        start: None,
        duration: None,
        inputs: inputs.clone(),
        output: output.to_string_lossy().into_owned(),
        unlocked: false,
        engine: "native".into(),
        downmix: true,
        channel_order: String::new(),
        es_phase_shift: false,
        rear_minus_3db: false,
        source_layout: None,
        core_x96: false,
        pcm_format: String::new(),
        bitstream: Default::default(),
        speed: "fast".into(),
        threads: 0,
        write_log: false,
    };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    execute(&spec, false, &cancel, &mut |_, _| {}).unwrap_or_else(|_| panic!("{stream} failed"));
    output
}

#[test]
fn lossless_comes_back_identical_and_lossy_gets_a_figure() {
    let Some(ffmpeg) = sw_backend_ffmpeg::find_ffmpeg() else {
        eprintln!("FFmpeg not found: skipping");
        return;
    };
    let dir = std::env::temp_dir().join(format!("sw_compare_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut inputs = HashMap::new();
    let mut files: HashMap<Channel, PathBuf> = HashMap::new();
    for (i, ch) in ["L", "R"].iter().enumerate() {
        let path = dir.join(format!("{ch}.wav"));
        write_wav(&path, i as u32 + 1);
        inputs.insert(ch.to_string(), path.to_string_lossy().into_owned());
        files.insert(Channel::from_id(ch).unwrap(), path);
    }
    let layout = ChannelLayout::find("2.0").unwrap();
    let source = sw_verify::Source {
        layout,
        inputs: &files,
        order: sw_pcm::ChannelOrder::Auto,
        start: None,
        duration: None,
    };
    let check = |path: &Path| {
        let mut report = sw_verify::verify(path, &sw_verify::Options::default()).unwrap();
        sw_verify::compare_with_source(&mut report, path, &ffmpeg, &source).unwrap();
        report
    };

    let flac = check(&encode(&dir, &inputs, "flac", "files", None, "flac"));
    let c = flac.comparison.as_ref().unwrap();
    assert!(c.exact && c.lag_samples == 0, "{c:?}");
    assert!(flac
        .checks
        .iter()
        .any(|k| k.name == "Identical to the source" && k.status == sw_verify::Status::Pass));

    let ac3 = check(&encode(&dir, &inputs, "ac3", "generic", Some(192), "ac3"));
    let c = ac3.comparison.as_ref().unwrap();
    assert!(!c.exact && c.lag_samples > 0, "{c:?}");
    // an AC-3 encode at 192 kbps keeps these tones well above the noise
    assert!(c.channels.iter().all(|m| m.snr_db.unwrap() > 15.0), "{c:?}");
    assert!(ac3.checks.iter().any(|k| k.name == "Closeness to the source (lossy)"));
}
