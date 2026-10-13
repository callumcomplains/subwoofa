//! Statistics gathered while decoding: levels, silence, clipping, channel activity.

use crate::{Collector, Status};

pub(crate) struct Stats {
    pub(crate) channels: usize,
    pub(crate) frames: u64,
    pub(crate) peak: Vec<i32>,
    pub(crate) sum: Vec<i64>,
    pub(crate) sumsq: Vec<f64>,
    pub(crate) clipped: Vec<u64>,
    pub(crate) zero_run: u64,
    pub(crate) longest_zero: u64,
    pub(crate) longest_zero_at: u64,
    pub(crate) cur_zero_start: u64,
    pub(crate) first_sound: Option<u64>,
    pub(crate) last_sound: u64,
    pub(crate) or_bits: Vec<u32>,
    pub(crate) hashes: Vec<u64>,
    pub(crate) silent_all: Vec<bool>,
}

impl Stats {
    pub(crate) fn new(channels: usize) -> Stats {
        Stats {
            channels,
            frames: 0,
            peak: vec![0; channels],
            sum: vec![0; channels],
            sumsq: vec![0.0; channels],
            clipped: vec![0; channels],
            zero_run: 0,
            longest_zero: 0,
            longest_zero_at: 0,
            cur_zero_start: 0,
            first_sound: None,
            last_sound: 0,
            or_bits: vec![0; channels],
            hashes: vec![0xcbf29ce484222325; channels],
            silent_all: vec![true; channels],
        }
    }
    pub(crate) fn feed(&mut self, d: &[u8]) {
        let ch = self.channels;
        for frame in d.chunks_exact(3 * ch) {
            let mut all_zero = true;
            for k in 0..ch {
                let b = &frame[k * 3..k * 3 + 3];
                let v = ((b[0] as i32) | ((b[1] as i32) << 8) | ((b[2] as i32) << 16)) << 8 >> 8;
                if v != 0 {
                    all_zero = false;
                    self.silent_all[k] = false;
                }
                let a = v.abs();
                if a > self.peak[k] {
                    self.peak[k] = a;
                }
                self.sum[k] += v as i64;
                self.sumsq[k] += (v as f64) * (v as f64);
                if a >= 8_388_607 {
                    self.clipped[k] += 1;
                }
                self.or_bits[k] |= v as u32 & 0xFF_FFFF;
                self.hashes[k] = (self.hashes[k] ^ (v as u32 as u64)).wrapping_mul(0x100000001b3);
            }
            if all_zero {
                if self.zero_run == 0 {
                    self.cur_zero_start = self.frames;
                }
                self.zero_run += 1;
                if self.zero_run > self.longest_zero {
                    self.longest_zero = self.zero_run;
                    self.longest_zero_at = self.cur_zero_start;
                }
            } else {
                self.zero_run = 0;
                if self.first_sound.is_none() {
                    self.first_sound = Some(self.frames);
                }
                self.last_sound = self.frames;
            }
            self.frames += 1;
        }
    }
    pub(crate) fn report(&self, c: &mut Collector, rate: u32) {
        let r = rate as f64;
        let db = |x: f64| {
            if x <= 0.0 {
                "-inf".to_string()
            } else {
                format!("{:.1}", 20.0 * (x / 8_388_608.0).log10())
            }
        };
        let mut rows = vec![(
            "samples per channel".to_string(),
            format!("{} ({:.3} s at {rate} Hz)", self.frames, self.frames as f64 / r),
        )];
        let mut clip_total = 0u64;
        for k in 0..self.channels {
            let rms = (self.sumsq[k] / self.frames.max(1) as f64).sqrt();
            let dc = self.sum[k] as f64 / self.frames.max(1) as f64;
            let wasted = if self.or_bits[k] == 0 {
                24
            } else {
                self.or_bits[k].trailing_zeros()
            };
            clip_total += self.clipped[k];
            rows.push((
                format!("channel {}", k + 1),
                if self.silent_all[k] {
                    "digital silence throughout".to_string()
                } else {
                    format!(
                        "peak {} dBFS, RMS {} dBFS, DC {:.1}, {} unused low bit(s), {} full-scale sample(s)",
                        db(self.peak[k] as f64),
                        db(rms),
                        dc,
                        wasted,
                        self.clipped[k]
                    )
                },
            ));
        }
        c.section("Decoded audio", rows);
        let silent: Vec<usize> = (0..self.channels)
            .filter(|k| self.silent_all[*k])
            .map(|k| k + 1)
            .collect();
        c.check(
            "Channels carry audio",
            if silent.is_empty() || silent.len() == self.channels {
                if silent.len() == self.channels {
                    Status::Warn
                } else {
                    Status::Pass
                }
            } else {
                Status::Info
            },
            if silent.is_empty() {
                "every channel has signal".to_string()
            } else if silent.len() == self.channels {
                "the whole programme is digital silence".to_string()
            } else {
                format!(
                    "channel(s) {silent:?} are digital silence throughout (normal for unused LFE or height channels)"
                )
            },
        );
        let mut dup = Vec::new();
        for a in 0..self.channels {
            for b in a + 1..self.channels {
                if !self.silent_all[a] && self.hashes[a] == self.hashes[b] {
                    dup.push(format!("{}={}", a + 1, b + 1));
                }
            }
        }
        if !dup.is_empty() {
            c.check(
                "Identical channels",
                Status::Info,
                format!("these channels are sample-for-sample identical: {}", dup.join(", ")),
            );
        }
        c.check("Clipping", if clip_total == 0 { Status::Pass } else { Status::Warn }, if clip_total == 0 { "no sample reaches full scale".to_string() } else { format!("{clip_total} full-scale sample(s) in all channels (decoded audio, so this includes any clipping in the source)") });
        if let Some(f) = self.first_sound {
            c.kv("Silence before the first sound", format!("{:.3} s", f as f64 / r));
            c.kv(
                "Silence after the last sound",
                format!("{:.3} s", (self.frames - 1 - self.last_sound) as f64 / r),
            );
        }
        if self.longest_zero as f64 / r > 1.0
            && self.first_sound.is_some_and(|f| self.longest_zero_at > f)
            && self.longest_zero_at + self.longest_zero < self.last_sound
        {
            c.check(
                "Dropouts",
                Status::Warn,
                format!(
                    "{:.2} s of digital silence in all channels at {:.2} s",
                    self.longest_zero as f64 / r,
                    self.longest_zero_at as f64 / r
                ),
            );
        } else {
            c.check(
                "Dropouts",
                Status::Pass,
                "no stretch of silence longer than a second inside the programme",
            );
        }
    }
}
