//! The check of a lossy encode: its decode is lined up with the source and measured channel by channel
//! (`sw_verify::compare_with_source`), each channel against the decoded channel FFmpeg's order puts it in. A lossy
//! codec changes the audio on purpose, so the bar is low but real: the decode must cover the whole source, and every
//! audible channel must keep a signal-to-noise ratio that any working encoder clears easily. A short file, a silent
//! or missing channel, or one channel's audio in another's place fails it. Pieces: `verify`, `judge`.

use crate::run::processed::Edit;
use crate::run::spec::{EncodeSpec, Failure};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use sw_core::{Channel, ChannelLayout};
use sw_verify::Comparison;

/// The worst audible channel's signal to noise below which the encode is broken, in dB. Real lossy encodes measure
/// far above it even at low bit rates; a silent channel measures 0 dB and another channel's audio about -3 dB.
const MIN_SNR_DB: f64 = 3.0;
/// Source channels quieter than this (RMS, dBFS) have too little signal for a meaningful figure (as in `sw-verify`).
const QUIET_DBFS: f64 = -60.0;
/// Frames the decode may fall short of the source: one frame of the longest-framed codec (4096 samples).
const SHORT_FRAMES: u64 = 4096;

/// Compares the decode of the finished file with the source over `(start, span)`; returns a line for the log.
pub(crate) fn verify(
    spec: &EncodeSpec,
    layout: &'static ChannelLayout,
    inputs: &HashMap<Channel, PathBuf>,
    ffmpeg: &Path,
    (start, span): (f64, f64),
    edit: Edit,
    log: &mut String,
) -> Result<String, Failure> {
    let order = sw_pcm::ChannelOrder::parse(&spec.channel_order).ok_or("channel order must be auto, mask or file")?;
    let source = sw_verify::Source {
        layout,
        inputs,
        order,
        start: Some(start),
        duration: Some(span),
        delay_frames: edit.delay_frames,
        gain_db: edit.gain_db,
        order_known: true,
    };
    let mut report = sw_verify::Report {
        lossless: Some(false),
        ..Default::default()
    };
    sw_verify::compare_with_source(&mut report, Path::new(&spec.output), ffmpeg, &source)
        .map_err(|e| format!("the encode could not be compared with the source: {e}"))?;
    let comparison = report.comparison.ok_or("the comparison gave no result")?;
    match judge(&comparison) {
        Ok(line) => {
            log.push_str(&format!("Verify: {line}\n"));
            Ok(line)
        }
        Err(problem) => {
            log.push_str(&format!("Verify: FAILED: {problem}\n"));
            Err(Failure::Error(format!(
                "the encoded file failed verification: {problem}"
            )))
        }
    }
}

