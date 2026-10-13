//! A spectrogram of one channel, small enough to carry in a report: a fixed number of log-spaced bands and a few
//! hundred columns, each the peak level in the band at that moment, in dB below full scale.

use crate::analysis::fft::fft;
use crate::analysis::Spectrogram;
use std::f64::consts::PI;

/// Samples in the window of each column.
const WINDOW: usize = 4096;
/// Bands from the lowest to the highest frequency.
const BANDS: usize = 64;
/// The most columns kept.
const MAX_COLUMNS: usize = 1200;
/// Levels below this (dB) all look alike.
const FLOOR_DB: f64 = -100.0;

/// The highest frequency shown: the top of the audible range, or just under the Nyquist frequency.
fn high_hz(rate: u32) -> f64 {
    (rate as f64 / 2.0 * 0.99).min(20_000.0)
}

fn low_hz(rate: u32) -> f64 {
    30.0f64.min(high_hz(rate) / 4.0)
}

/// Collects the columns of one channel while its samples go by.
pub(crate) struct Spectro {
    rate: u32,
    ring: Vec<f32>,
    pos: usize,
    seen: u64,
    /// The sample count at which the next column is taken, and the distance between columns.
    next_at: u64,
    stride: u64,
    window: Vec<f64>,
    /// First and one-past-last FFT bin of each band.
    bins: Vec<(usize, usize)>,
    columns: Vec<u8>,
}

impl Spectro {
    /// For a channel of about `expected_frames` samples (0 when it is not known).
    pub(crate) fn new(rate: u32, expected_frames: u64) -> Spectro {
        let (low, high) = (low_hz(rate), high_hz(rate));
        let edge = |b: usize| low * (high / low).powf(b as f64 / BANDS as f64);
        let bin_of = |f: f64| f * WINDOW as f64 / rate as f64;
        let bins = (0..BANDS)
            .map(|b| {
                let lo = (bin_of(edge(b)).floor() as usize).max(1);
                let hi = (bin_of(edge(b + 1)).ceil() as usize).max(lo + 1);
                (lo, hi.min(WINDOW / 2))
            })
            .collect();
        let stride = if expected_frames > 0 {
            (expected_frames / 600).max(WINDOW as u64 / 2)
        } else {
            rate as u64 / 4
        };
        Spectro {
            rate,
            ring: vec![0.0; WINDOW],
            pos: 0,
            seen: 0,
            next_at: WINDOW as u64,
            stride,
            window: (0..WINDOW)
                .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f64 / WINDOW as f64).cos())
                .collect(),
            bins,
            columns: Vec::new(),
        }
    }

    pub(crate) fn push(&mut self, samples: &[f32]) {
        for &x in samples {
            self.ring[self.pos] = x;
            self.pos = (self.pos + 1) % WINDOW;
            self.seen += 1;
            if self.seen >= self.next_at {
                self.next_at += self.stride;
                if self.columns.len() < MAX_COLUMNS * BANDS {
                    self.take_column();
                }
            }
        }
    }

    /// The spectrum of the last window as one column of band levels.
    fn take_column(&mut self) {
        let mut re: Vec<f64> = (0..WINDOW)
            .map(|i| self.ring[(self.pos + i) % WINDOW] as f64 * self.window[i])
            .collect();
        let mut im = vec![0.0; WINDOW];
        fft(&mut re, &mut im);
        // a full-scale sine reads 0 dB: its bin holds a quarter of the window length
        let full = WINDOW as f64 / 4.0;
        for &(lo, hi) in &self.bins {
            let peak = (lo..hi.max(lo + 1))
                .map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt())
                .fold(0.0, f64::max);
            let db = if peak > 0.0 {
                20.0 * (peak / full).log10()
            } else {
                FLOOR_DB
            };
            let level = ((db.clamp(FLOOR_DB, 0.0) - FLOOR_DB) * 2.55).round();
            self.columns.push(level as u8);
        }
    }

    /// The finished picture.
    pub(crate) fn finish(self, channel: String) -> Spectrogram {
        Spectrogram {
            channel,
            bands: BANDS,
            columns: self.columns.len() / BANDS,
            seconds: self.seen as f64 / self.rate as f64,
            low_hz: low_hz(self.rate),
            high_hz: high_hz(self.rate),
            levels: base64(&self.columns),
        }
    }
}

/// Standard base64, which a page can turn back into bytes.
pub(crate) fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n =
            (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_examples() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn a_tone_shows_in_the_band_that_holds_it() {
        let rate = 48_000u32;
        let mut s = Spectro::new(rate, rate as u64 * 4);
        let tone: Vec<f32> = (0..rate as usize * 4)
            .map(|i| (0.5 * (2.0 * PI * 1000.0 * i as f64 / rate as f64).sin()) as f32)
            .collect();
        s.push(&tone);
        assert!(s.columns.len() / BANDS > 10);
        let column = &s.columns[..BANDS];
        let (band, level) = column.iter().enumerate().max_by_key(|(_, v)| **v).unwrap();
        let (low, high) = (low_hz(rate), high_hz(rate));
        let centre = low * (high / low).powf((band as f64 + 0.5) / BANDS as f64);
        assert!((centre / 1000.0).log2().abs() < 0.3, "band centre {centre} Hz");
        // half scale is -6 dB
        let db = *level as f64 / 2.55 - 100.0;
        assert!((db + 6.0).abs() < 2.0, "{db} dB");
    }
}
