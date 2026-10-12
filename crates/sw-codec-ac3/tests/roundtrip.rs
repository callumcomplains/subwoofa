//! Encode synthetic multichannel audio, decode it with FFmpeg (a conforming third-party
//! decoder) and check level, timing, CRCs and signal-to-noise per channel.

use std::process::Command;
use sw_codec_ac3::{Config, Encoder, SAMPLES_PER_FRAME};

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Deterministic broadband test signal: a few tones plus shaped noise, distinct per channel.
fn signal(ch: usize, n: usize, level: f32) -> Vec<f32> {
    let mut x = 0x1234_5678u32.wrapping_add(ch as u32 * 7919);
    let mut lp = 0f32;
    (0..n)
        .map(|i| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let noise = (x as f32 / u32::MAX as f32 - 0.5) * 0.3;
            lp += 0.15 * (noise - lp); // low-passed noise, speech-like spectrum tilt
            let t = i as f32 / 48_000.0;
            let tones = (2.0 * std::f32::consts::PI * (220.0 + 110.0 * ch as f32) * t).sin() * 0.4
                + (2.0 * std::f32::consts::PI * (3100.0 + 400.0 * ch as f32) * t).sin() * 0.1;
            (tones + lp * 2.0) * level
        })
        .collect()
}

/// LFE content: only the lowest few bins are coded, so feed it bass like a real LFE stem.
fn bass(n: usize, level: f32) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let t = i as f32 / 48_000.0;
            ((2.0 * std::f32::consts::PI * 45.0 * t).sin() * 0.5 + (2.0 * std::f32::consts::PI * 90.0 * t).sin() * 0.3)
                * level
        })
        .collect()
}

fn run(acmod: u8, lfe: bool, bitrate: u32, decoded_order: &[usize], min_snr: f32) {
    run_codec(false, acmod, lfe, bitrate, decoded_order, min_snr)
}

