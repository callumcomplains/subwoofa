//! DTS 96/24: the X96 frequency extension (ETSI TS 102 114 clause 6.2), carried in the core substream
//! after the core data of every frame.
//!
//! The extension codes 64 subbands at the 96 kHz rate: the upper 32 bands as they are, the lower 32 as
//! the residual the core left over. Bits are shared out by reverse water-filling up to a byte budget; the
//! quantizers are the core's own (mid-tread, block codes up to 25 levels, plain codes above).

use crate::bits::BitWriter;
use crate::tables::{QUANT_LEVELS, SCALE_6, STEP_LOSSY};

pub const SYNC_X96: u32 = 0x1D95_F262;
pub const BANDS: usize = 64;
const SEL_BITS: [u32; 10] = [1, 2, 2, 2, 2, 3, 3, 3, 3, 3];
const GROUP_SIZE: [u32; 10] = [1, 3, 3, 3, 3, 7, 7, 7, 7, 7];
const BLOCK_BITS: [u32; 7] = [7, 10, 12, 13, 15, 17, 19];
const MAX_ABITS: usize = 14;

fn step(k: usize) -> f64 {
    STEP_LOSSY[k] as f64 / 4_194_304.0
}

fn max_q(k: usize) -> i32 {
    (QUANT_LEVELS[k] as i32 - 1) / 2
}

fn sample_bits(k: usize, n: usize) -> usize {
    if k <= 7 {
        n / 4 * BLOCK_BITS[k - 1] as usize
    } else {
        n * (k - 3)
    }
}

