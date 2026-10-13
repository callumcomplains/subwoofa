//! One ALS block (one channel of one frame): prediction, then Rice-coded residuals.
//!
//! Layout of a coded block, most significant bit first, padded to a whole byte:
//! * `1` (the block has audio), the joint-stereo flag, `0`/`1` for one or four sub-blocks,
//! * the Rice parameter of the first sub-block (4 bits, 5 for 24-bit), then the changes for the others,
//! * `0` (no shifted-out low bits), the predictor order, the quantised reflection coefficients,
//! * the residuals, sub-block by sub-block.
//!
//! A block of pure silence is just `0 0 flag 00000`.

use super::lpc::{self, dequantise, taps_from};
use crate::bitwriter::BitWriter;

/// How the coefficients are Rice coded, by table (0 for 48 kHz, 1 for 96 kHz, 2 for 192 kHz): for each of the
/// first 20 coefficients, the offset subtracted from its index and the Rice parameter.
const COEFF_CODES: [[(i32, u32); 20]; 3] = [
    [
        (-52, 4),
        (-29, 5),
        (-31, 4),
        (19, 4),
        (-16, 4),
        (12, 3),
        (-7, 3),
        (9, 3),
        (-5, 3),
        (6, 3),
        (-4, 3),
        (3, 3),
        (-3, 2),
        (3, 2),
        (-2, 2),
        (3, 2),
        (-1, 2),
        (2, 2),
        (-1, 2),
        (2, 2),
    ],
    [
        (-58, 3),
        (-42, 4),
        (-46, 4),
        (37, 5),
        (-36, 4),
        (29, 4),
        (-29, 4),
        (25, 4),
        (-23, 4),
        (20, 4),
        (-17, 4),
        (16, 4),
        (-12, 4),
        (12, 3),
        (-10, 4),
        (7, 3),
        (-4, 4),
        (3, 3),
        (-1, 3),
        (1, 3),
    ],
    [
        (-59, 3),
        (-45, 5),
        (-50, 4),
        (38, 4),
        (-39, 4),
        (32, 4),
        (-30, 4),
        (25, 3),
        (-23, 3),
        (20, 3),
        (-20, 3),
        (16, 3),
        (-13, 3),
        (10, 3),
        (-7, 3),
        (3, 3),
        (0, 3),
        (-1, 3),
        (2, 3),
        (-1, 2),
    ],
];

/// Coefficients 20 to 126 use Rice parameter 2 (and an offset of their parity), later ones parameter 1.
const MID_PARAM: u32 = 2;
const TAIL_PARAM: u32 = 1;
const FIRST_MID: usize = 20;
const FIRST_TAIL: usize = 127;

/// Which predictor orders are coded for real and compared (the analysis picks the order it likes best first).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Candidates {
    /// Only that order: half the work of `All`, and files 0.02% to 0.06% larger.
    One,
    /// That order and half of it.
    Two,
    /// None (a block coded without prediction), that order, half of it and double it.
    All,
}

impl Candidates {
    pub(crate) fn for_effort(effort: sw_core::Effort) -> Candidates {
        match effort {
            sw_core::Effort::Best => Candidates::All,
            sw_core::Effort::Balanced => Candidates::One,
            sw_core::Effort::Fast => Candidates::Two,
        }
    }
}

/// What the block needs to know about the stream.
#[derive(Clone, Copy)]
pub(crate) struct Settings {
    pub max_order: usize,
    pub bits: u32,
    pub table: usize,
    /// Try cutting the frame into shorter blocks (3 times the work for 0.1% to 0.7% smaller files).
    pub switching: bool,
    pub candidates: Candidates,
}

impl Settings {
    /// Largest Rice parameter the block can name (the field is 4 bits wide, or 5 from 24 bit).
    fn max_rice(&self) -> u32 {
        if self.bits > 16 {
            31
        } else {
            15
        }
    }

    fn rice_field_bits(&self) -> u32 {
        if self.bits > 16 {
            5
        } else {
            4
        }
    }
}

/// `value` as an ALS Rice code with parameter `k`: unary, sign, low bits (k = 0 folds the sign into the unary).
pub(crate) fn put_rice(w: &mut BitWriter, value: i32, k: u32) {
    let magnitude = if value >= 0 { value as u32 } else { !(value as u32) };
    if k == 0 {
        put_unary(w, magnitude * 2 + (value < 0) as u32);
    } else {
        put_unary(w, magnitude >> (k - 1));
        w.put((value >= 0) as u64, 1);
        w.put((magnitude & ((1 << (k - 1)) - 1)) as u64, k - 1);
    }
}

