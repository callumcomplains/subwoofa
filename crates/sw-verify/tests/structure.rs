//! Streams from the encoders must verify; a damaged copy must not.

use std::path::PathBuf;
use sw_codec_ac3::{Config, Encoder, SAMPLES_PER_FRAME};
use sw_verify::{verify, Options, Status};

fn stream(eac3: bool, frames: usize) -> Vec<u8> {
    let cfg = Config {
        acmod: 7,
        lfe: true,
        sample_rate: 48_000,
        bitrate_kbps: 384,
        dialnorm: 27,
        dolby_surround: false,
        bandwidth_hz: None,
        eac3,
    };
    let mut enc = Encoder::new(cfg).unwrap();
    let mut out = Vec::new();
    for f in 0..frames {
        let chans: Vec<Vec<f32>> = (0..6)
            .map(|c| {
                (0..SAMPLES_PER_FRAME)
                    .map(|i| (((f * SAMPLES_PER_FRAME + i) as f32) * (0.01 + 0.002 * c as f32)).sin() * 0.3)
                    .collect()
            })
            .collect();
        let refs: Vec<&[f32]> = chans.iter().map(|c| c.as_slice()).collect();
        out.extend(enc.encode_frame(&refs));
    }
    out
}

fn write(name: &str, data: &[u8]) -> PathBuf {
    let p = std::env::temp_dir().join(format!("sw_verify_{}_{}", std::process::id(), name));
    std::fs::write(&p, data).unwrap();
    p
}

#[test]
fn encoder_streams_pass_and_damaged_ones_fail() {
    for (eac3, name) in [(false, "a.ac3"), (true, "b.eac3")] {
        let good = stream(eac3, 20);
        let path = write(name, &good);
        let r = verify(&path, &Options::default()).unwrap();
        assert!(r.passed, "{name}: {}", r.to_text());
        assert!(r.checks.iter().any(|c| c.status == Status::Pass));
        // flip one bit in the middle of the third frame: the checksum must catch it
        let mut bad = good.clone();
        bad[good.len() / 20 * 3 + 100] ^= 0x04;
        let r = verify(&write(&format!("bad_{name}"), &bad), &Options::default()).unwrap();
        assert!(!r.passed, "{name}: damage went unnoticed");
        // cut the last frame short
        let r = verify(
            &write(&format!("cut_{name}"), &good[..good.len() - 100]),
            &Options::default(),
        )
        .unwrap();
        assert!(!r.passed, "{name}: truncation went unnoticed");
    }
}

/// An AC-3 frame at 640 kbps and 48 kHz: 1536 samples, 2560 bytes.
const CORE_FRAME_BYTES: usize = 2560;

/// Blu-ray's Dolby Digital Plus 7.1: every AC-3 5.1 frame is followed by an E-AC-3 dependent substream frame with the
/// four surround channels.
fn ac3_with_extension(frames: usize) -> Vec<u8> {
    let config = |acmod, lfe, kbps, eac3| Config {
        acmod,
        lfe,
        sample_rate: 48_000,
        bitrate_kbps: kbps,
        dialnorm: 27,
        dolby_surround: false,
        bandwidth_hz: None,
        eac3,
    };
    let mut core = Encoder::new(config(7, true, 640, false)).unwrap();
    let mut extension = Encoder::new(config(6, false, 384, true)).unwrap();
    // Ls, Rs and the Lrs/Rrs pair (custom channel map), as the native 7.1 E-AC-3 encoder writes them
    extension.set_dependent((1 << 12) | (1 << 11) | (1 << 9));
    let tone = |f: usize, c: usize| -> Vec<f32> {
        (0..SAMPLES_PER_FRAME)
            .map(|i| (((f * SAMPLES_PER_FRAME + i) as f32) * (0.01 + 0.002 * c as f32)).sin() * 0.3)
            .collect()
    };
    let mut out = Vec::new();
    for f in 0..frames {
        let six: Vec<Vec<f32>> = (0..6).map(|c| tone(f, c)).collect();
        let four: Vec<Vec<f32>> = (6..10).map(|c| tone(f, c)).collect();
        out.extend(core.encode_frame(&six.iter().map(|c| c.as_slice()).collect::<Vec<_>>()));
        out.extend(extension.encode_frame(&four.iter().map(|c| c.as_slice()).collect::<Vec<_>>()));
    }
    out
}

