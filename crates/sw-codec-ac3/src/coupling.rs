//! Channel coupling: above a start frequency the channels share one coupling channel plus a coordinate per channel
//! and band. Used for multichannel AC-3 at low bit rates.

use crate::bits::BitWriter;
use crate::exponents::Strat;

/// The coupling channel's reference exponent is sent halved, so it has to be even: make it so and bring the
/// groups after it back inside the +-2 step limit by lowering exponents (never raising them).
pub(crate) fn pin_reference(virt: &mut [i32]) {
    virt[0] &= !1;
    loop {
        let mut changed = false;
        for i in 1..virt.len() {
            if virt[i] > virt[i - 1] + 2 {
                virt[i] = virt[i - 1] + 2;
                changed = true;
            }
        }
        for i in (1..virt.len() - 1).rev() {
            if virt[i] > virt[i + 1] + 2 {
                virt[i] = virt[i + 1] + 2;
                changed = true;
            }
        }
        if virt.len() > 1 && virt[0] > virt[1] + 2 {
            virt[0] = ((virt[1] + 2) & !1).max(0);
            changed = true;
        }
        if !changed {
            break;
        }
    }
}

/// Exponents of the coupling channel: a 4-bit reference (half the even starting exponent), then the groups.
/// `virt[0]` is the reference and `virt[1 + i]` the exponent of bin `start + i`.
pub(crate) fn write_cpl_exponents(w: &mut BitWriter, virt: &[i32], strat: Strat) {
    let g = strat.gsize();
    let nbins = virt.len() - 1;
    w.put((virt[0] >> 1) as u32, 4);
    let mut prev = virt[0];
    for grp in 0..nbins / (3 * g) {
        let mut code = 0;
        for j in 0..3 {
            let e = virt[1 + (grp * 3 + j) * g];
            let m = (e - prev + 2) as u32;
            debug_assert!(m <= 4, "coupling exponent step out of range: {prev} -> {e}");
            code = code * 5 + m;
            prev = e;
        }
        w.put(code, 7);
    }
}

/// Coupling coordinate (master 2 bits, then exponent and mantissa 4 bits each per band) for the gains `g`
/// the decoder multiplies the coupling channel with (8 x mantissa x 2^-(exp + 3 master)).
pub(crate) fn quantize_coords(g: &[f64]) -> (u32, Vec<(u32, u32)>) {
    let e_of = |g: f64| -> i32 {
        let v = (g / 8.0).max(1e-9);
        ((-v.log2()).floor() as i32).clamp(0, 24)
    };
    let es: Vec<i32> = g.iter().map(|g| e_of(*g)).collect();
    let emax = es.iter().copied().max().unwrap_or(0);
    let mstr = (((emax - 14).max(0) + 2) / 3).min(3);
    let out = g
        .iter()
        .zip(&es)
        .map(|(g, e)| {
            let v = (g / 8.0).max(1e-9);
            let exp = (*e - 3 * mstr).clamp(0, 14);
            let total = exp + 3 * mstr;
            let mant = ((v * 2f64.powi(total) * 32.0).round() as i32 - 16).clamp(0, 15);
            (exp as u32, mant as u32)
        })
        .collect();
    (mstr as u32, out)
}

/// Value of a quantized coupling coordinate (the gain applied to the coupling channel).
#[allow(dead_code)] // the coupling-coordinate gain formula, kept as the reference for the coupling encoder
pub(crate) fn coord_gain(mstr: u32, exp: u32, mant: u32) -> f64 {
    8.0 * (mant + 16) as f64 / 32.0 * 2f64.powi(-((exp + 3 * mstr) as i32))
}

/// Coupling in use for a frame.
#[derive(Clone)]
pub(crate) struct CplInfo {
    pub(crate) begf: usize,
    pub(crate) endf: usize,
    pub(crate) strt: usize,
    pub(crate) end: usize,
    /// `[set][channel] = (master, [(exp, mant)] per band)`
    pub(crate) coords: Vec<Vec<(u32, Vec<(u32, u32)>)>>,
    /// 2/0 only: `[set][band]`, the right channel is the inverse of the left in that band.
    pub(crate) phase: Vec<Vec<bool>>,
}