/// Whether the comparison shows a working encode: the problem if not, a summary if so.
fn judge(c: &Comparison) -> Result<String, String> {
    let short = c.source_frames.saturating_sub(c.decoded_frames);
    if short > SHORT_FRAMES {
        return Err(format!("the decode is {short} frames shorter than the source"));
    }
    let worst = c
        .channels
        .iter()
        .filter(|m| m.rms_dbfs.is_some_and(|r| r > QUIET_DBFS))
        .filter_map(|m| m.snr_db.map(|snr| (snr, m.name.as_str())))
        .min_by(|a, b| a.0.total_cmp(&b.0));
    match worst {
        Some((snr, name)) if snr < MIN_SNR_DB => Err(format!(
            "the {name} channel does not match the source (signal to noise {snr:.1} dB): missing, silent or swapped"
        )),
        Some((snr, name)) => Ok(format!(
            "decodes cleanly and matches the source (worst channel {name}: signal to noise {snr:.1} dB)"
        )),
        None => Ok("decodes cleanly and matches the source exactly".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sw_verify::ChannelMatch;

    fn channel(name: &str, snr_db: f64) -> ChannelMatch {
        ChannelMatch {
            name: name.into(),
            decoded_index: 0,
            rms_dbfs: Some(-20.0),
            snr_db: Some(snr_db),
            peak_error_dbfs: None,
            mismatches: 1,
        }
    }

    fn comparison(channels: Vec<ChannelMatch>, decoded_frames: u64) -> Comparison {
        Comparison {
            lag_samples: 256,
            compared_frames: decoded_frames,
            source_frames: 48_000,
            decoded_frames,
            exact: false,
            reordered: false,
            channels,
        }
    }

    #[test]
    fn good_encode_passes() {
        let c = comparison(vec![channel("L", 40.0), channel("R", 25.0)], 48_000);
        assert!(judge(&c).unwrap().contains("R: signal to noise 25.0"));
    }

    #[test]
    fn short_file_fails() {
        let c = comparison(vec![channel("L", 40.0)], 40_000);
        assert!(judge(&c).unwrap_err().contains("8000 frames shorter"));
    }

    #[test]
    fn swapped_channel_fails() {
        // another channel's audio measures about -3 dB
        let c = comparison(vec![channel("L", 40.0), channel("C", -3.0)], 48_000);
        assert!(judge(&c).unwrap_err().contains("C channel"));
    }

    /// A 3-second 24-bit mono WAV of a sine at `freq` Hz, -12 dBFS, over quieter noise (a pure tone repeats, so it
    /// can be lined up at the wrong period; real audio cannot).
    fn tone_wav(path: &Path, freq: f64) {
        let rate = 48_000u32;
        let mut data = Vec::new();
        let mut x = freq as u32 | 1;
        for i in 0..rate * 3 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let noise = ((x >> 8) as i32 - (1 << 23)) as f64 / 40.0;
            let v = (i as f64 / rate as f64 * freq * std::f64::consts::TAU).sin() * 2_100_000.0 + noise;
            data.extend_from_slice(&(v as i32).to_le_bytes()[..3]);
        }
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        for v in [16u32, 0x0001_0001, rate, rate * 3, 0x0018_0003] {
            wav.extend_from_slice(&v.to_le_bytes());
        }
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
        wav.extend_from_slice(&data);
        std::fs::write(path, wav).unwrap();
    }

    #[test]
    fn swap_caught_end_to_end() {
        let Some(ffmpeg) = sw_backend_ffmpeg::find_ffmpeg() else {
            return; // needs FFmpeg to decode
        };
        let dir = std::env::temp_dir().join(format!("sw-closeness-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (low, high) = (dir.join("low.wav"), dir.join("high.wav"));
        tone_wav(&low, 440.0);
        tone_wav(&high, 1_250.0);
        let spec: EncodeSpec = serde_json::from_value(serde_json::json!({
            "dest": "generic", "stream": "ac3", "layout": "2.0", "sample_rate": 48000, "bitrate": 192,
            "dialnorm": 27, "start": null, "duration": null, "engine": "native", "speed": "fast",
            "inputs": { "L": low, "R": high }, "output": dir.join("out.ac3"),
        }))
        .unwrap();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        crate::run::execute(&spec, false, &cancel, &mut |_, _| {}).unwrap_or_else(|_| panic!("encode failed"));
        let layout = ChannelLayout::find("2.0").unwrap();
        let check = |left: &PathBuf, right: &PathBuf| {
            let inputs = HashMap::from([(Channel::L, left.clone()), (Channel::R, right.clone())]);
            verify(
                &spec,
                layout,
                &inputs,
                &ffmpeg,
                (0.0, 3.0),
                Edit::default(),
                &mut String::new(),
            )
        };
        assert!(check(&low, &high).is_ok(), "the right source must pass");
        assert!(check(&high, &low).is_err(), "left and right swapped must fail");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quiet_channel_ignored() {
        let mut lfe = channel("LFE", -3.0);
        lfe.rms_dbfs = Some(-90.0);
        assert!(judge(&comparison(vec![channel("L", 40.0), lfe], 48_000)).is_ok());
    }
}
