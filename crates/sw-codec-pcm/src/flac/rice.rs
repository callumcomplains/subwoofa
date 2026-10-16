//! Rice coding of the prediction error: the cheapest parameters for a block's partitions, and writing them.

use crate::bitwriter::{put_rice, BitWriter};

/// Zigzag mapping of a signed residual to an unsigned number (0, -1, 1, -2 ... becomes 0, 1, 2, 3 ...).
pub(crate) fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

/// How a residual is split into partitions and what Rice parameter each uses.
pub(crate) struct RiceCoding {
    pub(crate) partition_order: u32,
    pub(crate) parameters: Vec<u32>,
    /// Parameters take 5 bits instead of 4 (needed when one exceeds 14).
    pub(crate) wide: bool,
    pub(crate) bits: usize,
}

/// The cheapest partitioning of `residual`, which follows `predictor_order` warm-up samples in a block of
/// `block_len` samples.
///
/// A partition of a coarser order is a run of partitions of the finest order, so the cost of every Rice parameter
/// is worked out once for each finest partition and added up for the coarser ones.
pub(crate) fn rice_code(residual: &[i32], predictor_order: usize, block_len: usize, max_partition: u32) -> RiceCoding {
    let coded: Vec<u64> = residual.iter().map(|&r| zigzag(r as i64)).collect();
    let mut finest = 0;
    for order in 1..=max_partition {
        let parts = 1usize << order;
        if !block_len.is_multiple_of(parts) || block_len / parts <= predictor_order {
            break;
        }
        finest = order;
    }
    let fine_parts = 1usize << finest;
    let size = block_len / fine_parts;
    // the residual covers each partition of the block less the warm-up samples at the start of the first
    let spans: Vec<(usize, usize)> = (0..fine_parts)
        .map(|p| {
            (
                if p == 0 { 0 } else { p * size - predictor_order },
                (p + 1) * size - predictor_order,
            )
        })
        .collect();
    let plain_sums: Vec<u64> = spans.iter().map(|&(a, b)| coded[a..b].iter().sum()).collect();
    // the parameters worth trying for a partition are the ones around log2 of its mean
    let guess = |sum: u64, len: usize| 64 - (sum / len.max(1) as u64).leading_zeros();
    let (mut lowest, mut highest) = (u32::MAX, 0u32);
    for order in 0..=finest {
        let group = 1usize << (finest - order);
        for run in plain_sums.chunks(group).zip(spans.chunks(group)) {
            let len = run.1.last().unwrap().1 - run.1[0].0;
            let g = guess(run.0.iter().sum(), len);
            lowest = lowest.min(g.saturating_sub(2));
            highest = highest.max((g + 1).min(30));
        }
    }
    // shifted_sums[p][k - lowest]: sum over the finest partition p of (value >> k)
    let shifted_sums: Vec<Vec<u64>> = spans
        .iter()
        .map(|&(a, b)| {
            (lowest..=highest)
                .map(|k| coded[a..b].iter().map(|&v| v >> k).sum())
                .collect()
        })
        .collect();
    let mut best: Option<RiceCoding> = None;
    for order in 0..=finest {
        let group = 1usize << (finest - order);
        let parts = 1usize << order;
        let mut parameters = Vec::with_capacity(parts);
        let mut payload = 0usize;
        for p in 0..parts {
            let fine = p * group..(p + 1) * group;
            let len = spans[fine.end - 1].1 - spans[fine.start].0;
            let g = guess(plain_sums[fine.clone()].iter().sum(), len);
            let mut choice = (0u32, usize::MAX);
            for k in g.saturating_sub(2)..=(g + 1).min(30) {
                let shifted: u64 = fine.clone().map(|f| shifted_sums[f][(k - lowest) as usize]).sum();
                let bits = shifted as usize + len * (1 + k as usize);
                if bits < choice.1 {
                    choice = (k, bits);
                }
            }
            parameters.push(choice.0);
            payload += choice.1;
        }
        let wide = parameters.iter().any(|&k| k > 14);
        let bits = 2 + 4 + parts * (4 + usize::from(wide)) + payload;
        if best.as_ref().is_none_or(|b| bits < b.bits) {
            best = Some(RiceCoding {
                partition_order: order,
                parameters,
                wide,
                bits,
            });
        }
    }
    best.expect("partition order 0 always fits")
}

pub(crate) fn write_rice(
    w: &mut BitWriter,
    residual: &[i32],
    predictor_order: usize,
    block_len: usize,
    coding: &RiceCoding,
) {
    w.put(u32::from(coding.wide), 2);
    w.put(coding.partition_order, 4);
    let parts = 1usize << coding.partition_order;
    let size = block_len / parts;
    let mut start = 0usize;
    for p in 0..parts {
        let end = if p == 0 { size - predictor_order } else { start + size };
        let k = coding.parameters[p];
        w.put(k, if coding.wide { 5 } else { 4 });
        for &r in &residual[start..end] {
            put_rice(w, zigzag(r as i64), k);
        }
        start = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitwriter::BitWriter;

    #[test]
    fn rice_roundtrip_size() {
        let residual: Vec<i32> = (0..256).map(|i| ((i * 37) % 11) - 5).collect();
        let coding = rice_code(&residual, 0, 256, 8);
        let mut w = BitWriter::default();
        write_rice(&mut w, &residual, 0, 256, &coding);
        w.align(8);
        assert_eq!(w.bytes().len(), coding.bits.div_ceil(8));
    }
}
