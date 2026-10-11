//! Core plus lossless extension: the decoder must reproduce the source exactly.
use std::process::Command;
use sw_codec_dts::ma::{map_layout, map_layout_with};
use sw_codec_dts::ma_core::{MaCoreConfig, MaCoreEncoder};
use sw_core::{ChannelLayout, Downmix};

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
    core_roundtrip_x(name, layout_id, rate, frames, bit_res, false)
}

fn core_roundtrip_x(name: &str, layout_id: &str, rate: u32, frames: usize, bit_res: u32, x96: bool) {
    core_roundtrip_dm(name, layout_id, rate, frames, bit_res, x96, Downmix::default())
}

fn core_roundtrip_dm(name: &str, layout_id: &str, rate: u32, frames: usize, bit_res: u32, x96: bool, downmix: Downmix) {
    if !have_ffmpeg() {
        return;
    }
    let layout = ChannelLayout::find(layout_id).unwrap();
    let map = map_layout_with(layout, &downmix);
    let mut enc = MaCoreEncoder::new(MaCoreConfig { layout, sample_rate: rate, bit_res, core_bitrate: 1_509_000, dialnorm: 0, x96, wide_remap: false, downmix }).unwrap();
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

/// A stereo downmix carried as a channel set of its own, with custom levels, must leave the full decode exact.
#[test]
fn embedded_stereo_set_is_lossless() {
    let mut dm = Downmix::default();
    dm.embed_stereo = true;
    core_roundtrip_dm("emb51", "5.1", 48_000, 12, 24, false, dm.clone());
    core_roundtrip_dm("emb71", "7.1-side-rear", 48_000, 12, 24, false, dm.clone());
    dm.stereo_a[0] = Some(3.0);
    dm.stereo_b[3] = Some(9.0);
    dm.primary = [Some(1.5), Some(4.5), Some(3.0), Some(6.0), Some(0.0), Some(9.0)];
    dm.stereo_a[1] = Some(1.5);
    core_roundtrip_dm("emb71_custom", "7.1-wide", 48_000, 12, 24, false, dm.clone());
    core_roundtrip_dm("emb61", "6.1-discrete", 48_000, 12, 24, false, dm);
}

/// The 5.1 downmix levels of a 7.x layout change the embedded set, not the decoded programme.
#[test]
fn custom_five_one_downmix_is_lossless() {
    let mut dm = Downmix::default();
    dm.primary = [Some(6.0), Some(1.5), Some(4.5), Some(9.0), Some(12.0), Some(0.0)];
    dm.extra_a = [Some(4.0), None, Some(2.0), Some(5.0), None, Some(13.0)];
    dm.extra_b = [None, Some(10.0), None, Some(16.0), Some(7.0), None];
    core_roundtrip_dm("dm71", "7.1-side-rear", 48_000, 12, 24, false, dm);
}

/// A 96/24 core (X96 in the core frames) must not disturb the lossless extension.
#[test]
fn core_with_x96_plus_xll_five_one_96k() {
    core_roundtrip_x("c51x96", "5.1", 96_000, 12, 24, true);
}

#[test]
fn core_plus_xll_five_one_48k() {
    core_roundtrip("c51", "5.1", 48_000, 24, 24);
}

#[test]
fn core_plus_xll_seven_one_48k() {
    core_roundtrip("c71", "7.1-side-rear", 48_000, 24, 24);
}

/// A 5.1-only decoder (reading just the first channel set) must get the documented downmix: every channel
/// at -3 dB (the Downmix tab's default scale), the side channels with the rear channels folded in at -3 dB each.
#[test]
fn seven_one_embedded_five_one_downmix() {
    if !have_ffmpeg() {
        return;
    }
    let layout = ChannelLayout::find("7.1-side-rear").unwrap();
    let map = map_layout(layout);
    let frames = 16;
    let mut enc = MaCoreEncoder::new(MaCoreConfig { layout, sample_rate: 48_000, bit_res: 24, core_bitrate: 1_509_000, dialnorm: 0 , x96: false, wide_remap: false, downmix: Default::default() }).unwrap();
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
            0 => scaled(s(1), 0.0),
            1 => scaled(s(2), 0.0),
            2 => scaled(s(0), 0.0),
            3 => scaled(s(3), 0.0),
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

/// At 192 kHz the core is synthesised from band 0 of the lossless split; the decode must stay bit-exact.
#[test]
fn core_plus_xll_five_one_192k() {
    core_roundtrip_x("c51_192", "5.1", 192_000, 8, 24, false);
}
