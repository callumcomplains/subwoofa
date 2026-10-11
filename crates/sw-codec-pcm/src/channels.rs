//! Where each speaker goes in a PCM file.
//!
//! WAV (and FLAC, which borrows its convention) numbers speakers with the bits of a "channel mask" and stores the
//! channels in ascending bit order. CAF instead labels every channel. Both need a translation from this program's
//! speaker names.

use sw_core::{Channel, ChannelLayout};

/// WAV speaker position bits (the `dwChannelMask` of WAVEFORMATEXTENSIBLE).
pub mod wav_bit {
    pub const FRONT_LEFT: u32 = 0x1;
    pub const FRONT_RIGHT: u32 = 0x2;
    pub const FRONT_CENTER: u32 = 0x4;
    pub const LFE: u32 = 0x8;
    pub const BACK_LEFT: u32 = 0x10;
    pub const BACK_RIGHT: u32 = 0x20;
    pub const FRONT_LEFT_OF_CENTER: u32 = 0x40;
    pub const FRONT_RIGHT_OF_CENTER: u32 = 0x80;
    pub const BACK_CENTER: u32 = 0x100;
    pub const SIDE_LEFT: u32 = 0x200;
    pub const SIDE_RIGHT: u32 = 0x400;
    pub const TOP_CENTER: u32 = 0x800;
    pub const TOP_FRONT_LEFT: u32 = 0x1000;
    pub const TOP_FRONT_CENTER: u32 = 0x2000;
    pub const TOP_FRONT_RIGHT: u32 = 0x4000;
    pub const TOP_BACK_LEFT: u32 = 0x8000;
    pub const TOP_BACK_CENTER: u32 = 0x10000;
    pub const TOP_BACK_RIGHT: u32 = 0x20000;
}

/// The WAV mask bit for a speaker, or `None` when WAV has no position for it.
///
/// "Ls/Rs" are the *side* speakers when the layout also has a rear pair (7.1) and the *back* speakers otherwise
/// (5.1), which is how players and the WAV 5.1 / 7.1 defaults read them.
fn wav_bit_of(channel: Channel, layout: &ChannelLayout) -> Option<u32> {
    use wav_bit::*;
    use Channel::*;
    let has_rear_pair = layout.channels.iter().any(|c| matches!(c, Lsr | Rsr));
    Some(match channel {
        L | Lt => FRONT_LEFT,
        R | Rt => FRONT_RIGHT,
        C => FRONT_CENTER,
        Lfe => LFE,
        Ls if has_rear_pair => SIDE_LEFT,
        Rs if has_rear_pair => SIDE_RIGHT,
        Ls => BACK_LEFT,
        Rs => BACK_RIGHT,
        Lss => SIDE_LEFT,
        Rss => SIDE_RIGHT,
        Lsr => BACK_LEFT,
        Rsr => BACK_RIGHT,
        Cs => BACK_CENTER,
        Lw => FRONT_LEFT_OF_CENTER,
        Rw => FRONT_RIGHT_OF_CENTER,
        Lh | Tfl => TOP_FRONT_LEFT,
        Rh | Tfr => TOP_FRONT_RIGHT,
        Ch | Tfc => TOP_FRONT_CENTER,
        Oh | Tc => TOP_CENTER,
        Tbl => TOP_BACK_LEFT,
        Tbr => TOP_BACK_RIGHT,
        Tbc => TOP_BACK_CENTER,
        _ => return None,
    })
}

/// Order and mask of a WAV-style file.
pub struct MaskOrder {
    /// Layout channel index for every channel of the file, in file order.
    pub order: Vec<usize>,
    /// The channel mask, or 0 when the layout has speakers WAV cannot name (the channels then stay in layout
    /// order and players treat them as unpositioned).
    pub mask: u32,
}

/// Channel order for WAV and FLAC: ascending mask bit when every speaker has a distinct position, else layout order.
pub fn wav_order(layout: &ChannelLayout) -> MaskOrder {
    let bits: Vec<Option<u32>> = layout.channels.iter().map(|&c| wav_bit_of(c, layout)).collect();
    let all_named = bits.iter().all(|b| b.is_some());
    let mask = bits.iter().flatten().fold(0u32, |m, b| m | b);
    let distinct = mask.count_ones() as usize == layout.channels.len();
    if all_named && distinct {
        let mut order: Vec<usize> = (0..layout.channels.len()).collect();
        order.sort_by_key(|&i| bits[i].unwrap());
        MaskOrder { order, mask }
    } else {
        MaskOrder { order: (0..layout.channels.len()).collect(), mask: 0 }
    }
}

/// CAF channel label (Core Audio's `kCAFChannelLabel_*`), or a numbered "discrete" label for speakers it lacks.
pub fn caf_label(channel: Channel, index: usize) -> u32 {
    use Channel::*;
    match channel {
        L | Lt => 1,
        R | Rt => 2,
        C => 3,
        Lfe => 4,
        Ls => 5,
        Rs => 6,
        Lw => 7,
        Rw => 8,
        Cs => 9,
        Lss => 10,
        Rss => 11,
        Oh | Tc => 12,
        Lh | Tfl => 13,
        Ch | Tfc => 14,
        Rh | Tfr => 15,
        Tbl => 16,
        Tbc => 17,
        Tbr => 18,
        Lsr => 33,
        Rsr => 34,
        Lfe2 => 37,
        // no Core Audio name: a discrete channel numbered by its place
        _ => (1 << 16) | index as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(id: &str) -> &'static ChannelLayout {
        ChannelLayout::find(id).unwrap()
    }

    #[test]
    fn five_one_is_the_standard_wav_mask() {
        let m = wav_order(layout("5.1"));
        assert_eq!(m.mask, 0x3F); // FL FR FC LFE BL BR
        // layout order is L R C LFE Ls Rs: the file wants L R C LFE Ls Rs, i.e. unchanged
        assert_eq!(m.order, vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn seven_one_side_rear_is_the_standard_wav_mask() {
        let m = wav_order(layout("7.1-side-rear"));
        assert_eq!(m.mask, 0x63F); // FL FR FC LFE BL BR SL SR
        // file order L R C LFE Lsr Rsr Lss Rss
        assert_eq!(m.order, vec![0, 1, 2, 3, 6, 7, 4, 5]);
    }

    #[test]
    fn nhk_has_no_wav_mask() {
        let m = wav_order(layout("22.2"));
        assert_eq!(m.mask, 0);
        assert_eq!(m.order.len(), 24);
    }
}
