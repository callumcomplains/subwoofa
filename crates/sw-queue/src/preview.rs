//! A listening preview: a short excerpt of a job is encoded with the job's settings, decoded again, and both it and the
//! source excerpt are mixed down to stereo the same way and lined up, so the page can switch between them while playing
//! and a person can hear what the format does to the audio before the whole file is committed to.

use crate::run::{execute, open_sources, EncodeSpec, Failure};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use sw_core::{Channel, ChannelLayout};
use sw_pcm::StemSet;

/// The longest excerpt a preview makes, in seconds (it is held in memory).
pub const MAX_SECONDS: f64 = 60.0;

/// What the page needs to play and describe a preview.
#[derive(Debug, Serialize)]
pub struct Preview {
    pub rate: u32,
    /// Where in the programme the excerpt starts, and how long it is.
    pub start: f64,
    pub seconds: f64,
    /// Stereo 16-bit WAV files: the source, the decoded encode, and the source minus the encode.
    pub source: PathBuf,
    pub encoded: PathBuf,
    pub difference: PathBuf,
    /// Size of the encoded excerpt and the bit rate it works out at.
    pub bytes: u64,
    pub kbps: f64,
    /// Signal to noise of the stereo mix against the source, in dB (`None` when they are identical).
    pub snr_db: Option<f64>,
    /// The channels left out of the stereo mix, by name.
    pub note: String,
}

/// Makes the preview of `spec` in `dir`: `seconds` of audio from `spec.start`, or from the middle of the programme.
pub fn build(
    spec: &EncodeSpec,
    ffmpeg: &Path,
    seconds: f64,
    dir: &Path,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(f32),
) -> Result<Preview, Failure> {
    let layout: &'static ChannelLayout = ChannelLayout::find(&spec.layout).ok_or("unknown layout")?;
    let mut sources = open_sources(spec, layout)?;
    let rate = sources.stems.sample_rate;
    let (start, want) = choose_excerpt(spec, &mut sources.stems, seconds)?;

    // encode it
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let extension = Path::new(&spec.output)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin")
        .to_string();
    let mut excerpt = spec.clone();
    excerpt.start = Some(start);
    excerpt.duration = Some(want);
    excerpt.output = dir.join(format!("preview.{extension}")).to_string_lossy().into_owned();
    excerpt.write_log = false;
    // a gain or delay would show up as a difference from the source, and Normalise would measure only the excerpt
    // (so not the gain the real job gets): the preview is about what the format does to the sound, so both stay out
    excerpt.normalize_lufs = None;
    excerpt.delay_ms = 0;
    let outcome = execute(&excerpt, false, cancel, &mut |p, _| on_progress(p * 0.8))?;

    // the source excerpt and the decoded encode, mixed down the same way and lined up
    sources
        .stems
        .select_span(start, Some(want))
        .map_err(|e| e.to_string())?;
    let mut source = Vec::new();
    let frames = sources
        .stems
        .read_planar(&mut source, (want * rate as f64) as usize)
        .map_err(|e| e.to_string())?;
    on_progress(0.85);
    let decoded = sw_verify::decode_planar(Path::new(&excerpt.output), ffmpeg)?;
    let (a, b) = aligned_stereo(layout, &source[..], frames, decoded, rate)?;
    on_progress(0.95);

    let (snr_db, scale) = compare(&a, &b);
    let difference: Vec<Vec<f32>> = (0..2)
        .map(|ch| a[ch].iter().zip(&b[ch]).map(|(x, y)| x - y).collect())
        .collect();
    let write = |name: &str, planes: &[Vec<f32>]| -> Result<PathBuf, String> {
        let path = dir.join(name);
        std::fs::write(&path, wav16(rate, planes, scale)).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(path)
    };
    let preview = Preview {
        rate,
        start,
        seconds: a[0].len() as f64 / rate as f64,
        source: write("source.wav", &a)?,
        encoded: write("encoded.wav", &b)?,
        difference: write("difference.wav", &difference)?,
        bytes: outcome.bytes,
        kbps: outcome.bytes as f64 * 8.0 / want / 1000.0,
        snr_db,
        note: note(layout, spec),
    };
    on_progress(1.0);
    Ok(preview)
}

/// What the preview leaves out, for the page to show.
fn note(layout: &ChannelLayout, spec: &EncodeSpec) -> String {
    let mut parts = Vec::new();
    if layout.channels.contains(&Channel::Lfe) {
        parts.push("The stereo mix leaves out the LFE channel.");
    }
    if spec.normalize_lufs.is_some() || spec.delay_ms != 0 {
        parts.push("Normalise and Delay are left out (level and timing only).");
    }
    parts.join(" ")
}

/// Where the excerpt starts and how long it is (in seconds): the job's start or the middle of the programme, inside the
/// audio.
fn choose_excerpt(spec: &EncodeSpec, stems: &mut StemSet, seconds: f64) -> Result<(f64, f64), Failure> {
    let rate = stems.sample_rate;
    let all = stems.select_span(0.0, None).map_err(|e| e.to_string())?;
    let length = all as f64 / rate as f64;
    if length < 2.0 {
        return Err("the input is too short to preview".to_string().into());
    }
    let want = seconds
        .clamp(2.0, MAX_SECONDS)
        .min(spec.duration.unwrap_or(f64::MAX))
        .min(length);
    let start = spec
        .start
        .unwrap_or(((length - want) / 2.0).max(0.0))
        .min(length - want)
        .max(0.0);
    Ok((start, want))
}

