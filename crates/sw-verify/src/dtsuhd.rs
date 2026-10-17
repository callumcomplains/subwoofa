//! DTS-UHD (ETSI TS 103 491, the format of DTS:X Profile 2): recognised and described, not decoded, as nothing here
//! can decode it (FFmpeg has no decoder). Every frame starts with a sync word (clause 6.4.4.1): 0x40411BF2 for a sync
//! frame, where a decoder may start, 0x71C442E8 for the frames between. A sync frame's table of contents (FTOC)
//! states its size and the stream parameters (clause 6.4.5, Table 6-12) and ends in a CRC-16 (clause 6.4.18), which
//! proves a sync word real. Frames are found by their sync words, as the frame size is only known after the
//! metadata, which this check does not unpack.
//!
//! Order: `verify`, the scan (`find_frames`), the FTOC fields (`read_ftoc`, `var_len`), the reports.

use crate::bits::Bits;
use crate::{crc, Collector, Probe, Status, Window};

const SYNC_FRAME: u32 = 0x4041_1BF2;
const OTHER_FRAME: u32 = 0x71C4_42E8;
/// Table 6-13: the base duration of a frame, in clock periods (index 3 is reserved).
const BASE_DURATION: [u32; 4] = [512, 480, 384, 0];
/// Table 6-14: the clock rate (code 3 is unused).
const CLOCK_HZ: [u32; 4] = [32_000, 44_100, 48_000, 0];
/// Bytes scanned at a time for sync words.
const CHUNK: usize = 1 << 20;

/// What a sync frame's table of contents says (clause 6.4.6).
#[derive(Clone, Copy, PartialEq)]
struct Params {
    /// A full channel-based mix (stereo, 5.1 ... without objects), which simplifies the rest of the stream.
    channel_based: bool,
    version: (u32, u32),
    /// Samples per frame and the sampling rate.
    samples: u32,
    rate: u32,
}

/// Where the frames start, and what the sync frames said.
#[derive(Default)]
struct Scan {
    starts: Vec<u64>,
    sync_frames: u64,
    /// Sync-word patterns whose table of contents did not check out (chance matches inside the data, or damage).
    rejected: u64,
    first: Option<Params>,
    changes: u64,
}

pub(crate) fn verify(w: &mut Window, c: &mut Collector) -> Probe {
    let scan = find_frames(w);
    let mut p = Probe {
        no_decoder: true,
        ..Default::default()
    };
    let Some(params) = scan.first else {
        c.check(
            "Sync frames",
            Status::Fail,
            "no sync frame with a valid table of contents was found",
        );
        return p;
    };
    let frames = scan.starts.len() as u64;
    p.sample_rate = params.rate;
    p.duration_secs = frames as f64 * params.samples as f64 / params.rate as f64;
    for (i, &start) in scan.starts.iter().enumerate() {
        let end = scan.starts.get(i + 1).copied().unwrap_or(w.len);
        p.frame_bytes.push((end - start) as u32);
        p.frame_samples.push(params.samples);
    }
    report_facts(c, &scan, &params, &p, w.len);
    report_checks(c, &scan, w);
    p
}

/// Every frame start in the file: sync words whose table of contents checks out, and the other frames' sync words
/// (which have no checksum of their own in a channel-based stream).
fn find_frames(w: &mut Window) -> Scan {
    let mut s = Scan::default();
    let mut pos = 0u64;
    while pos + 4 <= w.len {
        let chunk = w.get(pos, CHUNK + 3).to_vec();
        let last = chunk.len().saturating_sub(3);
        for i in 0..last.min(CHUNK) {
            let word = u32::from_be_bytes([chunk[i], chunk[i + 1], chunk[i + 2], chunk[i + 3]]);
            let at = pos + i as u64;
            if word == SYNC_FRAME {
                match read_ftoc(w, at) {
                    Some(params) => {
                        if s.first.is_some_and(|f| f != params) {
                            s.changes += 1;
                        }
                        s.first.get_or_insert(params);
                        s.sync_frames += 1;
                        s.starts.push(at);
                    }
                    None => s.rejected += 1,
                }
            } else if word == OTHER_FRAME && s.first.is_some() {
                s.starts.push(at);
            }
        }
        if chunk.len() <= CHUNK {
            break;
        }
        pos += CHUNK as u64;
    }
    s
}

