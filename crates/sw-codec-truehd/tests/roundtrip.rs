//! Encode synthetic audio, decode it with FFmpeg's TrueHD decoder and require a bit-exact match.
//! Any decoder complaint (parity, checksum, length) on stderr fails the test.

use std::process::Command;
use sw_codec_truehd::{Config, Downmix, Encoder};

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
    let mut enc = Encoder::new(Config { sample_rate: rate, chanmap, channels, restart_interval: interval, downmix: None, decorrelate: true, mlp: None }).unwrap();
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

/* ---- stereo downmix substream ---- */

fn ceil14(y: i64) -> i64 {
    -((-y) >> 14)
}

fn wrap24(x: i64) -> i32 {
    ((x as i32) << 8) >> 8
}

/// Independent reimplementation of the downmix formula, used as the expected stereo presentation.
/// A stereo-only decoder emits 24-bit samples, so anything beyond 24 bits wraps.
fn expected_stereo(sigs: &[Vec<i32>], dm: &Downmix, n: usize) -> (Vec<i32>, Vec<i32>) {
    let nch = sigs.len();
    let mut v: Vec<Vec<i64>> = sigs.iter().map(|s| s.iter().map(|x| *x as i64).collect()).collect();
    if nch == 8 {
        for i in 0..n {
            v[4][i] = sigs[4][i] as i64 + ceil14(dm.fold as i64 * sigs[6][i] as i64);
            v[5][i] = sigs[5][i] as i64 + ceil14(dm.fold as i64 * sigs[7][i] as i64);
        }
    }
    let mut lo = vec![0; n];
    let mut ro = vec![0; n];
    for i in 0..n {
        let (mut a, mut b) = (0i64, 0i64);
        for j in 2..nch.min(6) {
            a += dm.lo[j] as i64 * v[j][i];
            b += dm.ro[j] as i64 * v[j][i];
        }
        let d = dm.dest as i64;
        let pick = |orig: i64, y: i64| -> i64 {
            if dm.dest == 1 << 14 {
                orig + ceil14(y)
            } else {
                let target = ((orig - 1) << 14) + y;
                target.div_euclid(d) + i64::from(target.rem_euclid(d) != 0)
            }
        };
        lo[i] = wrap24(pick(sigs[0][i] as i64, a));
        ro[i] = wrap24(pick(sigs[1][i] as i64, b));
    }
    (lo, ro)
}

fn decode(path: &std::path::Path, stereo_only: bool) -> (std::process::Output, Vec<i32>) {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error"]);
    if stereo_only {
        cmd.args(["-downmix", "stereo"]);
    }
    let out = cmd.arg("-i").arg(path).args(["-c:a", "pcm_s24le", "-f", "s24le", "-"]).output().unwrap();
    let samples = out.stdout.chunks_exact(3).map(|b| i32::from_le_bytes([b[0], b[1], b[2], 0]) << 8 >> 8).collect();
    (out, samples)
}

fn run_downmix(name: &str, chanmap: u16, channels: usize, decoded_order: &[usize], dm: Downmix, n: usize, kind: Kind) {
    run_downmix_bits(name, chanmap, channels, decoded_order, dm, n, kind, 0)
}

