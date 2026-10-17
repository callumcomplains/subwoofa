//! DSD: DSF and DSDIFF.

use crate::audiofile::bytes::{be16, be32, be64, le32, le64, size_check};
use crate::audiofile::Facts;
use crate::{Collector, Status, Window};

pub(crate) fn dsf(w: &mut Window, c: &mut Collector) -> Facts {
    let len = w.len;
    let h = w.get(0, 92).to_vec();
    if h.len() < 92 || &h[28..32] != b"fmt " || &h[80..84] != b"data" {
        c.check(
            "DSF chunks",
            Status::Fail,
            "the DSD, fmt and data chunks are not where a DSF file has them",
        );
        return Facts::default();
    }
    size_check(c, "File size", le64(&h[12..20]), len);
    let channels = le32(&h[52..56]);
    let rate = le32(&h[56..60]);
    let bit_order = le32(&h[60..64]);
    let samples = le64(&h[64..72]);
    let block = le32(&h[72..76]) as u64;
    c.kv(
        "Bit order",
        if bit_order == 1 {
            "least significant bit first"
        } else {
            "most significant bit first"
        },
    );
    c.kv("DSD rate", format!("DSD{} ({} Hz)", rate / 44_100, rate));
    c.kv("Channel type", le32(&h[48..52]).to_string());
    let data = le64(&h[84..92]).saturating_sub(12);
    let want = (samples.div_ceil(8 * block.max(1)) * block.max(1)) * channels as u64;
    c.check(
        "Audio data size",
        if data == want { Status::Pass } else { Status::Fail },
        format!("{data} bytes of audio, {want} expected for {samples} samples in {channels} channels"),
    );
    c.check(
        "Block size",
        if block == 4096 { Status::Pass } else { Status::Warn },
        format!("{block} bytes per channel in each block"),
    );
    Facts {
        rate,
        channels,
        bits: 1,
        frames: Some(samples),
        lossless: false,
        no_decoder: false,
    }
}

pub(crate) fn dff(w: &mut Window, c: &mut Collector) -> Facts {
    let len = w.len;
    let h = w.get(0, 16).to_vec();
    size_check(c, "FRM8 size", be64(&h[4..12]) + 12, len);
    let mut facts = Facts::default();
    let mut pos = 16u64;
    let mut data_bytes = 0u64;
    while pos + 12 <= len {
        let ch = w.get(pos, 12).to_vec();
        if ch.len() < 12 {
            break;
        }
        let size = be64(&ch[4..12]);
        let body = pos + 12;
        match &ch[..4] {
            b"PROP" => {
                let end = body + size;
                let mut p = body + 4;
                while p + 12 <= end {
                    let sub = w.get(p, 12).to_vec();
                    let sub_size = be64(&sub[4..12]);
                    match &sub[..4] {
                        b"FS  " => facts.rate = be32(w.get(p + 12, 4)),
                        b"CHNL" => facts.channels = be16(w.get(p + 12, 2)) as u32,
                        _ => {}
                    }
                    p += 12 + sub_size + sub_size % 2;
                }
            }
            b"DSD " => data_bytes = size,
            _ => {}
        }
        pos = body + size + size % 2;
    }
    if facts.rate > 0 && facts.channels > 0 {
        facts.frames = Some(data_bytes / facts.channels as u64 * 8);
        c.kv("DSD rate", format!("DSD{} ({} Hz)", facts.rate / 44_100, facts.rate));
    }
    c.check(
        "Audio data",
        if data_bytes > 0 && facts.channels > 0 && data_bytes.is_multiple_of(facts.channels as u64) {
            Status::Pass
        } else {
            Status::Fail
        },
        format!("{data_bytes} bytes for {} channels", facts.channels),
    );
    facts.bits = 1;
    facts
}
