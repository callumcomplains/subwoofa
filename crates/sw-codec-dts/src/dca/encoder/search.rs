//! Choosing how many bits each band of a frame gets: for a quality offset, the quantizer of every band and the
//! size of the result, and a search for the highest quality that fits the frame.

use crate::dca::encoder::tables::{max_q, raw_bits, step_of, BLOCK_BITS, GROUP_SIZE, SEL_BITS};
use crate::dca::encoder::Encoder;
use crate::dca::qmf::BANDS;
use crate::dca::tables::{BITALLOC_HUFF, QUANT_HUFF, SCALE_7, TMODE_HUFF};

/// Everything decided for one frame.
pub(crate) struct Plan {
    pub(crate) abits: Vec<[u8; BANDS]>,
    pub(crate) scale_idx: Vec<[u8; BANDS]>,
    /// quantized indexes [ch][band][sample]; empty until `fill_indexes` (only the plan that is kept needs them)
    pub(crate) q: Vec<Vec<Vec<i32>>>,
    /// SEL per channel and abits 1..=10 (== GROUP_SIZE means no Huffman)
    pub(crate) sel: Vec<[u8; 10]>,
    pub(crate) bhuff: Vec<u8>,
    pub(crate) bits: usize,
}

/// One band coded with one quantiser: the scale factor, the indexes, and what each Huffman book would cost.
pub(crate) struct BandCode {
    pub(crate) scale_idx: u8,
    pub(crate) q: Vec<i32>,
    /// Bits of the indexes with book `s` (only for quantisers 1 to 10, which have books).
    pub(crate) cost: [u32; 10],
}

/// What the search of one frame works out once instead of once per candidate offset: the masking level of every
/// band, the noise each quantiser leaves in it, and the `BandCode` of every (channel, band, quantiser) met.
/// Neighbouring candidate offsets give most bands the same quantiser.
pub(crate) struct BandCache {
    pub(crate) mask: Vec<[f64; BANDS]>,
    /// `noise[ch][band][k]`: the noise power of quantiser `k` (1 to 25) on this band.
    pub(crate) noise: Vec<Vec<[f64; 26]>>,
    pub(crate) codes: Vec<std::sync::OnceLock<BandCode>>,
}

pub(crate) const QUANTISERS: usize = 27;

impl BandCache {
    fn new(power: &[[f64; BANDS]], peak: &[[f64; BANDS]]) -> BandCache {
        let mask = power
            .iter()
            .map(|p| {
                let pdb: Vec<f64> = p.iter().map(|p| 10.0 * (p + 1e-6).log10()).collect();
                let mut mask = [0f64; BANDS];
                for (b, m) in mask.iter_mut().enumerate() {
                    // masking by the neighbourhood: shallow upward spread, steep downward
                    *m = f64::NEG_INFINITY;
                    for (b2, p) in pdb.iter().enumerate() {
                        let d = b as i32 - b2 as i32;
                        let att = if d >= 0 { 12.0 * d as f64 } else { 25.0 * (-d) as f64 };
                        *m = m.max(p - att);
                    }
                }
                mask
            })
            .collect();
        let noise = peak
            .iter()
            .map(|p| {
                p.iter()
                    .map(|peak| {
                        let mut n = [0f64; 26];
                        for (k, n) in n.iter_mut().enumerate().skip(1) {
                            let delta = peak / max_q(k) as f64;
                            *n = delta * delta / 12.0;
                        }
                        n
                    })
                    .collect()
            })
            .collect();
        BandCache {
            mask,
            noise,
            codes: (0..power.len() * BANDS * QUANTISERS)
                .map(|_| std::sync::OnceLock::new())
                .collect(),
        }
    }

    fn get(&self, ch: usize, band: usize, k: usize, make: impl FnOnce() -> BandCode) -> &BandCode {
        self.codes[(ch * BANDS + band) * QUANTISERS + k].get_or_init(make)
    }
}