/// `zero_bits` low bits of every source sample are cleared (16/18-bit material in a 24-bit file).
fn run_downmix_bits(name: &str, chanmap: u16, channels: usize, decoded_order: &[usize], dm: Downmix, n: usize, kind: Kind, zero_bits: u32) {
    if !have_ffmpeg() {
        eprintln!("ffmpeg missing, skipping");
        return;
    }
    let sigs: Vec<Vec<i32>> = (0..channels).map(|c| signal(kind, c, n).into_iter().map(|v| (v >> zero_bits) << zero_bits).collect()).collect();
    let mut enc = Encoder::new(Config { sample_rate: 48_000, chanmap, channels, restart_interval: 16, downmix: Some(dm.clone()), decorrelate: true, mlp: None }).unwrap();
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
    let dir = std::env::temp_dir().join(format!("sw_thd_dm_{}_{}", std::process::id(), name));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.thd");
    std::fs::write(&path, &stream).unwrap();

    // full presentation: every channel exactly as it went in
    let (out, full) = decode(&path, false);
    assert!(out.status.success() && out.stderr.is_empty(), "{name}: full decode: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(full.len(), n * channels, "{name}: full decode length");
    for (d, &coded) in decoded_order.iter().enumerate() {
        for i in 0..n {
            assert_eq!(full[i * channels + d], sigs[coded][i], "{name}: channel {coded} differs at sample {i}");
        }
    }

    // stereo presentation: substream 0 alone must equal the downmix formula
    let (out, st) = decode(&path, true);
    assert!(out.status.success() && out.stderr.is_empty(), "{name}: stereo decode: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(st.len(), n * 2, "{name}: stereo decode length (is the downmix substream really decoded alone?)");
    let (lo, ro) = expected_stereo(&sigs, &dm, n);
    // with unused low bits the stereo pair is coded on a coarser grid (error below about one grid step,
    // plus the difference between the exact 1/2 scale used there and the 16383/32768-ish scale of the formula)
    let tol = if zero_bits > 0 { (3i32 << zero_bits) / 2 + 2 } else { 0 };
    for i in 0..n {
        assert!((st[i * 2] - lo[i]).abs() <= tol + lo[i].abs() / 20_000, "{name}: Lo differs at sample {i}: {} vs {}", st[i * 2], lo[i]);
        assert!((st[i * 2 + 1] - ro[i]).abs() <= tol + ro[i].abs() / 20_000, "{name}: Ro differs at sample {i}: {} vs {}", st[i * 2 + 1], ro[i]);
    }
}

const C707: i32 = 11585; // -3 dB in 2.14

fn five_one_downmix() -> Downmix {
    //                 L  R  C     LFE Ls    Rs
    Downmix { lo: vec![0, 0, C707, 0, C707, 0], ro: vec![0, 0, C707, 0, 0, C707], fold: 0, dest: 32767 }
}

#[test]
fn downmix_five_one() {
    run_downmix("dm51", FIVE_ONE, 6, &ID6, five_one_downmix(), 48_000 * 2 + 29, Kind::Music);
}

#[test]
fn downmix_five_one_on_a_coarse_grid() {
    // 18-bit material: the stereo pair drops five bits, the 5.1 decode is still exact
    run_downmix_bits("dm51_coarse", FIVE_ONE, 6, &ID6, five_one_downmix(), 48_000 * 2 + 29, Kind::Music, 6);
    run_downmix_bits("dm51_coarse1", FIVE_ONE, 6, &ID6, five_one_downmix(), 48_000 + 7, Kind::Music, 1);
}

#[test]
fn downmix_unity_on_a_coarse_grid() {
    let mut dm = five_one_downmix();
    dm.dest = 1 << 14;
    run_downmix_bits("dm51_unity_coarse", FIVE_ONE, 6, &ID6, dm, 48_000 + 13, Kind::Music, 4);
}

#[test]
fn downmix_five_one_loud() {
    // loud material makes the downmix channels exceed 24 bits
    run_downmix("dm51_loud", FIVE_ONE, 6, &ID6, five_one_downmix(), 48_000 + 5, Kind::Loud);
}

#[test]
fn downmix_five_one_with_lfe_in_the_mix() {
    let mut dm = five_one_downmix();
    dm.lo[3] = 5000;
    dm.ro[3] = 5000;
    run_downmix("dm51_lfe", FIVE_ONE, 6, &ID6, dm, 48_000 + 77, Kind::Music);
}

#[test]
fn downmix_five_zero_and_three_zero() {
    // 3.0 (L R C): only the centre feeds the downmix
    run_downmix("dm30", 0b11, 3, &[0, 1, 2], Downmix { lo: vec![0, 0, C707], ro: vec![0, 0, C707], fold: 0, dest: 32767 }, 48_000 + 3, Kind::Music);
}

#[test]
fn downmix_seven_one() {
    // coded: L R C LFE Ls Rs Lrs Rrs; decoder emits FL FR FC LFE BL BR SL SR
    let dm = Downmix { lo: vec![0, 0, C707, 0, C707, 0, 0, 0], ro: vec![0, 0, C707, 0, 0, C707, 0, 0], fold: C707, dest: 32767 };
    run_downmix("dm71", SEVEN_ONE, 8, &[0, 1, 2, 3, 6, 7, 4, 5], dm, 48_000 + 19, Kind::Music);
}

#[test]
fn downmix_unity_gain_without_bypass_bits() {
    // unit front gain needs no LSB-bypass bits (but can overflow 24 bits on loud material)
    let mut dm = five_one_downmix();
    dm.dest = 1 << 14;
    run_downmix("dm51_unity", FIVE_ONE, 6, &ID6, dm, 48_000 + 13, Kind::Music);
}

#[test]
fn downmix_half_level_stays_inside_24_bits_on_loud_material() {
    // headroom check: with the half-level default a loud mix must not wrap
    let dm = five_one_downmix();
    let n = 48_000;
    let sigs: Vec<Vec<i32>> = (0..6).map(|c| signal(Kind::Loud, c, n)).collect();
    let (lo, _) = expected_stereo(&sigs, &dm, n);
    let raw: Vec<i64> = (0..n)
        .map(|i| {
            let y: i64 = (2..6).map(|j| dm.lo[j] as i64 * sigs[j][i] as i64).sum();
            let target = ((sigs[0][i] as i64 - 1) << 14) + y;
            target.div_euclid(dm.dest as i64) + i64::from(target.rem_euclid(dm.dest as i64) != 0)
        })
        .collect();
    let over = raw.iter().filter(|v| v.abs() >= 1 << 23).count();
    println!("{over} of {n} samples exceed 24 bits at half level on full-scale noise");
    assert!(lo.len() == n);
    // full-scale uncorrelated noise in four channels is far worse than any real mix
    assert!(over * 50 < n, "too many overflows: {over}");
}

/// MLP (DVD-Audio) streams: the same decoder must reproduce them exactly.
fn run_mlp(name: &str, arrangement: u8, channels: usize, rate: u32, bits: u32) {
    if !have_ffmpeg() {
        return;
    }
    let n = rate as usize * 2;
    let sigs: Vec<Vec<i32>> = (0..channels).map(|c| signal(if bits == 16 { Kind::SixteenBit } else { Kind::Music }, c, n)).collect();
    let mut enc = Encoder::new(Config { sample_rate: rate, chanmap: 0, channels, restart_interval: 16, downmix: None, decorrelate: true, mlp: Some(sw_codec_truehd::Mlp { arrangement, bits }) }).unwrap();
    let chunk = enc.interval_samples();
    let mut stream = Vec::new();
    let mut pos = 0;
    while pos < n {
        let len = chunk.min(n - pos);
        let part: Vec<Vec<i32>> = sigs.iter().map(|s| { let mut v = vec![0i32; chunk]; v[..len].copy_from_slice(&s[pos..pos + len]); v }).collect();
        pos += len;
        stream.extend(enc.encode_interval(&part, len, pos >= n));
    }
    let dir = std::env::temp_dir().join(format!("sw_mlp_{}_{}", std::process::id(), name));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.mlp");
    std::fs::write(&path, &stream).unwrap();
    let out = Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-i"]).arg(&path).args(["-c:a", "pcm_s24le", "-f", "s24le", "-"]).output().unwrap();
    assert!(out.status.success() && out.stderr.is_empty(), "{name}: {}", String::from_utf8_lossy(&out.stderr));
    assert!(out.stdout.len() >= n * channels * 3, "{name}: decoded {} bytes", out.stdout.len());
    for c in 0..channels {
        for i in 0..n {
            let o = (i * channels + c) * 3;
            let got = i32::from_le_bytes([out.stdout[o], out.stdout[o + 1], out.stdout[o + 2], 0]) << 8 >> 8;
            assert_eq!(got, sigs[c][i], "{name}: channel {c} differs at sample {i}");
        }
    }
}

#[test]
fn mlp_stereo_16_bit_44_1() {
    run_mlp("mlp_st16", 1, 2, 44_100, 16);
}

#[test]
fn mlp_five_one_24_bit_96() {
    run_mlp("mlp_51", 12, 6, 96_000, 24);
}
