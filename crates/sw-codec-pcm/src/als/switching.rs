//! Block switching: a frame of one channel (or of a joint-stereo pair) is cut into 1 to 16 blocks, halving as often
//! as it pays.
//!
//! The cut is a binary tree. Node 0 is the whole frame, node `n` has the halves `2n + 1` and `2n + 2`. A 16-bit
//! field at the start of the unit says which nodes are split: its first bit is the "channels are independent"
//! flag (always 0 here: pairs are coded together), the next 15 are the split flags of nodes 0 to 14. Nodes 7 to 14
//! are the deepest ones that can be split, so the smallest block is a sixteenth of the frame.
//!
//! The encoder codes every node as one block, then keeps the split wherever the two halves together are smaller.

/// Config value of the block switching field: 2 means a 16-bit field.
pub(super) const CONFIG_LEVEL: u64 = 2;

/// Bits of the split field.
const FIELD_BITS: u32 = 16;

/// Nodes with an index above this cannot be split (they are the leaves of the deepest level).
const LAST_SPLITTABLE: usize = FIELD_BITS as usize - 2;

/// Blocks shorter than this are not split further.
const MIN_SPLIT_LEN: usize = 64;

/// What coding a stretch of the frame produced: one block per channel of the unit (one or two), already padded.
pub(super) type Blocks = Vec<Vec<u8>>;

fn size(blocks: &Blocks) -> usize {
    blocks.iter().map(|b| b.len()).sum()
}

/// The cheapest cut of the stretch `start..start + len`, as the blocks in time order and the split nodes.
/// `code(start, len)` codes one stretch as a single block per channel.
fn best(
    start: usize,
    len: usize,
    node: usize,
    code: &dyn Fn(usize, usize) -> Blocks,
) -> (usize, Vec<Blocks>, Vec<usize>) {
    let whole = code(start, len);
    let whole_size = size(&whole);
    if node > LAST_SPLITTABLE || len % 2 != 0 || len < MIN_SPLIT_LEN {
        return (whole_size, vec![whole], Vec::new());
    }
    let half = len / 2;
    let (left_size, mut blocks, mut splits) = best(start, half, 2 * node + 1, code);
    let (right_size, right_blocks, right_splits) = best(start + half, half, 2 * node + 2, code);
    if left_size + right_size < whole_size {
        blocks.extend(right_blocks);
        splits.extend(right_splits);
        splits.push(node);
        (left_size + right_size, blocks, splits)
    } else {
        (whole_size, vec![whole], Vec::new())
    }
}

/// The coded unit: the split field, then every block of every channel in the order the decoder reads them (block by
/// block, the channels of a pair alternating). A short last frame is not cut.
pub(super) fn code_unit(len: usize, full: bool, code: &dyn Fn(usize, usize) -> Blocks) -> Vec<u8> {
    let (blocks, splits) = if full {
        let (_, blocks, splits) = best(0, len, 0, code);
        (blocks, splits)
    } else {
        (vec![code(0, len)], Vec::new())
    };
    let mut field = 0u16;
    for node in splits {
        field |= 1 << (FIELD_BITS - 2 - node as u32);
    }
    let mut out = field.to_be_bytes().to_vec();
    for block in blocks {
        for channel in block {
            out.extend(channel);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A coder whose cost is a fixed overhead per block plus a cost per sample.
    fn flat(overhead: usize) -> impl Fn(usize, usize) -> Blocks {
        move |_, len| vec![vec![0u8; overhead + len]]
    }

    #[test]
    fn even_material_is_not_split() {
        let unit = code_unit(1024, true, &flat(10));
        assert_eq!(&unit[..2], &[0, 0]);
        assert_eq!(unit.len(), 2 + 10 + 1024);
    }

    #[test]
    fn a_change_of_level_is_split() {
        // each block pays its loudest sample on every sample, as a Rice parameter would
        let costs: Vec<usize> = (0..1024).map(|i| if i < 512 { 1 } else { 9 }).collect();
        let code = |start: usize, len: usize| -> Blocks {
            let loudest = costs[start..start + len].iter().copied().max().unwrap();
            vec![vec![0u8; 5 + loudest * len / 4]]
        };
        let unit = code_unit(1024, true, &code);
        assert_eq!(unit[0] & 0x40, 0x40, "the root is split");
        assert!(unit.len() < 2 + 5 + 9 * 1024 / 4);
    }

    #[test]
    fn a_short_frame_is_one_block() {
        let unit = code_unit(300, false, &flat(10));
        assert_eq!(unit.len(), 2 + 10 + 300);
    }
}
