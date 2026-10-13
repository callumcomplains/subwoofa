//! Encode synthetic audio and decode it with FFmpeg's DTS decoder.
use std::process::Command;
use sw_codec_dts::{Config, Encoder};

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run(name: &str, channels: usize, amode: u8, lfe: bool, bitrate: u32, blocks: usize) -> (f64, Vec<f64>) {
    run_ext(name, channels, amode, lfe, false, bitrate, blocks)
}

/// `xch`: an extra centre-surround channel after the primaries.
fn run_ext(
    name: &str,
    channels: usize,
    amode: u8,
    lfe: bool,
    xch: bool,
    bitrate: u32,
    blocks: usize,
) -> (f64, Vec<f64>) {
    let mut enc = Encoder::new(Config {
        sample_rate: 48_000,
        amode,
        channels,
        lfe,
        bitrate,
        dialnorm: 0,
        pcm_bits: 24,
        es: xch,
        xch,
        perfect: false,
        blocks,
    })
    .unwrap();
    let n = enc.frame_samples();
    let frames = 60;
    let total = n * frames;
    let nin = channels + usize::from(xch) + usize::from(lfe);
    let sig: Vec<Vec<f64>> = (0..nin)
        .map(|c| {
            (0..total)
                .map(|i| {
                    let t = i as f64 / 48_000.0;
                    let is_lfe = lfe && c == channels + usize::from(xch);
                    let f = if is_lfe { 40.0 } else { 300.0 + 700.0 * c as f64 };
                    (2.0 * std::f64::consts::PI * f * t).sin() * 1_000_000.0
                        + if is_lfe {
                            0.0
                        } else {
                            (2.0 * std::f64::consts::PI * (f * 7.3) * t).sin() * 200_000.0
                        }
                })
                .collect()
        })
        .collect();
    // an XCH decoder takes the centre surround back out of the surrounds, so the core carries them mixed
    let mut feed = sig.clone();
    if xch {
        for i in 0..total {
            let cs = sig[channels][i] * std::f64::consts::FRAC_1_SQRT_2;
            feed[3][i] += cs;
            feed[4][i] += cs;
        }
    }
    let mut stream = Vec::new();
    for f in 0..frames {
        let part: Vec<Vec<f64>> = feed.iter().map(|s| s[f * n..(f + 1) * n].to_vec()).collect();
        stream.extend(enc.encode_frame(&part));
    }
    let dir = std::env::temp_dir().join(format!("sw_dts_{}_{}", std::process::id(), name));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.dts");
    std::fs::write(&path, &stream).unwrap();
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(&path)
        .args(["-f", "f64le", "-"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
    let dec: Vec<f64> = out
        .stdout
        .chunks_exact(8)
        .map(|b| f64::from_le_bytes(b.try_into().unwrap()))
        .collect();
    let nout = nin;
    assert_eq!(dec.len() % nout, 0);
    let _ = enc.delay();
    let mut snrs = Vec::new();
    let mut gain0 = 0.0;
    for c in 0..nout {
        let ch: Vec<f64> = dec.iter().skip(c).step_by(nout).copied().collect();
        let mut best = (0.0f64, 0usize, 0.0f64, 0usize);
        for (ic, s) in sig.iter().enumerate() {
            for delay in (0..1300usize).step_by(1) {
                let a = 4 * n;
                let b = total - 2 * n;
                let (mut xy, mut xx, mut yy) = (0.0, 0.0, 0.0);
                for i in (a..b).step_by(7) {
                    let x = s[i - delay];
                    let y = ch[i];
                    xy += x * y;
                    xx += x * x;
                    yy += y * y;
                }
                let corr = xy * xy / (xx * yy + 1e-30);
                if corr > best.0 {
                    best = (corr, ic, xy / xx, delay);
                }
            }
        }
        println!(
            "  out ch {c}: matches input {} delay {} gain {:.3e}",
            best.1, best.3, best.2
        );
        if c == 0 {
            gain0 = best.2;
        }
        snrs.push(10.0 * (best.0 / (1.0 - best.0 + 1e-12)).log10());
    }
    (gain0, snrs)
}

#[test]
fn stereo_roundtrip() {
    if !have_ffmpeg() {
        return;
    }
    let (gain, snr) = run("st", 2, 2, false, 768_000, 16);
    println!("stereo: gain {gain:.5}, snr {snr:?}");
    assert!(snr.iter().all(|s| *s > 30.0), "{snr:?}");
}

#[test]
fn five_one_roundtrip() {
    if !have_ffmpeg() {
        return;
    }
    let (gain, snr) = run("51", 5, 9, true, 1_509_000, 16);
    println!("5.1: gain {gain:.5}, snr {snr:?}");
    assert!(snr.iter().all(|s| *s > 30.0), "{snr:?}");
}

#[test]
fn es_discrete_roundtrip() {
    if !have_ffmpeg() {
        return;
    }
    let (gain, snr) = run_ext("xch", 5, 9, true, true, 1_509_000, 16);
    println!("6.1 ES: gain {gain:.5}, snr {snr:?}");
    assert_eq!(snr.len(), 7);
    assert!(snr.iter().all(|s| *s > 30.0), "{snr:?}");
}

/// Place noise in one subband at a time and compare FFmpeg's decode with our own synthesis bank.
#[test]
fn band_mapping_matches_synthesis() {
    if !have_ffmpeg() {
        return;
    }
    use sw_codec_dts::dca::qmf::Synthesis;
    for band in [0usize, 1, 2, 3, 4, 5, 6, 7, 8, 15, 16, 17, 30, 31] {
        let mut enc = Encoder::new(Config {
            sample_rate: 48_000,
            amode: 0,
            channels: 1,
            lfe: false,
            bitrate: 1_000_000,
            dialnorm: 0,
            pcm_bits: 24,
            es: false,
            xch: false,
            perfect: false,
            blocks: 16,
        })
        .unwrap();
        let frames = 40;
        let mut stream = Vec::new();
        let mut sy = Synthesis::new(false);
        let mut reference = Vec::new();
        let mut seed = 7u32;
        for _ in 0..frames {
            let mut sub = vec![vec![vec![0f64; 16]; 32]];
            for b in 0..16 {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                sub[0][band][b] = ((seed >> 8) as f64 / 8_388_608.0 - 1.0) * 100_000.0;
                let mut x = [0f64; 32];
                x[band] = sub[0][band][b];
                let mut out = [0f64; 32];
                sy.step(&x, &mut out);
                reference.extend_from_slice(&out);
            }
            stream.extend(enc.encode_subbands(&sub, None));
        }
        let dir = std::env::temp_dir().join(format!("sw_dts_band_{}_{}", std::process::id(), band));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.dts");
        std::fs::write(&path, &stream).unwrap();
        let out = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-i"])
            .arg(&path)
            .args(["-f", "f64le", "-"])
            .output()
            .unwrap();
        let dec: Vec<f64> = out
            .stdout
            .chunks_exact(8)
            .map(|b| f64::from_le_bytes(b.try_into().unwrap()))
            .collect();
        let mut best = (0.0f64, 0usize, 0.0f64);
        for delay in 0..700usize {
            let (mut xy, mut xx, mut yy) = (0.0, 0.0, 0.0);
            for i in 2000..reference.len() - 2000 {
                if i + delay >= dec.len() {
                    break;
                }
                let x = reference[i];
                let y = dec[i + delay];
                xy += x * y;
                xx += x * x;
                yy += y * y;
            }
            let c = xy / (xx * yy + 1e-30).sqrt();
            if c.abs() > best.0.abs() {
                best = (c, delay, xy / xx);
            }
        }
        println!(
            "band {band:2}: corr {:+.4} delay {} gain {:.3e}",
            best.0, best.1, best.2
        );
    }
}