#[derive(Clone)]
struct Cand {
    /// Quantizer (0 = nothing coded).
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
    if peak < 0.5 {
        return out;
    }
    for k in 1..=MAX_ABITS {
        let st = step(k);
        let mq = max_q(k);
        let mut idx = 0usize;
        while idx < 62 && (SCALE_6[idx] as f64) * st * mq as f64 + 0.5 < peak {
            idx += 1;
        }
        let delta = SCALE_6[idx] as f64 * st;
        let q: Vec<i32> = r.iter().map(|v| ((v / delta).round() as i32).clamp(-mq, mq)).collect();
        let dist: f64 = r.iter().zip(&q).map(|(v, qi)| (v - *qi as f64 * delta).powi(2)).sum();
        out.push(Cand {
            abits: k,
            scale: idx,
            bits: sample_bits(k, n),
            dist,
            q,
        });
        if dist < 1e-2 * n as f64 {
            break;
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

pub struct X96Encoder {
    blocks: usize,
}

struct Layout {
    high_res: bool,
    sbs: usize,
    /// Number of coded subbands per channel (SBE + 1).
    nsub: Vec<usize>,
}

impl X96Encoder {
    pub fn new(blocks: usize) -> X96Encoder {
        X96Encoder { blocks }
    }

    /// `residual[ch][band 0..64][block]`; `total` is the size of the whole X96 block in bytes (it is padded to
    /// that size).
    pub fn encode(&self, residual: &[Vec<Vec<f64>>], total: usize) -> Vec<u8> {
        let cands = self.candidates_for(residual);
        let best = self.solve(&cands, &|p| self.size(&cands, p), total);
        let mut bytes = self.pack(&cands, &best, false);
        assert!(bytes.len() <= total, "X96 overflow: {} > {}", bytes.len(), total);
        bytes.resize(total, 0);
        // FSIZE96 covers the whole block
        bytes[4] = ((total - 1) >> 4) as u8;
        bytes[5] = (((total - 1) & 0xF) as u8) << 4 | (bytes[5] & 0xF);
        bytes
    }

    /// The X96 frame of an extension substream: a frame header and one channel set per group of channels
    /// (`sets[i][ch][band 0..64][block]`), `total` bytes in all. Bits are shared out over all channels together.
    pub fn encode_exss(&self, sets: &[&[Vec<Vec<f64>>]], total: usize) -> Vec<u8> {
        let flat: Vec<Vec<Vec<f64>>> = sets.iter().flat_map(|s| s.iter().cloned()).collect();
        let cands = self.candidates_for(&flat);
        let mut bounds = vec![0usize];
        for s in sets {
            bounds.push(bounds.last().unwrap() + s.len());
        }
        let hdr = frame_header_bytes(sets.len());
        let size = |p: &Vec<Vec<usize>>| -> usize {
            hdr + (0..sets.len())
                .map(|i| self.size_set(&cands[bounds[i]..bounds[i + 1]], &p[bounds[i]..bounds[i + 1]], true))
                .sum::<usize>()
        };
        let best = self.solve(&cands, &size, total);
        let mut parts: Vec<Vec<u8>> = (0..sets.len())
            .map(|i| self.pack(&cands[bounds[i]..bounds[i + 1]], &best[bounds[i]..bounds[i + 1]], true))
            .collect();
        let used: usize = hdr + parts.iter().map(|p| p.len()).sum::<usize>();
        assert!(used <= total, "X96 overflow: {used} > {total}");
        // whatever is left pads the last channel set
        let last = parts.len() - 1;
        let pad = total - used;
        let new_len = parts[last].len() + pad;
        parts[last].resize(new_len, 0);
        let mut w = BitWriter::new();
        w.put(SYNC_X96, 32);
        w.put(hdr as u32 - 1, 6);
        w.put(1, 4); // revision 1
        w.put(1, 1); // channel set headers carry a CRC
        w.put(sets.len() as u32 - 1, 2);
        for p in &parts {
            w.put(p.len() as u32 - 1, 12);
        }
        for s in sets {
            w.put(s.len() as u32 - 1, 3);
        }
        w.align(8);
        let mut out = w.into_bytes();
        let crc = crate::xll::crc16(&out[4..]);
        out.extend_from_slice(&crc.to_be_bytes());
        assert_eq!(out.len(), hdr);
        for p in parts {
            out.extend_from_slice(&p);
        }
        out
    }

    fn candidates_for(&self, residual: &[Vec<Vec<f64>>]) -> Vec<Vec<Vec<Cand>>> {
        use rayon::prelude::*;
        residual
            .par_iter()
            .map(|c| c.par_iter().map(|b| candidates(b)).collect())
            .collect()
    }

    /// Lagrangian search for the choices that fit `budget` bytes (everything at full quality when it fits).
    fn solve(
        &self,
        cands: &[Vec<Vec<Cand>>],
        size: &dyn Fn(&Vec<Vec<usize>>) -> usize,
        budget: usize,
    ) -> Vec<Vec<usize>> {
        let fits = |p: &Vec<Vec<usize>>| size(p) <= budget;
        let mut lo = 1e-9f64;
        let mut hi = 1e15f64;
        let all = self.pick(cands, lo);
        if fits(&all) {
            return all;
        }
        let mut best = self.pick(cands, hi);
        for _ in 0..90 {
            let mid = (lo * hi).sqrt();
            let p = self.pick(cands, mid);
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
    }

    fn pick(&self, cands: &[Vec<Vec<Cand>>], lambda: f64) -> Vec<Vec<usize>> {
        cands
            .iter()
            .map(|ch| {
                ch.iter()
                    .map(|b| {
                        let mut best = (f64::MAX, 0usize);
                        for (i, c) in b.iter().enumerate() {
                            let j = c.dist + lambda * (c.bits + if c.abits > 0 { 0 } else { 0 }) as f64;
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

    fn layout(&self, cands: &[Vec<Vec<Cand>>], pick: &[Vec<usize>]) -> Layout {
        let nch = cands.len();
        let mut sbs = 27usize;
        let mut nsub = vec![32usize; nch];
        let mut max_a = 0usize;
        for ch in 0..nch {
            for b in 0..BANDS {
                let c = &cands[ch][b][pick[ch][b]];
                if c.abits > 0 {
                    sbs = sbs.min(b);
                    nsub[ch] = nsub[ch].max(b + 1);
                    max_a = max_a.max(c.abits);
                }
            }
        }
        Layout {
            high_res: max_a > 6,
            sbs: sbs.min(27),
            nsub,
        }
    }

    fn size(&self, cands: &[Vec<Vec<Cand>>], pick: &[Vec<usize>]) -> usize {
        self.size_set(cands, pick, false)
    }

    /// Bytes of one channel set (core form: the whole X96 block; extension form: header and data).
    fn size_set(&self, cands: &[Vec<Vec<Cand>>], pick: &[Vec<usize>], exss: bool) -> usize {
        let lay = self.layout(cands, pick);
        let nch = cands.len();
        let abits_bits = if lay.high_res { 4 } else { 3 };
        let mut bits = if exss { 0 } else { 32 + 12 + 4 + header_bits(&lay, nch) };
        for ch in 0..nch {
            for b in lay.sbs..lay.nsub[ch] {
                let c = &cands[ch][b][pick[ch][b]];
                bits += 1 + abits_bits + 6 + c.bits;
            }
        }
        bits += 16; // DSYNC
        if exss {
            set_header_bytes(&lay, nch) + bits.div_ceil(8)
        } else {
            bits.div_ceil(8)
        }
    }

    fn pack(&self, cands: &[Vec<Vec<Cand>>], pick: &[Vec<usize>], exss: bool) -> Vec<u8> {
        let lay = self.layout(cands, pick);
        let nch = cands.len();
        let abits_bits = if lay.high_res { 4 } else { 3 };
        let mut w = BitWriter::new();
        if exss {
            w.put(set_header_bytes(&lay, nch) as u32 - 1, 7);
        } else {
            w.put(SYNC_X96, 32);
            w.put(0, 12); // FSIZE96, patched by the caller
            w.put(1, 4); // revision 1
        }
        w.put(u32::from(lay.high_res), 1);
        w.put(lay.sbs as u32, 5);
        for ch in 0..nch {
            w.put(lay.nsub[ch] as u32 - 1, 6);
        }
        for _ in 0..nch {
            w.put(0, 3); // no joint intensity
        }
        for _ in 0..nch {
            w.put(5, 3); // linear 6-bit scale factors
        }
        for _ in 0..nch {
            w.put(7, 3); // linear bit allocation indexes
        }
        for n in 0..(6 + 4 * lay.high_res as usize) {
            for _ in 0..nch {
                w.put(GROUP_SIZE[n], SEL_BITS[n]); // block codes / plain codes, no Huffman
            }
        }
        let mut head = Vec::new();
        if exss {
            // the channel set header is a whole number of bytes ending in its CRC
            w.align(8);
            head = w.into_bytes();
            let crc = crate::xll::crc16(&head);
            head.extend_from_slice(&crc.to_be_bytes());
            w = BitWriter::new();
        }
        // subframe: side information
        for ch in 0..nch {
            for _ in lay.sbs..lay.nsub[ch] {
                w.put(0, 1); // no ADPCM
            }
        }
        for ch in 0..nch {
            for b in lay.sbs..lay.nsub[ch] {
                let c = &cands[ch][b][pick[ch][b]];
                // allocation index: 0 nothing, 1 VQ, quantizer k is index k + 1
                w.put(if c.abits == 0 { 0 } else { c.abits as u32 + 1 }, abits_bits);
            }
        }
        for ch in 0..nch {
            for b in lay.sbs..lay.nsub[ch] {
                let c = &cands[ch][b][pick[ch][b]];
                w.put(if c.abits == 0 { 0 } else { c.scale as u32 }, 6);
            }
        }
        // audio data
        let nssc = self.blocks / 8;
        for ss in 0..nssc {
            for ch in 0..nch {
                for b in lay.sbs..lay.nsub[ch] {
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
        head.extend_from_slice(&w.into_bytes());
        head
    }
}

/// Bits of the channel set header's fixed part (core form, from the high-resolution flag on).
fn header_bits(lay: &Layout, nch: usize) -> usize {
    let mut bits = 1 + 5 + nch * (6 + 3 + 3 + 3);
    for n in 0..(6 + 4 * lay.high_res as usize) {
        bits += SEL_BITS[n] as usize * nch;
    }
    bits
}

/// Bytes of a channel set header in an extension substream: length, the fields above, padding, CRC16.
fn set_header_bytes(lay: &Layout, nch: usize) -> usize {
    (7 + header_bits(lay, nch) + 16).div_ceil(8)
}

/// Bytes of the X96 frame header in an extension substream for `nsets` channel sets.
fn frame_header_bytes(nsets: usize) -> usize {
    (32 + 6 + 4 + 1 + 2 + 15 * nsets + 16).div_ceil(8)
}
