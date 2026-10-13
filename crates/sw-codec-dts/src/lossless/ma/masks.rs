//! Speaker masks: the lossless extension's own channel mask and the extension substream's speaker groups.

use sw_core::Channel;

/// Speaker position bit of the lossless extension's channel mask.
pub(crate) fn xll_bit(c: Channel) -> u32 {
    use Channel::*;
    match c {
        C => 0,
        L | Lt => 1,
        R | Rt => 2,
        Ls => 3,
        Rs => 4,
        Lfe => 5,
        Cs => 6,
        Lsr => 7,
        Rsr => 8,
        Lss => 9,
        Rss => 10,
        Lh => 13,
        Ch => 14,
        Rh => 15,
        Lhs => 20,
        Rhs => 21,
        Lw => 17,
        Rw => 18,
        Oh | Tc => 19,
        Lfe2 => 16,
        Tfl => 13,
        Tfc => 14,
        Tfr => 15,
        Tsl => 20,
        Tsr => 21,
        Tbc => 22,
        Tbl => 23,
        Tbr => 24,
        Bfc => 25,
        Bfl => 26,
        Bfr => 27,
    }
}

/// Loudspeaker group bit of the extension substream's activity mask (Table 7-10).
pub fn exss_bit(c: Channel) -> u32 {
    use Channel::*;
    match c {
        C => 0x1,
        L | R | Lt | Rt => 0x2,
        Ls | Rs => 0x4,
        Lfe => 0x8,
        Cs => 0x10,
        Lh | Rh => 0x20,
        Lsr | Rsr => 0x40,
        Ch => 0x80,
        Oh => 0x100,
        Lw | Rw => 0x400,
        Lss | Rss => 0x800,
        Lfe2 => 0x1000,
        Lhs | Rhs => 0x2000,
        Tbc => 0x4000,
        Tbl | Tbr => 0x8000,
        Tfl | Tfr => 0x20,
        Tfc => 0x80,
        Tc => 0x100,
        Tsl | Tsr => 0x2000,
        _ => 0, // the bottom layer has no group in the 16-bit activity mask
    }
}