fn run_codec(eac3: bool, acmod: u8, lfe: bool, bitrate: u32, decoded_order: &[usize], min_snr: f32) {
    if !have_ffmpeg() {
        eprintln!("ffmpeg missing, skipping");
        return;
    }
    let nfch = [2, 1, 2, 3, 3, 4, 4, 5][acmod as usize];
    let nch = nfch + usize::from(lfe);
    let secs = 3;
    let n = 48_000 * secs;
    let sigs: Vec<Vec<f32>> = (0..nch)
        .map(|c| {
            if lfe && c == nfch {
                bass(n, 0.6)
            } else {
                signal(c, n, 0.6)
            }
        })
        .collect();
    let cfg = Config {
        acmod,
        lfe,
        sample_rate: 48_000,
        bitrate_kbps: bitrate,
        dialnorm: 31,
        dolby_surround: false,
        bandwidth_hz: None,
        eac3,
    };
    let mut enc = Encoder::new(cfg).unwrap();
    let frames = (n + 256).div_ceil(SAMPLES_PER_FRAME);
    let mut bytes = Vec::new();
    for f in 0..frames {
        let chunk: Vec<Vec<f32>> = sigs
            .iter()
            .map(|s| {
                (0..SAMPLES_PER_FRAME)
                    .map(|i| s.get(f * SAMPLES_PER_FRAME + i).copied().unwrap_or(0.0))
                    .collect()
            })
            .collect();
        let refs: Vec<&[f32]> = chunk.iter().map(|v| v.as_slice()).collect();
        bytes.extend(enc.encode_frame(&refs));
    }
    let dir = std::env::temp_dir().join(format!(
        "sw_ac3_rt_{}_{}_{}_{}_{}",
        std::process::id(),
        acmod,
        lfe,
        eac3,
        bitrate
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(if eac3 { "t.eac3" } else { "t.ac3" });
    std::fs::write(&path, &bytes).unwrap();

    let out = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-err_detect",
            "crccheck+explode",
            "-i",
        ])
        .arg(&path)
        .args(["-f", "f32le", "-"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "decoder rejected the stream: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stderr.is_empty(),
        "decoder warnings: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let dec: Vec<f32> = out
        .stdout
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    assert_eq!(dec.len() % nch, 0);
    let dframes = dec.len() / nch;
    assert!(dframes >= n, "decoded {dframes} frames, expected at least {n}");

    for (src_ch, &dec_ch) in decoded_order.iter().enumerate() {
        let (mut sig, mut err) = (0f64, 0f64);
        for i in 2000..n - 2000 {
            let a = sigs[src_ch][i] as f64;
            let b = dec[(i + 256) * nch + dec_ch] as f64; // decoders delay by one 256-sample block
            sig += a * a;
            err += (a - b) * (a - b);
        }
        let snr = 10.0 * (sig / err.max(1e-30)).log10();
        // the LFE carries seven bins of near-pure bass tones: at low rates it legitimately
        // gets fewer dB than the full-bandwidth channels, so it has a looser floor
        let is_lfe = lfe && src_ch == nfch;
        let floor = if is_lfe { min_snr as f64 - 6.0 } else { min_snr as f64 };
        assert!(
            snr >= floor,
            "acmod {acmod} channel {src_ch}: SNR {snr:.1} dB < {floor} dB"
        );
    }
}

#[test]
fn stereo_192() {
    run(2, false, 192, &[0, 1], 20.0);
}

#[test]
fn mono_96() {
    run(1, false, 96, &[0], 20.0);
}

#[test]
fn three_zero_320() {
    // source order L C R; decoder output order L R C
    run(3, false, 320, &[0, 2, 1], 20.0);
}

#[test]
fn five_one_448() {
    // source order L C R Ls Rs LFE; decoder output order L R C LFE Ls Rs
    run(7, true, 448, &[0, 2, 1, 4, 5, 3], 20.0);
}

#[test]
fn five_one_192_low_rate() {
    run(7, true, 192, &[0, 2, 1, 4, 5, 3], 12.0);
}

// ---- Enhanced AC-3 (Dolby Digital Plus) ----

#[test]
fn eac3_stereo_192() {
    run_codec(true, 2, false, 192, &[0, 1], 20.0);
}

#[test]
fn eac3_five_one_448() {
    run_codec(true, 7, true, 448, &[0, 2, 1, 4, 5, 3], 20.0);
}

#[test]
fn eac3_five_one_odd_bitrate_300() {
    // not a legal AC-3 rate; E-AC-3 sizes frames in words so any rate works
    run_codec(true, 7, true, 300, &[0, 2, 1, 4, 5, 3], 18.0);
}

#[test]
fn eac3_five_one_1024() {
    run_codec(true, 7, true, 1024, &[0, 2, 1, 4, 5, 3], 28.0);
}

#[test]
fn eac3_five_one_low_rate_160() {
    run_codec(true, 7, true, 160, &[0, 2, 1, 4, 5, 3], 10.0);
}

#[test]
fn eac3_mono_64() {
    run_codec(true, 1, false, 64, &[0], 18.0);
}

/// Quiet tone with sharp noise bursts (claps): returns the decoded stream's error energy in the
/// 256 samples before each burst, the place where pre-echo from long transforms lands.
fn pre_echo_error(switching: bool) -> (f64, usize) {
    let n: usize = 48_000 * 3;
    let mut x = 0x2468_ACE1u32;
    let sig: Vec<f32> = (0..n)
        .map(|i| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let noise = x as f32 / u32::MAX as f32 - 0.5;
            let t = i as f32 / 48_000.0;
            let tone = (2.0 * std::f32::consts::PI * 330.0 * t).sin() * 0.01;
            let since = i % 8000;
            let burst = if since >= 4000 {
                noise * 1.6 * (-(since as f32 - 4000.0) / 250.0).exp()
            } else {
                0.0
            };
            tone + burst
        })
        .collect();
    let mut enc = Encoder::new(Config {
        acmod: 1,
        lfe: false,
        sample_rate: 48_000,
        bitrate_kbps: 96,
        dialnorm: 31,
        dolby_surround: false,
        bandwidth_hz: None,
        eac3: false,
    })
    .unwrap();
    enc.set_block_switching(switching);
    let frames = (n + 256).div_ceil(SAMPLES_PER_FRAME);
    let mut bytes = Vec::new();
    for f in 0..frames {
        let chunk: Vec<f32> = (0..SAMPLES_PER_FRAME)
            .map(|i| sig.get(f * SAMPLES_PER_FRAME + i).copied().unwrap_or(0.0))
            .collect();
        bytes.extend(enc.encode_frame(&[chunk.as_slice()]));
    }
    let dir = std::env::temp_dir().join(format!("sw_ac3_pre_{}_{}", std::process::id(), switching));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.ac3");
    std::fs::write(&path, &bytes).unwrap();
    let out = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-err_detect",
            "crccheck+explode",
            "-i",
        ])
        .arg(&path)
        .args(["-f", "f32le", "-"])
        .output()
        .unwrap();
    assert!(
        out.status.success() && out.stderr.is_empty(),
        "decoder: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let dec: Vec<f32> = out
        .stdout
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    let (mut err, mut count) = (0.0, 0usize);
    let mut burst_start = 4000;
    while burst_start + 8000 < n - 4000 {
        for i in burst_start - 256..burst_start {
            let d = dec[i + 256] as f64 - sig[i] as f64;
            err += d * d;
        }
        count += 1;
        burst_start += 8000;
    }
    (err, count)
}

