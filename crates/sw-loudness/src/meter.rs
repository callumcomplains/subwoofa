//! Measuring in 100 ms steps: every 400 ms block (75 % overlap) and every 3 s window is a number of those steps
//! averaged, so the meter keeps one energy per step and works the rest out when the audio ends.

use crate::filter::KWeighting;
use crate::peak::TruePeak;
use crate::{ChannelPeak, Loudness};
use sw_core::Channel;

/// The offset that makes a full-scale 997 Hz sine in one channel read -3.01 LUFS (the filter's gain there is
/// +0.69 dB).
const OFFSET: f64 = -0.691;
/// Blocks quieter than this never count.
const ABSOLUTE_GATE: f64 = -70.0;
/// A block must be within this many LU of the average to count in the integrated loudness.
const RELATIVE_GATE: f64 = -10.0;
/// The gate of the loudness range.
const RANGE_GATE: f64 = -20.0;

/// How much a channel counts in the sum: 1.41 (+1.5 dB) for the surrounds at ear height, 1.0 for the front and
/// every height channel, nothing for the low-frequency channels. BS.1770-4 (Table 4) gives 1.41 only below 30 degrees
/// of elevation, so the height layers count 1.0. The rear surrounds (135-150 degrees) also get 1.41, as in FFmpeg's
/// `ebur128` and common practice, although the table's 60-120 degree band would give them 1.0.
pub fn channel_weight(c: Channel) -> f64 {
    use Channel::*;
    match c {
        Lfe | Lfe2 => 0.0,
        Ls | Rs | Cs | Lsr | Rsr | Lss | Rss => 1.41,
        _ => 1.0,
    }
}

fn lufs(energy: f64) -> f64 {
    OFFSET + 10.0 * energy.log10()
}

fn energy_of(lufs: f64) -> f64 {
    10f64.powf((lufs - OFFSET) / 10.0)
}

fn db(x: f64) -> Option<f64> {
    (x > 0.0).then(|| 20.0 * x.log10())
}

/// A loudness meter for one programme.
pub struct Meter {
    rate: u32,
    names: Vec<String>,
    weights: Vec<f64>,
    filters: Vec<KWeighting>,
    peaks: Vec<TruePeak>,
    sample_peaks: Vec<f64>,
    /// Samples in a 100 ms step, and what the current step has gathered.
    step_len: usize,
    step_filled: usize,
    step_sum: f64,
    /// The mean weighted square of every finished step.
    steps: Vec<f64>,
    frames: u64,
}

impl Meter {
    /// A meter for a programme of these channels at this sample rate.
    pub fn new(rate: u32, channels: &[Channel]) -> Meter {
        Meter {
            rate,
            names: channels.iter().map(|c| c.short().to_string()).collect(),
            weights: channels.iter().map(|&c| channel_weight(c)).collect(),
            filters: vec![KWeighting::new(rate); channels.len()],
            peaks: vec![TruePeak::new(rate); channels.len()],
            sample_peaks: vec![0.0; channels.len()],
            step_len: (rate / 10) as usize,
            step_filled: 0,
            step_sum: 0.0,
            steps: Vec::new(),
            frames: 0,
        }
    }

    /// Takes `frames` samples of every channel: `planar[channel][frame]`, full scale being 1.0.
    pub fn push(&mut self, planar: &[Vec<f32>], frames: usize) {
        for i in 0..frames {
            let mut sum = 0.0;
            for (ch, samples) in planar.iter().enumerate().take(self.filters.len()) {
                let x = samples[i] as f64;
                self.sample_peaks[ch] = self.sample_peaks[ch].max(x.abs());
                self.peaks[ch].push(x);
                if self.weights[ch] > 0.0 {
                    let y = self.filters[ch].step(x);
                    sum += self.weights[ch] * y * y;
                }
            }
            self.step_sum += sum;
            self.step_filled += 1;
            if self.step_filled == self.step_len {
                self.steps.push(self.step_sum / self.step_len as f64);
                self.step_filled = 0;
                self.step_sum = 0.0;
            }
        }
        self.frames += frames as u64;
    }

