//! Monkey's Audio.

use crate::audiofile::bytes::{le16, le32, size_check};
use crate::audiofile::Facts;
use crate::{Collector, Status, Window};

/// The descriptor, header and seek table must agree with each other and with the size of the file.
pub(crate) fn monkeys(w: &mut Window, c: &mut Collector) -> Facts {
    let len = w.len;
    let h = w.get(0, 76).to_vec();
    if h.len() < 76 {
        c.check(
            "Descriptor",
            Status::Fail,
            "the file is shorter than the descriptor and header",
        );
        return Facts::default();
    }
    let version = le16(&h[4..6]);
    c.kv("Version", format!("{}.{:02}", version / 1000, version % 1000 / 10));
    if version < 3980 {
        c.check(
            "Version",
            Status::Warn,
            "older than 3.98: only the header fields are read",
        );
        return Facts::default();
    }
    let (descriptor, header, seek) = (le32(&h[8..12]) as u64, le32(&h[12..16]) as u64, le32(&h[16..20]) as u64);
    let (wav_head, audio, wav_tail) = (
        le32(&h[20..24]) as u64,
        le32(&h[24..28]) as u64 | (le32(&h[28..32]) as u64) << 32,
        le32(&h[32..36]) as u64,
    );
    let (level, blocks_per_frame, final_blocks, frames) = (
        le16(&h[52..54]),
        le32(&h[56..60]) as u64,
        le32(&h[60..64]) as u64,
        le32(&h[64..68]) as u64,
    );
    let (bits, channels, rate) = (le16(&h[68..70]) as u32, le16(&h[70..72]) as u32, le32(&h[72..76]));
    c.kv("Compression level", level.to_string());
    c.check(
        "Seek table size",
        if seek >= frames * 4 { Status::Pass } else { Status::Fail },
        format!("{frames} frames"),
    );
    size_check(
        c,
        "Audio data size",
        descriptor + header + seek + wav_head + audio + wav_tail,
        len,
    );
    let samples = if frames > 0 {
        blocks_per_frame * (frames - 1) + final_blocks
    } else {
        0
    };
    Facts {
        rate,
        channels,
        bits,
        frames: Some(samples),
        lossless: true,
        no_decoder: false,
    }
}
