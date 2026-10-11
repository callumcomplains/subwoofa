//! Linear prediction of the three lowest bands of DTS Express (LBR): the time samples of those bands are coded as
//! the residual of an eight-tap predictor whose reflection coefficients (four bits each) travel with the frame.
//!
//! The decoder runs the synthesis filter `y[n] = r[n] - sum(c[j] * y[n-1-j])` on the dequantized residual, with
//! the coefficient set depending on the position in the frame (the first sixteen samples use the previous
//! frame's last set), so the encoder has to quantize in a closed loop over what the decoder will have produced.

use std::f64::consts::PI;

/// Reflection coefficient of a four-bit code (`sin((i - 8) * pi / 17)` below eight, `/ 15` from eight).
pub fn table(code: usize) -> f64 {
    let d = if code < 8 { 17.0 } else { 15.0 };
    ((code as f64 - 8.0) * PI / d).sin()
}

/// Direct-form coefficients of eight reflection codes (the decoder's step-up recursion).
pub fn direct(codes: &[u8; 8]) -> [f64; 8] {
    let mut c = [0f64; 8];
    for i in 0..8 {
        let rc = table(codes[i] as usize);
        for j in 0..(i + 1) / 2 {
            let (t1, t2) = (c[j], c[i - j - 1]);
            c[j] = t1 + rc * t2;
            c[i - j - 1] = t2 + rc * t1;
        }
        c[i] = rc;
    }
    c
}

/// The code of the table entry nearest to a reflection coefficient.
fn nearest_code(k: f64) -> u8 {
    // the extreme codes (about +-0.996) make a synthesis filter that rings and overloads the quantizer
    (1..15).min_by(|&a, &b| (table(a) - k).abs().partial_cmp(&(table(b) - k).abs()).unwrap()).unwrap() as u8
}

/// Reflection codes of the best eight-tap predictor of a stretch of samples. Each coefficient is quantized before
/// the next is derived, so the later ones make up for the rounding of the earlier ones.
pub fn analyse(seg: &[f64]) -> [u8; 8] {
    let zero = [8u8; 8];
    let mut r = [0f64; 9];
    for (lag, v) in r.iter_mut().enumerate() {
        *v = seg.iter().skip(lag).zip(seg).map(|(a, b)| a * b).sum();
    }
    if r[0] < 1e-9 {
        return zero;
    }
    // a light lag window and a little white noise keep the filter well behaved
    for (lag, v) in r.iter_mut().enumerate().skip(1) {
        *v *= (-0.5 * (2.0 * PI * 0.03 * lag as f64).powi(2)).exp();
    }
    r[0] *= 1.001;
    let mut codes = zero;
    let mut a = [0f64; 9]; // a[0] = 1, prediction error filter 1 + sum a[j] z^-j
    a[0] = 1.0;
    for i in 1..=8 {
        let acc: f64 = (0..i).map(|j| a[j] * r[i - j]).sum();
        let e: f64 = (0..i).map(|j| a[j] * r[j]).sum();
        if e <= 1e-12 {
            break;
        }
        let code = nearest_code(-acc / e);
        let k = table(code as usize);
        codes[i - 1] = code;
        let prev = a;
        for j in 1..i {
            a[j] = prev[j] + k * prev[i - j];
        }
        a[i] = k;
    }
    codes
}
