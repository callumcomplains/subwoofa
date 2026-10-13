//! What the core is fed: the source samples of each channel, decimated and aligned.

use crate::lossless::ma_core::resample::half_len;
use crate::lossless::ma_core::MaCoreEncoder;

impl MaCoreEncoder {
    /// The programme the core is made from, per coded channel.
    pub(crate) fn core_src(&self) -> &Vec<Vec<i32>> {
        if self.cbuf.is_empty() {
            &self.buf
        } else {
            &self.cbuf
        }
    }

    /// Core-rate input samples for one channel and frame, in 24-bit units.
    pub(crate) fn core_input(&self, x: usize, frame: usize) -> Vec<f64> {
        let n48 = self.n / self.ratio;
        let src = &self.core_src()[x];
        let gain = (1u64 << self.shift) as f64;
        if self.ratio == 1 {
            let start = frame * self.n + self.l48 - self.dropped;
            return src[start..start + n48].iter().map(|v| *v as f64 * gain).collect();
        }
        if self.from_band0 {
            // split the stretch of source this frame's core samples draw on into the two bands (as the lossless
            // stage will), and decimate band 0 by two with the 96 kHz filter
            let half2 = half_len(2);
            let first_m = frame * n48 + self.l48;
            let lo_b0 = (2 * first_m) as isize - half2 as isize - 8;
            let hi_b0 = (2 * (first_m + n48)) as isize + half2 as isize + 8 + self.phase as isize / 2;
            let src_lo = (2 * lo_b0 - self.dropped as isize).max(0) as usize & !1;
            let src_hi = ((2 * hi_b0 - self.dropped as isize) as usize).min(src.len()) & !1;
            let mut out = vec![0f64; n48];
            if src_hi > src_lo + 64 {
                let (b0, _, _) = crate::lossless::xll::bands::split(&src[src_lo..src_hi]);
                let b0_origin = ((src_lo + self.dropped) / 2) as isize; // band-0 index of b0[0]
                for (i, o) in out.iter_mut().enumerate() {
                    let centre = (2 * (first_m + i) + self.phase) as isize - b0_origin;
                    let mut acc = 0.0;
                    for (j, h) in self.taps_b0.iter().enumerate() {
                        let idx = centre + j as isize - half2 as isize;
                        if idx >= 0 && (idx as usize) < b0.len() {
                            acc += h * b0[idx as usize] as f64;
                        }
                    }
                    *o = acc * gain * self.cgain;
                }
            }
            return out;
        }
        let mut out = Vec::with_capacity(n48);
        for i in 0..n48 {
            let m = frame * n48 + i + self.l48;
            let centre = (self.ratio * m + self.phase) as isize - self.dropped as isize;
            let mut acc = 0.0;
            for (j, h) in self.taps.iter().enumerate() {
                let idx = centre + j as isize - self.half as isize;
                if idx >= 0 && (idx as usize) < src.len() {
                    acc += h * src[idx as usize] as f64;
                }
            }
            out.push(acc * gain * self.cgain);
        }
        out
    }

    /// What the core encodes for `frame`: its input per core slot (primaries in stream order, the XCH plane, then
    /// the LFE).
    pub(crate) fn core_inputs(&self, frame: usize) -> Vec<Vec<f64>> {
        let ncore = self.xll_to_core.iter().flatten().copied().max().map_or(0, |m| m + 1);
        let mut core_in: Vec<Vec<f64>> = vec![Vec::new(); ncore];
        for (x, slot) in self.xll_to_core.iter().enumerate() {
            let Some(slot) = *slot else { continue };
            let mut v = self.core_input(x, frame);
            if let Some(h) = &self.hier {
                let g = h.core_gain(x);
                v.iter_mut().for_each(|s| *s *= g);
            }
            core_in[slot] = v;
        }
        if let Some(xs) = self.xch_x {
            // the XCH plane sits between the primaries and the LFE
            let v = self.core_input(xs, frame);
            core_in.insert(self.nprim, v);
        }
        core_in
    }
}
