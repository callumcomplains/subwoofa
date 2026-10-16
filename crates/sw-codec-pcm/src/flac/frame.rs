//! A whole frame: how a stereo pair is coded, the channels' subframes and the frame header.

use crate::bitwriter::BitWriter;
use crate::flac::check::{crc16, crc8};
use crate::flac::predict::{code_channel, write_subframe, Subframe};
use crate::flac::search::Search;
use crate::roughness;

/// How the channels of a stereo frame are stored.
#[derive(Clone, Copy)]
pub(crate) enum Assignment {
    Independent,
    LeftSide,
    RightSide,
    MidSide,
}

/// A frame (all channels of one block) as bytes.
pub(crate) fn encode_frame(
    channels: &[Vec<i32>],
    bits: u32,
    frame_number: u64,
    block_size_code: u8,
    search: Search,
) -> Vec<u8> {
    let n = channels[0].len();
    let (assignment, subframes) = if channels.len() == 2 {
        choose_stereo(&channels[0], &channels[1], bits, search)
    } else {
        (
            Assignment::Independent,
            channels.iter().map(|c| code_channel(c, bits, search)).collect(),
        )
    };
    let mut w = BitWriter::default();
    w.put(0xFFF8, 16); // sync code and fixed block size
    w.put(block_size_code as u32, 4);
    w.put(0, 4); // sample rate from STREAMINFO
    w.put(
        match assignment {
            Assignment::Independent => (channels.len() as u64 - 1) as u32,
            Assignment::LeftSide => 8,
            Assignment::RightSide => 9,
            Assignment::MidSide => 10,
        },
        4,
    );
    w.put(0, 3); // sample size from STREAMINFO
    w.put(0, 1);
    put_utf8_number(&mut w, frame_number);
    match block_size_code {
        6 => w.put((n as u64 - 1) as u32, 8),
        7 => w.put((n as u64 - 1) as u32, 16),
        _ => {}
    }
    let crc = crc8(w.bytes());
    w.put(crc as u32, 8);
    for s in &subframes {
        write_subframe(&mut w, s);
    }
    w.align(8);
    let crc = crc16(w.bytes());
    w.put(crc as u32, 16);
    w.into_bytes()
}

/// Codes a stereo block as left/right, left/side, right/side or mid/side, whichever is smallest.
pub(crate) fn choose_stereo(left: &[i32], right: &[i32], bits: u32, search: Search) -> (Assignment, Vec<Subframe>) {
    let side: Vec<i32> = left.iter().zip(right).map(|(l, r)| l - r).collect();
    let mid: Vec<i32> = left
        .iter()
        .zip(right)
        .map(|(l, r)| ((*l as i64 + *r as i64) >> 1) as i32)
        .collect();
    if !search.all_stereo {
        let (assignment, _) = stereo_options(roughness(left), roughness(right), roughness(&side), roughness(&mid));
        let code = |x: &[i32], extra: u32| code_channel(x, bits + extra, search);
        let subframes = match assignment {
            Assignment::Independent => vec![code(left, 0), code(right, 0)],
            Assignment::LeftSide => vec![code(left, 0), code(&side, 1)],
            Assignment::RightSide => vec![code(&side, 1), code(right, 0)],
            Assignment::MidSide => vec![code(&mid, 0), code(&side, 1)],
        };
        return (assignment, subframes);
    }
    let l = code_channel(left, bits, search);
    let r = code_channel(right, bits, search);
    let s = code_channel(&side, bits + 1, search);
    let m = code_channel(&mid, bits, search);
    let (assignment, _) = stereo_options(l.bits, r.bits, s.bits, m.bits);
    let subframes = match assignment {
        Assignment::Independent => vec![l, r],
        Assignment::LeftSide => vec![l, s],
        Assignment::RightSide => vec![s, r],
        Assignment::MidSide => vec![m, s],
    };
    (assignment, subframes)
}

/// The cheapest of the four ways to code a pair, given a cost for each of left, right, side and mid.
pub(crate) fn stereo_options(left: usize, right: usize, side: usize, mid: usize) -> (Assignment, usize) {
    [
        (Assignment::Independent, left + right),
        (Assignment::LeftSide, left + side),
        (Assignment::RightSide, side + right),
        (Assignment::MidSide, mid + side),
    ]
    .into_iter()
    .min_by_key(|&(_, b)| b)
    .unwrap()
}

pub(crate) fn put_utf8_number(w: &mut BitWriter, value: u64) {
    if value < 0x80 {
        w.put(value as u32, 8);
        return;
    }
    let continuation_bytes = if value < 0x800 {
        1
    } else if value < 0x1_0000 {
        2
    } else if value < 0x20_0000 {
        3
    } else if value < 0x400_0000 {
        4
    } else if value < 0x8000_0000 {
        5
    } else {
        6
    };
    let lead_mark = (0xFF00u32 >> (continuation_bytes + 1)) as u8; // 110xxxxx, 1110xxxx ...
    let lead_bits = value >> (6 * continuation_bytes);
    w.put((lead_mark as u64 | lead_bits) as u32, 8);
    for i in (0..continuation_bytes).rev() {
        w.put((0x80 | ((value >> (6 * i)) & 0x3F)) as u32, 8);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_frame_numbers() {
        let mut w = BitWriter::default();
        put_utf8_number(&mut w, 0x7F);
        put_utf8_number(&mut w, 0x80);
        put_utf8_number(&mut w, 0x7FF);
        put_utf8_number(&mut w, 0x800);
        assert_eq!(w.bytes(), vec![0x7F, 0xC2, 0x80, 0xDF, 0xBF, 0xE0, 0xA0, 0x80]);
    }
}