impl Encoder {
    /// Highest quality plan that fits the frame (the budget is the frame minus any X96 reserve), searched around
    /// the offset `hint`: quality offsets vary slowly from frame to frame. `spread` lets the search use many
    /// threads for this one frame.
    pub(crate) fn plan_for(&self, sub: &[Vec<Vec<f64>>], hint: f64, spread: bool) -> (f64, Plan) {
        let blocks = self.cfg.blocks;
        let mut budget = (self.frame_bytes - self.x96_reserve) * 8;
        if let Some(aux) = &self.aux {
            // the audio ends in front of the auxiliary data, with its 6-bit size field
            budget = self.aux_sync(aux) * 8 - 8;
        }
        let power: Vec<[f64; BANDS]> = sub
            .iter()
            .map(|c| {
                let mut p = [0f64; BANDS];
                for (band, s) in c.iter().enumerate() {
                    p[band] = s.iter().map(|v| v * v).sum::<f64>() / blocks as f64;
                }
                p
            })
            .collect();
        let peak: Vec<[f64; BANDS]> = sub
            .iter()
            .map(|c| {
                let mut p = [0f64; BANDS];
                for (band, s) in c.iter().enumerate() {
                    p[band] = s.iter().fold(0f64, |m, v| m.max(v.abs()));
                }
                p
            })
            .collect();
        let cache = BandCache::new(&power, &peak);
        let (offset, mut best) = self.search_offset(sub, &power, &peak, budget, &cache, hint, spread);
        self.fill_indexes(&mut best, sub, &cache, &peak);
        (offset, best)
    }

    /// Highest quality offset (dB) whose plan fits `budget` bits, with that plan. On a multi-core machine the
    /// candidates of each round are evaluated in parallel; resolution is 0.375 dB either way.
    pub(crate) fn search_offset(
        &self,
        sub: &[Vec<Vec<f64>>],
        power: &[[f64; BANDS]],
        peak: &[[f64; BANDS]],
        budget: usize,
        cache: &BandCache,
        hint: f64,
        spread: bool,
    ) -> (f64, Plan) {
        use rayon::prelude::*;
        if spread && rayon::current_num_threads() >= 6 {
            let base = hint;
            let first: Vec<(f64, Plan)> = (-4..=4)
                .into_par_iter()
                .map(|i| {
                    let o = base + 3.0 * i as f64;
                    (o, self.make_plan(sub, power, peak, o, cache))
                })
                .collect();
            let top_fit = first.iter().rposition(|(_, p)| p.bits <= budget);
            if let Some(i) = top_fit {
                if i + 1 < first.len() {
                    // the answer lies between candidate i (fits) and i + 1 (does not): refine in eighths
                    let lo = first[i].0;
                    let mut best = (lo, first.into_iter().nth(i).unwrap().1);
                    let fine: Vec<(f64, Plan)> = (1..8)
                        .into_par_iter()
                        .map(|j| {
                            let o = lo + 3.0 * j as f64 / 8.0;
                            (o, self.make_plan(sub, power, peak, o, cache))
                        })
                        .collect();
                    for (o, p) in fine {
                        if p.bits <= budget {
                            best = (o, p);
                        }
                    }
                    return best;
                }
                // everything fits: continue upwards from the top candidate
                let top = first.into_iter().last().unwrap();
                return self.search_sequential(sub, power, peak, budget, top.0 - 12.0, cache);
            }
            return self.search_sequential(sub, power, peak, budget, base - 12.0 - 24.0, cache);
        }
        self.search_sequential(sub, power, peak, budget, hint - 12.0, cache)
    }

    pub(crate) fn search_sequential(
        &self,
        sub: &[Vec<Vec<f64>>],
        power: &[[f64; BANDS]],
        peak: &[[f64; BANDS]],
        budget: usize,
        start: f64,
        cache: &BandCache,
    ) -> (f64, Plan) {
        let mut lo = start;
        let mut best = self.make_plan(sub, power, peak, lo, cache);
        let mut guard = 0;
        while best.bits > budget && lo > -200.0 {
            lo -= 24.0;
            best = self.make_plan(sub, power, peak, lo, cache);
            guard += 1;
            if guard > 12 {
                break;
            }
        }
        if best.bits > budget {
            lo = -200.0;
            best = self.make_plan(sub, power, peak, lo, cache);
        } else {
            let mut hi = lo + 24.0;
            loop {
                let p = self.make_plan(sub, power, peak, hi, cache);
                if p.bits <= budget && hi < 200.0 {
                    lo = hi;
                    best = p;
                    hi += 24.0;
                } else {
                    break;
                }
            }
            for _ in 0..6 {
                let mid = 0.5 * (lo + hi);
                let p = self.make_plan(sub, power, peak, mid, cache);
                if p.bits <= budget {
                    lo = mid;
                    best = p;
                } else {
                    hi = mid;
                }
            }
        }
        (lo, best)
    }

