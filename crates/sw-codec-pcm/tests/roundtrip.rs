//! Every container and sample format must decode (through FFmpeg) to exactly the samples that went in.
use std::collections::HashMap;
use std::process::Command;
use sw_codec_pcm::{channels, encode_to_file, Container, Request, SampleFormat};
use sw_core::ChannelLayout;

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// Test signal for channel `k`: a tone plus noise. Channel 1 is silent and channel 2 has its low eight bits clear,
/// so the "constant" and "wasted bits" paths of FLAC are exercised too.
fn samples(k: usize, frames: usize) -> Vec<i32> {
    let mut seed = (k as u32 + 1).wrapping_mul(2654435761);
    (0..frames)
        .map(|i| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let noise = ((seed >> 12) as i32 & 0xFFFF) - 32768;
            let tone = ((i as f64 * (0.01 + 0.003 * k as f64)).sin() * 3_000_000.0) as i32;
            match k {
                1 => 0,
                2 => (tone + noise) & !0xFF,
                _ => tone + noise,
            }
        })
        .collect()
}

fn write_wav(path: &std::path::Path, rate: u32, data: &[i32]) {
    let mut bytes = Vec::with_capacity(data.len() * 3);
    for v in data {
        bytes.extend_from_slice(&v.to_le_bytes()[..3]);
    }
    let mut f = Vec::new();
    f.extend_from_slice(b"RIFF");
    f.extend_from_slice(&((36 + bytes.len()) as u32).to_le_bytes());
    f.extend_from_slice(b"WAVEfmt ");
    f.extend_from_slice(&16u32.to_le_bytes());
    f.extend_from_slice(&1u16.to_le_bytes());
    f.extend_from_slice(&1u16.to_le_bytes());
    f.extend_from_slice(&rate.to_le_bytes());
    f.extend_from_slice(&(rate * 3).to_le_bytes());
    f.extend_from_slice(&3u16.to_le_bytes());
    f.extend_from_slice(&24u16.to_le_bytes());
    f.extend_from_slice(b"data");
    f.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    f.extend_from_slice(&bytes);
    std::fs::write(path, f).unwrap();
}

/// What FFmpeg is asked to decode to, and how to turn the 24-bit source into the expected value.
fn decode_format(format: SampleFormat) -> &'static str {
    match format {
        SampleFormat::Int16 => "s16le",
        SampleFormat::Int24 => "s24le",
        SampleFormat::Int32 => "s32le",
        SampleFormat::Float32 => "f32le",
        SampleFormat::Float64 => "f64le",
    }
}

