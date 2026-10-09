//! DTS-HD High Resolution: the XBR extension must improve on the core when decoded.
use std::process::Command;
use sw_codec_dts::exss::{dtshd_header, DtsHdHeader, ExssParams};
use sw_codec_dts::hra::HraStage;
use sw_codec_dts::{Config, Encoder};

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn snr(dec: &[f64], nout: usize, ch: usize, src: &[f64], lag: usize) -> f64 {
    let x: Vec<f64> = dec.iter().skip(ch).step_by(nout).map(|v| v * 8_388_608.0).collect();
    let (mut s, mut e) = (0.0, 0.0);
    for i in 4000..src.len() - 4000 {
        let y = x[i + lag];
        s += src[i] * src[i];
        e += (y - src[i]).powi(2);
    }
    10.0 * (s / e).log10()
}

#[test]
fn xbr_improves_on_the_core() {
    if !have_ffmpeg() {
        return;
    }
    let (channels, bitrate) = (2usize, 1_509_000u32);
    let mut enc = Encoder::new(Config { sample_rate: 48_000, amode: 2, channels, lfe: false, bitrate, dialnorm: 0, pcm_bits: 24, es: false, xch: false, perfect: true, blocks: 16 }).unwrap();
    enc.keep_residual = true;
    let n = enc.frame_samples();
    let exss = ExssParams { sample_rate: 48_000, channels, bit_res: 24, spk_mask: 0x2, with_core: true, dialnorm: 0, frame_duration_code: 0, x96_in_core: false };
    let mut stage = HraStage::new(2500, 48_000, n, 16, exss);
    let frames = 60;
    let total = n * frames;
    let mut seed = 5u32;
    let sig: Vec<Vec<f64>> = (0..channels)
        .map(|c| {
            (0..total)
                .map(|i| {
                    seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    let noise = ((seed >> 12) as f64 - 524288.0) / 524288.0;
                    let t = i as f64 / 48_000.0;
                    (2.0 * std::f64::consts::PI * (330.0 + 410.0 * c as f64) * t).sin() * 1_500_000.0 + noise * 600_000.0
                })
                .collect()
        })
        .collect();
    let mut stream = Vec::new();
    for f in 0..frames {
        let part: Vec<Vec<f64>> = sig.iter().map(|s| s[f * n..(f + 1) * n].to_vec()).collect();
        let core = enc.encode_frame(&part);
        stream.extend(stage.wrap(core, &enc.last_residual));
    }
    let mut file = dtshd_header(&DtsHdHeader { sample_rate: 48_000, frames: frames as u32, samples_per_frame: n as u16, total_samples: total as u64, ch_mask: 0x2, initial_padding: 0 }, stream.len() as u64);
    file.extend_from_slice(&stream);
    let dir = std::env::temp_dir().join(format!("sw_dts_hra_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.dtshd");
    std::fs::write(&path, &file).unwrap();
    let decode = |core_only: bool| -> Vec<f64> {
        let mut c = Command::new("ffmpeg");
        c.args(["-hide_banner", "-loglevel", "error"]);
        if core_only {
            c.args(["-core_only", "1"]);
        }
        let out = c.arg("-i").arg(&path).args(["-f", "f64le", "-"]).output().unwrap();
        assert!(out.status.success() && out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
        out.stdout.chunks_exact(8).map(|b| f64::from_le_bytes(b.try_into().unwrap())).collect()
    };
    let (core, full) = (decode(true), decode(false));
    let lag = enc.delay();
    for ch in 0..channels {
        let a = snr(&core, channels, ch, &sig[ch], lag);
        let b = snr(&full, channels, ch, &sig[ch], lag);
        println!("channel {ch}: core {a:.1} dB, with XBR {b:.1} dB");
        assert!(b > a + 10.0, "XBR gained only {:.1} dB", b - a);
    }
    // constant total rate: about 2500 kbit/s
    let kbps = stream.len() as f64 * 8.0 / (total as f64 / 48_000.0) / 1000.0;
    assert!((kbps - 2500.0).abs() < 25.0, "{kbps} kbit/s");
}
