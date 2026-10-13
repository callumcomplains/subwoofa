//! What a QC report shows beyond the checks: the loudness of the decoded programme, how the bit rate moves, and a
//! spectrogram of every channel. It is gathered while the deep verification decodes the file.

mod fft;
mod spectro;

use crate::Probe;
use serde::Serialize;
use spectro::Spectro;
use sw_core::Channel;
use sw_loudness::{Loudness, Meter};

/// The spectrogram of one channel: `columns` columns of `bands` log-spaced bands each, as one byte per cell (0 for
/// -100 dBFS up to 255 for 0 dBFS), column after column, as base64.
#[derive(Debug, Clone, Serialize)]
pub struct Spectrogram {
    pub channel: String,
    pub bands: usize,
    pub columns: usize,
    pub seconds: f64,
    pub low_hz: f64,
    pub high_hz: f64,
    pub levels: String,
}

/// The measurements for a report.
#[derive(Debug, Clone, Serialize)]
pub struct Analysis {
    pub loudness: Loudness,
    /// Mean bit rate (kbps) of each stretch of about a second, for streams made of frames.
    pub bitrate_kbps: Vec<f32>,
    pub spectrograms: Vec<Spectrogram>,
}

/// The channels a decode of `n` channels carries, in the order FFmpeg writes them.
fn decoded_channels(n: usize) -> Vec<Channel> {
    use Channel::*;
    let standard = [L, R, C, Lfe, Ls, Rs, Lsr, Rsr];
    match n {
        1 => vec![C],
        2 => vec![L, R],
        3 => vec![L, R, C],
        4 => vec![L, R, Ls, Rs],
        5 => vec![L, R, C, Ls, Rs],
        n if n <= standard.len() => standard[..n].to_vec(),
        n => {
            // wider programmes: the 7.1 order, then the rest counted as ordinary channels
            let mut v = standard.to_vec();
            v.resize(n, C);
            v
        }
    }
}

/// Listens to the decoded audio (24-bit little-endian, interleaved) as it goes by.
pub(crate) struct AudioTap {
    channels: usize,
    meter: Meter,
    spectros: Vec<Spectro>,
    names: Vec<String>,
    planar: Vec<Vec<f32>>,
}

impl AudioTap {
    /// For `channels` channels at `rate`, of about `expected_frames` samples each (0 when not known).
    pub(crate) fn new(rate: u32, channels: usize, expected_frames: u64) -> AudioTap {
        let layout = decoded_channels(channels);
        AudioTap {
            channels,
            meter: Meter::new(rate, &layout),
            spectros: (0..channels).map(|_| Spectro::new(rate, expected_frames)).collect(),
            names: layout.iter().map(|c| c.short().to_string()).collect(),
            planar: vec![Vec::new(); channels],
        }
    }

    /// Whole frames of interleaved samples.
    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        let frame_bytes = 3 * self.channels;
        let frames = bytes.len() / frame_bytes;
        for plane in &mut self.planar {
            plane.clear();
        }
        for frame in bytes.chunks_exact(frame_bytes) {
            for (ch, sample) in frame.chunks_exact(3).enumerate() {
                let v = i32::from_le_bytes([0, sample[0], sample[1], sample[2]]) >> 8;
                self.planar[ch].push(v as f32 / 8_388_608.0);
            }
        }
        self.meter.push(&self.planar, frames);
        for (spectro, plane) in self.spectros.iter_mut().zip(&self.planar) {
            spectro.push(plane);
        }
    }

    /// The measurements, with the bit rate chart of the stream's frames if it has them.
    pub(crate) fn finish(self, probe: Option<&Probe>) -> Analysis {
        Analysis {
            loudness: self.meter.finish(),
            bitrate_kbps: probe.map(bitrate_series).unwrap_or_default(),
            spectrograms: self
                .spectros
                .into_iter()
                .zip(self.names)
                .map(|(s, name)| s.finish(name))
                .collect(),
        }
    }
}

/// The most points a chart keeps.
const MAX_POINTS: usize = 1200;

/// Mean bit rate of every stretch of about a second, from the frame sizes and lengths.
fn bitrate_series(p: &Probe) -> Vec<f32> {
    if p.sample_rate == 0 {
        return Vec::new();
    }
    let (mut bytes, mut samples) = (0u64, 0u64);
    let mut series = Vec::new();
    for (b, s) in p.frame_bytes.iter().zip(&p.frame_samples) {
        bytes += *b as u64;
        samples += *s as u64;
        if samples >= p.sample_rate as u64 {
            series.push((bytes as f64 * 8.0 * p.sample_rate as f64 / samples as f64 / 1000.0) as f32);
            (bytes, samples) = (0, 0);
        }
    }
    if samples * 2 >= p.sample_rate as u64 {
        series.push((bytes as f64 * 8.0 * p.sample_rate as f64 / samples as f64 / 1000.0) as f32);
    }
    if series.len() > MAX_POINTS {
        let chunk = series.len().div_ceil(MAX_POINTS);
        series = series
            .chunks(chunk)
            .map(|c| c.iter().sum::<f32>() / c.len() as f32)
            .collect();
    }
    series
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tap_reads_what_a_decode_writes() {
        // one second of a half-scale 1 kHz tone in two channels, as 24-bit little-endian frames
        let rate = 48_000u32;
        let mut bytes = Vec::new();
        for i in 0..rate as usize {
            let x = (0.5 * (2.0 * std::f64::consts::PI * 997.0 * i as f64 / rate as f64).sin() * 8_388_607.0) as i32;
            for _ in 0..2 {
                bytes.extend_from_slice(&x.to_le_bytes()[..3]);
            }
        }
        let mut tap = AudioTap::new(rate, 2, rate as u64);
        tap.feed(&bytes[..3 * 2 * 1000]);
        tap.feed(&bytes[3 * 2 * 1000..]);
        let a = tap.finish(None);
        // a half-scale sine in two channels: -6 dBFS peak each, so -6.02 LUFS in all
        let peak = a.loudness.channels[0].sample_peak.unwrap();
        assert!((peak + 6.0).abs() < 0.1, "{peak}");
        assert_eq!(a.spectrograms.len(), 2);
        assert_eq!(a.spectrograms[0].channel, "L");
        assert!(a.bitrate_kbps.is_empty());
    }
}
