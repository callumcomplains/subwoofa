//! MXF sound, as used in digital cinema packages.

use crate::audiofile::bytes::{be16, be32};
use crate::audiofile::Facts;
use crate::{Collector, Status, Window};

/// Key prefix of a partition pack (the byte after it says header, body or footer) and of a generic-container
/// essence element.
pub(crate) const PARTITION_PREFIX: [u8; 13] = [
    0x06, 0x0e, 0x2b, 0x34, 0x02, 0x05, 0x01, 0x01, 0x0d, 0x01, 0x02, 0x01, 0x01,
];

pub(crate) const ESSENCE_PREFIX: [u8; 12] = [0x06, 0x0e, 0x2b, 0x34, 0x01, 0x02, 0x01, 0x01, 0x0d, 0x01, 0x03, 0x01];

pub(crate) const DESCRIPTOR_KEY_TAIL: [u8; 4] = [0x01, 0x01, 0x48, 0x00];

/// Walks the KLV triplets: partition packs must sit where they say they do, every essence frame must be the same
/// size, and the descriptor's duration must match the number of frames.
/// What the walk through the KLV triplets found.
#[derive(Default)]
struct Klv {
    /// Where the walk ended in a file of `file_len` bytes, and whether a triplet was too damaged to follow.
    end: u64,
    file_len: u64,
    broken: bool,
    partitions: u32,
    misplaced: u32,
    footer_at: Option<u64>,
    /// Essence frames and the different sizes they have.
    frames: u64,
    sizes: std::collections::BTreeSet<u64>,
    descriptor: Vec<u8>,
}

pub(crate) fn mxf(w: &mut Window, c: &mut Collector) -> Facts {
    let klv = walk_klv(w);
    report_structure(c, &klv);
    report_descriptor(c, &klv)
}

/// Walks the KLV triplets: partition packs must sit where they say they do, every essence frame is counted, and the
/// wave audio descriptor is kept.
fn walk_klv(w: &mut Window) -> Klv {
    let len = w.len;
    let mut k = Klv {
        file_len: len,
        ..Default::default()
    };
    let mut at = 0u64;
    while at + 17 <= len {
        let head = w.get(at, 24).to_vec();
        let (value_len, header) = match head.get(16) {
            Some(&b) if b < 0x80 => (b as u64, 17u64),
            Some(&b) if (0x81..=0x88).contains(&b) && head.len() >= 17 + (b & 0x7F) as usize => {
                let n = (b & 0x7F) as usize;
                (
                    head[17..17 + n].iter().fold(0u64, |acc, &x| acc << 8 | x as u64),
                    17 + n as u64,
                )
            }
            _ => {
                k.broken = true;
                break;
            }
        };
        if head.starts_with(&PARTITION_PREFIX) && matches!(head[13], 2..=4) {
            k.partitions += 1;
            let pack = w.get(at + header, 40).to_vec();
            if pack.len() >= 24 && u64::from_be_bytes(pack[8..16].try_into().unwrap()) != at {
                k.misplaced += 1;
            }
            if head[13] == 0x04 {
                k.footer_at = Some(at);
            }
        } else if head.starts_with(&ESSENCE_PREFIX) {
            k.frames += 1;
            k.sizes.insert(value_len);
        } else if head[8..12] == [0x0d, 0x01, 0x01, 0x01] && head[12..16] == DESCRIPTOR_KEY_TAIL {
            k.descriptor = w.get(at + header, value_len as usize).to_vec();
        }
        at += header + value_len;
    }
    k.end = at;
    k
}

fn report_structure(c: &mut Collector, k: &Klv) {
    c.check(
        "KLV structure",
        if k.broken {
            Status::Fail
        } else if k.end == k.file_len {
            Status::Pass
        } else {
            Status::Warn
        },
        format!("{} partitions, {} essence frames", k.partitions, k.frames),
    );
    c.check(
        "Partition positions",
        if k.misplaced == 0 && k.partitions >= 2 {
            Status::Pass
        } else {
            Status::Fail
        },
        "each partition pack says where it starts",
    );
    c.check(
        "Frame sizes",
        if k.sizes.len() == 1 { Status::Pass } else { Status::Fail },
        format!("{} different sizes (a sound track file has one)", k.sizes.len()),
    );
    c.check(
        "Footer",
        if k.footer_at.is_some() {
            Status::Pass
        } else {
            Status::Fail
        },
        "a footer partition closes the file",
    );
}

/// The wave audio descriptor's properties, which is where the format of the audio is written.
fn descriptor_properties(descriptor: &[u8]) -> std::collections::HashMap<u16, Vec<u8>> {
    let mut tags = std::collections::HashMap::new();
    let mut o = 0;
    while o + 4 <= descriptor.len() {
        let (tag, l) = (be16(&descriptor[o..]), be16(&descriptor[o + 2..]) as usize);
        if o + 4 + l > descriptor.len() {
            break;
        }
        tags.insert(tag, descriptor[o + 4..o + 4 + l].to_vec());
        o += 4 + l;
    }
    tags
}

fn report_descriptor(c: &mut Collector, k: &Klv) -> Facts {
    let tags = descriptor_properties(&k.descriptor);
    let number = |tag: u16| {
        tags.get(&tag)
            .map(|v| v.iter().fold(0u64, |acc, &x| acc << 8 | x as u64))
            .unwrap_or(0)
    };
    let rate = tags.get(&0x3d03).map(|v| be32(v)).unwrap_or(0);
    let duration = number(0x3002);
    c.check(
        "Duration",
        if duration == k.frames {
            Status::Pass
        } else {
            Status::Fail
        },
        format!("the descriptor says {duration} frames, the file holds {}", k.frames),
    );
    let edit = tags.get(&0x3001).map(|v| (be32(v), be32(&v[4..]))).unwrap_or((0, 1));
    let seconds = if edit.0 > 0 {
        k.frames as f64 * edit.1 as f64 / edit.0 as f64
    } else {
        0.0
    };
    Facts {
        rate,
        channels: number(0x3d07) as u32,
        bits: number(0x3d01) as u32,
        frames: Some((seconds * rate as f64).round() as u64),
        lossless: true,
    }
}