    pub(crate) fn make_plan(
        &self,
        sub: &[Vec<Vec<f64>>],
        power: &[[f64; BANDS]],
        peak: &[[f64; BANDS]],
        offset_db: f64,
        cache: &BandCache,
    ) -> Plan {
        let nch = self.nplan();
        let blocks = self.cfg.blocks;
        let mut abits = vec![[0u8; BANDS]; nch];
        for ch in 0..nch {
            for b in 0..BANDS {
                let noise_db = self.mask_weight * cache.mask[ch][b] - offset_db;
                let noise = 10f64.powf(noise_db / 10.0);
                if power[ch][b] <= noise || peak[ch][b] < 0.5 {
                    continue;
                }
                // the first quantiser coarse enough to stay within the noise (26 when none is)
                let k = (1..26).find(|&k| cache.noise[ch][b][k] <= noise).unwrap_or(26);
                abits[ch][b] = k as u8;
            }
        }
        self.finish_plan(sub, abits, peak, blocks, cache)
    }

    /// Codes band `b` of channel `ch` with quantiser `k`.
    pub(crate) fn code_band(&self, sub: &[f64], peak: f64, k: usize) -> BandCode {
        let step = step_of(k);
        let mq = max_q(k) as f64;
        let mut idx = 0usize;
        while idx < 124 && (SCALE_7[idx] as f64) * step * mq < peak {
            idx += 1;
        }
        let delta = SCALE_7[idx] as f64 * step;
        let q: Vec<i32> = sub
            .iter()
            .map(|v| ((v / delta).round() as i32).clamp(-(mq as i32), mq as i32))
            .collect();
        let mut cost = [0u32; 10];
        if k <= 10 {
            let off = (QUANT_HUFF[k - 1][0].len() as i32 - 1) / 2;
            for (s, c) in cost.iter_mut().enumerate().take(GROUP_SIZE[k - 1]) {
                let t = QUANT_HUFF[k - 1][s];
                *c = q.iter().map(|v| t[(*v + off) as usize].1 as u32).sum();
            }
        }
        BandCode {
            scale_idx: idx as u8,
            q,
            cost,
        }
    }

    /// Puts the quantised indexes into a plan found by the search.
    pub(crate) fn fill_indexes(
        &self,
        plan: &mut Plan,
        sub: &[Vec<Vec<f64>>],
        cache: &BandCache,
        peak: &[[f64; BANDS]],
    ) {
        let nch = self.nplan();
        plan.q = vec![vec![Vec::new(); BANDS]; nch];
        for ch in 0..nch {
            for b in 0..BANDS {
                let k = plan.abits[ch][b] as usize;
                if k > 0 {
                    let code = cache.get(ch, b, k, || self.code_band(&sub[ch][b], peak[ch][b], k));
                    plan.q[ch][b] = code.q.clone();
                }
            }
        }
    }

