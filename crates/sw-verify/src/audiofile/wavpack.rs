//! WavPack.

use crate::audiofile::bytes::le32;
use crate::audiofile::Facts;
use crate::{Collector, Status, Window};

/// Walks the blocks: each starts with `wvpk`, names its own size, and the sample counts of the blocks that open a
/// stretch of time must add up to the total in the first header.
pub(crate) fn wavpack(w: &mut Window, c: &mut Collector) -> Facts {
    const HEADER: usize = 32;
    const RATES: [u32; 15] = [
        6000, 8000, 9600, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000, 64000, 88200, 96000, 192000,
    ];
    let len = w.len;
    let mut at = 0u64;
    let (mut blocks, mut frames, mut total, mut channels, mut counting) = (0u64, 0u64, 0u64, 0u32, true);
    let (mut rate, mut bits) = (0u32, 0u32);
    while at + HEADER as u64 <= len {
        let h = w.get(at, HEADER).to_vec();
        if h.len() < HEADER || &h[..4] != b"wvpk" {
            c.check("Block header", Status::Fail, format!("no \"wvpk\" at byte {at}"));
            return Facts::default();
        }
        let flags = le32(&h[24..28]);
        if blocks == 0 {
            total = le32(&h[12..16]) as u64 | (h[11] as u64) << 32;
            bits = ((flags & 3) + 1) * 8;
            rate = RATES.get(((flags >> 23) & 0xF) as usize).copied().unwrap_or(0);
        }
        if flags & 0x800 != 0 {
            frames += le32(&h[20..24]) as u64;
            if blocks > 0 && channels > 0 {
                counting = false;
            }
        }
        if counting {
            channels += if flags & 0x4 != 0 { 1 } else { 2 };
        }
        blocks += 1;
        at += 8 + le32(&h[4..8]) as u64;
    }
    c.check(
        "Blocks fill the file",
        if at == len { Status::Pass } else { Status::Fail },
        format!("{blocks} blocks"),
    );
    c.check(
        "Sample count",
        if total == 0 || total == frames {
            Status::Pass
        } else {
            Status::Fail
        },
        format!("the header says {total}, the blocks hold {frames}"),
    );
    Facts {
        rate,
        channels,
        bits,
        frames: Some(frames),
        lossless: true,
        no_decoder: false,
    }
}
