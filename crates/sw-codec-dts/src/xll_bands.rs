//! The two-band split XLL uses above 96 kHz.
//!
//! A 192 kHz signal is carried as two 96 kHz bands. The decoder joins them with a chain of integer
//! lifting steps (ETSI TS 102 114, XLL frequency band assembly) and interleaves the result: even output
//! samples come from band 1, odd output samples from band 0 delayed by 7 band samples. Every step is
//! `dst -= round(src * coeff)`, so the chain is exactly invertible: this module runs it backwards.
//!
//! The decoder joins each frame on its own, starting from seven history samples of band 0 that the
//! stream carries (`HISTORY` values per channel). Seven band-0 samples at the end of a frame are never
//! output by that frame; the inverse is free to pick them, and picks zero.

/// Band-0 samples carried in front of each frame.
pub const HISTORY: usize = 7;
const OFF: usize = 8;

const COEFF: [i32; 20] = [
    868669, -5931642, -1228483, 4194304, -20577, 122631, -393647, 904476, -1696305, 2825313, -4430736, 6791313, 41153, -245210, 785564, -1788164, 3259333, -5074941, 6928550, -8204883,
];

fn mul(x: i32, c: i32, bits: u32) -> i32 {
    ((x as i64 * c as i64 + (1i64 << (bits - 1))) >> bits) as i32
}

/// One lifting step: `dst[k - shift] -= mul(src[k], coeff)` (when `to_a`, with `a` indexed from -8).
struct Step {
    to_a: bool,
    shift: usize,
    bits: u32,
    coeff: i32,
}

fn steps() -> Vec<Step> {
    let mut v = vec![
        Step { to_a: true, shift: 0, bits: 22, coeff: COEFF[0] },
        Step { to_a: false, shift: 0, bits: 22, coeff: COEFF[1] },
        Step { to_a: true, shift: 0, bits: 22, coeff: COEFF[2] },
        Step { to_a: false, shift: 0, bits: 22, coeff: COEFF[3] },
    ];
    for i in 0..8 {
        v.push(Step { to_a: true, shift: i, bits: 23, coeff: COEFF[i + 4] });
        v.push(Step { to_a: false, shift: i, bits: 23, coeff: COEFF[i + 12] });
        v.push(Step { to_a: true, shift: i, bits: 23, coeff: COEFF[i + 4] });
    }
    v
}

/// `a` has `n + 8` entries (position -8 first), `b` has `n`.
fn run(a: &mut [i32], b: &mut [i32], inverse: bool) {
    let n = b.len();
    let list = steps();
    let apply = |a: &mut [i32], b: &mut [i32], s: &Step| {
        for k in 0..n {
            if s.to_a {
                let d = &mut a[OFF + k - s.shift];
                let m = mul(b[k], s.coeff, s.bits);
                *d = if inverse { d.wrapping_add(m) } else { d.wrapping_sub(m) };
            } else {
                let m = mul(a[OFF + k - s.shift], s.coeff, s.bits);
                b[k] = if inverse { b[k].wrapping_add(m) } else { b[k].wrapping_sub(m) };
            }
        }
    };
    if inverse {
        for s in list.iter().rev() {
            apply(a, b, s);
        }
    } else {
        for s in &list {
            apply(a, b, s);
        }
    }
}

/// The decoder's assembly of one frame: band 0, band 1 and the seven history samples of band 0 to
/// `2 * n` output samples.
pub fn join(band0: &[i32], band1: &[i32], history: &[i32; HISTORY]) -> Vec<i32> {
    let n = band1.len();
    let mut a = vec![0i32; n + OFF];
    a[1..=HISTORY].copy_from_slice(history);
    a[OFF..].copy_from_slice(band0);
    let mut b = band1.to_vec();
    run(&mut a, &mut b, false);
    let mut out = vec![0i32; 2 * n];
    for k in 0..n {
        out[2 * k] = b[k];
        out[2 * k + 1] = a[k + 1];
    }
    out
}

/// Split `2 * n` samples into the two bands and the history the decoder needs to join them again.
pub fn split(x: &[i32]) -> (Vec<i32>, Vec<i32>, [i32; HISTORY]) {
    let n = x.len() / 2;
    let mut a = vec![0i32; n + OFF];
    let mut b = vec![0i32; n];
    for k in 0..n {
        b[k] = x[2 * k];
        a[k + 1] = x[2 * k + 1];
    }
    run(&mut a, &mut b, true);
    let mut h = [0i32; HISTORY];
    h.copy_from_slice(&a[1..=HISTORY]);
    (a[OFF..].to_vec(), b, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_then_join_is_exact() {
        let mut s = 12345u32;
        let x: Vec<i32> = (0..2048)
            .map(|i| {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                ((s >> 8) as i32 - (1 << 23)) / 2 + ((i as f64 * 0.05).sin() * 2.0e6) as i32
            })
            .collect();
        let (b0, b1, h) = split(&x);
        assert_eq!(join(&b0, &b1, &h), x);
    }

    #[test]
    fn a_tone_lands_in_one_band() {
        // 3 kHz at 192 kHz sits in the low band; 60 kHz in the high band
        let tone = |f: f64| -> Vec<i32> { (0..4096).map(|i| ((i as f64 * f / 192000.0 * std::f64::consts::TAU).sin() * 4.0e6) as i32).collect() };
        let energy = |v: &[i32]| v[64..v.len() - 64].iter().map(|x| (*x as f64).powi(2)).sum::<f64>();
        let (l0, l1, _) = split(&tone(3000.0));
        assert!(energy(&l0) > 50.0 * energy(&l1), "{} {}", energy(&l0), energy(&l1));
        let (h0, h1, _) = split(&tone(60000.0));
        assert!(energy(&h1) > 50.0 * energy(&h0), "{} {}", energy(&h0), energy(&h1));
    }
}
