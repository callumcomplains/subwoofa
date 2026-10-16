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

fn spec_for(
    dir: &Path,
    inputs: &HashMap<String, String>,
    stream: &str,
    dest: &str,
    bitrate: Option<u32>,
    ext: &str,
) -> EncodeSpec {
    let output = dir.join(format!("out_{stream}.{ext}"));
    EncodeSpec {
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
        normalize_lufs: None,
        delay_ms: 0,
        write_log: false,
    }
}

fn encode(
    dir: &Path,
    inputs: &HashMap<String, String>,
    stream: &str,
    dest: &str,
    bitrate: Option<u32>,
    ext: &str,
) -> PathBuf {
    let spec = spec_for(dir, inputs, stream, dest, bitrate, ext);
    let cancel = std::sync::atomic::AtomicBool::new(false);
    execute(&spec, false, &cancel, &mut |_, _| {}).unwrap_or_else(|_| panic!("{stream} failed"));
    PathBuf::from(spec.output)
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

/// The samples of a stereo 16-bit WAV written by the preview.
fn read_stereo_wav(path: &Path) -> Vec<(i16, i16)> {
    let bytes = std::fs::read(path).unwrap();
    bytes[44..]
        .chunks_exact(4)
        .map(|f| (i16::from_le_bytes([f[0], f[1]]), i16::from_le_bytes([f[2], f[3]])))
        .collect()
}

#[test]
fn a_preview_lines_the_encode_up_with_the_source() {
    let Some(ffmpeg) = sw_backend_ffmpeg::find_ffmpeg() else {
        eprintln!("FFmpeg not found: skipping");
        return;
    };
    let dir = std::env::temp_dir().join(format!("sw_preview_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut inputs = HashMap::new();
    for (i, ch) in ["L", "R"].iter().enumerate() {
        let path = dir.join(format!("{ch}.wav"));
        write_wav(&path, i as u32 + 1);
        inputs.insert(ch.to_string(), path.to_string_lossy().into_owned());
    }
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let make = |stream: &str, dest: &str, bitrate: Option<u32>, ext: &str| {
        let spec = spec_for(&dir, &inputs, stream, dest, bitrate, ext);
        sw_queue::preview::build(&spec, &ffmpeg, 3.0, &dir.join(stream), &cancel, &mut |_| {})
            .unwrap_or_else(|_| panic!("{stream} preview failed"))
    };

    // lossless: the encode is the source, so the difference is silence
    let flac = make("flac", "files", None, "flac");
    assert!(flac.snr_db.is_none(), "{:?}", flac.snr_db);
    assert!(read_stereo_wav(&flac.difference).iter().all(|&(l, r)| l == 0 && r == 0));
    assert_eq!(read_stereo_wav(&flac.source), read_stereo_wav(&flac.encoded));
    assert!((flac.seconds - 3.0).abs() < 0.1, "{}", flac.seconds);

    // lossy: close to the source but not the same, and lined up (a shifted copy would be far worse than 15 dB)
    let ac3 = make("ac3", "generic", Some(192), "ac3");
    let snr = ac3.snr_db.expect("a lossy encode differs");
    assert!(snr > 15.0 && snr < 90.0, "{snr}");
    assert!(ac3.kbps > 150.0 && ac3.kbps < 230.0, "{}", ac3.kbps);
}

#[test]
fn normalising_brings_the_loudness_to_the_target() {
    let dir = std::env::temp_dir().join(format!("sw_normalise_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut inputs = HashMap::new();
    for (i, ch) in ["L", "R"].iter().enumerate() {
        let path = dir.join(format!("{ch}.wav"));
        write_wav(&path, i as u32 + 1);
        inputs.insert(ch.to_string(), path.to_string_lossy().into_owned());
    }
    let mut spec = spec_for(&dir, &inputs, "wav", "files", None, "wav");
    spec.pcm_format = "s24".into();
    spec.normalize_lufs = Some(-30.0);
    let cancel = std::sync::atomic::AtomicBool::new(false);
    execute(&spec, false, &cancel, &mut |_, _| {}).unwrap_or_else(|_| panic!("normalised encode failed"));

    // the stereo output file, read as left and right
    let out: HashMap<Channel, PathBuf> = [
        (Channel::L, PathBuf::from(&spec.output)),
        (Channel::R, PathBuf::from(&spec.output)),
    ]
    .into();
    let measure = |files: &HashMap<Channel, PathBuf>| {
        let request = sw_queue::measure::Measure {
            layout: ChannelLayout::find("2.0").unwrap(),
            inputs: files,
            order: sw_pcm::ChannelOrder::Auto,
            start: None,
            duration: None,
        };
        sw_queue::measure::loudness(&request, &cancel, &mut |_| {})
            .unwrap()
            .integrated
            .unwrap()
    };
    let loudness = measure(&out);
    assert!((loudness + 30.0).abs() < 0.2, "{loudness} LUFS");
}

/// The 24-bit samples of the data chunk of a stereo WAV: left, right, one pair per frame.
fn read_pairs(path: &Path) -> Vec<(i32, i32)> {
    let bytes = std::fs::read(path).unwrap();
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        if &bytes[at..at + 4] == b"data" {
            let data = &bytes[at + 8..(at + 8 + size).min(bytes.len())];
            let s = |b: &[u8]| i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8;
            return data.chunks_exact(6).map(|f| (s(&f[..3]), s(&f[3..]))).collect();
        }
        at += 8 + size + (size & 1);
    }
    Vec::new()
}

#[test]
fn a_delay_puts_silence_in_front_or_leaves_the_start_out() {
    let dir = std::env::temp_dir().join(format!("sw_delay_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut inputs = HashMap::new();
    for (i, ch) in ["L", "R"].iter().enumerate() {
        let path = dir.join(format!("{ch}.wav"));
        write_wav(&path, i as u32 + 1);
        inputs.insert(ch.to_string(), path.to_string_lossy().into_owned());
    }
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let make = |delay_ms: i32| {
        let mut spec = spec_for(&dir, &inputs, "wav", "files", None, "wav");
        spec.pcm_format = "s24".into();
        spec.delay_ms = delay_ms;
        spec.output = dir.join(format!("delay_{delay_ms}.wav")).to_string_lossy().into_owned();
        execute(&spec, false, &cancel, &mut |_, _| {}).unwrap_or_else(|_| panic!("{delay_ms} ms failed"));
        read_pairs(Path::new(&spec.output))
    };
    let plain = make(0);
    // 100 ms at 48 kHz is 4800 frames of silence, then the same audio
    let later = make(100);
    assert_eq!(later.len(), plain.len() + 4800);
    assert!(later[..4800].iter().all(|&p| p == (0, 0)));
    assert_eq!(&later[4800..], &plain[..]);
    // -50 ms leaves out the first 2400 frames
    let earlier = make(-50);
    assert_eq!(earlier.len(), plain.len() - 2400);
    assert_eq!(&earlier[..], &plain[2400..]);
}

/// Lossless encodes with Normalise's gain or a delay, verified: the check must allow for what was changed.
#[test]
fn verify_after_edits() {
    if sw_backend_ffmpeg::find_ffmpeg().is_none() {
        eprintln!("FFmpeg not found: skipping");
        return;
    }
    let dir = std::env::temp_dir().join(format!("sw_edit_verify_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for layout in ["5.1", "7.1-side-rear"] {
        let channels = ChannelLayout::find(layout).unwrap().channels;
        let mut inputs = HashMap::new();
        for (i, ch) in channels.iter().enumerate() {
            let path = dir.join(format!("{}.wav", ch.short()));
            write_wav(&path, i as u32 + 1);
            inputs.insert(ch.short().to_string(), path.to_string_lossy().into_owned());
        }
        let streams = [("truehd", "generic", "thd"), ("dtshd-ma", "bluray-primary", "dtshd")];
        for (stream, dest, ext) in streams {
            for (normalize, delay_ms) in [(Some(-31.0), 0), (None, 20), (None, -15), (Some(-28.0), 10)] {
                let mut spec = spec_for(&dir, &inputs, stream, dest, None, ext);
                spec.layout = layout.into();
                spec.normalize_lufs = normalize;
                spec.delay_ms = delay_ms;
                let name = format!("edit_{layout}_{stream}_{normalize:?}_{delay_ms}.{ext}");
                spec.output = dir.join(name).to_string_lossy().into_owned();
                let cancel = std::sync::atomic::AtomicBool::new(false);
                let case = format!("{layout} {stream} normalise {normalize:?} delay {delay_ms}");
                match execute(&spec, true, &cancel, &mut |_, _| {}) {
                    Ok(o) => assert!(o.verified, "{case}: not verified: {}", o.message),
                    Err(sw_queue::run::Failure::Error(m)) => panic!("{case}: {m}"),
                    Err(_) => panic!("{case}: canceled"),
                }
            }
        }
    }
}
