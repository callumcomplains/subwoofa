//! Encode synthetic audio, decode it with FFmpeg's TrueHD decoder and require a bit-exact match.
//! Any decoder complaint (parity, checksum, length) on stderr fails the test.

use std::process::Command;
use sw_codec_truehd::{Config, Encoder};

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().map(|o| o.status.success()).unwrap_or(false)
}

#[derive(Clone, Copy)]
enum Kind {
    /// tones + shaped noise: compressible, like music
    Music,
    /// full-scale white-ish noise: nearly incompressible
    Loud,
    /// mostly digital silence with a burst in the middle
    Gappy,
    /// 16-bit source scaled into 24 bits (eight trailing zero bits)
    SixteenBit,
    /// strong pure tone over very quiet noise (very predictable: extreme predictor coefficients)
    PureTone,
}

fn signal(kind: Kind, ch: usize, n: usize) -> Vec<i32> {
    let mut x = 0x9E37_79B9u32.wrapping_add(ch as u32 * 7919);
    let mut lp = 0f64;
    (0..n)
        .map(|i| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let white = x as f64 / u32::MAX as f64 - 0.5;
            lp += 0.1 * (white - lp);
            let t = i as f64 / 48_000.0;
            let v = match kind {
                Kind::Music => {
                    ((2.0 * std::f64::consts::PI * (180.0 + 61.0 * ch as f64) * t).sin() * 0.35
                        + (2.0 * std::f64::consts::PI * (2400.0 + 130.0 * ch as f64) * t).sin() * 0.08
                        + lp * 1.2)
                        * 0.8
                }
                Kind::Loud => white * 1.98,
                Kind::Gappy => {
                    if (n / 3..n / 2).contains(&i) { (2.0 * std::f64::consts::PI * 440.0 * t).sin() * 0.5 } else { 0.0 }
                }
                Kind::PureTone => {
                    let noise = (white * 2.0 * 8_388_608.0 / 40.0) / 8_388_608.0;
                    ((2.0 * std::f64::consts::PI * (200.0 + 90.0 * ch as f64) * t).sin() * 0.2384 + noise)
                }
                Kind::SixteenBit => {
                    let s = ((2.0 * std::f64::consts::PI * (300.0 + 40.0 * ch as f64) * t).sin() * 0.4 + lp) * 32768.0;
                    return (s.round().clamp(-32768.0, 32767.0) as i32) << 8;
                }
            };
            (v * 8_388_608.0).round().clamp(-8_388_608.0, 8_388_607.0) as i32
        })
        .collect()
}

/// `decoded_order[d]` = coded channel index for decoder output channel `d`.
fn run(name: &str, rate: u32, chanmap: u16, channels: usize, decoded_order: &[usize], kind: Kind, n: usize) {
    run_interval(name, rate, chanmap, channels, decoded_order, kind, n, 16)
}