    pub(crate) fn finish_plan(
        &self,
        sub: &[Vec<Vec<f64>>],
        abits: Vec<[u8; BANDS]>,
        peak: &[[f64; BANDS]],
        blocks: usize,
        cache: &BandCache,
    ) -> Plan {
        let nch = self.nplan();
        let mut scale_idx = vec![[0u8; BANDS]; nch];
        for ch in 0..nch {
            for b in 0..BANDS {
                let k = abits[ch][b] as usize;
                if k > 0 {
                    scale_idx[ch][b] = cache
                        .get(ch, b, k, || self.code_band(&sub[ch][b], peak[ch][b], k))
                        .scale_idx;
                }
            }
        }
        // codebook choices
        let mut sel = vec![[0u8; 10]; nch];
        let mut bhuff = vec![6u8; nch];
        let mut bits = self.header_bits();
        let nssc = blocks / 8;
        let mut xch_bits = 0usize;
        for ch in 0..nch {
            let bits_before = bits;
            // ABITS coding
            let all_huff = abits[ch].iter().all(|a| (1..=12).contains(a));
            let all4 = abits[ch].iter().all(|a| *a <= 15);
            let mut best = (5 * BANDS, 6u8);
            if all4 && 4 * BANDS < best.0 {
                best = (4 * BANDS, 5);
            }
            if all_huff {
                for (j, t) in BITALLOC_HUFF.iter().enumerate() {
                    let c: usize = abits[ch].iter().map(|a| t[*a as usize - 1].1 as usize).sum();
                    if c < best.0 {
                        best = (c, j as u8);
                    }
                }
            }
            bits += best.0;
            bhuff[ch] = best.1;
            // transient mode flags and scale factors
            let active = abits[ch].iter().filter(|a| **a > 0).count();
            if nssc > 1 {
                bits += active * TMODE_HUFF[0][0].1 as usize;
            }
            bits += active * 7;
            // sample coding per quantizer
            for k in 1..=26usize {
                let bands: Vec<usize> = (0..BANDS).filter(|b| abits[ch][*b] as usize == k).collect();
                if bands.is_empty() {
                    if k <= 10 {
                        sel[ch][k - 1] = GROUP_SIZE[k - 1] as u8;
                    }
                    continue;
                }
                let nsamp = bands.len() * blocks;
                if k > 10 {
                    bits += nsamp * raw_bits(k) as usize;
                    continue;
                }
                let g = GROUP_SIZE[k - 1];
                let plain = if k <= 7 {
                    nsamp / 4 * BLOCK_BITS[k - 1] as usize
                } else {
                    nsamp * raw_bits(k) as usize
                };
                let mut best = (plain, g as u8);
                for s in 0..g {
                    let mut c = 2usize; // adjustment index
                    for b in &bands {
                        c += cache
                            .get(ch, *b, k, || self.code_band(&sub[ch][*b], peak[ch][*b], k))
                            .cost[s] as usize;
                    }
                    if c < best.0 {
                        best = (c, s as u8);
                    }
                }
                sel[ch][k - 1] = best.1;
                bits += best.0;
            }
            bits += SEL_BITS.iter().sum::<u32>() as usize;
            for (i, g) in GROUP_SIZE.iter().enumerate() {
                if (sel[ch][i] as usize) < *g {
                    bits += 2;
                }
            }
            if ch >= self.cfg.channels {
                // the extension channel is counted apart
                xch_bits = bits - bits_before;
                bits = bits_before;
            }
        }
        if self.cfg.lfe {
            bits += 8 * (2 * 2 * nssc) + 8;
        }
        bits += 16; // DSYNC
        if self.cfg.xch {
            // the extension starts on a 32-bit boundary; sync, size, arrangement, channel count, the header
            // arrays (21 bits) and PMODE flags (32 bits) of its channel, and its DSYNC
            // decoders look for the extension from the end of the frame and want it at least 96 bytes long
            let x = (xch_bits + 49 + 21 + BANDS + 16).max(96 * 8);
            bits = bits.div_ceil(32) * 32 + x;
            if x.div_ceil(8) > 1000 {
                bits = usize::MAX / 4; // XChFSIZE is 10 bits
            }
        }
        Plan {
            abits,
            scale_idx,
            q: Vec::new(),
            sel,
            bhuff,
            bits,
        }
    }

    /// Frame header + primary audio coding header + fixed parts of the subframe.
    pub(crate) fn header_bits(&self) -> usize {
        if let Some(h) = self.header_bits_override {
            return h;
        }
        let nch = self.cfg.channels;
        104 + 7 + 5 * nch + 5 * nch + 3 * nch + 2 * nch + 3 * nch + 3 * nch + 5 + BANDS * nch
    }
}
