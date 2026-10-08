//! DTS Express: FFmpeg must decode what the encoder writes, and the audio must resemble the input.
use std::process::Command;
use sw_codec_dts::exss::{dtshd_header, exss_frame_lbr, DtsHdHeader, ExssParams};
use sw_codec_dts::lbr::{LbrConfig, LbrEncoder};

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
fn mono_stream_decodes() {
    if !have_ffmpeg() {
        return;
    }
    let bitrate = 192_000;
    let mut enc = LbrEncoder::new(LbrConfig { sample_rate: 48_000, channels: 1, speaker_mask: 1, lfe: false, bitrate }).unwrap();
    let frames = 12;
    let total = 4096 * frames;
    let mut seed = 3u32;
    let x: Vec<f32> = (0..total)
        .map(|i| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let noise = (seed >> 8) as f32 / 8_388_608.0 - 1.0;
            let t = i as f32 / 48_000.0;
            0.3 * (2.0 * std::f32::consts::PI * 440.0 * t).sin() + 0.1 * (2.0 * std::f32::consts::PI * 3300.0 * t).sin() + 0.02 * noise
        })
        .collect();
    let mut payloads = Vec::new();
    for chunk in x.chunks(4096) {
        payloads.extend(enc.push(&[chunk.to_vec()]));
    }
    payloads.extend(enc.finish());
    assert!(payloads.len() >= frames, "{} frames", payloads.len());
    let exss = ExssParams { sample_rate: 48_000, channels: 1, bit_res: 16, spk_mask: 1, with_core: false, dialnorm: 0, frame_duration_code: 7 };
    let mut stream = Vec::new();
    for p in &payloads {
        stream.extend(exss_frame_lbr(&exss, p));
    }
    let mut file = dtshd_header(&DtsHdHeader { sample_rate: 48_000, frames: payloads.len() as u32, samples_per_frame: 4096, total_samples: total as u64, ch_mask: 1, initial_padding: 0 }, stream.len() as u64);
    file.extend_from_slice(&stream);
    let dir = std::env::temp_dir().join(format!("sw_dts_lbr_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.dtshd");
    std::fs::write(&path, &file).unwrap();
    let out = Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "warning", "-i"]).arg(&path).args(["-f", "f32le", "-"]).output().unwrap();
    println!("ffmpeg stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success());
    let y: Vec<f32> = out.stdout.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
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