fn put_unary(w: &mut BitWriter, mut ones: u32) {
    while ones >= 32 {
        w.put(u32::MAX as u64, 32);
        ones -= 32;
    }
    w.put(((1u64 << ones) - 1) << 1, ones + 1);
}

/// Bits [`put_rice`] would write.
fn rice_bits(value: i32, k: u32) -> u64 {
    let magnitude = if value >= 0 { value as u32 } else { !(value as u32) } as u64;
    if k == 0 {
        magnitude * 2 + (value < 0) as u64 + 1
    } else {
        (magnitude >> (k - 1)) + 1 + 1 + (k - 1) as u64
    }
}

/// The cheapest Rice parameter for a stretch of residuals, and its cost in bits.
fn best_param(residual: &[i32], max_k: u32) -> (u32, u64) {
    let mean = residual.iter().map(|&r| r.unsigned_abs() as u64).sum::<u64>() / residual.len().max(1) as u64;
    let guess = (64 - mean.leading_zeros()).min(max_k);
    (guess.saturating_sub(2)..=(guess + 2).min(max_k))
        .map(|k| (k, residual.iter().map(|&r| rice_bits(r, k)).sum::<u64>()))
        .min_by_key(|&(_, bits)| bits)
        .expect("range is never empty")
}

/// Codes `signal[start..]` as one block, given the samples before `start` for the predictor. `joint` marks a block
/// that carries the difference of the channel pair. Returns the bytes, padded to a whole byte.
pub(crate) fn encode(signal: &[i32], start: usize, joint: bool, cfg: Settings) -> Vec<u8> {
    let own = &signal[start..];
    if own.iter().all(|&s| s == 0) {
        let mut w = BitWriter::default();
        w.put((joint as u64) << 5, 8);
        return w.bytes;
    }
    let mut best: Option<Vec<u8>> = None;
    let analysis = lpc::analyse(own, cfg.max_order.min(order_limit(own.len(), cfg.max_order)));
    for order in order_candidates(&analysis, own.len(), cfg.candidates) {
        let bytes = encode_with_order(signal, start, joint, cfg, order, &analysis);
        if best.as_ref().is_none_or(|b| bytes.len() < b.len()) {
            best = Some(bytes);
        }
    }
    best.expect("there is always an order to try")
}

/// Orders worth coding for real: the one the analysis likes best, half and double of it, and none.
fn order_candidates(analysis: &lpc::Analysis, len: usize, which: Candidates) -> Vec<usize> {
    let n = len as f64;
    let estimate = |m: usize| 0.5 * n * (analysis.error[m] / n + 1e-9).log2() + 6.0 * m as f64;
    let best = (0..=analysis.reflection.len())
        .min_by(|&a, &b| estimate(a).total_cmp(&estimate(b)))
        .unwrap_or(0);
    let mut candidates = vec![0, best, best / 2, (best * 2).min(analysis.reflection.len())];
    match which {
        Candidates::One => candidates = vec![best],
        Candidates::Two => candidates = vec![best, best / 2],
        Candidates::All => {}
    }
    candidates.sort_unstable();
    candidates.dedup();
    candidates
}

/// The largest order the block header can name for a block of `len` samples.
fn order_limit(len: usize, max_order: usize) -> usize {
    (1usize << order_bits(len, max_order)) - 1
}

/// Width of the predictor order field: `ceil(log2(clip(len / 8 - 1, 2, max_order + 1)))`.
fn order_bits(len: usize, max_order: usize) -> u32 {
    let clipped = ((len >> 3) as i64 - 1).clamp(2, max_order as i64 + 1) as u32;
    32 - (clipped - 1).leading_zeros()
}

