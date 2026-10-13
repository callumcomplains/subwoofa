//! WavPack's decorrelation: a chain of small adaptive predictors ("terms") that each take away what the past of the
//! signal says about the present.
//!
//! A term has a *delay* and a *rate*:
//! * delay 1 to 8 predicts a channel from its own output that many samples back;
//! * delay 17 and 18 extrapolate from the last two outputs (`2a - b`, and `(3a - b) / 2`);
//! * delay -1, -2, -3 predict one channel from the other (stereo only).
//!
//! Each term multiplies its reference by a weight (in 1/1024 units) and subtracts the result; the weight then
//! follows the sign of the match, in steps of the term's rate. The decoder runs the chain forwards and adds the
//! predictions back, so the encoder runs it backwards: the last term of the stored list sees the signal first.
//!
//! The decoder starts every block from stored weights and histories, which are limited in precision (see
//! [`super::log_tables`]). [`Chain::start_block`] rounds the encoder's state the same way and writes it out.

use super::log_tables::{log2_signed, rounded};

/// How many outputs of a channel a term remembers (delays go up to 8).
const HISTORY: usize = 8;

/// Weights are stored in 8 bits: `stored * 8`, plus a small correction (see [`unstore_weight`]).
const WEIGHT_UNIT: i32 = 8;

/// Cross-channel terms clip their weights to this many 1/1024 units.
const CROSS_LIMIT: i32 = 1024;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Term {
    pub delay: i32,
    pub rate: i32,
}

/// One term and its running state, for up to two channels.
struct Stage {
    term: Term,
    weight: [i32; 2],
    /// `recent[channel][0]` is the newest output of this stage.
    recent: [[i32; HISTORY]; 2],
    /// Latest outputs used by the cross-channel terms: the left channel's and the right channel's.
    cross: [i32; 2],
}

pub(crate) struct Chain {
    stages: Vec<Stage>,
    stereo: bool,
}

/// `weight * value` in the format's fixed-point arithmetic.
fn apply(weight: i32, value: i32) -> i32 {
    ((weight as i64 * value as i64 + 512) >> 10) as i32
}

/// Moves a weight towards the input (same sign) or away from it (opposite sign).
fn adapt(weight: &mut i32, rate: i32, reference: i32, input: i32, limit: Option<i32>) {
    if reference == 0 || input == 0 {
        return;
    }
    if (reference ^ input) < 0 {
        *weight -= rate;
        if let Some(limit) = limit {
            *weight = (*weight).max(-limit);
        }
    } else {
        *weight += rate;
        if let Some(limit) = limit {
            *weight = (*weight).min(limit);
        }
    }
}

/// The weight the decoder rebuilds from a stored byte.
fn unstore_weight(stored: i8) -> i32 {
    let weight = stored as i32 * WEIGHT_UNIT;
    if weight > 0 {
        weight + ((weight + 64) >> 7)
    } else {
        weight
    }
}

/// The inverse of [`unstore_weight`], to the nearest storable value.
fn store_weight(weight: i32) -> i8 {
    let weight = weight.min(CROSS_LIMIT);
    let base = if weight > 0 {
        weight - ((weight + 64) >> 7)
    } else {
        weight
    };
    ((base + WEIGHT_UNIT / 2) >> 3).clamp(-127, 127) as i8
}

impl Stage {
    fn new(term: Term) -> Stage {
        Stage {
            term,
            weight: [0; 2],
            recent: [[0; HISTORY]; 2],
            cross: [0; 2],
        }
    }

    /// What this term predicts for `channel` from the channel's own history.
    fn own_reference(&self, channel: usize) -> i32 {
        let r = &self.recent[channel];
        match self.term.delay {
            d if d > 8 && d & 1 == 1 => r[0].wrapping_mul(2).wrapping_sub(r[1]),
            d if d > 8 => r[0].wrapping_mul(3).wrapping_sub(r[1]) >> 1,
            d => r[d as usize - 1],
        }
    }

    fn push(&mut self, channel: usize, output: i32) {
        let r = &mut self.recent[channel];
        r.copy_within(0..HISTORY - 1, 1);
        r[0] = output;
    }

    /// One channel through a term that looks only at itself. Returns the term's input.
    fn step_own(&mut self, channel: usize, output: i32) -> i32 {
        let reference = self.own_reference(channel);
        let input = output.wrapping_sub(apply(self.weight[channel], reference));
        adapt(&mut self.weight[channel], self.term.rate, reference, input, None);
        self.push(channel, output);
        input
    }

    /// Both channels through a cross-channel term. Returns the term's inputs.
    fn step_cross(&mut self, left: i32, right: i32) -> (i32, i32) {
        let rate = self.term.rate;
        let limit = Some(CROSS_LIMIT);
        if self.term.delay == -1 {
            let old_right = self.cross[0];
            let left_in = left.wrapping_sub(apply(self.weight[0], old_right));
            adapt(&mut self.weight[0], rate, old_right, left_in, limit);
            let right_in = right.wrapping_sub(apply(self.weight[1], left));
            adapt(&mut self.weight[1], rate, left, right_in, limit);
            self.cross[0] = right;
            (left_in, right_in)
        } else {
            let old_left = self.cross[1];
            let right_in = right.wrapping_sub(apply(self.weight[1], old_left));
            adapt(&mut self.weight[1], rate, old_left, right_in, limit);
            let reference = if self.term.delay == -3 {
                std::mem::replace(&mut self.cross[0], right)
            } else {
                right
            };
            let left_in = left.wrapping_sub(apply(self.weight[0], reference));
            adapt(&mut self.weight[0], rate, reference, left_in, limit);
            self.cross[1] = left;
            (left_in, right_in)
        }
    }
}

