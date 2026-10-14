//! Runs the engine's everyday jobs for real when a Dolby Encoding Engine is installed (skipped otherwise). The folders
//! have spaces in their names on purpose: the engine cannot read such paths, so this checks the staging as well.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use sw_backend_dee::{
    find_dee, run_task, work_folder, EncodeOptions, Loudness, Measured, Source, Task, TaskDone, TaskJob,
};

const CHANNELS: [(&str, &str); 6] = [
    ("L", "L"),
    ("R", "R"),
    ("C", "C"),
    ("LFE", "LFE"),
    ("LS", "Ls"),
    ("RS", "Rs"),
];

/// A mono 24-bit WAV of `secs` seconds: a tone and some noise, different for every `seed`.
fn mono_wav(path: &Path, seed: u32, secs: usize) {
    let rate = 48_000usize;
    let mut data = Vec::new();
    let mut x = seed.wrapping_mul(2654435761).wrapping_add(7);
    for i in 0..rate * secs {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        let t = i as f64 / rate as f64;
        let v = (t * (220.0 + 110.0 * seed as f64) * std::f64::consts::TAU).sin() * 1_600_000.0
            + ((x >> 8) as i32 - (1 << 23)) as f64 / 60.0;
        data.extend_from_slice(&(v as i32).to_le_bytes()[..3]);
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    for v in [16u32, 0x0001_0001, rate as u32, rate as u32 * 3, 0x0018_0003] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    std::fs::write(path, out).unwrap();
}

struct Bench {
    dee: PathBuf,
    dir: PathBuf,
    work: PathBuf,
}

fn bench(name: &str) -> Option<Bench> {
    let Some(dee) = find_dee(None) else {
        eprintln!("no Dolby Encoding Engine here, skipping");
        return None;
    };
    let dir = std::env::temp_dir().join(format!("sw dee tasks {name} {}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let work = work_folder().unwrap().join(format!("{name}{}", std::process::id()));
    Some(Bench { dee, dir, work })
}

impl Bench {
    fn stems(&self, secs: usize) -> Source {
        let mut files = Vec::new();
        for (i, (engine, file)) in CHANNELS.iter().enumerate() {
            let path = self.dir.join(format!("stem {file}.wav"));
            mono_wav(&path, i as u32 + 1, secs);
            files.push((engine.to_string(), path));
        }
        Source::WavList {
            layout: "5.1".into(),
            files,
        }
    }

    fn run(&self, source: Source, output: &str, task: Task) -> TaskDone {
        let job = TaskJob {
            source,
            output: self.dir.join(output),
            task,
            start: None,
            end: None,
        };
        let mut last = 0.0;
        let done = run_task(
            &self.dee,
            &job,
            &self.work,
            &mut |p| last = p,
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap_or_else(|e| panic!("{output}: {e}"));
        assert!(last > 0.99, "progress reached {last}");
        assert!(done.output.is_file(), "{} was not written", done.output.display());
        assert!(!done.messages.iter().any(|m| m.contains("Features enabled")));
        done
    }
}

/// The channel count in the `fmt ` chunk of a WAV file.
fn wav_channels(bytes: &[u8]) -> u16 {
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        if &bytes[at..at + 4] == b"fmt " {
            return u16::from_le_bytes([bytes[at + 10], bytes[at + 11]]);
        }
        at += 8 + size + (size & 1);
    }
    0
}

fn verified(path: &Path) -> sw_verify::Report {
    let r = sw_verify::verify(path, &sw_verify::Options::default()).unwrap();
    assert!(r.passed, "{}", r.to_text());
    r
}

#[test]
fn loudness_is_measured_with_dialogue_detection() {
    let Some(b) = bench("measure") else { return };
    let done = b.run(
        b.stems(6),
        "measured.xml",
        Task::Measure {
            metering: "1770-3".into(),
            dialogue_intelligence: true,
            speech_threshold: 15,
        },
    );
    let m: Measured = done.measured.expect("a measurement");
    assert_eq!(m.channels, 6);
    assert!(m.integrated.is_some_and(|l| (-50.0..0.0).contains(&l)), "{m:?}");
    assert_eq!(m.true_peak.len(), 6);
    assert!(m.dialnorm().is_some());
}

#[test]
fn pcm_becomes_dolby_digital_and_digital_plus_and_decodes_again() {
    let Some(b) = bench("encode") else { return };
    let stems = b.stems(6);
    let dd = b.run(
        stems.clone(),
        "out.ac3",
        Task::Encode(Box::new(EncodeOptions {
            mode: "dd".into(),
            data_rate_kbps: 448,
            ..Default::default()
        })),
    );
    let r = verified(&dd.output);
    assert!(r.format.contains("AC-3"), "{}", r.format);
    let ddp = b.run(
        stems,
        "out.ec3",
        Task::Encode(Box::new(EncodeOptions {
            loudness: Loudness::Correct {
                preset: "ebu_r128".into(),
            },
            ..Default::default()
        })),
    );
    verified(&ddp.output);
    // and back to PCM
    let wav = b.run(
        Source::Stream {
            path: ddp.output.clone(),
            atmos: false,
        },
        "decoded.wav",
        Task::Decode { drc: "none".into() },
    );
    let bytes = std::fs::read(&wav.output).unwrap();
    assert_eq!(wav_channels(&bytes), 6, "six channels come back");
    assert!(bytes.len() > 6 * 3 * 48_000 * 4, "about six seconds of audio");
}

#[test]
fn streams_are_transcoded_trimmed_and_joined() {
    let Some(b) = bench("streams") else { return };
    let dd = b.run(
        b.stems(6),
        "src.ac3",
        Task::Encode(Box::new(EncodeOptions {
            mode: "dd".into(),
            ..Default::default()
        })),
    );
    let ddp = b.run(
        Source::Stream {
            path: dd.output,
            atmos: false,
        },
        "trans.ec3",
        Task::Transcode {
            data_rate_kbps: Some(384),
            correct: None,
        },
    );
    verified(&ddp.output);
    let joined = b.run(
        Source::Streams {
            paths: vec![ddp.output.clone(), ddp.output.clone()],
        },
        "joined.ec3",
        Task::Edit {
            start: "0:0:0.0".into(),
            end: "end_of_file".into(),
            invert_trim: false,
        },
    );
    let single = std::fs::metadata(&ddp.output).unwrap().len();
    let both = std::fs::metadata(&joined.output).unwrap().len();
    assert!(
        both > single * 3 / 2,
        "the joined stream is about twice as long ({single} then {both})"
    );
    verified(&joined.output);
}

#[test]
fn pcm_loudness_is_corrected() {
    let Some(b) = bench("pcm") else { return };
    // a multichannel WAV to work from: decode an encode
    let ddp = b.run(b.stems(6), "src.ec3", Task::Encode(Box::default()));
    let wav = b.run(
        Source::Stream {
            path: ddp.output,
            atmos: false,
        },
        "src.wav",
        Task::Decode { drc: "none".into() },
    );
    let corrected = b.run(
        Source::Wav {
            path: wav.output.clone(),
        },
        "corrected.wav",
        Task::Correct {
            preset: "ebu_r128".into(),
        },
    );
    assert!(std::fs::metadata(&corrected.output).unwrap().len() > 1_000_000);
}