    /// Mean energy of each window of `len` steps, one per step.
    fn windows(&self, len: usize) -> Vec<f64> {
        if self.steps.len() < len {
            return Vec::new();
        }
        let mut sum: f64 = self.steps[..len].iter().sum();
        let mut out = vec![sum / len as f64];
        for i in len..self.steps.len() {
            sum += self.steps[i] - self.steps[i - len];
            out.push(sum / len as f64);
        }
        out
    }

    /// Integrated loudness: the mean of the 400 ms blocks above the absolute gate and within 10 LU of their mean.
    fn integrated(blocks: &[f64]) -> Option<f64> {
        let absolute = energy_of(ABSOLUTE_GATE);
        let above: Vec<f64> = blocks.iter().copied().filter(|&e| e > absolute).collect();
        if above.is_empty() {
            return None;
        }
        let relative = energy_of(lufs(above.iter().sum::<f64>() / above.len() as f64) + RELATIVE_GATE);
        let kept: Vec<f64> = above.into_iter().filter(|&e| e > relative).collect();
        (!kept.is_empty()).then(|| lufs(kept.iter().sum::<f64>() / kept.len() as f64))
    }

    /// Loudness range: the spread between the 10th and 95th percentile of the 3 s windows above a gate 20 LU below
    /// their mean.
    fn range(windows: &[f64]) -> Option<f64> {
        let absolute = energy_of(ABSOLUTE_GATE);
        let above: Vec<f64> = windows.iter().copied().filter(|&e| e > absolute).collect();
        if above.is_empty() {
            return None;
        }
        let relative = energy_of(lufs(above.iter().sum::<f64>() / above.len() as f64) + RANGE_GATE);
        let mut kept: Vec<f64> = above.into_iter().filter(|&e| e > relative).map(lufs).collect();
        if kept.is_empty() {
            return None;
        }
        kept.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let at = |p: f64| kept[(((kept.len() - 1) as f64) * p + 0.5) as usize];
        Some(at(0.95) - at(0.10))
    }