impl Chain {
    /// `terms` in stored order: the first one acts on the signal first when encoding.
    pub fn new(terms: &[Term], stereo: bool) -> Chain {
        Chain {
            stages: terms.iter().map(|&t| Stage::new(t)).collect(),
            stereo,
        }
    }

    pub fn terms(&self) -> impl Iterator<Item = Term> + '_ {
        self.stages.iter().map(|s| s.term)
    }

    /// Residual of one mono sample.
    pub fn mono(&mut self, sample: i32) -> i32 {
        self.stages.iter_mut().fold(sample, |x, stage| stage.step_own(0, x))
    }

    /// Residuals of one stereo sample pair.
    pub fn pair(&mut self, left: i32, right: i32) -> (i32, i32) {
        self.stages.iter_mut().fold((left, right), |(l, r), stage| {
            if stage.term.delay < 0 {
                stage.step_cross(l, r)
            } else {
                (stage.step_own(0, l), stage.step_own(1, r))
            }
        })
    }

    /// Rounds the state to what the decoder can be told and returns the three block sub-blocks that tell it:
    /// terms, weights and histories.
    pub fn start_block(&mut self) -> StartState {
        let channels = if self.stereo { 2 } else { 1 };
        let mut state = StartState::default();
        for stage in &mut self.stages {
            state
                .terms
                .push(((stage.term.delay + 5) & 0x1F) as u8 | (stage.term.rate << 5) as u8);
            for c in 0..channels {
                let stored = store_weight(stage.weight[c]);
                stage.weight[c] = unstore_weight(stored);
                state.weights.push(stored as u8);
            }
            stage.write_history(channels, &mut state.history);
        }
        state
    }
}

impl Stage {
    /// The values the decoder is given as this stage's history, in its order, rounded as the decoder will see them.
    fn write_history(&mut self, channels: usize, out: &mut Vec<u8>) {
        let delay = self.term.delay;
        let mut put = |value: &mut i32| {
            *value = rounded(*value);
            out.extend_from_slice(&(log2_signed(*value) as i16).to_le_bytes());
        };
        if delay > 8 {
            for c in 0..channels {
                put(&mut self.recent[c][0]);
                put(&mut self.recent[c][1]);
            }
        } else if delay < 0 {
            put(&mut self.cross[0]);
            put(&mut self.cross[1]);
        } else {
            // the decoder reads slot j for the sample `delay - j` back: oldest first
            for slot in 0..delay as usize {
                for c in 0..channels {
                    put(&mut self.recent[c][delay as usize - 1 - slot]);
                }
            }
        }
    }
}

/// The three sub-blocks that give the decoder its starting state.
#[derive(Default)]
pub(crate) struct StartState {
    pub terms: Vec<u8>,
    pub weights: Vec<u8>,
    pub history: Vec<u8>,
}

/// The terms the encoder uses, in stored order. Chosen by trying variations on film music (5.1, 24 bit): this
/// set lands within a fraction of a percent of FFmpeg's default WavPack encoder; more terms gained under 1%.
pub(crate) fn preset(stereo: bool) -> Vec<Term> {
    let own = |delay, rate| Term { delay, rate };
    if stereo {
        vec![own(18, 2), own(18, 2), own(2, 2), own(3, 2), own(-2, 2), own(5, 2)]
    } else {
        vec![own(18, 2), own(18, 2), own(2, 2), own(3, 2), own(5, 2)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decoder side of one own-history term, written from the format's description.
    fn decode_mono(terms: &[Term], residuals: &[i32]) -> Vec<i32> {
        let mut stages: Vec<Stage> = terms.iter().map(|&t| Stage::new(t)).collect();
        residuals
            .iter()
            .map(|&residual| {
                stages.iter_mut().rev().fold(residual, |input, stage| {
                    let reference = stage.own_reference(0);
                    let output = input.wrapping_add(apply(stage.weight[0], reference));
                    adapt(&mut stage.weight[0], stage.term.rate, reference, input, None);
                    stage.push(0, output);
                    output
                })
            })
            .collect()
    }

    #[test]
    fn mono_round_trip() {
        let signal: Vec<i32> = (0..20_000)
            .map(|n| ((n as f64 * 0.05).sin() * 9000.0) as i32 + (n * 7919 % 13))
            .collect();
        let mut chain = Chain::new(&preset(false), false);
        let residuals: Vec<i32> = signal.iter().map(|&s| chain.mono(s)).collect();
        assert_eq!(decode_mono(&preset(false), &residuals), signal);
        let energy = |v: &[i32]| v.iter().map(|&x| (x as i64).pow(2)).sum::<i64>();
        assert!(energy(&residuals) < energy(&signal) / 20);
    }

    #[test]
    fn weight_storage_is_stable() {
        for stored in [-127i8, -5, 0, 1, 64, 127] {
            assert_eq!(store_weight(unstore_weight(stored)), stored);
        }
    }
}
