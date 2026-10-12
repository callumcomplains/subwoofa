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

#[test]
fn unknown_data_is_reported() {
    let r = verify(&write("junk.bin", &[0x12; 4096]), &Options::default()).unwrap();
    assert!(!r.passed);
}
