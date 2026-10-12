//! The "neural" filters of Monkey's Audio: long adaptive FIR filters that predict a sample from the last `order`
//! outputs of the same filter, sign-LMS style.
//!
//! The decoder runs them first (smallest order first) on the entropy-decoded residuals; the encoder therefore runs
//! them last and in the opposite order. A filter step is
//! * prediction = (sum of coefficient * past output + rounding) >> `shift`,
//! * input = output - prediction,
//! * each coefficient moves by the sign of the input times its own *adapt* value, which was set when that past
//!   output was produced (its sign, scaled by how large it was compared with the running average).

/// A filter's size and precision, and one level of the compression setting.
#[derive(Clone, Copy)]
pub(crate) struct Spec {
    pub order: usize,
    pub shift: u32,
}

/// The adapt values of the last outputs that get halved after each step (counted back from the newest).
const HALVED: [usize; 3] = [1, 2, 8];

/// How far the sliding windows may run before they are moved back to the start.
const SLACK: usize = 4096;

pub(crate) struct Filter {
    spec: Spec,
    coeffs: Vec<i16>,
    /// Past outputs (clipped to 16 bit); the window is `outputs[start..start + order]`.
    outputs: Vec<i16>,
    /// Adapt values, aligned with `outputs`.
    adapt: Vec<i16>,
    start: usize,
    /// Running average of recent output magnitudes.
    average: u32,
}

impl Filter {
    pub fn new(spec: Spec) -> Filter {
        let size = spec.order + SLACK;
        Filter {
            spec,
            coeffs: vec![0; spec.order],
            outputs: vec![0; size],
            adapt: vec![0; size],
            start: 0,
            average: 0,
        }
    }

    /// Takes the output the decoder must produce and returns the input that gets it there.
    pub fn encode(&mut self, output: i32) -> i32 {
        let order = self.spec.order;
        let window = self.start..self.start + order;
        let sum = dot(&self.coeffs, &self.outputs[window.clone()]);
        let rounding = 1i64 << (self.spec.shift - 1);
        let prediction = ((sum as i64 + rounding) >> self.spec.shift) as i32;
        let input = output.wrapping_sub(prediction);

        nudge(&mut self.coeffs, &self.adapt[window], input);
        self.push(output);
        input
    }

    /// Records an output and the adapt value that goes with it.
    fn push(&mut self, output: i32) {
        let order = self.spec.order;
        if self.start + order == self.outputs.len() {
            self.outputs.copy_within(self.start.., 0);
            self.adapt.copy_within(self.start.., 0);
            self.start = 0;
        }
        let at = self.start + order;
        self.outputs[at] = output.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        self.adapt[at] = self.adapt_value(output);
        for back in HALVED {
            self.adapt[at - back] >>= 1;
        }
        self.start += 1;
    }

    /// Sign of the output, scaled up when it is large against the running average (and the average moves).
    fn adapt_value(&mut self, output: i32) -> i16 {
        let size = output.unsigned_abs();
        let average = self.average;
        let scale = if size == 0 {
            0
        } else {
            let big = (size as u64 > average as u64 * 3) as u32;
            let medium = (size > average.wrapping_add(average / 3)) as u32;
            8i16 << (big + medium)
        };
        let step = (size.wrapping_sub(average) as i32) / 16;
        self.average = average.wrapping_add(step as u32);
        if output < 0 {
            -scale
        } else {
            scale
        }
    }
}

/// Sum of products, wrapping at 32 bits as the format does.
fn dot(coeffs: &[i16], outputs: &[i16]) -> i32 {
    coeffs
        .iter()
        .zip(outputs)
        .fold(0i32, |acc, (&c, &o)| acc.wrapping_add(c as i32 * o as i32))
}

/// Moves every coefficient by the adapt value of its output, towards the sign of the input.
fn nudge(coeffs: &mut [i16], adapt: &[i16], input: i32) {
    match input.signum() {
        1 => coeffs.iter_mut().zip(adapt).for_each(|(c, &a)| *c = c.wrapping_add(a)),
        -1 => coeffs.iter_mut().zip(adapt).for_each(|(c, &a)| *c = c.wrapping_sub(a)),
        _ => {}
    }
}

/// One channel's chain of filters for a compression level, in the order the decoder applies them.
pub(crate) struct Chain {
    filters: Vec<Filter>,
}

impl Chain {
    pub fn new(specs: &[Spec]) -> Chain {
        Chain {
            filters: specs.iter().map(|&s| Filter::new(s)).collect(),
        }
    }

    /// The residual that goes to the entropy coder for a predictor output (last filter first).
    pub fn encode(&mut self, value: i32) -> i32 {
        self.filters.iter_mut().rev().fold(value, |x, filter| filter.encode(x))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A decoder-side filter, written from the format's description.
    fn decode(spec: Spec, inputs: &[i32]) -> Vec<i32> {
        let mut f = Filter::new(spec);
        inputs
            .iter()
            .map(|&input| {
                let order = spec.order;
                let window = f.start..f.start + order;
                let sum = dot(&f.coeffs, &f.outputs[window.clone()]);
                let prediction = ((sum as i64 + (1i64 << (spec.shift - 1))) >> spec.shift) as i32;
                let output = input.wrapping_add(prediction);
                nudge(&mut f.coeffs, &f.adapt[window], input);
                f.push(output);
                output
            })
            .collect()
    }

    #[test]
    fn filter_round_trip() {
        let spec = Spec { order: 16, shift: 11 };
        let signal: Vec<i32> = (0..30_000)
            .map(|n| ((n as f64 * 0.02).sin() * 12_000.0) as i32 + (n * 31 % 7) as i32)
            .collect();
        let mut f = Filter::new(spec);
        let inputs: Vec<i32> = signal.iter().map(|&s| f.encode(s)).collect();
        assert_eq!(decode(spec, &inputs), signal);
        let energy = |v: &[i32]| v.iter().map(|&x| (x as i64).pow(2)).sum::<i64>();
        assert!(energy(&inputs) < energy(&signal) / 10);
    }
}
