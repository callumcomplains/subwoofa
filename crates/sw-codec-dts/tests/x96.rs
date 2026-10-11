//! DTS 96/24: the extension must carry what is above the core's 24 kHz.
use std::process::Command;
use sw_codec_dts::dts96::{Dts96Config, Dts96Encoder};
use sw_codec_dts::native::LayoutMap;

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
fn tone_above_24_khz_survives() {
    if !have_ffmpeg() {
        return;
    }
    let map = LayoutMap { amode: 2, order: vec![0, 1], lfe: None };
    let mut enc = Dts96Encoder::new(Dts96Config { map, bitrate: 1_509_000, dialnorm: 0, pcm_bits: 24, x96_bytes: 400, core_rate: 48_000, blocks: 16, es: false, aux: None }).unwrap();
    let n = enc.frame_samples();
    let frames = 60;
    let total = n * frames;
    let sig: Vec<Vec<f64>> = (0..2)
        .map(|c| {
            (0..total)
                .map(|i| {
                    let t = i as f64 / 96_000.0;
                    (2.0 * std::f64::consts::PI * (2_000.0 + 500.0 * c as f64) * t).sin() * 1_000_000.0 + (2.0 * std::f64::consts::PI * 31_000.0 * t).sin() * 300_000.0
                })
                .collect()
        })
        .collect();
    let mut stream = Vec::new();
    for chunk in 0..frames {
        let part: Vec<Vec<f64>> = sig.iter().map(|s| s[chunk * n..(chunk + 1) * n].to_vec()).collect();
        for f in enc.push(&part) {
            stream.extend(f);
        }
    }
    for f in enc.finish() {
        stream.extend(f);
    }
    let dir = std::env::temp_dir().join(format!("sw_dts_x96_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.dts");
    std::fs::write(&path, &stream).unwrap();
    let out = Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-i"]).arg(&path).args(["-f", "f64le", "-"]).output().unwrap();
    assert!(out.status.success() && out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
    let dec: Vec<f64> = out.stdout.chunks_exact(8).map(|b| f64::from_le_bytes(b.try_into().unwrap()) * 8_388_608.0).collect();
    assert!(dec.len() >= 2 * total, "{} samples", dec.len());
    // project the decoded signal on the 31 kHz tone and compare with the source level
    for c in 0..2 {
        let x: Vec<f64> = dec.iter().skip(c).step_by(2).copied().collect();
        let (mut re, mut im, mut sre, mut sim) = (0.0, 0.0, 0.0, 0.0);
        for i in 8 * n..total - 8 * n {
            let w = 2.0 * std::f64::consts::PI * 31_000.0 * i as f64 / 96_000.0;
            re += x[i] * w.cos();
            im += x[i] * w.sin();
            sre += sig[c][i] * w.cos();
            sim += sig[c][i] * w.sin();
        }
        let got = (re * re + im * im).sqrt();
        let want = (sre * sre + sim * sim).sqrt();
        println!("channel {c}: 31 kHz tone at {:.1}% of the source level", 100.0 * got / want);
        assert!(got > 0.9 * want && got < 1.1 * want, "31 kHz tone {got} vs {want}");
    }
}
