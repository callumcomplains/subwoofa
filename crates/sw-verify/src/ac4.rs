//! Dolby AC-4 (ETSI TS 103 190): recognised and its framing checked, not decoded, as nothing here can decode it
//! (FFmpeg has no decoder). A raw stream is a run of sync frames (TS 103 190-2 Annex G): the sync word 0xAC40, or
//! 0xAC41 when a CRC-16 follows the frame; the frame size (16 bits, or 24 when the 16 read 0xFFFF); the frame; then
//! the CRC. The CRC is the AC-3 one, over everything after the sync word. Each frame opens with a table of contents
//! (`ac4_toc`, TS 103 190-2 clause 6.2.1) giving the bitstream version, sampling rate, frame rate and whether the
//! frame is an I-frame (one a decoder can start on).
//!
//! Order: `verify`, the frame walk (`walk`, `read_toc`), the reports.

use crate::bits::Bits;
use crate::{crc, Collector, Probe, Status, Window};

const SYNC: u16 = 0xAC40;
const SYNC_CRC: u16 = 0xAC41;
/// `frame_rate_index` (TS 103 190-2 Table 83): frames per second at 48 kHz. Index 13 is a frame of 2 048 samples,
/// whatever the sampling rate.
const FRAME_RATES: [f64; 13] = [
    23.976, 24.0, 25.0, 29.97, 30.0, 47.95, 48.0, 50.0, 59.94, 60.0, 100.0, 119.88, 120.0,
];
const LONG_FRAME: u32 = 13;

/// What a frame's table of contents says.
#[derive(Clone, Copy)]
struct Toc {
    version: u32,
    rate: u32,
    frame_rate: u32,
    presentations: u32,
}

impl Toc {
    /// Frames per second.
    fn fps(&self) -> f64 {
        match self.frame_rate {
            LONG_FRAME => self.rate as f64 / 2048.0,
            i => FRAME_RATES.get(i as usize).copied().unwrap_or(0.0),
        }
    }
}

/// What the walk through the frames found.
#[derive(Default)]
struct Tally {
    frames: u64,
    iframes: u64,
    crc_frames: u64,
    bad_crc: u64,
    lost_sync: bool,
    truncated: bool,
    changes: u64,
    first: Option<Toc>,
}

pub(crate) fn verify(w: &mut Window, c: &mut Collector) -> Probe {
    let mut p = Probe {
        no_decoder: true,
        ..Default::default()
    };
    let t = walk(w, c, &mut p);
    let Some(toc) = t.first else {
        c.check("Frames", Status::Fail, "no AC-4 frame could be read");
        return p;
    };
    p.sample_rate = toc.rate;
    if toc.fps() > 0.0 {
        p.duration_secs = t.frames as f64 / toc.fps();
        let samples = (toc.rate as f64 / toc.fps()).round() as u32;
        p.frame_samples = vec![samples; p.frame_bytes.len()];
    }
    report_facts(c, &t, &toc, &p, w.len);
    report_checks(c, &t);
    p
}

/// Walks the sync frames from the start of the file.
fn walk(w: &mut Window, c: &mut Collector, p: &mut Probe) -> Tally {
    let mut t = Tally::default();
    let mut pos = 0u64;
    while pos + 4 <= w.len {
        let head = w.get(pos, 7).to_vec();
        let sync = u16::from_be_bytes([head[0], head[1]]);
        if sync != SYNC && sync != SYNC_CRC {
            t.lost_sync = true;
            c.finding(Status::Fail, Some(pos), "no AC-4 sync word where a frame should start");
            break;
        }
        let (size, header) = match u16::from_be_bytes([head[2], head[3]]) {
            0xFFFF if head.len() >= 7 => (u32::from_be_bytes([0, head[4], head[5], head[6]]) as u64, 7),
            n => (n as u64, 4),
        };
        let check = if sync == SYNC_CRC { 2 } else { 0 };
        let whole = header + size + check;
        if pos + whole > w.len {
            t.truncated = true;
            c.finding(Status::Fail, Some(pos), "the last frame is cut short");
            break;
        }
        let frame = w.get(pos, whole as usize).to_vec();
        if check > 0 {
            t.crc_frames += 1;
            // the CRC covers the size field and the frame; with the CRC word itself it leaves zero
            if crc::ac3(&frame[2..]) != 0 {
                t.bad_crc += 1;
                c.finding(Status::Fail, Some(pos), "frame checksum (CRC) mismatch");
            }
        }
        let (toc, iframe) = read_toc(&frame[header as usize..]);
        // the presentation fields vary between frames of some streams (IMS), so only the stream-wide ones count
        if t.first
            .is_some_and(|f| (f.version, f.rate, f.frame_rate) != (toc.version, toc.rate, toc.frame_rate))
        {
            t.changes += 1;
        }
        t.first.get_or_insert(toc);
        t.iframes += u64::from(iframe);
        t.frames += 1;
        p.frame_bytes.push(whole as u32);
        pos += whole;
    }
    t
}

