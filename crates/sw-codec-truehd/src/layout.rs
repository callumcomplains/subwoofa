//! Which TrueHD (and MLP) channel positions a layout's speakers take, and in what order they are coded.

use crate::tables::GROUP_CHANNELS;
use crate::{Error, MAX_CHANNELS};
use sw_core::Channel;
use sw_core::ChannelLayout;

/// Channel position in TrueHD order: (group index, 0 = left/only, 1 = right).
pub(crate) fn thd_position(c: Channel) -> Option<(usize, usize)> {
    use Channel::*;
    Some(match c {
        L | Lt => (0, 0),
        R | Rt => (0, 1),
        C => (1, 0),
        Lfe => (2, 0),
        Ls | Lss => (3, 0),
        Rs | Rss => (3, 1),
        Lh | Lhs => (4, 0),
        Rh | Rhs => (4, 1),
        Lsr => (6, 0),
        Rsr => (6, 1),
        Cs => (7, 0),
        Oh => (8, 0),
        Lw => (10, 0),
        Rw => (10, 1),
        Ch => (11, 0),
        _ => return None,
    })
}

/// How a layout maps onto TrueHD channels.
#[derive(Debug, Clone)]
pub struct LayoutMap {
    /// 13-bit channel group map for the full presentation.
    pub chanmap: u16,
    /// Index into `layout.channels` for each coded channel, in TrueHD order.
    pub source_index: Vec<usize>,
}

pub fn map_layout(layout: &ChannelLayout) -> Result<LayoutMap, Error> {
    let mut items: Vec<((usize, usize), usize)> = Vec::new();
    for (i, c) in layout.channels.iter().enumerate() {
        let pos = thd_position(*c)
            .ok_or_else(|| Error::Unsupported(format!("channel {} has no TrueHD position", c.short())))?;
        items.push((pos, i));
    }
    items.sort();
    items.windows(2).try_for_each(|w| {
        if w[0].0 == w[1].0 {
            Err(Error::Unsupported(format!(
                "layout '{}' has two channels in the same TrueHD position",
                layout.id
            )))
        } else {
            Ok(())
        }
    })?;
    if items.len() > MAX_CHANNELS {
        return Err(Error::Unsupported("TrueHD carries at most 8 channels here".into()));
    }
    let mut chanmap = 0u16;
    for ((g, _), _) in &items {
        chanmap |= 1 << g;
    }
    // a group present with only one of its two channels is not expressible
    let expected: usize = (0..13)
        .filter(|g| chanmap >> g & 1 == 1)
        .map(|g| GROUP_CHANNELS[g])
        .sum();
    if expected != items.len() {
        return Err(Error::Unsupported(format!(
            "layout '{}' has an unpaired channel (TrueHD groups channels in L/R pairs)",
            layout.id
        )));
    }
    Ok(LayoutMap {
        chanmap,
        source_index: items.into_iter().map(|(_, i)| i).collect(),
    })
}

/// MLP stream parameters.
#[derive(Debug, Clone, Copy)]
pub struct Mlp {
    /// Channel assignment code of the major sync (see `map_layout_mlp`).
    pub arrangement: u8,
    /// Source word length (16, 20 or 24).
    pub bits: u32,
}

/// `(channel occupancy, summary info)` of the MLP channel assignments 0..=12.
pub(crate) const MLP_CHANNEL_INFO: [(u8, u8); 13] = [
    (0x01, 0x1f),
    (0x03, 0x1b),
    (0x07, 0x1f),
    (0x0F, 0x19),
    (0x07, 0x03),
    (0x0F, 0x1f),
    (0x1F, 0x01),
    (0x07, 0x1a),
    (0x0F, 0x1f),
    (0x1F, 0x18),
    (0x0F, 0x02),
    (0x1F, 0x1f),
    (0x3F, 0x00),
];

/// MLP channel assignment and coded channel order for a layout (the layouts the format can describe).
pub fn map_layout_mlp(layout: &ChannelLayout) -> Result<(u8, Vec<usize>), Error> {
    use sw_core::Channel::*;
    const TABLE: &[(u8, &[sw_core::Channel])] = &[
        (0, &[C]),
        (1, &[L, R]),
        (2, &[L, R, Cs]),
        (3, &[L, R, Ls, Rs]),
        (4, &[L, R, Lfe]),
        (7, &[L, R, C]),
        (8, &[L, R, C, Cs]),
        (9, &[L, R, C, Ls, Rs]),
        (10, &[L, R, C, Lfe]),
        (11, &[L, R, C, Lfe, Cs]),
        (12, &[L, R, C, Lfe, Ls, Rs]),
    ];
    for (code, order) in TABLE {
        if order.len() == layout.channels.len() && order.iter().all(|c| layout.channels.contains(c)) {
            let idx = order
                .iter()
                .map(|c| layout.channels.iter().position(|x| x == c).unwrap())
                .collect();
            return Ok((*code, idx));
        }
    }
    Err(Error::Unsupported(format!(
        "layout '{}' has no MLP channel assignment (mono to 5.1 in the DVD-Audio arrangements)",
        layout.id
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_one_layout_order() {
        let m = map_layout(ChannelLayout::find("5.1").unwrap()).unwrap();
        // L R C LFE Ls Rs is already TrueHD order
        assert_eq!(m.source_index, vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(m.chanmap, 0b1111);
    }

    #[test]
    fn seven_one_has_rear_group() {
        let m = map_layout(ChannelLayout::find("7.1-side-rear").unwrap()).unwrap();
        assert_eq!(m.source_index.len(), 8);
        assert_eq!(m.chanmap, 0b100_1111);
    }

    #[test]
    fn a_layout_with_a_lone_surround_is_accepted_when_it_pairs_up() {
        assert!(map_layout(ChannelLayout::find("4.0-lcrs").unwrap()).is_ok());
    }
}
