//! Core plus lossless extension: the decoder must reproduce the source exactly.
use std::process::Command;
use sw_codec_dts::ma::map_layout;
use sw_codec_dts::ma_core::{MaCoreConfig, MaCoreEncoder};
use sw_core::ChannelLayout;

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn signal(c: usize, n: usize, seed: &mut u32) -> Vec<i32> {
    (0..n)
        .map(|i| {
            *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let noise = ((*seed >> 12) as i32 & 0xFFF) - 2048;
            let t = i as f64 / 48_000.0;
            ((2.0 * std::f64::consts::PI * (200.0 + 130.0 * c as f64) * t).sin() * 2_000_000.0 + (2.0 * std::f64::consts::PI * 3100.0 * t).sin() * 300_000.0) as i32 + noise * 4
        })
        .collect()
}

fn core_roundtrip(name: &str, layout_id: &str, rate: u32, frames: usize, bit_res: u32) {
    if !have_ffmpeg() {
        return;
    }
    let layout = ChannelLayout::find(layout_id).unwrap();
    let map = map_layout(layout);
    let mut enc = MaCoreEncoder::new(MaCoreConfig { layout, sample_rate: rate, bit_res, core_bitrate: 1_509_000, dialnorm: 0 }).unwrap();
    let n = enc.frame_samples();
    let nch = map.order.len();
    let mut seed = 7u32;
    let shift = 24 - bit_res;
    let sigs: Vec<Vec<i32>> = (0..nch).map(|c| signal(c, n * frames, &mut seed).into_iter().map(|v| v >> shift).collect()).collect();
    let mut stream = Vec::new();
    for f in 0..frames {
        let part: Vec<Vec<i32>> = sigs.iter().map(|s| s[f * n..(f + 1) * n].to_vec()).collect();
        for fr in enc.push(&part) {
            stream.extend(fr);
        }
    }
    for fr in enc.finish() {
        stream.extend(fr);
    }
    let dir = std::env::temp_dir().join(format!("sw_dts_macore_{}_{}", std::process::id(), name));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.dts");
    std::fs::write(&path, &stream).unwrap();
    let out = Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "warning", "-i"]).arg(&path).args(["-c:a", "pcm_s24le", "-f", "s24le", "-"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.stderr.is_empty(), "{name}: ffmpeg complained: {}", String::from_utf8_lossy(&out.stderr));
    let dec: Vec<i32> = out.stdout.chunks_exact(3).map(|b| i32::from_le_bytes([b[0], b[1], b[2], 0]) << 8 >> 8).collect();
    assert!(dec.len() >= n * frames * nch, "{name}: decoded length {} < {}", dec.len(), n * frames * nch);
    let mut bad = 0usize;
    for (j, &lc) in map.order.iter().enumerate() {
        for i in 0..n * frames {
            if (dec[i * nch + lc] >> shift) != sigs[j][i] {
                bad += 1;
                if bad < 5 {
                    eprintln!("{name}: channel {lc} differs at sample {i}: got {} want {}", dec[i * nch + lc], sigs[j][i]);
                }
            }
        }
    }
    assert_eq!(bad, 0, "{name}: {bad} samples differ");
}

#[test]
fn core_plus_xll_five_one_48k() {
    core_roundtrip("c51", "5.1", 48_000, 24, 24);
}
