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

/// Where FFmpeg puts a layout channel in its output: channels come in its native (AV) order.
fn out_position(layout: &ChannelLayout, idx: usize) -> usize {
    use sw_core::Channel::*;
    let rank = |c: sw_core::Channel| match c {
        L | Lt => 0,
        R | Rt => 1,
        C => 2,
        Lfe => 3,
        Lsr => 4,
        Rsr => 5,
        Cs => 8,
        Ls | Lss => 9,
        Rs | Rss => 10,
        Oh => 11,
        Lh => 12,
        Ch => 13,
        Rh => 14,
        Lhs => 15,
        Rhs => 16,
        Lw => 18,
        Rw => 19,
        _ => unreachable!(),
    };
    let me = rank(layout.channels[idx]);
    layout.channels.iter().filter(|c| rank(**c) < me).count()
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
        let pos = out_position(layout, lc);
        // a decoder starts multi-set streams on the lossy downmix: the first frame is not exact
        let skip = if map.set_sizes.len() > 1 { n } else { 0 };
        for i in skip..n * frames {
            if (dec[i * nch + pos] >> shift) != sigs[j][i] {
                bad += 1;
                if bad < 5 || i % 2048 == 0 {
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

#[test]
fn core_plus_xll_seven_one_48k() {
    core_roundtrip("c71", "7.1-side-rear", 48_000, 24, 24);
}

/// A 5.1-only decoder (reading just the first channel set) must get the documented downmix:
/// the side channels with the rear channels folded in at -3 dB each.
#[test]
fn seven_one_embedded_five_one_downmix() {
    if !have_ffmpeg() {
        return;
    }
    let layout = ChannelLayout::find("7.1-side-rear").unwrap();
    let map = map_layout(layout);
    let frames = 16;
    let mut enc = MaCoreEncoder::new(MaCoreConfig { layout, sample_rate: 48_000, bit_res: 24, core_bitrate: 1_509_000, dialnorm: 0 }).unwrap();
    let n = enc.frame_samples();
    let mut seed = 5u32;
    let sigs: Vec<Vec<i32>> = (0..8).map(|c| signal(c, n * frames, &mut seed)).collect();
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
    let path = std::env::temp_dir().join(format!("sw_dts_dmix_{}.dts", std::process::id()));
    std::fs::write(&path, &stream).unwrap();
    let out = Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "warning", "-downmix", "5.1(side)", "-i"]).arg(&path).args(["-c:a", "pcm_s24le", "-f", "s24le", "-"]).output().unwrap();
    assert!(out.status.success());
    let dec: Vec<i32> = out.stdout.chunks_exact(3).map(|b| i32::from_le_bytes([b[0], b[1], b[2], 0]) << 8 >> 8).collect();
    assert_eq!(dec.len(), n * frames * 6);
    // coded channel order is C L R LFE Lss Rss | Lsr Rsr; the decoder outputs L R C LFE Ls Rs
    let _ = map;
    // the table value nearest to 1/sqrt(2) (23170/32768) is both the scale and, through the inverse, the coefficient
    let scaled = |side: f64, rear: f64| -> f64 {
        let sum = side as i64 + ((rear as i64 * 32767 + 16384) >> 15);
        ((sum * 23170 + 16384) >> 15) as f64
    };
    let want = |ch: usize, i: usize| -> f64 {
        // layout order: L R C LFE Lss Rss Lsr Rsr, signal index = coded index
        let s = |coded: usize| sigs[coded][i] as f64;
        // coded indices follow ascending speaker bit: C0 L1 R2 LFE5 Lss9 Rss10 Lsr7 Rsr8 -> set 0: C L R LFE Lss Rss, set 1: Lsr Rsr
        match ch {
            0 => s(1),
            1 => s(2),
            2 => s(0),
            3 => s(3),
            4 => scaled(s(4), s(6)),
            _ => scaled(s(5), s(7)),
        }
    };
    let mut worst = 0.0f64;
    let mut per = [0.0f64; 6];
    for i in n * 2..n * frames - n {
        for ch in 0..6 {
            let e = (dec[i * 6 + ch] as f64 - want(ch, i)).abs();
            per[ch] = per[ch].max(e);
            worst = worst.max(e);
        }
    }
    eprintln!("per-channel worst error {per:?}");
    assert!(worst <= 1.0, "downmix deviates by {worst}");
}

#[test]
fn core_plus_xll_six_one_and_six_zero_48k() {
    core_roundtrip("c61", "6.1-discrete", 48_000, 24, 24);
    core_roundtrip("c60", "6.0-discrete", 48_000, 24, 24);
    core_roundtrip("c70", "7.0-side-rear", 48_000, 24, 24);
}