    /// Ends the measurement.
    pub fn finish(self) -> Loudness {
        let blocks = self.windows(4);
        let short = self.windows(30);
        let max = |v: &[f64]| v.iter().copied().reduce(f64::max).filter(|&e| e > 0.0).map(lufs);
        Loudness {
            integrated: Self::integrated(&blocks),
            short_term_max: max(&short),
            momentary_max: max(&blocks),
            range: Self::range(&short),
            channels: self
                .names
                .iter()
                .enumerate()
                .map(|(i, name)| ChannelPeak {
                    name: name.clone(),
                    true_peak: db(self.peaks[i].peak.max(self.sample_peaks[i])),
                    sample_peak: db(self.sample_peaks[i]),
                })
                .collect(),
            seconds: self.frames as f64 / self.rate as f64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// A sine of `db` dBFS peak at 997 Hz, for `secs` seconds.
    fn sine(rate: u32, db: f64, secs: f64) -> Vec<f32> {
        let a = 10f64.powf(db / 20.0);
        (0..(rate as f64 * secs) as usize)
            .map(|i| (a * (2.0 * PI * 997.0 * i as f64 / rate as f64).sin()) as f32)
            .collect()
    }

    fn measure(rate: u32, channels: &[Channel], planar: &[Vec<f32>]) -> Loudness {
        let mut m = Meter::new(rate, channels);
        let frames = planar[0].len();
        // in pieces, as a file is read
        for start in (0..frames).step_by(7001) {
            let end = (start + 7001).min(frames);
            let piece: Vec<Vec<f32>> = planar.iter().map(|c| c[start..end].to_vec()).collect();
            m.push(&piece, end - start);
        }
        m.finish()
    }

    #[test]
    fn a_stereo_sine_reads_its_level() {
        // each channel at -20 dBFS peak: mean squares add to 0.01, which is -20 LUFS
        let tone = sine(48_000, -20.0, 10.0);
        let l = measure(48_000, &[Channel::L, Channel::R], &[tone.clone(), tone]);
        assert!((l.integrated.unwrap() + 20.0).abs() < 0.1, "{:?}", l.integrated);
        assert!((l.momentary_max.unwrap() + 20.0).abs() < 0.1);
        assert!((l.short_term_max.unwrap() + 20.0).abs() < 0.1);
        assert!(l.range.unwrap() < 0.5);
    }

    #[test]
    fn the_same_at_other_rates() {
        for rate in [44_100, 96_000] {
            let tone = sine(rate, -20.0, 6.0);
            let l = measure(rate, &[Channel::L, Channel::R], &[tone.clone(), tone]);
            assert!((l.integrated.unwrap() + 20.0).abs() < 0.1, "{rate}: {:?}", l.integrated);
        }
    }

    #[test]
    fn surrounds_count_more_and_the_lfe_not_at_all() {
        let tone = sine(48_000, -20.0, 6.0);
        let silence = vec![0.0f32; tone.len()];
        let front = measure(48_000, &[Channel::L, Channel::Lfe], &[tone.clone(), tone.clone()]);
        let surround = measure(48_000, &[Channel::Ls, Channel::Lfe], &[tone.clone(), silence.clone()]);
        let one = measure(48_000, &[Channel::L, Channel::Lfe], &[tone, silence]);
        // the LFE adds nothing; a surround is 1.5 dB louder than a front channel
        assert!((front.integrated.unwrap() - one.integrated.unwrap()).abs() < 0.01);
        assert!((surround.integrated.unwrap() - one.integrated.unwrap() - 1.5).abs() < 0.05);
    }

    #[test]
    fn heights_count_as_fronts() {
        let tone = sine(48_000, -20.0, 6.0);
        let front = measure(48_000, &[Channel::L], std::slice::from_ref(&tone));
        for height in [Channel::Tfl, Channel::Tsl, Channel::Tbl, Channel::Tbc] {
            let h = measure(48_000, &[height], std::slice::from_ref(&tone));
            assert!(
                (h.integrated.unwrap() - front.integrated.unwrap()).abs() < 0.01,
                "{height:?}"
            );
        }
    }

    #[test]
    fn silence_does_not_pull_the_integrated_loudness_down() {
        let mut tone = vec![0.0f32; 48_000 * 5];
        tone.extend(sine(48_000, -20.0, 10.0));
        let l = measure(48_000, &[Channel::C], std::slice::from_ref(&tone));
        // a mono sine at -20 dBFS peak is -23.01 LUFS
        assert!((l.integrated.unwrap() + 23.01).abs() < 0.1, "{:?}", l.integrated);
    }

    #[test]
    fn digital_silence_has_no_loudness() {
        let l = measure(48_000, &[Channel::C], &[vec![0.0f32; 48_000 * 3]]);
        assert!(l.integrated.is_none() && l.true_peak().is_none());
        assert!(l.dialnorm().is_none());
    }

    #[test]
    fn the_range_follows_a_change_of_level() {
        // ten seconds at -40 dBFS, then ten at -30 dBFS: the 10th to 95th percentile spans about 10 LU
        let mut tone = sine(48_000, -40.0, 20.0);
        tone.extend(sine(48_000, -30.0, 20.0));
        let l = measure(48_000, &[Channel::C], std::slice::from_ref(&tone));
        assert!((l.range.unwrap() - 10.0).abs() < 1.0, "{:?}", l.range);
    }

    #[test]
    fn dialnorm_follows_the_integrated_loudness() {
        let tone = sine(48_000, -20.0, 6.0);
        let l = measure(48_000, &[Channel::L, Channel::R], &[tone.clone(), tone]);
        assert_eq!(l.dialnorm(), Some(20));
        let loud = sine(48_000, 0.0, 6.0);
        let l = measure(48_000, &[Channel::L, Channel::R], &[loud.clone(), loud]);
        assert_eq!(l.dialnorm(), Some(1));
    }

    #[test]
    fn a_peak_between_samples_is_reported_as_a_true_peak() {
        let rate = 48_000;
        let x: Vec<f32> = (0..rate * 4)
            .map(|i| (0.9 * (2.0 * PI * 0.25 * i as f64 + PI / 4.0).sin()) as f32)
            .collect();
        let l = measure(rate as u32, &[Channel::C], std::slice::from_ref(&x));
        let c = &l.channels[0];
        // samples reach 0.9 * 0.7071 = -3.9 dBFS; the waveform reaches 0.9 = -0.9 dBTP
        assert!((c.sample_peak.unwrap() + 3.93).abs() < 0.1);
        assert!((c.true_peak.unwrap() + 0.92).abs() < 0.3, "{:?}", c.true_peak);
    }
}
