//! DTS-HD High Resolution: the XBR bit-rate extension (ETSI TS 102 114 clause 6.3).
//!
//! The extension carries a second, finer quantization of the core's subband samples: the residual
//! between the original subband samples and what the core decoder reconstructs. Bits are shared out by
//! reverse water-filling (every band ends up with about the same noise level), up to a byte budget.

use crate::bits::BitWriter;
use crate::dca::tables::{QUANT_LEVELS, SCALE_7};
use crate::lossless::xll::crc16;

pub const SYNC_XBR: u32 = 0x655E_315E;
const MAX_ABITS: usize = 26;

/// Quantizer step sizes of the lossless family, 22-bit fractions (clause D.2.2).
const STEP_LOSSLESS: [u32; 27] = [
    0, 4194304, 2097152, 1384120, 1048576, 696254, 524288, 348127, 262144, 131072, 65431, 33026, 16450, 8208, 4100,
    2049, 1024, 512, 256, 128, 64, 32, 16, 8, 4, 2, 1,
];
/// Bits of one block code word (4 samples) for abits 1..=7.
const BLOCK_BITS: [u32; 7] = [7, 10, 12, 13, 15, 17, 19];

fn step(k: usize) -> f64 {
    STEP_LOSSLESS[k] as f64 / 4_194_304.0
}

fn max_q(k: usize) -> i32 {
    (QUANT_LEVELS[k] as i32 - 1) / 2
}

fn sample_bits(k: usize, nsamples: usize) -> usize {
    if k <= 7 {
        nsamples / 4 * BLOCK_BITS[k - 1] as usize
    } else {
        nsamples * (k - 3)
    }
}

/// One candidate coding of a band.
#[derive(Clone)]
struct Cand {
    abits: usize,
    scale: usize,
    bits: usize,
    dist: f64,
    q: Vec<i32>,
}

fn candidates(r: &[f64]) -> Vec<Cand> {
    let n = r.len();
    let energy: f64 = r.iter().map(|v| v * v).sum();
    let peak = r.iter().fold(0f64, |m, v| m.max(v.abs()));
    let mut out = vec![Cand {
        abits: 0,
        scale: 0,
        bits: 0,
        dist: energy,
        q: Vec::new(),
    }];
    if peak < 0.25 {
        return out;
    }
    for k in 1..=MAX_ABITS {
        let st = step(k);
        let mq = max_q(k);
        let mut idx = 0usize;
        while idx < 124 && (SCALE_7[idx] as f64) * st * mq as f64 + 0.5 < peak {
            idx += 1;
        }
        let delta = SCALE_7[idx] as f64 * st;
        let q: Vec<i32> = r.iter().map(|v| ((v / delta).round() as i32).clamp(-mq, mq)).collect();
        let dist: f64 = r.iter().zip(&q).map(|(v, qi)| (v - *qi as f64 * delta).powi(2)).sum();
        out.push(Cand {
            abits: k,
            scale: idx,
            bits: sample_bits(k, n),
            dist,
            q,
        });
        if dist < 1e-3 * n as f64 {
            break; // finer quantizers cannot help: the decoder works with integers
        }
    }
    prune(out, |c| c.bits as f64, |c| c.dist)
}

/// Keep only the candidates on the lower convex hull of (bits, distortion): the Lagrangian search never
/// picks any other.
fn prune<T: Clone>(mut c: Vec<T>, bits: impl Fn(&T) -> f64, dist: impl Fn(&T) -> f64) -> Vec<T> {
    c.sort_by(|a, b| {
        bits(a)
            .partial_cmp(&bits(b))
            .unwrap()
            .then(dist(a).partial_cmp(&dist(b)).unwrap())
    });
    let mut h: Vec<T> = Vec::new();
    for p in c {
        if let Some(last) = h.last() {
            if dist(&p) >= dist(last) {
                continue; // more bits, no better
            }
        }
        while h.len() >= 2 {
            let (a, b) = (&h[h.len() - 2], &h[h.len() - 1]);
            // b is above the line a -> p: drop it
            let cross = (bits(b) - bits(a)) * (dist(&p) - dist(a)) - (dist(b) - dist(a)) * (bits(&p) - bits(a));
            if cross <= 0.0 {
                h.pop();
            } else {
                break;
            }
        }
        h.push(p);
    }
    h
}

fn bits_for(max: usize) -> u32 {
    (usize::BITS - max.leading_zeros()).max(1)
}

pub struct XbrEncoder {
    /// Subframe length in samples per band.
    nsamples: usize,
}

impl XbrEncoder {
    pub fn new(blocks: usize) -> XbrEncoder {
        XbrEncoder { nsamples: blocks }
    }