/// Codes the block with a predictor of `order`, whose coefficients are the first `order` of `analysis` (the
/// reflection coefficients of a lower order are the first ones of a higher order's).
fn encode_with_order(
    signal: &[i32],
    start: usize,
    joint: bool,
    cfg: Settings,
    order: usize,
    analysis: &lpc::Analysis,
) -> Vec<u8> {
    let own = &signal[start..];
    let (indices, taps) = if order > 0 {
        let indices = quantise(&analysis.reflection[..order.min(analysis.reflection.len())]);
        let par: Vec<i32> = indices.iter().enumerate().map(|(k, &q)| dequantise(k, q)).collect();
        (indices, taps_from(&par))
    } else {
        (Vec::new(), Vec::new())
    };
    let order = taps.len();
    let residual = lpc::residual(signal, start, &taps);

    let mut w = BitWriter::default();
    w.put(1, 1); // a block with audio
    w.put(joint as u64, 1);
    let (parts, params) = best_partition(&residual, cfg);
    w.put((parts == 4) as u64, 1);
    w.put(params[0] as u64, cfg.rice_field_bits());
    for pair in params.windows(2) {
        put_rice(&mut w, pair[1] as i32 - pair[0] as i32, 0);
    }
    w.put(0, 1); // no shifted-out low bits
    w.put(order as u64, order_bits(own.len(), cfg.max_order));
    for (k, &q) in indices.iter().enumerate() {
        put_coefficient(&mut w, k, q, cfg.table);
    }
    let sub_len = residual.len() / parts;
    for (part, &k) in params.iter().enumerate() {
        for &r in &residual[part * sub_len..(part + 1) * sub_len] {
            put_rice(&mut w, r, k);
        }
    }
    w.align();
    w.bytes
}

/// One Rice parameter for the whole block, or one for each quarter; whichever costs fewer bits in all.
fn best_partition(residual: &[i32], cfg: Settings) -> (usize, Vec<u32>) {
    let max_k = cfg.max_rice();
    let (k, whole_bits) = best_param(residual, max_k);
    let whole = (whole_bits + cfg.rice_field_bits() as u64, vec![k]);
    if !residual.len().is_multiple_of(4) || residual.len() < 32 {
        return (1, whole.1);
    }
    let quarters: Vec<(u32, u64)> = residual
        .chunks(residual.len() / 4)
        .map(|c| best_param(c, max_k))
        .collect();
    let params: Vec<u32> = quarters.iter().map(|q| q.0).collect();
    let mut bits = cfg.rice_field_bits() as u64 + quarters.iter().map(|q| q.1).sum::<u64>();
    for pair in params.windows(2) {
        bits += rice_bits(pair[1] as i32 - pair[0] as i32, 0);
    }
    if bits < whole.0 {
        (4, params)
    } else {
        (1, whole.1)
    }
}

/// Quantiser indices for reflection coefficients (the first two companded, with the second's sign flipped).
fn quantise(reflection: &[f64]) -> Vec<i32> {
    reflection
        .iter()
        .enumerate()
        .map(|(k, &value)| match k {
            0 => lpc::companded_index(value),
            1 => lpc::companded_index(-value),
            _ => lpc::uniform_index(value),
        })
        .collect()
}

fn put_coefficient(w: &mut BitWriter, k: usize, index: i32, table: usize) {
    if k < FIRST_MID {
        let (offset, param) = COEFF_CODES[table][k];
        put_rice(w, index - offset, param);
    } else if k < FIRST_TAIL {
        put_rice(w, index - (k & 1) as i32, MID_PARAM);
    } else {
        put_rice(w, index, TAIL_PARAM);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rice_codes_have_the_sizes_the_format_gives() {
        for (value, k) in [(0, 0), (-1, 0), (5, 0), (0, 1), (-7, 3), (100, 4), (-100, 6)] {
            let mut w = BitWriter::default();
            put_rice(&mut w, value, k);
            w.align();
            let written = w.bytes.len() as u64 * 8;
            let bits = rice_bits(value, k);
            assert!(
                written >= bits && written < bits + 8,
                "{value} k={k}: {written} bits written vs {bits}"
            );
        }
    }

    #[test]
    fn order_field_follows_the_block_length() {
        assert_eq!(order_bits(2048, 32), 6);
        assert_eq!(order_bits(16, 32), 1);
        assert_eq!(order_limit(2048, 32), 63);
    }

    #[test]
    fn silence_is_one_byte() {
        let cfg = Settings {
            max_order: 32,
            bits: 16,
            table: 0,
            switching: true,
            candidates: Candidates::All,
        };
        assert_eq!(encode(&vec![0; 3000], 0, false, cfg).len(), 1);
    }
}