fn check(name: &str, container: Container, ext: &str, format: SampleFormat, layout_id: &str, rate: u32, frames: usize) {
    if !have_ffmpeg() {
        return;
    }
    let layout = ChannelLayout::find(layout_id).unwrap();
    let dir = std::env::temp_dir().join(format!("sw_pcm_{}_{}", std::process::id(), name));
    std::fs::create_dir_all(&dir).unwrap();
    let sources: Vec<Vec<i32>> = (0..layout.channels.len()).map(|k| samples(k, frames)).collect();
    let mut inputs = HashMap::new();
    for (k, ch) in layout.channels.iter().enumerate() {
        let path = dir.join(format!("{}.wav", ch.short()));
        write_wav(&path, rate, &sources[k]);
        inputs.insert(*ch, path);
    }
    let mut stems = sw_pcm::StemSet::open(layout, &inputs).unwrap();
    let out = dir.join(format!("out.{ext}"));
    let req = Request { layout, container, format, start_secs: 0.0, duration_secs: None, output: &out, dsd_multiplier: 0 };
    let stats = encode_to_file(&mut stems, &req, &mut |_| {}, &std::sync::atomic::AtomicBool::new(false)).unwrap();
    assert_eq!(stats.frames, frames as u64, "{name}: frames written");
    assert_eq!(stats.bytes, std::fs::metadata(&out).unwrap().len(), "{name}: reported size");

    let dec = Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-err_detect", "crccheck+explode", "-i"]).arg(&out).args(["-f", decode_format(format), "-"]).output().unwrap();
    assert!(dec.status.success() && dec.stderr.is_empty(), "{name}: decoder: {}", String::from_utf8_lossy(&dec.stderr));

    // file order of the channels
    let order: Vec<usize> = match container {
        Container::Wav | Container::Flac | Container::OggFlac | Container::Alac | Container::Tta | Container::WavPack | Container::MonkeysAudio => channels::wav_order(layout).order,
        _ => (0..layout.channels.len()).collect(),
    };
    let width = format.bytes();
    let nch = order.len();
    assert_eq!(dec.stdout.len(), frames * nch * width, "{name}: decoded length");
    let mut wrong = 0usize;
    for i in 0..frames {
        for (file_ch, &layout_ch) in order.iter().enumerate() {
            let at = (i * nch + file_ch) * width;
            let got = &dec.stdout[at..at + width];
            let src = sources[layout_ch][i];
            let want: Vec<u8> = match format {
                SampleFormat::Int16 => ((src as f64 / 8_388_608.0 * 32768.0).round().clamp(-32768.0, 32767.0) as i16).to_le_bytes().to_vec(),
                SampleFormat::Int24 => src.to_le_bytes()[..3].to_vec(),
                SampleFormat::Int32 => (src << 8).to_le_bytes().to_vec(),
                SampleFormat::Float32 => (src as f32 / 8_388_608.0).to_le_bytes().to_vec(),
                SampleFormat::Float64 => (src as f64 / 8_388_608.0).to_le_bytes().to_vec(),
            };
            if got != want.as_slice() {
                wrong += 1;
                if wrong < 4 {
                    eprintln!("{name}: frame {i} file channel {file_ch}: got {got:?} want {want:?}");
                }
            }
        }
    }
    assert_eq!(wrong, 0, "{name}: {wrong} samples differ");
    let _ = std::fs::remove_dir_all(&dir);
}

use Container::*;
use SampleFormat::*;

#[test]
fn wav_16_bit_stereo() {
    check("wav16", Wav, "wav", Int16, "2.0", 48_000, 50_000);
}

#[test]
fn wav_24_bit_five_one() {
    check("wav24", Wav, "wav", Int24, "5.1", 48_000, 50_000);
}

#[test]
fn wav_32_bit_seven_one_at_96k() {
    check("wav32", Wav, "wav", Int32, "7.1-side-rear", 96_000, 30_000);
}

#[test]
fn wav_float32_five_one() {
    check("wavf32", Wav, "wav", Float32, "5.1", 48_000, 40_000);
}

#[test]
fn wav_float64_stereo() {
    check("wavf64", Wav, "wav", Float64, "2.0", 192_000, 30_000);
}

#[test]
fn wav_immersive_layout_without_a_mask() {
    check("wav714", Wav, "wav", Int24, "7.1.4", 48_000, 20_000);
}

#[test]
fn aiff_24_bit_five_one() {
    check("aiff24", Aiff, "aif", Int24, "5.1", 48_000, 50_000);
}

#[test]
fn aiff_float32_stereo() {
    check("aifff32", Aiff, "aif", Float32, "2.0", 96_000, 30_000);
}

#[test]
fn caf_24_bit_five_one() {
    check("caf24", Caf, "caf", Int24, "5.1", 48_000, 50_000);
}

#[test]
fn caf_float32_seven_one() {
    check("caff32", Caf, "caf", Float32, "7.1-side-rear", 48_000, 30_000);
}

#[test]
fn flac_16_bit_stereo() {
    check("flac16", Flac, "flac", Int16, "2.0", 44_100, 100_001);
}

#[test]
fn flac_24_bit_five_one_at_96k() {
    check("flac24", Flac, "flac", Int24, "5.1", 96_000, 70_000);
}

#[test]
fn flac_24_bit_seven_one() {
    check("flac71", Flac, "flac", Int24, "7.1-side-rear", 48_000, 30_000);
}

#[test]
fn flac_shorter_than_one_block() {
    check("flacshort", Flac, "flac", Int24, "2.0", 48_000, 1_000);
}

