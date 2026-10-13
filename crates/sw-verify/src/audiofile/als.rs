//! Raw MPEG-4 ALS.

use crate::audiofile::bytes::{be16, be32};
use crate::audiofile::Facts;
use crate::{Collector, Status, Window};

/// A raw `.als` file starts with its configuration record. The frames have no index, so only the record is read.
pub(crate) fn als(w: &mut Window, c: &mut Collector) -> Facts {
    let h = w.get(0, 22).to_vec();
    if h.len() < 22 {
        c.check(
            "Configuration",
            Status::Fail,
            "the file is shorter than the ALS configuration record",
        );
        return Facts::default();
    }
    let (rate, samples, channels) = (be32(&h[4..8]), be32(&h[8..12]) as u64, be16(&h[12..14]) as u32 + 1);
    let bits = (((h[14] >> 2) & 7) as u32 + 1) * 8;
    let frame_length = be16(&h[15..17]) as u32 + 1;
    c.kv("Frame length", format!("{frame_length} samples"));
    c.check(
        "Configuration",
        if rate > 0 && samples > 0 {
            Status::Pass
        } else {
            Status::Fail
        },
        "the record names a rate and a length",
    );
    c.check(
        "Frames",
        Status::Info,
        "frame sizes are not indexed in a raw file; open it as MP4 for a full decode",
    );
    Facts {
        rate,
        channels,
        bits,
        frames: Some(samples),
        lossless: true,
    }
}
