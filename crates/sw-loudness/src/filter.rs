//! The K-weighting filter of ITU-R BS.1770-4: a high shelf that models the head (stage 1) followed by a high-pass
//! (stage 2, the "revised low-frequency B" curve). The coefficients are worked out for any sample rate from the
//! analogue prototypes, as the standard's 48 kHz table is a single instance of them.

use std::f64::consts::PI;

/// One second-order section in transposed direct form II.
#[derive(Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    fn step(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// The high shelf: +4 dB above about 1.5 kHz.
fn head_shelf(rate: f64) -> Biquad {
    let (f0, gain_db, q) = (1681.974450955533, 3.999843853973347, 0.7071752369554196);
    let k = (PI * f0 / rate).tan();
    let vh = 10f64.powf(gain_db / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    Biquad {
        b: [
            (vh + vb * k / q + k * k) / a0,
            2.0 * (k * k - vh) / a0,
            (vh - vb * k / q + k * k) / a0,
        ],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        z: [0.0; 2],
    }
}

/// The high-pass that removes what is below about 38 Hz.
fn low_cut(rate: f64) -> Biquad {
    let (f0, q) = (38.13547087602444, 0.5003270373238773);
    let k = (PI * f0 / rate).tan();
    let a0 = 1.0 + k / q + k * k;
    Biquad {
        b: [1.0, -2.0, 1.0],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        z: [0.0; 2],
    }
}

/// The two stages for one channel.
#[derive(Clone, Copy)]
pub(crate) struct KWeighting {
    shelf: Biquad,
    cut: Biquad,
}

impl KWeighting {
    pub(crate) fn new(rate: u32) -> KWeighting {
        KWeighting {
            shelf: head_shelf(rate as f64),
            cut: low_cut(rate as f64),
        }
    }

    pub(crate) fn step(&mut self, x: f64) -> f64 {
        self.cut.step(self.shelf.step(x))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gain in dB of the whole filter at `freq`, measured with a long sine.
    fn gain_db(rate: u32, freq: f64) -> f64 {
        let mut f = KWeighting::new(rate);
        let n = rate as usize * 2;
        let (mut sum_in, mut sum_out) = (0.0, 0.0);
        for i in 0..n {
            let x = (2.0 * PI * freq * i as f64 / rate as f64).sin();
            let y = f.step(x);
            if i > n / 2 {
                sum_in += x * x;
                sum_out += y * y;
            }
        }
        10.0 * (sum_out / sum_in).log10()
    }

    #[test]
    fn shelf_and_cut_have_the_published_shape() {
        // about +0.7 dB at 1 kHz, +4 dB far above the knee, and strong attenuation under 20 Hz
        assert!((gain_db(48_000, 997.0) - 0.69).abs() < 0.05);
        assert!((gain_db(48_000, 10_000.0) - 4.0).abs() < 0.3);
        assert!(gain_db(48_000, 10.0) < -15.0);
    }

    #[test]
    fn the_shape_holds_at_other_rates() {
        for rate in [44_100, 96_000] {
            assert!((gain_db(rate, 997.0) - 0.69).abs() < 0.05, "{rate}");
        }
    }
}