#[test]
fn flac_refuses_what_it_cannot_hold() {
    let layout = ChannelLayout::find("5.1").unwrap();
    let dir = std::env::temp_dir().join(format!("sw_pcm_refuse_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut inputs = HashMap::new();
    for (k, ch) in layout.channels.iter().enumerate() {
        let path = dir.join(format!("{}.wav", ch.short()));
        write_wav(&path, 48_000, &samples(k, 1000));
        inputs.insert(*ch, path);
    }
    let mut stems = sw_pcm::StemSet::open(layout, &inputs).unwrap();
    let out = dir.join("x.flac");
    let req = Request { layout, container: Flac, format: Float32, start_secs: 0.0, duration_secs: None, output: &out, dsd_multiplier: 0 };
    assert!(encode_to_file(&mut stems, &req, &mut |_| {}, &std::sync::atomic::AtomicBool::new(false)).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ogg_flac_16_bit_stereo() {
    check("ogg16", OggFlac, "oga", Int16, "2.0", 44_100, 100_001);
}

#[test]
fn ogg_flac_24_bit_seven_one_at_96k() {
    // 8 channels of noisy 24-bit audio make frames larger than one Ogg page can hold
    check("ogg71", OggFlac, "oga", Int24, "7.1-side-rear", 96_000, 70_000);
}

#[test]
fn alac_16_bit_stereo() {
    check("alac16", Alac, "m4a", Int16, "2.0", 44_100, 100_001);
}

#[test]
fn alac_24_bit_stereo() {
    check("alac24", Alac, "m4a", Int24, "2.0", 96_000, 70_000);
}

#[test]
fn alac_mono() {
    check("alacmono", Alac, "m4a", Int24, "1.0", 48_000, 20_000);
}

#[test]
fn alac_24_bit_five_one() {
    check("alac51", Alac, "m4a", Int24, "5.1", 48_000, 50_000);
}

#[test]
fn alac_16_bit_seven_one_wide() {
    check("alac71", Alac, "m4a", Int16, "7.1-wide", 48_000, 30_000);
}

#[test]
fn alac_shorter_than_one_frame() {
    check("alacshort", Alac, "m4a", Int24, "2.0", 48_000, 1_000);
}

#[test]
fn tta_16_bit_stereo() {
    check("tta16", Tta, "tta", Int16, "2.0", 44_100, 100_001);
}

#[test]
fn tta_24_bit_five_one() {
    check("tta51", Tta, "tta", Int24, "5.1", 48_000, 120_000);
}

#[test]
fn tta_mono_shorter_than_a_frame() {
    check("ttashort", Tta, "tta", Int24, "1.0", 48_000, 1_000);
}

#[test]
fn wavpack_16_bit_stereo() {
    check("wv16", WavPack, "wv", Int16, "2.0", 44_100, 100_001);
}

#[test]
fn wavpack_24_bit_five_one() {
    check("wv51", WavPack, "wv", Int24, "5.1", 48_000, 120_000);
}

#[test]
fn wavpack_mono_short() {
    check("wvshort", WavPack, "wv", Int24, "1.0", 48_000, 1_000);
}

#[test]
fn monkeys_16_bit_stereo() {
    check("ape16", MonkeysAudio, "ape", Int16, "2.0", 44_100, 100_001);
}

#[test]
fn monkeys_24_bit_stereo() {
    check("ape24", MonkeysAudio, "ape", Int24, "2.0", 48_000, 120_000);
}

#[test]
fn monkeys_mono_short() {
    check("apeshort", MonkeysAudio, "ape", Int24, "1.0", 48_000, 1_000);
}

/// DSD cannot be compared sample for sample: encode a different tone on every channel and check that FFmpeg's
/// decode of the file has each tone, at half the level (the modulator runs at half scale), in the right channel only.
fn check_dsd(name: &str, container: Container, ext: &str, layout_id: &str, rate: u32, multiplier: u32) {
    if !have_ffmpeg() {
        return;
    }
    let layout = ChannelLayout::find(layout_id).unwrap();
    let dir = std::env::temp_dir().join(format!("sw_dsd_{}_{}", std::process::id(), name));
    std::fs::create_dir_all(&dir).unwrap();
    let seconds = 3usize;
    let frames = rate as usize * seconds;
    let nch = layout.channels.len();
    let freqs: Vec<f64> = (0..nch).map(|k| 700.0 + 1300.0 * k as f64).collect();
    let mut inputs = HashMap::new();
    for (k, ch) in layout.channels.iter().enumerate() {
        let tone: Vec<i32> = (0..frames).map(|i| ((2.0 * std::f64::consts::PI * freqs[k] * i as f64 / rate as f64).sin() * 0.8 * 8_388_607.0) as i32).collect();
        let path = dir.join(format!("{}.wav", ch.short()));
        write_wav(&path, rate, &tone);
        inputs.insert(*ch, path);
    }
    let mut stems = sw_pcm::StemSet::open(layout, &inputs).unwrap();
    let out = dir.join(format!("out.{ext}"));
    let req = Request { layout, container, format: Int24, start_secs: 0.0, duration_secs: None, output: &out, dsd_multiplier: multiplier };
    encode_to_file(&mut stems, &req, &mut |_| {}, &std::sync::atomic::AtomicBool::new(false)).unwrap();
    let dec = Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-err_detect", "crccheck+explode", "-i"]).arg(&out).args(["-f", "f32le", "-ar", "352800", "-"]).output().unwrap();
    assert!(dec.status.success() && dec.stderr.is_empty(), "{name}: decoder: {}", String::from_utf8_lossy(&dec.stderr));
    let y: Vec<f32> = dec.stdout.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    let n = y.len() / nch;
    assert!(n >= 352_800 * seconds - 400, "{name}: decoded {n} samples per channel");
    // the file's channels are in the order of the layout where it has the standard DSD arrangement (L R C LFE Ls Rs)
    let file_order = layout_id_to_file_order(layout_id);
    for (src, &file_ch) in file_order.iter().enumerate() {
        let amp = |c: usize, f: f64| -> f64 {
            let (mut sc, mut ss) = (0.0f64, 0.0f64);
            let (from, to) = (352_800 / 2, n - 352_800 / 2);
            for i in from..to {
                let a = 2.0 * std::f64::consts::PI * f * i as f64 / 352_800.0;
                sc += y[i * nch + c] as f64 * a.cos();
                ss += y[i * nch + c] as f64 * a.sin();
            }
            2.0 * (sc * sc + ss * ss).sqrt() / (to - from) as f64
        };
        let here = amp(file_ch, freqs[src]);
        assert!((here - 0.4).abs() < 0.01, "{name}: tone {src} came out at {here:.3} in file channel {file_ch}");
        for other in (0..nch).filter(|&c| c != file_ch) {
            let leak = amp(other, freqs[src]);
            assert!(leak < 0.002, "{name}: tone {src} leaks {leak:.4} into file channel {other}");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// For each layout channel, the channel of the decoded file it lands in (the arrangements used by the DSD formats).
fn layout_id_to_file_order(layout_id: &str) -> Vec<usize> {
    let layout = ChannelLayout::find(layout_id).unwrap();
    let d = sw_codec_pcm::dsd_order(layout).unwrap();
    // `d[k]` = layout channel placed at file channel k; invert it
    let mut inverse = vec![0usize; d.len()];
    for (file_ch, &layout_ch) in d.iter().enumerate() {
        inverse[layout_ch] = file_ch;
    }
    inverse
}

#[test]
fn dsd64_stereo_dsf_from_44_1k() {
    check_dsd("dsf441", Dsf, "dsf", "2.0", 44_100, 64);
}

#[test]
fn dsd64_stereo_dff_from_48k() {
    check_dsd("dff48", Dff, "dff", "2.0", 48_000, 64);
}

#[test]
fn dsd128_five_one_dsf_from_96k() {
    check_dsd("dsf51", Dsf, "dsf", "5.1", 96_000, 128);
}

#[test]
fn dsd64_five_one_dff_from_192k() {
    check_dsd("dff51", Dff, "dff", "5.1", 192_000, 64);
}

#[test]
fn dsd_three_zero_is_reordered() {
    // layout 3.0 is L C R; DSD wants L R C
    check_dsd("dsf30", Dsf, "dsf", "3.0", 44_100, 64);
}
