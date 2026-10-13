//! Downmix matrices (N channels in -> M channels out) and a saturation detector.
//!
//! Gains are stored in dB; `f32::NEG_INFINITY` means "not connected". Defaults follow the
//! common ITU-R BS.775 conventions (centre and surrounds at -3 dB into the fronts, LFE
//! omitted from stereo). They are starting points, editable per cell.

use crate::channel::Channel::{self, *};
use crate::layout::ChannelLayout;
use serde::{Deserialize, Serialize};

pub const OFF: f32 = f32::NEG_INFINITY;
const M3: f32 = -3.0103; // 20*log10(1/sqrt(2))

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    pub gain_db: f32,
    /// Invert polarity before mixing (180 degree phase).
    pub invert: bool,
}

impl Cell {
    const fn off() -> Cell {
        Cell {
            gain_db: OFF,
            invert: false,
        }
    }
    pub fn linear(&self) -> f32 {
        if self.gain_db == OFF {
            0.0
        } else {
            let g = 10f32.powf(self.gain_db / 20.0);
            if self.invert {
                -g
            } else {
                g
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Matrix {
    pub inputs: Vec<Channel>,
    pub outputs: Vec<Channel>,
    /// `cells[out][in]`.
    pub cells: Vec<Vec<Cell>>,
}

impl Matrix {
    pub fn empty(inputs: &[Channel], outputs: &[Channel]) -> Matrix {
        Matrix {
            inputs: inputs.to_vec(),
            outputs: outputs.to_vec(),
            cells: vec![vec![Cell::off(); inputs.len()]; outputs.len()],
        }
    }

    pub fn set(&mut self, out: Channel, inp: Channel, gain_db: f32) {
        if let (Some(o), Some(i)) = (self.out_idx(out), self.in_idx(inp)) {
            self.cells[o][i].gain_db = gain_db;
        }
    }

    pub fn out_idx(&self, c: Channel) -> Option<usize> {
        self.outputs.iter().position(|x| *x == c)
    }
    pub fn in_idx(&self, c: Channel) -> Option<usize> {
        self.inputs.iter().position(|x| *x == c)
    }

    /// Mix one interleaved input frame into `out` (length = outputs.len()).
    pub fn apply(&self, frame: &[f32], out: &mut [f32]) {
        for (o, row) in self.cells.iter().enumerate() {
            let mut acc = 0.0;
            for (i, cell) in row.iter().enumerate() {
                if cell.gain_db != OFF {
                    acc += cell.linear() * frame[i];
                }
            }
            out[o] = acc;
        }
    }

    /// Sum of linear gains feeding each output (worst-case amplitude if every input is full scale
    /// and in phase). Anything over 1.0 can clip.
    pub fn worst_case_gain(&self) -> Vec<f32> {
        self.cells
            .iter()
            .map(|r| r.iter().map(|c| c.linear().abs()).sum())
            .collect()
    }

    /// Attenuation in dB (<= 0) that guarantees no clipping from full-scale inputs.
    pub fn safe_trim_db(&self) -> f32 {
        let worst = self.worst_case_gain().into_iter().fold(0.0f32, f32::max);
        if worst <= 1.0 {
            0.0
        } else {
            -20.0 * worst.log10()
        }
    }

    /// Apply a uniform trim to every connected cell.
    pub fn trim(&mut self, db: f32) {
        for row in &mut self.cells {
            for c in row {
                if c.gain_db != OFF {
                    c.gain_db += db;
                }
            }
        }
    }

    /// Default Lo/Ro stereo downmix for any layout.
    pub fn lo_ro(layout: &ChannelLayout) -> Matrix {
        if layout.channels.iter().any(|c| matches!(c, Tfc | Tc | Bfc | Lfe2)) {
            return Matrix::pan(layout.channels, &[L, R]);
        }
        let mut m = Matrix::empty(layout.channels, &[L, R]);
        for &ch in layout.channels {
            let (l, r): (f32, f32) = match ch {
                L | Lt => (0.0, OFF),
                R | Rt => (OFF, 0.0),
                C | Ch => (M3, M3),
                Ls | Lss | Lsr | Lw => (M3, OFF),
                Rs | Rss | Rsr | Rw => (OFF, M3),
                Lh | Lhs => (M3, OFF),
                Rh | Rhs => (OFF, M3),
                Cs => (-6.0206, -6.0206),
                Oh => (-6.0206, -6.0206),
                _ => (OFF, OFF),
            };
            m.set(L, ch, l);
            m.set(R, ch, r);
        }
        m
    }

    /// Default 5.1 downmix for 6.x/7.x layouts (surplus channels folded into the nearest bed).
    pub fn to_51(layout: &ChannelLayout) -> Matrix {
        if layout.channels.iter().any(|c| matches!(c, Tfc | Tc | Bfc | Lfe2)) {
            return Matrix::pan(layout.channels, &[L, R, C, Lfe, Ls, Rs]);
        }
        let mut m = Matrix::empty(layout.channels, &[L, R, C, Lfe, Ls, Rs]);
        for &ch in layout.channels {
            // (target, gain dB) pairs
            let routes: &[(Channel, f32)] = match ch {
                L | Lt => &[(L, 0.0)],
                R | Rt => &[(R, 0.0)],
                C => &[(C, 0.0)],
                Lfe => &[(Lfe, 0.0)],
                Ls => &[(Ls, 0.0)],
                Rs => &[(Rs, 0.0)],
                // side/rear pairs: unity into the surround when it is the only one, else -3 dB
                Lss | Lsr => &[(Ls, M3)],
                Rss | Rsr => &[(Rs, M3)],
                Cs => &[(Ls, M3), (Rs, M3)],
                Lw => &[(L, M3)],
                Rw => &[(R, M3)],
                Lh | Lhs => &[(L, M3)],
                Rh | Rhs => &[(R, M3)],
                Ch => &[(C, M3)],
                Oh => &[(L, -6.0206), (R, -6.0206)],
                _ => &[],
            };
            for &(t, g) in routes {
                m.set(t, ch, g);
            }
        }
        m
    }
}

/// Detects digital clipping: `min_run` or more consecutive full-scale samples on one channel.
#[derive(Debug, Clone)]
pub struct SaturationCheck {
    channels: usize,
    min_run: usize,
    run: Vec<usize>,
    frame: u64,
    pub peak: Vec<f32>,
    pub events: Vec<SatEvent>,
    pub event_count: u64,
    max_events: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SatEvent {
    pub channel: usize,
    pub frame: u64,
}

impl SaturationCheck {
    pub fn new(channels: usize) -> Self {
        // The legacy tool treats three consecutive samples at 0 dBFS as a saturation.
        SaturationCheck {
            channels,
            min_run: 3,
            run: vec![0; channels],
            frame: 0,
            peak: vec![0.0; channels],
            events: Vec::new(),
            event_count: 0,
            max_events: 200,
        }
    }

    /// Feed interleaved frames.
    pub fn feed(&mut self, interleaved: &[f32]) {
        for f in interleaved.chunks_exact(self.channels) {
            for (c, &s) in f.iter().enumerate() {
                let a = s.abs();
                if a > self.peak[c] {
                    self.peak[c] = a;
                }
                if a >= 0.999_97 {
                    self.run[c] += 1;
                    if self.run[c] == self.min_run {
                        self.event_count += 1;
                        if self.events.len() < self.max_events {
                            self.events.push(SatEvent {
                                channel: c,
                                frame: self.frame,
                            });
                        }
                    }
                } else {
                    self.run[c] = 0;
                }
            }
            self.frame += 1;
        }
    }

    pub fn frames(&self) -> u64 {
        self.frame
    }

    pub fn peak_db(&self) -> Vec<f32> {
        self.peak
            .iter()
            .map(|p| if *p <= 0.0 { OFF } else { 20.0 * p.log10() })
            .collect()
    }
}

/// Nominal speaker position: azimuth in degrees (positive to the left), elevation in degrees.
fn position(ch: Channel, wide_is_front_centre: bool) -> Option<(f32, f32)> {
    Some(match ch {
        L | Lt => (30.0, 0.0),
        R | Rt => (-30.0, 0.0),
        C => (0.0, 0.0),
        Ls => (110.0, 0.0),
        Rs => (-110.0, 0.0),
        Cs => (180.0, 0.0),
        Lsr => (135.0, 0.0),
        Rsr => (-135.0, 0.0),
        Lss => (90.0, 0.0),
        Rss => (-90.0, 0.0),
        Lw => (if wide_is_front_centre { 15.0 } else { 60.0 }, 0.0),
        Rw => (if wide_is_front_centre { -15.0 } else { -60.0 }, 0.0),
        Lh => (30.0, 45.0),
        Rh => (-30.0, 45.0),
        Lhs => (90.0, 45.0),
        Rhs => (-90.0, 45.0),
        Ch => (0.0, 45.0),
        Tfc => (0.0, 45.0),
        Tfl => (45.0, 45.0),
        Tfr => (-45.0, 45.0),
        Tsl => (90.0, 45.0),
        Tsr => (-90.0, 45.0),
        Tbl => (135.0, 45.0),
        Tbr => (-135.0, 45.0),
        Tbc => (180.0, 45.0),
        Bfc => (0.0, -30.0),
        Bfl => (45.0, -30.0),
        Bfr => (-45.0, -30.0),
        Oh | Tc | Lfe | Lfe2 => return None,
    })
}

impl Matrix {
    /// Generic downmix by constant-power panning between the two nearest output speakers (in azimuth);
    /// elevated and lowered sources lose 3 dB, overhead sources are spread over every output speaker, and
    /// the LFE channels go to the output LFE (-3 dB each when there are two).
    pub fn pan(inputs: &[Channel], outputs: &[Channel]) -> Matrix {
        let mut m = Matrix::empty(inputs, outputs);
        let wide_front = inputs.contains(&Tfc);
        let mut spk: Vec<(f32, Channel)> = outputs
            .iter()
            .filter_map(|&o| {
                position(o, false)
                    .filter(|p| p.1 == 0.0)
                    .map(|p| (p.0.rem_euclid(360.0), o))
            })
            .collect();
        spk.sort_by(|a, b| a.0.total_cmp(&b.0));
        let lfe_n = inputs.iter().filter(|c| matches!(c, Lfe | Lfe2)).count();
        for &ch in inputs {
            if matches!(ch, Lfe | Lfe2) {
                m.set(Lfe, ch, if lfe_n > 1 { M3 } else { 0.0 });
                continue;
            }
            if spk.is_empty() {
                continue;
            }
            let Some((az, el)) = position(ch, wide_front) else {
                // overhead: equal power into every speaker, 3 dB down
                let g = -10.0 * (spk.len() as f32).log10() + M3;
                for &(_, o) in &spk {
                    m.set(o, ch, g);
                }
                continue;
            };
            let az = az.rem_euclid(360.0);
            let trim = if el != 0.0 { M3 } else { 0.0 };
            // neighbours around the circle
            let n = spk.len();
            if n == 1 {
                m.set(spk[0].1, ch, trim);
                continue;
            }
            let hi = spk.iter().position(|s| s.0 >= az).unwrap_or(0);
            let lo = (hi + n - 1) % n;
            let (a_lo, a_hi) = (spk[lo].0, spk[hi].0);
            if (a_hi - az).abs() < 1e-3 {
                m.set(spk[hi].1, ch, trim);
                continue;
            }
            let span = (a_hi - a_lo).rem_euclid(360.0);
            let t = ((az - a_lo).rem_euclid(360.0) / span).clamp(0.0, 1.0);
            let (g_lo, g_hi) = (
                (t * std::f32::consts::FRAC_PI_2).cos(),
                (t * std::f32::consts::FRAC_PI_2).sin(),
            );
            for (o, g) in [(spk[lo].1, g_lo), (spk[hi].1, g_hi)] {
                if g > 1e-4 {
                    m.set(o, ch, 20.0 * g.log10() + trim);
                }
            }
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lay(id: &str) -> &'static ChannelLayout {
        ChannelLayout::find(id).unwrap()
    }

    #[test]
    fn stereo_downmix_of_51() {
        let m = Matrix::lo_ro(lay("5.1"));
        // L, R, C, LFE, Ls, Rs
        let mut out = [0.0; 2];
        m.apply(&[1.0, 0.0, 0.0, 1.0, 0.0, 0.0], &mut out);
        assert!(
            (out[0] - 1.0).abs() < 1e-6 && out[1] == 0.0,
            "LFE must be dropped: {out:?}"
        );
        m.apply(&[0.0, 0.0, 1.0, 0.0, 0.0, 0.0], &mut out);
        assert!((out[0] - 0.7071).abs() < 1e-3 && (out[1] - 0.7071).abs() < 1e-3);
    }

    #[test]
    fn worst_case_and_safe_trim() {
        let m = Matrix::lo_ro(lay("5.1"));
        // 1 + 0.707 + 0.707
        assert!((m.worst_case_gain()[0] - 2.414).abs() < 1e-2);
        let mut t = m.clone();
        t.trim(m.safe_trim_db());
        assert!(t.worst_case_gain().iter().all(|g| *g <= 1.0001));
    }

    #[test]
    fn seven_one_to_51_folds_rears() {
        let m = Matrix::to_51(lay("7.1-ls-rs-lsr-rsr"));
        let ls = m.out_idx(Ls).unwrap();
        // Lsr feeds Ls at -3 dB, Ls feeds Ls at 0 dB
        assert_eq!(m.cells[ls][m.in_idx(Ls).unwrap()].gain_db, 0.0);
        assert!((m.cells[ls][m.in_idx(Lsr).unwrap()].gain_db - M3).abs() < 1e-3);
        // 5.1 passes through untouched
        let id = Matrix::to_51(lay("5.1"));
        assert!(id
            .cells
            .iter()
            .enumerate()
            .all(|(o, row)| row.iter().enumerate().all(|(i, c)| (c.gain_db == 0.0) == (o == i))));
    }

    #[test]
    fn saturation_needs_three_in_a_row() {
        let mut s = SaturationCheck::new(1);
        s.feed(&[1.0, 1.0, 0.2, 1.0, 1.0, 1.0, 1.0, 0.0]);
        assert_eq!(s.event_count, 1, "only the 4-run counts, once");
        assert_eq!(s.events[0].frame, 5);
    }

    #[test]
    fn phase_invert() {
        let mut m = Matrix::empty(&[L], &[L]);
        m.set(L, L, 0.0);
        m.cells[0][0].invert = true;
        let mut out = [0.0];
        m.apply(&[0.5], &mut out);
        assert_eq!(out[0], -0.5);
    }
}
