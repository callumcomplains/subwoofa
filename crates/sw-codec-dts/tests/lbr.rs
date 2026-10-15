//! DTS Express: FFmpeg must decode what the encoder writes, and the audio must resemble the input.
use std::process::Command;
use sw_codec_dts::container::exss::{dtshd_header, exss_frame_lbr, DtsHdHeader, ExssParams};
use sw_codec_dts::express::encoder::{LbrConfig, LbrEncoder};

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn mono_stream_decodes() {
    if !have_ffmpeg() {
        return;
    }
    let bitrate = 192_000;
    let mut enc = LbrEncoder::new(LbrConfig {
        sample_rate: 48_000,
        channels: 1,
        speaker_mask: 1,
        lfe: false,
        bitrate,
        dialog_mode: false,
        payload_bytes: None,
    })
    .unwrap();
    let frames = 12;
    let total = 4096 * frames;
    let mut seed = 3u32;
    let x: Vec<f32> = (0..total)
        .map(|i| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let noise = (seed >> 8) as f32 / 8_388_608.0 - 1.0;
            let t = i as f32 / 48_000.0;
            0.3 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
                + 0.1 * (2.0 * std::f32::consts::PI * 3300.0 * t).sin()
                + 0.02 * noise
        })
        .collect();
    let mut payloads = Vec::new();
    for chunk in x.chunks(4096) {
        payloads.extend(enc.push(&[chunk.to_vec()]));
    }
    payloads.extend(enc.finish());
    assert!(payloads.len() >= frames, "{} frames", payloads.len());
    let exss = ExssParams {
        sample_rate: 48_000,
        channels: 1,
        bit_res: 16,
        spk_mask: 1,
        with_core: false,
        dialnorm: 0,
        frame_duration_code: 7,
        x96_in_core: false,
        xch_in_core: false,
        secondary: false,
        extras: Default::default(),
    };
    let mut stream = Vec::new();
    for p in &payloads {
        stream.extend(exss_frame_lbr(&exss, p));
    }
    let mut file = dtshd_header(
        &DtsHdHeader {
            sample_rate: 48_000,
            frames: payloads.len() as u32,
            samples_per_frame: 4096,
            total_samples: total as u64,
            ch_mask: 1,
            initial_padding: 0,
        },
        stream.len() as u64,
    );
    file.extend_from_slice(&stream);
    let dir = std::env::temp_dir().join(format!("sw_dts_lbr_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.dtshd");
    std::fs::write(&path, &file).unwrap();
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "warning", "-i"])
        .arg(&path)
        .args(["-f", "f32le", "-"])
        .output()
        .unwrap();
    println!("ffmpeg stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success());
    let y: Vec<f32> = out
        .stdout
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    println!("decoded {} samples (expected at least {})", y.len(), total);
    assert!(y.len() >= total);
    // signal to noise against the input (the decoder output is aligned with the input)
    let (mut s, mut e) = (0.0f64, 0.0f64);
    for i in 8192..total - 8192 {
        s += (x[i] as f64).powi(2);
        e += (y[i] as f64 - x[i] as f64).powi(2);
    }
    let db = 10.0 * (s / e).log10();
    println!("SNR {db:.1} dB");
    assert!(db > 10.0, "SNR {db}");
}