    /// Encode `residual[ch][band][sample]` (core primary channels only) within `budget` bytes.
    pub fn encode(&self, residual: &[Vec<Vec<f64>>], budget: usize) -> Vec<u8> {
        self.encode_sets(residual, &[residual.len()], budget)
    }

    /// Like `encode`, with the channels split into channel sets of the given sizes (the original encoder gives the
    /// channel of an XCH extension a set of its own).
    pub fn encode_sets(&self, residual: &[Vec<Vec<f64>>], sets: &[usize], budget: usize) -> Vec<u8> {
        assert_eq!(sets.iter().sum::<usize>(), residual.len());
        let cands: Vec<Vec<Vec<Cand>>> = {
            use rayon::prelude::*;
            residual
                .par_iter()
                .map(|c| c.iter().map(|b| candidates(b)).collect())
                .collect()
        };
        // Lagrangian search: smallest distortion for at most `budget` bytes
        let fits = |p: &Vec<Vec<usize>>| self.size(&cands, p, sets) <= budget;
        let mut lo = 1e-9f64;
        let mut hi = 1e15f64;
        let spend_all = self.pick(&cands, lo);
        let best = if fits(&spend_all) {
            spend_all
        } else {
            let mut best = self.pick(&cands, hi);
            for _ in 0..90 {
                let mid = (lo * hi).sqrt();
                let p = self.pick(&cands, mid);
                if fits(&p) {
                    hi = mid;
                    best = p;
                } else {
                    lo = mid;
                }
                if hi / lo < 1.001 {
                    break;
                }
            }
            best
        };
        self.pack(&cands, &best, sets, budget)
    }

    fn pick(&self, cands: &[Vec<Vec<Cand>>], lambda: f64) -> Vec<Vec<usize>> {
        cands
            .iter()
            .map(|ch| {
                ch.iter()
                    .map(|b| {
                        let mut best = (f64::MAX, 0usize);
                        for (i, c) in b.iter().enumerate() {
                            let j = c.dist + lambda * (c.bits + if c.abits > 0 { 6 } else { 0 }) as f64;
                            if j < best.0 {
                                best = (j, i);
                            }
                        }
                        best.1
                    })
                    .collect()
            })
            .collect()
    }

    /// Exact byte size of the XBR frame for a choice.
    fn size(&self, cands: &[Vec<Vec<Cand>>], pick: &[Vec<usize>], sets: &[usize]) -> usize {
        let (hdr, set_bytes) = self.layout(cands, pick, sets);
        hdr + set_bytes.iter().sum::<usize>()
    }

    /// (frame header bytes, bytes of every channel set)
    fn layout(&self, cands: &[Vec<Vec<Cand>>], pick: &[Vec<usize>], sets: &[usize]) -> (usize, Vec<usize>) {
        let nch = cands.len();
        let mut per_ch_bits = Vec::new();
        for ch in 0..nch {
            let chosen: Vec<&Cand> = pick[ch].iter().enumerate().map(|(b, &i)| &cands[ch][b][i]).collect();
            let active = chosen.iter().rposition(|c| c.abits > 0).map_or(1, |p| p + 1);
            let max_a = chosen.iter().map(|c| c.abits).max().unwrap_or(0);
            let max_s = chosen
                .iter()
                .filter(|c| c.abits > 0)
                .map(|c| c.scale)
                .max()
                .unwrap_or(0);
            let nab = bits_for(max_a).clamp(2, 5) as usize;
            let mut bits = 2 + nab * active + 3;
            bits += chosen.iter().take(active).filter(|c| c.abits > 0).count() * bits_for(max_s) as usize;
            bits += chosen.iter().take(active).map(|c| c.bits).sum::<usize>();
            per_ch_bits.push(bits);
        }
        let mut set_bytes = Vec::new();
        let mut first = 0;
        for &n in sets {
            let bits: usize = per_ch_bits[first..first + n].iter().sum::<usize>() + 16; // DSYNC
            set_bytes.push(bits.div_ceil(8));
            first += n;
        }
        // frame header: sync, size, set count, set sizes, tmode flag, per set its channel count and band counts, CRC
        let hbits = 32 + 6 + 2 + 14 * sets.len() + 1 + sets.iter().map(|n| 3 + 2 + 5 * n).sum::<usize>();
        (hbits.div_ceil(8) + 2, set_bytes)
    }

    fn pack(&self, cands: &[Vec<Vec<Cand>>], pick: &[Vec<usize>], sets: &[usize], _budget: usize) -> Vec<u8> {
        let widths = Widths::of(cands, pick);
        // channel set data, one block per set
        let mut set_data: Vec<Vec<u8>> = Vec::new();
        let mut first = 0;
        for &n in sets {
            set_data.push(self.write_set(cands, pick, first..first + n, &widths));
            first += n;
        }
        frame_header(sets, &set_data, &widths.active, hdr_bits(sets))
    }