/// The source and the decoded encode (one vector of samples per channel, `decoded` in FFmpeg's order) lined up and
/// mixed down to stereo the same way. Both have the same length.
fn aligned_stereo(
    layout: &ChannelLayout,
    source: &[Vec<f32>],
    frames: usize,
    decoded: Vec<Vec<f32>>,
    rate: u32,
) -> Result<(Vec<Vec<f32>>, Vec<Vec<f32>>), Failure> {
    if decoded.len() != layout.channels.len() {
        return Err(format!(
            "the encode decodes to {} channels but the layout has {}: the preview cannot match them up",
            decoded.len(),
            layout.channels.len()
        )
        .into());
    }
    let order = sw_verify::decoded_order(layout);
    let decoded: Vec<Vec<f32>> = order.iter().map(|&i| decoded[i].clone()).collect();
    let lag = sw_verify::align(source, &decoded, rate)?;
    let (skip_decoded, skip_source) = (lag.max(0) as usize, (-lag).max(0) as usize);
    let n = frames
        .saturating_sub(skip_source)
        .min(decoded[0].len().saturating_sub(skip_decoded));
    if n < rate as usize {
        return Err("the encode is too short to compare with the source".to_string().into());
    }
    Ok((
        fold(layout.channels, source, skip_source, n),
        fold(layout.channels, &decoded, skip_decoded, n),
    ))
}

/// The signal to noise of `b` against `a` in dB (`None` when they are identical), and the one scale for all three
/// files so their levels stay comparable and nothing clips.
fn compare(a: &[Vec<f32>], b: &[Vec<f32>]) -> (Option<f64>, f32) {
    let (mut signal, mut error, mut peak) = (0.0f64, 0.0f64, 0.0f32);
    for ch in 0..2 {
        for (x, y) in a[ch].iter().zip(&b[ch]) {
            signal += (*x as f64).powi(2);
            error += ((*x - *y) as f64).powi(2);
            peak = peak.max(x.abs()).max(y.abs());
        }
    }
    let snr = (error > 0.0 && signal > 0.0).then(|| 10.0 * (signal / error).log10());
    (snr, if peak > 0.98 { 0.98 / peak } else { 1.0 })
}

/// A channel's share of the left and right ears in the stereo mix: the usual fold-down, with the centre and the
/// surrounds at -3 dB and the LFE left out.
fn ears(channel: Channel) -> (f32, f32) {
    const HALF_POWER: f32 = std::f32::consts::FRAC_1_SQRT_2;
    let name = channel.short();
    if name.starts_with("LFE") || name.starts_with("Lfe") {
        return (0.0, 0.0);
    }
    let front = matches!(channel, Channel::L | Channel::R | Channel::Lt | Channel::Rt);
    let gain = if front { 1.0 } else { HALF_POWER };
    // the top and bottom layers of 22.2 end in the side (Tfl, Tsr); everything else starts with it (Lss, Rhs)
    let upper_layer = name.starts_with(['T', 'B']);
    let left = if upper_layer {
        name.ends_with('l')
    } else {
        name.starts_with('L')
    };
    let right = if upper_layer {
        name.ends_with('r')
    } else {
        name.starts_with('R')
    };
    match (left, right) {
        (true, _) => (gain, 0.0),
        (_, true) => (0.0, gain),
        // centre, rear centre, overhead: both ears, sharing the power
        _ => (HALF_POWER, HALF_POWER),
    }
}

/// `n` frames from `from` of `planar` mixed down to a left and a right channel.
fn fold(channels: &[Channel], planar: &[Vec<f32>], from: usize, n: usize) -> Vec<Vec<f32>> {
    let mut stereo = vec![vec![0.0f32; n]; 2];
    for (channel, samples) in channels.iter().zip(planar) {
        let (l, r) = ears(*channel);
        if l == 0.0 && r == 0.0 {
            continue;
        }
        for (i, x) in samples[from..from + n].iter().enumerate() {
            stereo[0][i] += x * l;
            stereo[1][i] += x * r;
        }
    }
    stereo
}

/// A 16-bit stereo WAV file of `planes` (left, right) times `scale`.
fn wav16(rate: u32, planes: &[Vec<f32>], scale: f32) -> Vec<u8> {
    let frames = planes[0].len();
    let data = (frames * 4) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        for plane in planes {
            let v = (plane[i] * scale * 32767.0).round().clamp(-32768.0, 32767.0) as i16;
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_centre_reaches_both_ears_equally_and_the_lfe_neither() {
        let (l, r) = ears(Channel::C);
        assert!(
            (l - r).abs() < 1e-6 && (l - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3,
            "{l} {r}"
        );
        assert_eq!(ears(Channel::Lfe), (0.0, 0.0));
        assert_eq!(ears(Channel::L), (1.0, 0.0));
        assert_eq!(ears(Channel::Rs).0, 0.0);
        assert!(ears(Channel::Tfl).0 > 0.0 && ears(Channel::Tfl).1 == 0.0);
        assert!(ears(Channel::Tbr).1 > 0.0 && ears(Channel::Tbr).0 == 0.0);
    }

    #[test]
    fn a_wav_has_a_sound_header() {
        let wav = wav16(48_000, &[vec![0.5, -0.5], vec![0.0, 1.0]], 1.0);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(wav.len(), 44 + 8);
        assert_eq!(i16::from_le_bytes([wav[44], wav[45]]), 16384);
        assert_eq!(i16::from_le_bytes([wav[50], wav[51]]), 32767);
    }
}