#[test]
fn ac3_core_with_extension() {
    let good = ac3_with_extension(20);
    let r = verify(&write("bd.ac3", &good), &Options::default()).unwrap();
    assert!(r.passed, "{}", r.to_text());
    assert!(r
        .summary
        .iter()
        .any(|(k, v)| k == "Structure" && v.contains("dependent substream")));
    assert!(r.checks.iter().all(|c| c.status != Status::Warn), "{}", r.to_text());
    // damage in the first extension frame (right after the first core frame) is still caught
    let mut bad = good.clone();
    bad[CORE_FRAME_BYTES + 50] ^= 0x10;
    let r = verify(&write("bd_bad.ac3", &bad), &Options::default()).unwrap();
    assert!(!r.passed, "damage in the extension went unnoticed");
}

#[test]
fn unknown_data_is_reported() {
    let r = verify(&write("junk.bin", &[0x12; 4096]), &Options::default()).unwrap();
    assert!(!r.passed);
}

/// The table of contents of a real DTS-UHD sync frame (the first 11 bytes of a DTS:X Profile 2 sample): a 48 kHz
/// channel-based mix with 1 024-sample frames, ending in its CRC-16.
const UHD_SYNC_FTOC: [u8; 11] = [0x40, 0x41, 0x1B, 0xF2, 0x2A, 0x18, 0x07, 0x25, 0x10, 0x2E, 0xCC];

fn summary<'a>(r: &'a sw_verify::Report, key: &str) -> &'a str {
    r.summary.iter().find(|(k, _)| k == key).map_or("", |(_, v)| v.as_str())
}

#[test]
fn dts_uhd_recognised() {
    // a sync frame and two frames after it (their own sync word, then filler for the audio)
    let mut stream = UHD_SYNC_FTOC.to_vec();
    stream.extend([0u8; 40]);
    for _ in 0..2 {
        stream.extend([0x71, 0xC4, 0x42, 0xE8]);
        stream.extend([0u8; 40]);
    }
    let r = verify(&write("a.uhd", &stream), &Options::default()).unwrap();
    assert!(r.passed, "{}", r.to_text());
    assert!(summary(&r, "Frames").starts_with("3 (1 sync"), "{}", r.to_text());
    assert_eq!(summary(&r, "Frame length"), "1024 samples");
    assert!(summary(&r, "Sample rate").starts_with("48000 Hz"));
    // a damaged table of contents is no sync frame at all
    stream[6] ^= 0x01;
    let r = verify(&write("bad.uhd", &stream), &Options::default()).unwrap();
    assert!(!r.passed);
}

/// AC-3's CRC-16 (x^16 + x^15 + x^2 + 1, from zero), which AC-4 sync frames use too.
fn crc_8005(data: &[u8]) -> u16 {
    let mut c = 0u16;
    for &b in data {
        c ^= (b as u16) << 8;
        for _ in 0..8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ 0x8005 } else { c << 1 };
        }
    }
    c
}

/// AC-4 sync frames with a CRC: version 2, 48 kHz, 25 frames a second, one presentation, every frame an I-frame.
fn ac4_stream(frames: usize) -> Vec<u8> {
    // ac4_toc: version 10, sequence counter (10 bits), b_wait_frames 0, fs_index 1, frame_rate_index 0010,
    // b_iframe_global 1, b_single_presentation 1, then zero padding standing in for the rest of the frame
    let toc: u32 = 0b10 << 30 | 1 << 18 | 0b0010 << 14 | 1 << 13 | 1 << 12;
    let mut out = Vec::new();
    for _ in 0..frames {
        let mut frame = vec![0xAC, 0x41, 0x00, 0x20];
        frame.extend(toc.to_be_bytes());
        frame.extend([0u8; 28]);
        let crc = crc_8005(&frame[2..]);
        frame.extend(crc.to_be_bytes());
        out.extend(frame);
    }
    out
}

#[test]
fn ac4_recognised() {
    let good = ac4_stream(50);
    let r = verify(&write("a.ac4", &good), &Options::default()).unwrap();
    assert!(r.passed, "{}", r.to_text());
    assert_eq!(summary(&r, "Frame rate"), "25 frames/s");
    assert_eq!(summary(&r, "Duration"), "2.000 s");
    let mut bad = good.clone();
    bad[100] ^= 0x08;
    assert!(!verify(&write("bad.ac4", &bad), &Options::default()).unwrap().passed);
    assert!(
        !verify(&write("cut.ac4", &good[..good.len() - 7]), &Options::default())
            .unwrap()
            .passed
    );
}
