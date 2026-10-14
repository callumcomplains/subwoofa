//! Runs the real Dolby Encoding Engine when one is installed (skipped otherwise): a synthetic 7.1.4 bed becomes
//! Dolby Digital Plus Atmos, which Subwoofa's own verifier must recognise as Atmos.

use std::sync::atomic::AtomicBool;
use sw_backend_dee::{find_dee, run, AtmosJob, DdpOptions, Target};

/// A 12-channel 24-bit WAV of `secs` seconds of different tones per channel.
fn bed_wav(path: &std::path::Path, secs: usize) {
    let (rate, ch) = (48_000usize, 12usize);
    let n = rate * secs;
    let mut data = Vec::with_capacity(n * ch * 3);
    for i in 0..n {
        for c in 0..ch {
            let v = ((i as f64 * (0.02 + 0.007 * c as f64)).sin() * 0.25 * 8_388_607.0) as i32;
            data.extend_from_slice(&v.to_le_bytes()[..3]);
        }
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(ch as u16).to_le_bytes());
    out.extend_from_slice(&(rate as u32).to_le_bytes());
    out.extend_from_slice(&((rate * ch * 3) as u32).to_le_bytes());
    out.extend_from_slice(&((ch * 3) as u16).to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    std::fs::write(path, out).unwrap();
}

#[test]
fn channel_bed_becomes_dolby_digital_plus_atmos() {
    let Some(dee) = find_dee(None) else {
        eprintln!("no Dolby Encoding Engine here, skipping");
        return;
    };
    let dir = std::env::temp_dir().join(format!("sw_dee_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let wav = dir.join("bed.wav");
    bed_wav(&wav, 4);
    // the optional settings go in too, so the engine itself checks that it accepts them
    let options = DdpOptions {
        surround_trim_5_1: "-3".into(),
        height_trim_5_1: "-9".into(),
        ..Default::default()
    };
    let mut job = AtmosJob::new(&wav, dir.join("bed.ec3"), Target::Ddp(options));
    job.metering = Some("1770-3".into());
    job.dialogue_intelligence = Some(true);
    job.speech_threshold = Some(30);
    let mut last = 0.0;
    let done = run(
        &dee,
        &job,
        &dir.join("work"),
        &mut |p| last = p,
        &mut |_| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(last > 0.99, "progress reached {last}");
    let r = sw_verify::verify(&done.output, &sw_verify::Options::default()).unwrap();
    assert!(r.passed, "{}", r.to_text());
    assert!(
        r.summary
            .iter()
            .any(|(k, v)| k == "Dolby Atmos" && v.starts_with("yes")),
        "{}",
        r.to_text()
    );
    // the licence's feature list must not reach the lines we pass on
    assert!(!done.messages.iter().any(|m| m.contains("Features enabled")));
}
