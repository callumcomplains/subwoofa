//! Lossless (XLL-only) round trip through FFmpeg's decoder.
use std::process::Command;
use sw_codec_dts::exss::{dtshd_header, DtsHdHeader};
use sw_codec_dts::ma::{map_layout, MaConfig, MaEncoder};
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
            ((2.0 * std::f64::consts::PI * (200.0 + 130.0 * c as f64) * t).sin() * 2_000_000.0 + (2.0 * std::f64::consts::PI * 3100.0 * t).sin() * 300_000.0 + (2.0 * std::f64::consts::PI * 0.31 * i as f64).sin() * 150_000.0) as i32 + noise * 4
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


fn roundtrip(name: &str, layout_id: &str, rate: u32, frames: usize) {
    if !have_ffmpeg() {
        return;
    }
    let layout = ChannelLayout::find(layout_id).unwrap();
    let map = map_layout(layout);
    let mut enc = MaEncoder::new(MaConfig { sample_rate: rate, map: map.clone(), bit_res: 24, dialnorm: 0, wide_remap: false, repr_type: None });
    let n = enc.frame_samples();
    let nch = map.order.len();
    let mut seed = 99u32;
    let sigs: Vec<Vec<i32>> = (0..nch).map(|c| signal(c, n * frames, &mut seed)).collect();
    let mut stream = Vec::new();
    for f in 0..frames {
        let part: Vec<Vec<i32>> = sigs.iter().map(|s| s[f * n..(f + 1) * n].to_vec()).collect();
        stream.extend(enc.encode_frame(&part));
    }
    let mut file = dtshd_header(&DtsHdHeader { sample_rate: rate, frames: frames as u32, samples_per_frame: n as u16, total_samples: (n * frames) as u64, ch_mask: 0xF, initial_padding: 0 }, stream.len() as u64);
    file.extend_from_slice(&stream);
    let dir = std::env::temp_dir().join(format!("sw_dts_ma_{}_{}", std::process::id(), name));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.dtshd");
    std::fs::write(&path, &file).unwrap();
    let out = Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "warning", "-i"]).arg(&path).args(["-c:a", "pcm_s24le", "-f", "s24le", "-"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.stderr.is_empty(), "{name}: ffmpeg complained: {}", String::from_utf8_lossy(&out.stderr));
    let dec: Vec<i32> = out.stdout.chunks_exact(3).map(|b| i32::from_le_bytes([b[0], b[1], b[2], 0]) << 8 >> 8).collect();
    assert_eq!(dec.len(), n * frames * nch, "{name}: decoded length");
    // FFmpeg outputs channels in layout order (L R C LFE Ls Rs for 5.1); find each coded channel's output slot
    let want_order: Vec<usize> = (0..nch).collect();
    let _ = want_order;
    // map: output slot k holds layout channel k; coded channel j is layout channel map.order[j]
    for (j, &lc) in map.order.iter().enumerate() {
        let lc = out_position(layout, lc);
        for i in 0..n * frames {
            assert_eq!(dec[i * nch + lc], sigs[j][i], "{name}: channel {lc} differs at sample {i}");
        }
    }
}

#[test]
fn five_one_48k() {
    roundtrip("51", "5.1", 48_000, 12);
}

#[test]
fn stereo_48k() {
    roundtrip("20", "2.0", 48_000, 8);
}

#[test]
fn five_one_96k() {
    roundtrip("51_96", "5.1", 96_000, 8);
}

#[test]
fn stereo_and_five_one_192k_two_bands() {
    roundtrip("20_192", "2.0", 192_000, 6);
    roundtrip("51_192", "5.1", 192_000, 4);
}

#[test]
fn seven_one_48k() {
    roundtrip("71", "7.1-side-rear", 48_000, 8);
    roundtrip("71b", "7.1-ls-rs-lsr-rsr", 48_000, 8);
    roundtrip("71h", "7.1-height", 48_000, 8);
    roundtrip("71w", "7.1-wide", 48_000, 8);
}

#[test]
fn six_one_and_seven_zero_lossless_only() {
    roundtrip("61", "6.1-discrete", 48_000, 8);
    roundtrip("60", "6.0-discrete", 48_000, 8);
    roundtrip("70", "7.0-side-rear", 48_000, 8);
}

/// Immersive layouts (up to NHK 22.2's 24 channels): hierarchical XLL channel sets of at most eight channels. FFmpeg
/// lists the channels in speaker-bit order with `-channel_order coded`; whatever the order, every source channel
/// must come back sample for sample in exactly one output channel.
fn immersive(layout_id: &str, rate: u32) {
    if !have_ffmpeg() {
        return;
    }
    let layout = ChannelLayout::find(layout_id).unwrap();
    let map = map_layout(layout);
    let mut enc = MaEncoder::new(MaConfig { sample_rate: rate, map: map.clone(), bit_res: 24, dialnorm: 0, wide_remap: false, repr_type: None });
    let n = enc.frame_samples();
    let nch = map.order.len();
    assert_eq!(nch, layout.channels.len());
    let frames = 6;
    let mut seed = 7u32;
    let sigs: Vec<Vec<i32>> = (0..nch).map(|c| signal(c, n * frames, &mut seed)).collect();
    let mut stream = Vec::new();
    for f in 0..frames {
        let part: Vec<Vec<i32>> = sigs.iter().map(|s| s[f * n..(f + 1) * n].to_vec()).collect();
        stream.extend(enc.encode_frame(&part));
    }
    let mut file = dtshd_header(&DtsHdHeader { sample_rate: rate, frames: frames as u32, samples_per_frame: n as u16, total_samples: (n * frames) as u64, ch_mask: 0xF, initial_padding: 0 }, stream.len() as u64);
    file.extend_from_slice(&stream);
    let dir = std::env::temp_dir().join(format!("sw_dts_ma_{}_{}_{}", std::process::id(), layout_id, rate));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.dtshd");
    std::fs::write(&path, &file).unwrap();
    let out = Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "warning", "-channel_order", "coded", "-i"]).arg(&path).args(["-c:a", "pcm_s24le", "-f", "s24le", "-"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    // the one message FFmpeg prints is its guess at the layout, not a complaint
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.lines().all(|l| l.contains("Guessed Channel Layout")), "{layout_id}: ffmpeg complained: {err}");
    let dec: Vec<i32> = out.stdout.chunks_exact(3).map(|b| i32::from_le_bytes([b[0], b[1], b[2], 0]) << 8 >> 8).collect();
    assert_eq!(dec.len(), n * frames * nch, "{layout_id}: decoded length");
    let mut used = vec![false; nch];
    for (j, sig) in sigs.iter().enumerate() {
        let hit = (0..nch).find(|&k| !used[k] && (0..n * frames).all(|i| dec[i * nch + k] == sig[i]));
        let k = hit.unwrap_or_else(|| panic!("{layout_id}: source channel {j} did not come back intact"));
        used[k] = true;
    }
}

#[test]
fn twenty_two_point_two_is_lossless() {
    immersive("22.2", 48_000);
}

#[test]
fn height_layouts_are_lossless() {
    for id in ["5.1.2", "5.1.4", "7.1.2", "7.1.4", "9.1.6"] {
        immersive(id, 48_000);
    }
    immersive("7.1.4", 96_000);
}
