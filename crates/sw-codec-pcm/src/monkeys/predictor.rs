//! Monkey's Audio's two-stage predictor (version 3.95 and later), written as the encoder's mirror of the decoder.
//!
//! Stereo is coded as `Y = right - left` and `X = left + Y / 2`; each of the two channels then runs through
//! * stage A: a four-tap adaptive filter over its own recent output (the last value and three first differences),
//! * stage B (stereo only): a five-tap filter over the *other* channel's output, passed through a leaky first-order
//!   filter (`x - 31/32 * previous`),
//! * a leaky first-order filter on the way out (`value - 31/32 * previous value`).
//!
//! The taps follow the sign of the residual times the sign of their input (sign-sign LMS). Predictions are formed
//! in 32-bit arithmetic as the decoder does for streams up to 24 bit; the running state is 64 bits wide.

/// Starting weights of stage A: 0.35, 0.31, -0.11, 0.10 in 1/1024 units.
const START_A: [i64; 4] = [360, 317, -109, 98];

/// A weighted sum of taps is scaled by 2^10.
const WEIGHT_SHIFT: u32 = 10;

const TAPS_A: usize = 4;
const TAPS_B: usize = 5;

/// `x * 31/32`, the leak of the first-order filters.
fn leak(x: i64) -> i64 {
    x.wrapping_mul(31) >> 5
}

fn sign(x: i64) -> i64 {
    x.signum()
}

/// A line of taps: the newest value, then first differences between older values (most recent first).
struct Line {
    len: usize,
    /// The value the previous step started from, which the next first difference is taken against.
    previous: i64,
    differences: [i64; TAPS_B - 1],
}

impl Line {
    fn new(len: usize) -> Line {
        Line { len, previous: 0, differences: [0; TAPS_B - 1] }
    }

    /// The tap values for a step that starts from `newest`, and the fresh first difference among them.
    fn taps(&self, newest: i64) -> ([i64; TAPS_B], i64) {
        let fresh = newest.wrapping_sub(self.previous);
        let mut taps = [0; TAPS_B];
        taps[0] = newest;
        taps[1] = fresh;
        taps[2..self.len].copy_from_slice(&self.differences[..self.len - 2]);
        (taps, fresh)
    }

    fn advance(&mut self, newest: i64, fresh: i64) {
        self.previous = newest;
        self.differences.copy_within(0..TAPS_B - 2, 1);
        self.differences[0] = fresh;
    }
}

/// Weighted sum of taps, wrapping as 64-bit arithmetic does.
fn weigh(taps: &[i64], weights: &[i64]) -> i64 {
    taps.iter().zip(weights).fold(0i64, |sum, (&t, &w)| sum.wrapping_add(t.wrapping_mul(w)))
}

/// Moves each weight by the sign of its tap times the sign of the residual.
fn nudge(weights: &mut [i64], taps: &[i64], residual: i32) {
    let direction = sign(residual as i64);
    for (w, &t) in weights.iter_mut().zip(taps) {
        *w = w.wrapping_add(sign(t) * direction);
    }
}

/// One channel's state.
struct Channel {
    line_a: Line,
    weights_a: [i64; TAPS_A],
    line_b: Line,
    weights_b: [i64; TAPS_B],
    /// What the decoder holds before its output filter for the previous sample.
    last: i64,
    /// This channel's previous output (the input of the leaky output filter, and of the other channel's stage B).
    output: i64,
    /// The other channel's previous output as stage B last saw it (for the leak).
    seen_other: i64,
}

impl Channel {
    fn new() -> Channel {
        Channel {
            line_a: Line::new(TAPS_A),
            weights_a: START_A,
            line_b: Line::new(TAPS_B),
            weights_b: [0; TAPS_B],
            last: 0,
            output: 0,
            seen_other: 0,
        }
    }

    /// Takes the value the decoder must output; returns the residual that gets it there. `other` is the other
    /// channel's output the decoder will have at this point (`None` for mono: no stage B).
    fn step(&mut self, value: i32, other: Option<i64>) -> i32 {
        let (taps_a, fresh_a) = self.line_a.taps(self.last);
        let sum_a = weigh(&taps_a[..TAPS_A], &self.weights_a) as i32;

        let stage_b = other.map(|other| {
            let start = other.wrapping_sub(leak(self.seen_other));
            let (taps_b, fresh_b) = self.line_b.taps(start);
            (other, start, taps_b, fresh_b, weigh(&taps_b, &self.weights_b) as i32)
        });
        let sum_b = stage_b.map_or(0, |b| b.4);

        let prediction = sum_a.wrapping_add(sum_b >> 1) >> WEIGHT_SHIFT;
        let before_filter = value as i64 - leak(self.output);
        let residual = (before_filter as i32).wrapping_sub(prediction);

        nudge(&mut self.weights_a, &taps_a[..TAPS_A], residual);
        self.line_a.advance(self.last, fresh_a);
        if let Some((other, start, taps_b, fresh_b, _)) = stage_b {
            nudge(&mut self.weights_b, &taps_b, residual);
            self.line_b.advance(start, fresh_b);
            self.seen_other = other;
        }
        self.last = before_filter as i32 as i64;
        self.output = value as i64;
        residual
    }
}

/// The predictor of a mono or stereo stream.
pub(crate) struct Predictor {
    channels: [Channel; 2],
}

impl Predictor {
    pub fn new() -> Predictor {
        Predictor { channels: [Channel::new(), Channel::new()] }
    }

    pub fn mono(&mut self, value: i32) -> i32 {
        self.channels[0].step(value, None)
    }

    /// Residuals of one stereo pair: `(Y, X)`.
    pub fn stereo(&mut self, left: i32, right: i32) -> (i32, i32) {
        let side = right.wrapping_sub(left);
        let mid = left.wrapping_add(side / 2);
        let other = self.channels[1].output;
        let first = self.channels[0].step(side, Some(other));
        let other = self.channels[0].output;
        let second = self.channels[1].step(mid, Some(other));
        (first, second)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leak_is_a_32nd_short() {
        assert_eq!(leak(3200), 3100);
        assert_eq!(leak(-3200), -3100);
    }

    #[test]
    fn taps_form_differences() {
        let mut line = Line::new(TAPS_A);
        let (taps, fresh) = line.taps(10);
        assert_eq!(&taps[..4], &[10, 10, 0, 0]);
        line.advance(10, fresh);
        let (taps, _) = line.taps(14);
        assert_eq!(&taps[..4], &[14, 4, 10, 0]);
    }

    #[test]
    fn prediction_shrinks_a_tone() {
        let tone: Vec<i32> = (0..20_000).map(|n| ((n as f64 * 0.03).sin() * 8000.0) as i32).collect();
        let mut predictor = Predictor::new();
        let residuals: Vec<i32> = tone.iter().map(|&s| predictor.mono(s)).collect();
        let energy = |v: &[i32]| v.iter().map(|&x| (x as i64).pow(2)).sum::<i64>();
        assert!(energy(&residuals[5_000..]) < energy(&tone[5_000..]) / 100);
    }
}
