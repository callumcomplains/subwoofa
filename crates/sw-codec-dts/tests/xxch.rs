//! DTS-HD HRA 7.1: a 5.1 core plus an XXCH extension for the rear pair. A reference decoder must return every
//! channel, including the rears and the sides whose core copy carries the embedded downmix.
use std::process::Command;
use sw_codec_dts::dca::native::{encode_to_file, Request};
use sw_core::ChannelLayout;

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn write_wav(path: &std::path::Path, freq: f32, n: usize) {
    write_wav_rate(path, freq, n, 48_000)
}

fn write_wav_rate(path: &std::path::Path, freq: f32, n: usize, rate: u32) {
    let mut data = Vec::with_capacity(n * 3);
    for k in 0..n {
        let v = (0.3 * (2.0 * std::f32::consts::PI * freq * k as f32 / rate as f32).sin() * 8_388_607.0) as i32;
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

#[test]
fn hra_seven_one_returns_every_channel() {
    if !have_ffmpeg() {
        return;
    }
    let layout = ChannelLayout::find("7.1-ls-rs-lsr-rsr").unwrap(); // L R C LFE Ls Rs Lsr Rsr
    let dir = std::env::temp_dir().join(format!("sw_xxch_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let n = 48_000 * 3;
    let freqs = [330.0f32, 440.0, 550.0, 60.0, 660.0, 770.0, 880.0, 990.0];
    let mut inputs = std::collections::HashMap::new();
    for (i, ch) in layout.channels.iter().enumerate() {
        let path = dir.join(format!("{}.wav", ch.short()));
        write_wav(&path, freqs[i], n);
        inputs.insert(*ch, path);
    }
    let mut stems = sw_pcm::StemSet::open(layout, &inputs).unwrap();
    let out = dir.join("t.dtshd");
    let req = Request {
        layout,
        bitrate_kbps: 1509,
        dialnorm: 0,
        pcm_bits: 24,
        start_secs: 0.0,
        duration_secs: None,
        output: &out,
        es: None,
        hra_kbps: Some(2046),
        x96: false,
        bitstream: Default::default(),
        dvd: false,
    };
    encode_to_file(
        &mut stems,
        &req,
        &mut |_| {},
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    let dec = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(&out)
        .args(["-f", "f32le", "-"])
        .output()
        .unwrap();
    assert!(
        dec.status.success() && dec.stderr.is_empty(),
        "decoder: {}",
        String::from_utf8_lossy(&dec.stderr)
    );
    let y: Vec<f32> = dec
        .stdout
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    let frames = y.len() / 8;
    assert!(frames >= n);
    // FFmpeg's 7.1 order: FL FR FC LFE BL BR SL SR; our source order is L R C LFE Ls Rs Lsr Rsr
    let expected_out = [0usize, 1, 2, 3, 6, 7, 4, 5];
    for (src, &outc) in expected_out.iter().enumerate() {
        let amp = |c: usize| -> f64 {
            let (mut sc, mut ss) = (0.0f64, 0.0f64);
            for i in 20_000..frames - 20_000 {
                let a = 2.0 * std::f64::consts::PI * freqs[src] as f64 * i as f64 / 48_000.0;
                sc += y[i * 8 + c] as f64 * a.cos();
                ss += y[i * 8 + c] as f64 * a.sin();
            }
            2.0 * (sc * sc + ss * ss).sqrt() / (frames - 40_000) as f64
        };
        let here = amp(outc);
        let elsewhere = (0..8).filter(|&c| c != outc).map(amp).fold(0.0, f64::max);
        // the LFE passes through a 120 Hz low-pass, so its 60 Hz tone is allowed a little loss
        let tol = if src == 3 { 0.1 } else { 0.03 };
        assert!(
            (here - 0.3).abs() < tol,
            "source {src} came out at {here:.3} in channel {outc}"
        );
        assert!(
            elsewhere < 0.02,
            "source {src} leaks {elsewhere:.3} into another channel"
        );
    }
}

/// The same at 96 kHz (core + XXCH + X96 in the extension substream): every channel comes back, the rears and the
/// sides included, and tones above the core's 24 kHz survive in the core channels and in the XXCH pair.
#[test]
fn hra_seven_one_at_96k_returns_every_channel() {
    if !have_ffmpeg() {
        return;
    }
    let layout = ChannelLayout::find("7.1-ls-rs-lsr-rsr").unwrap(); // L R C LFE Ls Rs Lsr Rsr
    let dir = std::env::temp_dir().join(format!("sw_xxch96_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rate = 96_000u32;
    let n = rate as usize * 3;
    // L carries a 31 kHz tone, Rsr a 29 kHz tone: both live in X96 only
    let freqs = [31_000.0f32, 440.0, 550.0, 60.0, 660.0, 770.0, 880.0, 29_000.0];
    let mut inputs = std::collections::HashMap::new();
    for (i, ch) in layout.channels.iter().enumerate() {
        let path = dir.join(format!("{}.wav", ch.short()));
        write_wav_rate(&path, freqs[i], n, rate);
        inputs.insert(*ch, path);
    }
    let mut stems = sw_pcm::StemSet::open(layout, &inputs).unwrap();
    let out = dir.join("t.dtshd");
    let req = Request {
        layout,
        bitrate_kbps: 1509,
        dialnorm: 0,
        pcm_bits: 24,
        start_secs: 0.0,
        duration_secs: None,
        output: &out,
        es: None,
        hra_kbps: Some(5376),
        x96: false,
        bitstream: Default::default(),
        dvd: false,
    };
    encode_to_file(
        &mut stems,
        &req,
        &mut |_| {},
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    let dec = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(&out)
        .args(["-f", "f32le", "-"])
        .output()
        .unwrap();
    assert!(
        dec.status.success() && dec.stderr.is_empty(),
        "decoder: {}",
        String::from_utf8_lossy(&dec.stderr)
    );
    let y: Vec<f32> = dec
        .stdout
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    let frames = y.len() / 8;
    assert!(frames >= n, "decoded {frames} < {n}");
    let expected_out = [0usize, 1, 2, 3, 6, 7, 4, 5];
    for (src, &outc) in expected_out.iter().enumerate() {
        let amp = |c: usize| -> f64 {
            let (mut sc, mut ss) = (0.0f64, 0.0f64);
            for i in 40_000..frames - 40_000 {
                let a = 2.0 * std::f64::consts::PI * freqs[src] as f64 * i as f64 / rate as f64;
                sc += y[i * 8 + c] as f64 * a.cos();
                ss += y[i * 8 + c] as f64 * a.sin();
            }
            2.0 * (sc * sc + ss * ss).sqrt() / (frames - 80_000) as f64
        };
        let here = amp(outc);
        let elsewhere = (0..8).filter(|&c| c != outc).map(amp).fold(0.0, f64::max);
        let tol = if src == 3 { 0.1 } else { 0.05 };
        assert!(
            (here - 0.3).abs() < tol,
            "source {src} came out at {here:.3} in channel {outc}"
        );
        assert!(
            elsewhere < 0.02,
            "source {src} leaks {elsewhere:.3} into another channel"
        );
    }
}
