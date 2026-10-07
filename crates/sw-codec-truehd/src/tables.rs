//! Constant tables from the MLP / TrueHD bitstream definition.

/// Huffman codes for the three residual codebooks: `(code, length)` for symbol 0..=17.
/// Table 0 covers values -7..=+10, table 1 -7..=+8, table 2 -7..=+7.
pub const HUFFMAN: [[(u8, u8); 18]; 3] = [
    [
        (0x01, 9), (0x01, 8), (0x01, 7), (0x01, 6), (0x01, 5), (0x01, 4), (0x01, 3),
        (0x04, 3), (0x05, 3), (0x06, 3), (0x07, 3),
        (0x03, 3), (0x05, 4), (0x09, 5), (0x11, 6), (0x21, 7), (0x41, 8), (0x81, 9),
    ],
    [
        (0x01, 9), (0x01, 8), (0x01, 7), (0x01, 6), (0x01, 5), (0x01, 4), (0x01, 3),
        (0x02, 2), (0x03, 2),
        (0x03, 3), (0x05, 4), (0x09, 5), (0x11, 6), (0x21, 7), (0x41, 8), (0x81, 9),
        // padding so every table has 18 entries; these symbols are never used
        (0, 0), (0, 0),
    ],
    [
        (0x01, 9), (0x01, 8), (0x01, 7), (0x01, 6), (0x01, 5), (0x01, 4), (0x01, 3),
        (0x01, 1),
        (0x03, 3), (0x05, 4), (0x09, 5), (0x11, 6), (0x21, 7), (0x41, 8), (0x81, 9),
        (0, 0), (0, 0), (0, 0),
    ],
];

/// TrueHD channel groups in bitstream order. Bit `i` of the 13-bit channel map selects group `i`.
/// (L R), C, LFE, (Ls Rs), (Lvh Rvh), (Lc Rc), (Lrs Rrs), Cs, Ts, (Lsd Rsd), (Lw Rw), Cvh, LFE2.
pub const GROUP_CHANNELS: [usize; 13] = [2, 1, 1, 2, 2, 2, 2, 1, 1, 2, 2, 1, 1];