/// The start of `ac4_toc()`: the stream's parameters, and whether this is an I-frame.
fn read_toc(frame: &[u8]) -> (Toc, bool) {
    let mut b = Bits::new(frame);
    let mut version = b.get(2);
    if version == 3 {
        version += variable_bits(&mut b, 2);
    }
    b.skip(10); // sequence_counter
    if b.flag() {
        // b_wait_frames
        if b.get(3) > 0 {
            b.skip(2); // br_code
        }
    }
    let rate = if b.flag() { 48_000 } else { 44_100 };
    let frame_rate = b.get(4);
    let iframe = b.flag();
    let presentations = if b.flag() {
        1
    } else if b.flag() {
        variable_bits(&mut b, 2) + 2
    } else {
        0
    };
    (
        Toc {
            version,
            rate,
            frame_rate,
            presentations,
        },
        iframe,
    )
}

/// `variable_bits(n)` (TS 103 190-2 clause 4.3): a value sent in groups of `n` bits, each followed by a flag that
/// says another group follows.
fn variable_bits(b: &mut Bits, n: usize) -> u32 {
    let mut value = 0u32;
    loop {
        value += b.get(n);
        if !b.flag() || b.over {
            return value;
        }
        value = (value + 1) << n;
    }
}

fn report_facts(c: &mut Collector, t: &Tally, toc: &Toc, p: &Probe, file_len: u64) {
    c.kv("Format", "Dolby AC-4 (ETSI TS 103 190)");
    c.kv("Bitstream version", toc.version.to_string());
    c.kv("Sample rate", format!("{} Hz", toc.rate));
    c.kv(
        "Frame rate",
        if toc.frame_rate == LONG_FRAME {
            format!("{:.4} frames/s (2048 samples a frame)", toc.fps())
        } else if toc.fps() > 0.0 {
            format!("{} frames/s", toc.fps())
        } else {
            format!("reserved code {}", toc.frame_rate)
        },
    );
    c.kv("Presentations (first frame)", toc.presentations.to_string());
    c.kv(
        "Frames",
        format!("{} ({} I-frames, where playback can start)", t.frames, t.iframes),
    );
    c.kv("Duration", format!("{:.3} s", p.duration_secs));
    if p.duration_secs > 0.0 {
        c.kv(
            "Average bit rate",
            format!("{:.1} kbps", file_len as f64 * 8.0 / p.duration_secs / 1000.0),
        );
    }
}

fn report_checks(c: &mut Collector, t: &Tally) {
    let ok = |good: bool| if good { Status::Pass } else { Status::Fail };
    c.check(
        "Sync words",
        ok(!t.lost_sync),
        if t.lost_sync {
            "a frame does not start where the previous one ends".to_string()
        } else {
            format!("all {} frames start where the previous one ends", t.frames)
        },
    );
    if t.crc_frames > 0 {
        c.check(
            "CRC",
            ok(t.bad_crc == 0),
            if t.bad_crc == 0 {
                format!("{} of {} frames", t.crc_frames, t.crc_frames)
            } else {
                format!("{} of {} frames fail", t.bad_crc, t.crc_frames)
            },
        );
    } else {
        c.check("CRC", Status::Info, "the frames carry no CRC (sync word 0xAC40)");
    }
    c.check(
        "Complete file",
        ok(!t.truncated),
        if t.truncated {
            "the last frame is cut short"
        } else {
            "the file ends on a frame boundary"
        },
    );
    c.check(
        "Stable parameters",
        if t.changes == 0 { Status::Pass } else { Status::Warn },
        if t.changes == 0 {
            "version, sample rate and frame rate never change".to_string()
        } else {
            format!("the parameters change {} time(s)", t.changes)
        },
    );
    c.check(
        "Audio",
        Status::Skipped,
        "no AC-4 decoder is available (FFmpeg has none), so the audio itself is not checked",
    );
}
