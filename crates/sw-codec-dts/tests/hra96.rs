//! DTS-HD High Resolution at 96 kHz for 5.1 and 6.1 (matrix and discrete): core plus X96 in the extension
//! substream. A reference decoder must accept every frame, including the silent ones at the start (the discrete
//! stream's XCH extension must still be a valid 96 to 1020 byte block at the end of those frames) and return the
//! tone of every channel.
use std::process::Command;
use sw_codec_dts::native::{encode_to_file, Request};
use sw_core::ChannelLayout;

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// A 24-bit mono WAV: silence for `quiet` seconds, then a tone.
fn write_wav(path: &std::path::Path, rate: u32, seconds: f64, quiet: f64, freq: f64) {
    let n = (rate as f64 * seconds) as usize;
    let mut data = Vec::with_capacity(n * 3);
    for k in 0..n {
        let t = k as f64 / rate as f64;
        let v = if t < quiet { 0 } else { (0.3 * (2.0 * std::f64::consts::PI * freq * t).sin() * 8_388_607.0) as i32 };
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

/// Encodes a tone per channel (starting after a silent second), decodes, and checks every tone turns up in some
/// channel at about its level (`expected_channels` decoded channels).
fn check(name: &str, layout_id: &str, expected_channels: usize, cs_is_matrixed: bool) {
    if !have_ffmpeg() {
        return;
    }
    let layout = ChannelLayout::find(layout_id).unwrap();
    let dir = std::env::temp_dir().join(format!("sw_hra96_{}_{}", std::process::id(), name));
    std::fs::create_dir_all(&dir).unwrap();
    let rate = 96_000u32;
    let freqs = [330.0, 440.0, 550.0, 60.0, 770.0, 880.0, 990.0, 1100.0];
    let mut inputs = std::collections::HashMap::new();
    for (i, ch) in layout.channels.iter().enumerate() {
        let path = dir.join(format!("{}.wav", ch.short()));
        write_wav(&path, rate, 4.0, 1.0, freqs[i]);
        inputs.insert(*ch, path);
    }
    let mut stems = sw_pcm::StemSet::open(layout, &inputs).unwrap();
    let out = dir.join("t.dtshd");
    let es = sw_codec_dts::es_options(layout.channels.contains(&sw_core::Channel::Cs), layout, false, false);
    let req = Request { layout, bitrate_kbps: 1509, dialnorm: 0, pcm_bits: 24, start_secs: 0.0, duration_secs: None, output: &out, es, hra_kbps: Some(4224), x96: false, bitstream: Default::default(), dvd: false };
    encode_to_file(&mut stems, &req, &mut |_| {}, &std::sync::atomic::AtomicBool::new(false)).unwrap();
    let dec = Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-i"]).arg(&out).args(["-f", "f32le", "-"]).output().unwrap();
    assert!(dec.status.success() && dec.stderr.is_empty(), "{name}: decoder: {}", String::from_utf8_lossy(&dec.stderr));
    let y: Vec<f32> = dec.stdout.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    let nch = y.len() / 4 / (rate as usize) * 0 + expected_channels;
    let frames = y.len() / nch;
    assert!(frames >= 4 * rate as usize - 100, "{name}: decoded {frames} frames");
    let amp = |c: usize, f: f64| -> f64 {
        let (mut sc, mut ss) = (0.0f64, 0.0f64);
        let (from, to) = (2 * rate as usize, frames - rate as usize / 2);
        for i in from..to {
            let a = 2.0 * std::f64::consts::PI * f * i as f64 / rate as f64;
            sc += y[i * nch + c] as f64 * a.cos();
            ss += y[i * nch + c] as f64 * a.sin();
        }
        2.0 * (sc * sc + ss * ss).sqrt() / (to - from) as f64
    };
    for (i, ch) in layout.channels.iter().enumerate() {
        // a matrixed centre surround has no channel of its own: it is inside the two surrounds
        if cs_is_matrixed && *ch == sw_core::Channel::Cs {
            continue;
        }
        let best = (0..nch).map(|c| amp(c, freqs[i])).fold(0.0, f64::max);
        let tol = if i == 3 { 0.1 } else { 0.04 };
        assert!((best - 0.3).abs() < tol, "{name}: the tone of {} came out at {best:.3}", ch.short());
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn five_one() {
    check("51", "5.1", 6, false);
}

#[test]
fn six_one_matrix() {
    check("61m", "6.1-matrix", 6, true);
}

#[test]
fn six_one_discrete() {
    check("61d", "6.1-discrete", 7, false);
}
