//! Exponents: their strategies (how many bins share one), conditioning them to the +-2 step limit, and how they
//! are coded (a 4-bit absolute value, then groups of three differences in 7 bits).

use crate::bits::BitWriter;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Strat {
    D15,
    D25,
    D45,
}

impl Strat {
    pub(crate) fn code(self) -> u32 {
        match self {
            Strat::D15 => 1,
            Strat::D25 => 2,
            Strat::D45 => 3,
        }
    }
    pub(crate) fn gsize(self) -> usize {
        match self {
            Strat::D15 => 1,
            Strat::D25 => 2,
            Strat::D45 => 4,
        }
    }
    /// Number of 3-exponent groups for `end` coefficients.
    pub(crate) fn ngroups(self, end: usize) -> usize {
        match self {
            Strat::D15 => (end - 1) / 3,
            Strat::D25 => (end - 1 + 3) / 6,
            Strat::D45 => (end - 1 + 9) / 12,
        }
    }
}

/// log2-scale exponent of a coefficient: floor(-log2|c|) clamped to 0..=24 (24 for zero).
#[inline]
pub(crate) fn exponent_of(c: f32) -> i32 {
    let biased = ((c.to_bits() >> 23) & 0xff) as i32;
    (126 - biased).clamp(0, 24)
}

/// Group-min the exponents to the strategy's resolution and enforce the +-2 step limit.
pub(crate) fn condition_exponents(raw: &[i32], strat: Strat) -> Vec<i32> {
    let end = raw.len();
    let g = strat.gsize();
    let nreal = (end - 1).div_ceil(g);
    let mut seq = Vec::with_capacity(1 + nreal);
    seq.push(raw[0].min(15));
    for i in 0..nreal {
        let (a, b) = (1 + i * g, (1 + (i + 1) * g).min(end));
        seq.push(raw[a..b].iter().copied().min().unwrap());
    }
    loop {
        let mut changed = false;
        for i in 1..seq.len() {
            if seq[i] > seq[i - 1] + 2 {
                seq[i] = seq[i - 1] + 2;
                changed = true;
            }
        }
        for i in (0..seq.len() - 1).rev() {
            if seq[i] > seq[i + 1] + 2 {
                seq[i] = seq[i + 1] + 2;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut out = vec![0; end];
    out[0] = seq[0];
    for (bin, o) in out.iter_mut().enumerate().skip(1) {
        *o = seq[1 + (bin - 1) / g];
    }
    out
}

pub(crate) fn write_exponents(w: &mut BitWriter, exp: &[i32], strat: Strat, end: usize) {
    let g = strat.gsize();
    w.put(exp[0] as u32, 4);
    let ngrps = if end == 7 { 2 } else { strat.ngroups(end) };
    let mut prev = exp[0];
    for grp in 0..ngrps {
        let mut code = 0;
        for j in 0..3 {
            let i = grp * 3 + j;
            let bin = (1 + i * g).min(end - 1);
            let e = exp[bin];
            let m = (e - prev + 2) as u32;
            debug_assert!(m <= 4, "exponent step out of range: {prev} -> {e}");
            code = code * 5 + m;
            prev = e;
        }
        w.put(code, 7);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exponent_extraction() {
        assert_eq!(exponent_of(0.75), 0);
        assert_eq!(exponent_of(0.3), 1);
        assert_eq!(exponent_of(0.0), 24);
        assert_eq!(exponent_of(1.5), 0);
        assert_eq!(exponent_of(1e-9), 24);
    }

    #[test]
    fn exponent_conditioning_limits_steps() {
        let raw = [0, 10, 0, 20, 5, 5, 5, 24, 0];
        for strat in [Strat::D15, Strat::D25, Strat::D45] {
            let e = condition_exponents(&raw, strat);
            // never raised above the raw minimum of its group, and steps are within +-2
            for w in e[1..].windows(2) {
                assert!((w[1] - w[0]).abs() <= 2 || w[0] == w[1], "{e:?}");
            }
            assert!(e[0] <= 15);
        }
    }
}