    /// One channel set: bit widths, bit allocation, scale factors and the samples.
    fn write_set(
        &self,
        cands: &[Vec<Vec<Cand>>],
        pick: &[Vec<usize>],
        chans: std::ops::Range<usize>,
        widths: &Widths,
    ) -> Vec<u8> {
        let (active, nab, nscale) = (&widths.active, &widths.nab, &widths.nscale);
        let mut w = BitWriter::new();
        for ch in chans.clone() {
            w.put(nab[ch] - 2, 2);
        }
        for ch in chans.clone() {
            for b in 0..active[ch] {
                w.put(cands[ch][b][pick[ch][b]].abits as u32, nab[ch]);
            }
        }
        for ch in chans.clone() {
            w.put(nscale[ch], 3);
        }
        for ch in chans.clone() {
            for b in 0..active[ch] {
                let c = &cands[ch][b][pick[ch][b]];
                if c.abits > 0 {
                    w.put(c.scale as u32, nscale[ch]);
                }
            }
        }
        let nssc = self.nsamples / 8;
        for ss in 0..nssc {
            for ch in chans.clone() {
                for b in 0..active[ch] {
                    let c = &cands[ch][b][pick[ch][b]];
                    if c.abits == 0 {
                        continue;
                    }
                    let vals = &c.q[ss * 8..ss * 8 + 8];
                    if c.abits > 7 {
                        for v in vals {
                            w.put_signed(*v, c.abits as u32 - 3);
                        }
                    } else {
                        let levels = QUANT_LEVELS[c.abits] as i32;
                        let off = (levels - 1) / 2;
                        for blk in vals.chunks(4) {
                            let mut code = 0i32;
                            for v in blk.iter().rev() {
                                code = code * levels + (*v + off);
                            }
                            w.put(code as u32, BLOCK_BITS[c.abits - 1]);
                        }
                    }
                }
            }
        }
        w.put(0xFFFF, 16);
        w.put(0xFFFF, 16);
        w.into_bytes()
    }
}

/// Per channel: how many bands are coded and how many bits the bit allocation and the scale factors need.
struct Widths {
    active: Vec<usize>,
    nab: Vec<u32>,
    nscale: Vec<u32>,
}

impl Widths {
    fn of(cands: &[Vec<Vec<Cand>>], pick: &[Vec<usize>]) -> Widths {
        let mut w = Widths {
            active: Vec::new(),
            nab: Vec::new(),
            nscale: Vec::new(),
        };
        for ch in 0..pick.len() {
            let chosen: Vec<&Cand> = pick[ch].iter().enumerate().map(|(b, &i)| &cands[ch][b][i]).collect();
            w.active
                .push(chosen.iter().rposition(|c| c.abits > 0).map_or(1, |p| p + 1));
            let max_a = chosen.iter().map(|c| c.abits).max().unwrap_or(0);
            let max_s = chosen
                .iter()
                .filter(|c| c.abits > 0)
                .map(|c| c.scale)
                .max()
                .unwrap_or(0);
            w.nab.push(bits_for(max_a).clamp(2, 5));
            w.nscale.push(bits_for(max_s));
        }
        w
    }
}

/// Bits of the frame header before its CRC: sync, size, set count, set sizes, transient flag, per set its channel
/// count and band counts.
fn hdr_bits(sets: &[usize]) -> usize {
    32 + 6 + 2 + 14 * sets.len() + 1 + sets.iter().map(|n| 3 + 2 + 5 * n).sum::<usize>()
}

/// The frame header followed by the channel set data.
fn frame_header(sets: &[usize], set_data: &[Vec<u8>], active: &[usize], hbits: usize) -> Vec<u8> {
    let mut h = BitWriter::new();
    h.put(SYNC_XBR, 32);
    let hsize = hbits.div_ceil(8) + 2;
    h.put(hsize as u32 - 1, 6);
    h.put(sets.len() as u32 - 1, 2);
    for s in set_data {
        h.put(s.len() as u32 - 1, 14);
    }
    h.put(0, 1); // transient modes not used
    let mut first = 0;
    for &n in sets {
        h.put(n as u32 - 1, 3);
        h.put(0, 2); // 5-bit active band counts
        for ch in first..first + n {
            h.put(active[ch] as u32 - 1, 5);
        }
        first += n;
    }
    h.align(8);
    let mut hb = h.into_bytes();
    let crc = crc16(&hb[4..]);
    hb.extend_from_slice(&crc.to_be_bytes());
    assert_eq!(hb.len(), hsize);
    for s in set_data {
        hb.extend_from_slice(s);
    }
    hb
}
