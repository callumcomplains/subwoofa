//! True Audio.

use crate::audiofile::bytes::{crc32_ieee, le16, le32, size_check};
use crate::audiofile::Facts;
use crate::{Collector, Status, Window};

pub(crate) fn tta(w: &mut Window, c: &mut Collector) -> Facts {
    let len = w.len;
    let h = w.get(0, 22).to_vec();
    if h.len() < 22 {
        c.check("TTA header", Status::Fail, "the file is shorter than the header");
        return Facts::default();
    }
    let channels = le16(&h[6..8]) as u32;
    let bits = le16(&h[8..10]) as u32;
    let rate = le32(&h[10..14]);
    let samples = le32(&h[14..18]) as u64;
    c.check(
        "Header checksum",
        if crc32_ieee(&h[..18]) == le32(&h[18..22]) {
            Status::Pass
        } else {
            Status::Fail
        },
        "CRC-32 of the first 18 bytes",
    );
    let frame_samples = 256 * rate as u64 / 245;
    let frames = if frame_samples > 0 {
        samples.div_ceil(frame_samples)
    } else {
        0
    };
    let table = w.get(22, (frames * 4 + 4) as usize).to_vec();
    if table.len() as u64 == frames * 4 + 4 {
        let (sizes, crc) = table.split_at(frames as usize * 4);
        c.check(
            "Seek table checksum",
            if crc32_ieee(sizes) == le32(crc) {
                Status::Pass
            } else {
                Status::Fail
            },
            format!("{frames} frames"),
        );
        let audio: u64 = sizes.chunks_exact(4).map(|s| le32(s) as u64).sum();
        size_check(c, "Frame sizes add up", 22 + frames * 4 + 4 + audio, len);
    } else {
        c.check("Seek table", Status::Fail, "the file ends inside the seek table");
    }
    Facts {
        rate,
        channels,
        bits,
        frames: Some(samples),
        lossless: true,
        no_decoder: false,
    }
}
