//! The two filters a TrueHD channel can run before its residual is coded (an FIR and an IIR stage): their
//! quantised form, and applying them with the decoder's integer arithmetic.

use crate::{number_sbits, MAX_ORDER};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct Fir {
    pub(crate) order: u8,
    pub(crate) shift: u8,
    pub(crate) coeff_bits: u8,
    pub(crate) coeff_shift: u8,
    pub(crate) coeff: [i32; MAX_ORDER],
}

pub(crate) const NO_FILTER: Fir = Fir {
    order: 0,
    shift: 0,
    coeff_bits: 0,
    coeff_shift: 0,
    coeff: [0; MAX_ORDER],
};

pub(crate) fn quantize_fir(c: &[f64]) -> Option<Fir> {
    let order = c.len();
    if order == 0 || order > MAX_ORDER {
        return None;
    }
    let shift = coefficient_shift(c)?;
    quantize_with_shift(c, shift)
}

/// Largest shift (0..=15) so every coefficient fits in 16 signed bits.
pub(crate) fn coefficient_shift(c: &[f64]) -> Option<u32> {
    let peak = c.iter().fold(0f64, |m, v| m.max(v.abs()));
    if !peak.is_finite() || peak < 1e-6 {
        return None;
    }
    let mut shift = 15u32;
    while shift > 0 && (peak * (1u32 << shift) as f64).round() > 32767.0 {
        shift -= 1;
    }
    Some(shift)
}

/// FIR and IIR stages share one precision; this quantizes both with the coarser of the two.
pub(crate) fn quantize_pair(fir: &[f64], iir: &[f64]) -> Option<(Fir, Fir)> {
    let shift = coefficient_shift(fir)?.min(coefficient_shift(iir)?);
    Some((quantize_with_shift(fir, shift)?, quantize_with_shift(iir, shift)?))
}

pub(crate) fn quantize_with_shift(c: &[f64], shift: u32) -> Option<Fir> {
    let order = c.len();
    let mut q = [0i32; MAX_ORDER];
    for (i, v) in c.iter().enumerate() {
        q[i] = (v * (1u32 << shift) as f64).round() as i32;
    }
    if q[..order].iter().all(|v| *v == 0) {
        return None;
    }
    let mut bits = 1;
    let mut or_all = 0i32;
    for v in &q[..order] {
        bits = bits.max(number_sbits(*v));
        or_all |= v;
    }
    let tz = if or_all == 0 { 0 } else { or_all.trailing_zeros().min(7) };
    let coeff_bits = bits.saturating_sub(tz).max(1);
    let coeff_shift = tz.min(16 - coeff_bits);
    // coefficients must be multiples of 2^coeff_shift
    for v in &mut q[..order] {
        *v = (*v >> coeff_shift) << coeff_shift;
    }
    Some(Fir {
        order: order as u8,
        shift: shift as u8,
        coeff_bits: coeff_bits as u8,
        coeff_shift: coeff_shift as u8,
        coeff: q,
    })
}

/// Residual exactly as a decoder will undo it. The first 8 samples are left unfiltered.
/// The IIR stage filters the past residuals (`sample - prediction`), as the decoder's state does.
pub(crate) fn apply_filters(x: &[i32], fir: &Fir, iir: &Fir, mask: i32) -> Vec<i32> {
    let (fo, io) = (fir.order as usize, iir.order as usize);
    let mut res = x.to_vec();
    if fo == 0 {
        return res;
    }
    let mut e: Vec<i32> = x.to_vec();
    for i in 8..x.len() {
        let mut acc = 0i64;
        for k in 0..fo {
            acc += x[i - 1 - k] as i64 * fir.coeff[k] as i64;
        }
        for k in 0..io {
            acc += e[i - 1 - k] as i64 * iir.coeff[k] as i64;
        }
        let pred = (acc >> fir.shift) as i32;
        res[i] = x[i].wrapping_sub(pred & mask);
        e[i] = x[i].wrapping_sub(pred);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantized_filter_fits_the_format() {
        let f = quantize_fir(&[1.6, -0.7]).unwrap();
        assert!(f.coeff_bits as u32 + f.coeff_shift as u32 <= 16);
        assert!(f.shift <= 15);
    }
}