#[test]
fn block_switching_reduces_pre_echo() {
    if !have_ffmpeg() {
        return;
    }
    let (off, bursts) = pre_echo_error(false);
    let (on, _) = pre_echo_error(true);
    println!(
        "pre-echo error energy over {bursts} bursts: long blocks {off:.4}, switching {on:.4} ({:.1} dB better)",
        10.0 * (off / on).log10()
    );
    assert!(on < off * 0.8, "switching {on} vs long {off}");
}

/// Joint-stereo material (a shared signal plus a little independent content) coded at a low rate:
/// rematrixing must not lose to plain left/right and should win clearly.
fn joint_stereo_snr(remat: bool) -> f64 {
    joint_stereo_run(remat, 1.0, false)
}

fn joint_stereo_run(remat: bool, right_sign: f32, coupling: bool) -> f64 {
    let n: usize = 48_000 * 3;
    let common = signal(0, n, 0.5);
    let a = signal(1, n, 0.03);
    let b = signal(2, n, 0.03);
    let left: Vec<f32> = (0..n).map(|i| common[i] + a[i]).collect();
    let right: Vec<f32> = (0..n).map(|i| right_sign * common[i] + b[i]).collect();
    let mut enc = Encoder::new(Config {
        acmod: 2,
        lfe: false,
        sample_rate: 48_000,
        bitrate_kbps: 96,
        dialnorm: 31,
        dolby_surround: false,
        bandwidth_hz: None,
        eac3: false,
    })
    .unwrap();
    enc.set_rematrixing(remat);
    enc.set_coupling(coupling);
    let frames = (n + 256).div_ceil(SAMPLES_PER_FRAME);
    let mut bytes = Vec::new();
    for f in 0..frames {
        let l: Vec<f32> = (0..SAMPLES_PER_FRAME)
            .map(|i| left.get(f * SAMPLES_PER_FRAME + i).copied().unwrap_or(0.0))
            .collect();
        let r: Vec<f32> = (0..SAMPLES_PER_FRAME)
            .map(|i| right.get(f * SAMPLES_PER_FRAME + i).copied().unwrap_or(0.0))
            .collect();
        bytes.extend(enc.encode_frame(&[l.as_slice(), r.as_slice()]));
    }
    let dir = std::env::temp_dir().join(format!(
        "sw_ac3_js_{}_{}_{}_{}",
        std::process::id(),
        remat,
        right_sign,
        coupling
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.ac3");
    std::fs::write(&path, &bytes).unwrap();
    let out = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-err_detect",
            "crccheck+explode",
            "-i",
        ])
        .arg(&path)
        .args(["-f", "f32le", "-"])
        .output()
        .unwrap();
    assert!(
        out.status.success() && out.stderr.is_empty(),
        "decoder: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let dec: Vec<f32> = out
        .stdout
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    let (mut s, mut e) = (0.0, 0.0);
    for i in 2000..n - 2000 {
        for (c, x) in [&left, &right].iter().enumerate() {
            let d = dec[(i + 256) * 2 + c] as f64 - x[i] as f64;
            s += (x[i] as f64).powi(2);
            e += d * d;
        }
    }
    10.0 * (s / e).log10()
}

#[test]
fn rematrixing_helps_joint_stereo() {
    if !have_ffmpeg() {
        return;
    }
    let off = joint_stereo_snr(false);
    let on = joint_stereo_snr(true);
    println!("joint stereo at 96 kbps: L/R {off:.1} dB, rematrixed {on:.1} dB");
    assert!(on > off + 1.0, "{on} vs {off}");
}

/// Stereo coupling (2/0 with phase flags): an inverted right channel must survive it, which it only does
/// when the opposing bands are flagged; without flags the shared signal would cancel in the coupling channel.
#[test]
fn stereo_coupling_keeps_opposed_channels() {
    if !have_ffmpeg() {
        return;
    }
    let plain = joint_stereo_run(true, -1.0, false);
    let coupled = joint_stereo_run(true, -1.0, true);
    println!("opposed stereo at 96 kbps: plain {plain:.1} dB, coupled {coupled:.1} dB");
    assert!(coupled > plain - 1.5, "{coupled} vs {plain}");
}

/// 7.1 in E-AC-3: a 5.1 downmix substream plus a dependent substream with the true side and rear pairs.
/// Every source channel carries its own tone; a 7.1 decoder must return each one in its own output channel.
#[test]
fn eac3_seven_one_uses_a_dependent_substream() {
    if !have_ffmpeg() {
        return;
    }
    use sw_codec_ac3::native::{encode_to_file, Request};
    use sw_core::ChannelLayout;
    let layout = ChannelLayout::find("7.1-ls-rs-lsr-rsr").unwrap(); // L R C LFE Ls Rs Lsr Rsr
    let dir = std::env::temp_dir().join(format!("sw_eac3_71_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let n = 48_000 * 2;
    let freqs = [330.0f32, 440.0, 550.0, 60.0, 660.0, 770.0, 880.0, 990.0];
    let mut inputs = std::collections::HashMap::new();
    for (i, ch) in layout.channels.iter().enumerate() {
        let path = dir.join(format!("{}.wav", ch.short()));
        let mut data = Vec::with_capacity(n * 3);
        for k in 0..n {
            let v = (0.4 * (2.0 * std::f32::consts::PI * freqs[i] * k as f32 / 48_000.0).sin() * 8_388_607.0) as i32;
            data.extend_from_slice(&v.to_le_bytes()[..3]);
        }
        let mut f = Vec::new();
        f.extend_from_slice(b"RIFF");
        f.extend_from_slice(&((36 + data.len()) as u32).to_le_bytes());
        f.extend_from_slice(b"WAVEfmt ");
        f.extend_from_slice(&16u32.to_le_bytes());
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&48_000u32.to_le_bytes());
        f.extend_from_slice(&(48_000u32 * 3).to_le_bytes());
        f.extend_from_slice(&3u16.to_le_bytes());
        f.extend_from_slice(&24u16.to_le_bytes());
        f.extend_from_slice(b"data");
        f.extend_from_slice(&(data.len() as u32).to_le_bytes());
        f.extend_from_slice(&data);
        std::fs::write(&path, f).unwrap();
        inputs.insert(*ch, path);
    }
    let mut stems = sw_pcm::StemSet::open(layout, &inputs).unwrap();
    let out = dir.join("t.eac3");
    let req = Request {
        layout,
        bitrate_kbps: 768,
        dialnorm: 31,
        start_secs: 0.0,
        duration_secs: None,
        output: &out,
        eac3: true,
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
    assert_eq!(y.len() % 8, 0);
    let frames = y.len() / 8;
    assert!(frames >= n);
    // FFmpeg's channel order for 7.1: FL FR FC LFE BL BR SL SR; our source order is L R C LFE Ls Rs Lsr Rsr
    let expected_out = [0usize, 1, 2, 3, 6, 7, 4, 5];
    for (src, &outc) in expected_out.iter().enumerate() {
        // amplitude of this source's tone in every output channel
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
        assert!(
            (here - 0.4).abs() < 0.08,
            "source {src} came out at {here:.3} in channel {outc}"
        );
        assert!(
            elsewhere < 0.06,
            "source {src} leaks {elsewhere:.3} into another channel"
        );
    }
}
