//! Measuring the loudness of input files the way an encode would read them.

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::AtomicBool;
use sw_core::{Channel, ChannelLayout};
use sw_queue::measure::{loudness, Measure};

/// A mono 16-bit WAV file holding a 997 Hz sine of `db` dBFS peak.
fn write_sine(path: &std::path::Path, db: f64, secs: f64) {
    let rate = 48_000u32;
    let amp = 10f64.powf(db / 20.0) * 32767.0;
    let n = (rate as f64 * secs) as usize;
    let mut data = Vec::with_capacity(n * 2);
    for i in 0..n {
        let v = (amp * (2.0 * std::f64::consts::PI * 997.0 * i as f64 / rate as f64).sin()).round() as i16;
        data.extend_from_slice(&v.to_le_bytes());
    }
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(b"RIFF").unwrap();
    f.write_all(&((36 + data.len()) as u32).to_le_bytes()).unwrap();
    f.write_all(b"WAVEfmt ").unwrap();
    f.write_all(&16u32.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap();
    f.write_all(&rate.to_le_bytes()).unwrap();
    f.write_all(&(rate * 2).to_le_bytes()).unwrap();
    f.write_all(&2u16.to_le_bytes()).unwrap();
    f.write_all(&16u16.to_le_bytes()).unwrap();
    f.write_all(b"data").unwrap();
    f.write_all(&(data.len() as u32).to_le_bytes()).unwrap();
    f.write_all(&data).unwrap();
}

#[test]
fn a_mono_file_reads_its_level_and_the_span_limits_it() {
    let dir = std::env::temp_dir().join(format!("sw_measure_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tone.wav");
    write_sine(&path, -20.0, 8.0);
    let inputs: HashMap<Channel, std::path::PathBuf> = [(Channel::C, path)].into();
    let layout = ChannelLayout::find("1.0").unwrap();
    let request = |duration| Measure {
        layout,
        inputs: &inputs,
        order: sw_pcm::ChannelOrder::Auto,
        start: Some(1.0),
        duration,
    };
    let cancel = AtomicBool::new(false);
    let l = loudness(&request(None), &cancel, &mut |_| {}).unwrap();
    // a mono sine at -20 dBFS peak is -23.01 LUFS; one second in, seven seconds are left
    assert!((l.integrated.unwrap() + 23.01).abs() < 0.1, "{:?}", l.integrated);
    assert!((l.seconds - 7.0).abs() < 0.01);
    assert_eq!(l.dialnorm(), Some(23));
    let short = loudness(&request(Some(2.0)), &cancel, &mut |_| {}).unwrap();
    assert!((short.seconds - 2.0).abs() < 0.01);
}