#[allow(clippy::too_many_arguments)]
fn run_interval(name: &str, rate: u32, chanmap: u16, channels: usize, decoded_order: &[usize], kind: Kind, n: usize, interval: usize) {
    if !have_ffmpeg() {
        eprintln!("ffmpeg missing, skipping");
        return;
    }
    let sigs: Vec<Vec<i32>> = (0..channels).map(|c| signal(kind, c, n)).collect();
    let mut enc = Encoder::new(Config { sample_rate: rate, chanmap, channels, restart_interval: interval }).unwrap();
    let chunk = enc.interval_samples();
    let mut stream = Vec::new();
    let mut pos = 0;
    while pos < n {
        let len = chunk.min(n - pos);
        let part: Vec<Vec<i32>> = sigs
            .iter()
            .map(|s| {
                let mut v = vec![0i32; chunk];
                v[..len].copy_from_slice(&s[pos..pos + len]);
                v
            })
            .collect();
        pos += len;
        stream.extend(enc.encode_interval(&part, len, pos >= n));
    }
    let dir = std::env::temp_dir().join(format!("sw_thd_{}_{}", std::process::id(), name));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.thd");
    std::fs::write(&path, &stream).unwrap();

    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(&path)
        .args(["-c:a", "pcm_s24le", "-f", "s24le", "-"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{name}: decoder failed: {}", String::from_utf8_lossy(&out.stderr));
    assert!(out.stderr.is_empty(), "{name}: decoder complained: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.stdout.len(), n * channels * 3, "{name}: decoded length differs ({} samples expected)", n);

    for (d, &coded) in decoded_order.iter().enumerate() {
        for i in 0..n {
            let o = (i * channels + d) * 3;
            let got = i32::from_le_bytes([out.stdout[o], out.stdout[o + 1], out.stdout[o + 2], 0]) << 8 >> 8;
            assert_eq!(got, sigs[coded][i], "{name}: channel {coded} (decoded {d}) differs at sample {i}");
        }
    }
    let ratio = stream.len() as f64 / (n * channels * 3) as f64;
    println!("{name}: {:.1}% of PCM size", ratio * 100.0);
}

const STEREO: u16 = 0b1;
const FIVE_ONE: u16 = 0b1111;
const SEVEN_ONE: u16 = 0b100_1111;
const ID6: [usize; 6] = [0, 1, 2, 3, 4, 5];

#[test]
fn stereo_music() {
    run("stereo_music", 48_000, STEREO, 2, &[0, 1], Kind::Music, 48_000 * 3 + 17);
}

#[test]
fn mono() {
    run("mono", 48_000, 0b10, 1, &[0], Kind::Music, 48_000 + 3);
}

#[test]
fn stereo_loud_noise() {
    run("stereo_loud", 48_000, STEREO, 2, &[0, 1], Kind::Loud, 48_000 * 2 + 5);
}

#[test]
fn stereo_silence_gaps() {
    run("stereo_gappy", 48_000, STEREO, 2, &[0, 1], Kind::Gappy, 48_000 * 2);
}

#[test]
fn stereo_16_bit_source() {
    run("stereo_16bit", 48_000, STEREO, 2, &[0, 1], Kind::SixteenBit, 48_000 + 123);
}

#[test]
fn five_one_music() {
    run("five_one", 48_000, FIVE_ONE, 6, &ID6, Kind::Music, 48_000 * 2 + 41);
}

#[test]
fn five_one_44k1() {
    run("five_one_44k", 44_100, FIVE_ONE, 6, &ID6, Kind::Music, 44_100 + 7);
}

#[test]
fn five_one_96k() {
    run("five_one_96k", 96_000, FIVE_ONE, 6, &ID6, Kind::Music, 96_000 + 11);
}

#[test]
fn seven_one_music() {
    // coded order: L R C LFE Ls Rs Lrs Rrs; the decoder emits FL FR FC LFE BL BR SL SR
    run("seven_one", 48_000, SEVEN_ONE, 8, &[0, 1, 2, 3, 6, 7, 4, 5], Kind::Music, 48_000 + 19);
}

#[test]
fn stereo_music_default_interval() {
    run_interval("stereo_128", 48_000, STEREO, 2, &[0, 1], Kind::Music, 48_000 * 3 + 17, 128);
}

#[test]
fn stereo_music_interval_32() {
    run_interval("stereo_32", 48_000, STEREO, 2, &[0, 1], Kind::Music, 48_000 * 3 + 17, 32);
}

#[test]
fn stereo_music_interval_8() {
    run_interval("stereo_8", 48_000, STEREO, 2, &[0, 1], Kind::Music, 48_000 * 3 + 17, 8);
}

#[test]
fn five_one_pure_tone_default_interval() {
    run_interval("tone", 48_000, FIVE_ONE, 6, &ID6, Kind::PureTone, 192_000, 128);
}

#[test]
fn stereo_pure_tone() {
    run("tone_st", 48_000, STEREO, 2, &[0, 1], Kind::PureTone, 48_000 + 33);
}