/// The stream parameters of the sync frame at `at`, when its table of contents passes its CRC (Table 6-11).
fn read_ftoc(w: &mut Window, at: u64) -> Option<Params> {
    let head = w.get(at + 4, 8).to_vec();
    let mut b = Bits::new(&head);
    // the FTOC holds 1 to 5 408 bytes from the sync word through its CRC
    let size = var_len(&mut b, [5, 8, 10, 12]) as usize + 1;
    if size < 8 {
        return None;
    }
    let ftoc = w.get(at, size).to_vec();
    if ftoc.len() < size || crc::dts(&ftoc) != 0 {
        return None;
    }
    let mut b = Bits::new(&ftoc[4..]);
    var_len(&mut b, [5, 8, 10, 12]);
    let channel_based = b.flag();
    let version = if channel_based {
        (2, 0)
    } else {
        // 6.4.6.3: a short (6-bit) or long (12-bit) code, half major (from 2), half minor
        let bits = if b.flag() { 6 } else { 12 };
        let code = b.get(bits);
        ((code >> (bits / 2)) + 2, code & ((1 << (bits / 2)) - 1))
    };
    let base = BASE_DURATION[b.get(2) as usize];
    let samples = base * (b.get(3) + 1);
    let clock = CLOCK_HZ[b.get(2) as usize];
    (base > 0 && clock > 0).then_some(Params {
        channel_based,
        version,
        // the sampling rate is the clock rate times 1, 2, 4 or 8 (6.4.6.8), coded after the time code, which this
        // check does not unpack: the clock rate stands in for it, and the report says so
        samples,
        rate: clock,
    })
}

/// A field whose length is chosen by a prefix code 0, 10, 110 or 111 (Table 5-2); the value counts on from the
/// largest value of the shorter lengths.
fn var_len(b: &mut Bits, lengths: [usize; 4]) -> u32 {
    let index = if !b.flag() {
        0
    } else if !b.flag() {
        1
    } else if !b.flag() {
        2
    } else {
        3
    };
    let offset: u32 = lengths[..index].iter().map(|&n| 1u32 << n).sum();
    offset + b.get(lengths[index])
}

fn report_facts(c: &mut Collector, s: &Scan, params: &Params, p: &Probe, file_len: u64) {
    c.kv("Format", "DTS-UHD (ETSI TS 103 491; DTS:X Profile 2)");
    c.kv(
        "Content",
        if params.channel_based {
            "a channel-based mix (no objects)"
        } else {
            "objects and/or several presentations"
        },
    );
    c.kv("Stream version", format!("{}.{}", params.version.0, params.version.1));
    c.kv(
        "Sample rate",
        format!(
            "{} Hz (the clock rate; a stream at 2, 4 or 8 times it says so in a field this check does not read)",
            params.rate
        ),
    );
    c.kv("Frame length", format!("{} samples", params.samples));
    let frames = s.starts.len() as u64;
    c.kv(
        "Frames",
        format!("{frames} ({} sync frames, where playback can start)", s.sync_frames),
    );
    c.kv("Duration", format!("{:.3} s", p.duration_secs));
    if p.duration_secs > 0.0 {
        c.kv(
            "Average bit rate",
            format!("{:.1} kbps", file_len as f64 * 8.0 / p.duration_secs / 1000.0),
        );
        c.kv(
            "Sync frame every",
            format!("{:.2} s", p.duration_secs / s.sync_frames.max(1) as f64),
        );
    }
}

fn report_checks(c: &mut Collector, s: &Scan, w: &Window) {
    c.check(
        "Recognised stream",
        Status::Pass,
        "DTS-UHD frames, each starting with a sync word",
    );
    c.check(
        "Starts on a sync frame",
        if s.starts.first() == Some(&0) {
            Status::Pass
        } else {
            Status::Warn
        },
        if s.starts.first() == Some(&0) {
            "the file begins where a decoder can start".to_string()
        } else {
            format!(
                "the first sync frame is at byte {}",
                s.starts.first().copied().unwrap_or(w.len)
            )
        },
    );
    c.check(
        "Sync frame checksums",
        Status::Pass,
        format!("{} sync frames with a valid table of contents (CRC-16)", s.sync_frames),
    );
    if s.rejected > 0 {
        c.check(
            "Unconfirmed sync words",
            Status::Info,
            format!(
                "{} sync-word patterns did not check out (usually chance matches inside the audio data)",
                s.rejected
            ),
        );
    }
    c.check(
        "Stable parameters",
        if s.changes == 0 { Status::Pass } else { Status::Warn },
        if s.changes == 0 {
            "every sync frame states the same rate, frame length and content type".to_string()
        } else {
            format!("the stream parameters change {} time(s)", s.changes)
        },
    );
    c.check(
        "Audio",
        Status::Skipped,
        "no decoder for DTS-UHD is available (FFmpeg has none), so the audio itself is not checked",
    );
    if s.first.is_some_and(|p| p.channel_based) {
        c.check(
            "Frames between sync frames",
            Status::Info,
            "in a channel-based stream they carry no checksum, and their sizes are only known from metadata this \
             check does not unpack, so damage or a cut inside them cannot be seen here",
        );
    }
}
