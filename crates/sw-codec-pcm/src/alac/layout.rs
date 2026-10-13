//! Apple's channel arrangements: which layout channels form each element (single, pair) and the layout tag.

use sw_core::{Channel, ChannelLayout};

/// Syntax element kinds in the frame.
pub(crate) const ELEMENT_SINGLE: u32 = 0;

pub(crate) const ELEMENT_PAIR: u32 = 1;

pub(crate) const ELEMENT_END: u32 = 7;

/// How many single/pair elements make up each channel count (1 to 8), as Apple lays them out.
pub(crate) fn elements_for(channels: usize) -> &'static [u32] {
    const S: u32 = ELEMENT_SINGLE;
    const P: u32 = ELEMENT_PAIR;
    match channels {
        1 => &[S],
        2 => &[P],
        3 => &[S, P],
        4 => &[S, P, S],
        5 => &[S, P, P],
        6 => &[S, P, P, S],
        7 => &[S, P, P, S, S],
        _ => &[S, P, P, P, S],
    }
}

/// Apple's channel order and layout tag for a layout, or `None` when ALAC has no arrangement for it.
/// Returns the layout channel indexes in ALAC order.
pub fn alac_order(layout: &ChannelLayout) -> Option<(Vec<usize>, u32)> {
    use Channel::*;
    let (wanted, tag): (&[Channel], u32) = match layout.channels.len() {
        1 => (&[C], (100 << 16) | 1),
        2 => (&[L, R], (101 << 16) | 2),
        3 => (&[C, L, R], (114 << 16) | 3),
        4 => (&[C, L, R, Cs], (116 << 16) | 4),
        5 => (&[C, L, R, Ls, Rs], (120 << 16) | 5),
        6 => (&[C, L, R, Ls, Rs, Lfe], (124 << 16) | 6),
        7 => (&[C, L, R, Ls, Rs, Cs, Lfe], (142 << 16) | 7),
        8 => (&[C, Lw, Rw, L, R, Ls, Rs, Lfe], (127 << 16) | 8),
        _ => return None,
    };
    let order: Option<Vec<usize>> = wanted
        .iter()
        .map(|w| layout.channels.iter().position(|c| c == w))
        .collect();
    order.map(|o| (o, tag))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sw_core::ChannelLayout;

    #[test]
    fn five_one_goes_in_apples_order() {
        let layout = ChannelLayout::find("5.1").unwrap(); // L R C LFE Ls Rs
        let (order, tag) = alac_order(layout).unwrap();
        assert_eq!(order, vec![2, 0, 1, 4, 5, 3]); // C L R Ls Rs LFE
        assert_eq!(tag, (124 << 16) | 6);
    }

    #[test]
    fn layouts_without_an_apple_arrangement_are_refused() {
        assert!(alac_order(ChannelLayout::find("7.1-side-rear").unwrap()).is_none());
        assert!(alac_order(ChannelLayout::find("5.1.4").unwrap()).is_none());
    }
}