/// The LFE comes out in the right channel at the right level, with the narrow (below 256 kbps) and the wide coding.
fn check_lfe(bitrate: u32) {
    if !have_ffmpeg() {
        return;
    }
    let frames = 14;
    let total = 4096 * frames;
    let mut enc = LbrEncoder::new(LbrConfig {
        sample_rate: 48_000,
        channels: 5,
        speaker_mask: 0xF,
        lfe: true,
        bitrate,
        dialog_mode: false,
        payload_bytes: None,
    })
    .unwrap();
    let tone = |f: f32, a: f32, i: usize| a * (2.0 * std::f32::consts::PI * f * i as f32 / 48_000.0).sin();
    let chans: Vec<Vec<f32>> = (0..6)
        .map(|c| {
            (0..total)
                .map(|i| {
                    if c == 5 {
                        tone(60.0, 0.5, i)
                    } else if c == 4 {
                        tone(440.0, 0.2, i)
                    } else {
                        0.0
                    }
                })
                .collect()
        })
        .collect();
    let mut payloads = Vec::new();
    for s in (0..total).step_by(4096) {
        let block: Vec<Vec<f32>> = chans.iter().map(|c| c[s..s + 4096].to_vec()).collect();
        payloads.extend(enc.push(&block));
    }
    payloads.extend(enc.finish());
    let exss = ExssParams {
        sample_rate: 48_000,
        channels: 6,
        bit_res: 16,
        spk_mask: 0xF,
        with_core: false,
        dialnorm: 0,
        frame_duration_code: 7,
        x96_in_core: false,
        xch_in_core: false,
        secondary: false,
        extras: Default::default(),
    };
    let mut stream = Vec::new();
    for p in &payloads {
        stream.extend(exss_frame_lbr(&exss, p));
    }
    let mut file = dtshd_header(
        &DtsHdHeader {
            sample_rate: 48_000,
            frames: payloads.len() as u32,
            samples_per_frame: 4096,
            total_samples: total as u64,
            ch_mask: 0xF,
            initial_padding: 0,
        },
        stream.len() as u64,
    );
    file.extend_from_slice(&stream);
    let dir = std::env::temp_dir().join(format!("sw_dts_lbr_lfe_{bitrate}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.dtshd");
    std::fs::write(&path, &file).unwrap();
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "warning", "-i"])
        .arg(&path)
        .args(["-f", "f32le", "-"])
        .output()
        .unwrap();
    println!("ffmpeg stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success());
    let y: Vec<f32> = out
        .stdout
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    let n = y.len() / 6;
    println!("decoded {n} samples per channel");
    // amplitude of 60 Hz in each channel
    let mut best = (0usize, 0.0f64);
    for c in 0..6 {
        let (mut sc, mut ss) = (0.0f64, 0.0f64);
        for i in 16384..n - 16384 {
            let a = 2.0 * std::f64::consts::PI * 60.0 * i as f64 / 48_000.0;
            sc += y[i * 6 + c] as f64 * a.cos();
            ss += y[i * 6 + c] as f64 * a.sin();
        }
        let amp = 2.0 * (sc * sc + ss * ss).sqrt() / (n - 32768) as f64;
        println!("channel {c}: 60 Hz amplitude {amp:.3}");
        if amp > best.1 {
            best = (c, amp);
        }
    }
    assert_eq!(best.0, 3, "LFE should be output channel 3");
    assert!((best.1 - 0.5).abs() < 0.1, "LFE amplitude {}", best.1);
}

#[test]
fn lfe_channel_decodes_narrow() {
    check_lfe(192_000);
}

#[test]
fn lfe_channel_decodes_wide() {
    check_lfe(384_000);
}

/// Steady notes are sent as tonal components: at a low rate a signal of three sines reaches a much higher
/// SNR than the spectrum coding alone gives, and the decoder reads every tonal chunk cleanly.
#[test]
fn tonal_components_lift_a_steady_signal() {
    if !have_ffmpeg() {
        return;
    }
    let frames = 14;
    let total = 4096 * frames;
    let mut enc = LbrEncoder::new(LbrConfig {
        sample_rate: 48_000,
        channels: 1,
        speaker_mask: 1,
        lfe: false,
        bitrate: 48_000,
        dialog_mode: false,
        payload_bytes: None,
    })
    .unwrap();
    let mut seed = 7u32;
    let x: Vec<f32> = (0..total)
        .map(|i| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let noise = (seed >> 8) as f32 / 8_388_608.0 - 1.0;
            let t = i as f32 / 48_000.0;
            let tau = 2.0 * std::f32::consts::PI;
            0.24 * (tau * 1000.0 * t).sin()
                + 0.12 * (tau * 3010.0 * t + 1.0).sin()
                + 0.07 * (tau * 440.0 * t + 2.0).sin()
                + 0.002 * noise
        })
        .collect();
    let mut payloads = Vec::new();
    for chunk in x.chunks(4096) {
        payloads.extend(enc.push(&[chunk.to_vec()]));
    }
    payloads.extend(enc.finish());
    let exss = ExssParams {
        sample_rate: 48_000,
        channels: 1,
        bit_res: 16,
        spk_mask: 1,
        with_core: false,
        dialnorm: 0,
        frame_duration_code: 7,
        x96_in_core: false,
        xch_in_core: false,
        secondary: false,
        extras: Default::default(),
    };
    let mut stream = Vec::new();
    for p in &payloads {
        stream.extend(exss_frame_lbr(&exss, p));
    }
    let mut file = dtshd_header(
        &DtsHdHeader {
            sample_rate: 48_000,
            frames: payloads.len() as u32,
            samples_per_frame: 4096,
            total_samples: total as u64,
            ch_mask: 1,
            initial_padding: 0,
        },
        stream.len() as u64,
    );
    file.extend_from_slice(&stream);
    let dir = std::env::temp_dir().join(format!("sw_dts_lbr_tone_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.dtshd");
    std::fs::write(&path, &file).unwrap();
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "warning", "-i"])
        .arg(&path)
        .args(["-f", "f32le", "-"])
        .output()
        .unwrap();
    assert!(
        out.status.success() && out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let y: Vec<f32> = out
        .stdout
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    let (mut s, mut e) = (0.0f64, 0.0f64);
    for i in 16384..total - 16384 {
        s += (x[i] as f64).powi(2);
        e += (y[i] as f64 - x[i] as f64).powi(2);
    }
    let db = 10.0 * (s / e).log10();
    println!("SNR {db:.1} dB");
    assert!(db > 30.0, "SNR {db}");
}
