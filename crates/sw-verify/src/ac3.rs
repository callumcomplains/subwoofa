//! AC-3 and E-AC-3 (A/52 and its Annex E): sync, sizes, checksums, header fields, continuity.

use crate::bits::Bits;
use crate::{crc, Collector, Probe, Status, Window};

const RATES: [u32; 3] = [48_000, 44_100, 32_000];
const KBPS: [u32; 19] = [
    32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 576, 640,
];
/// Frame size in 16-bit words per `frmsizecod`, for 48, 44.1 and 32 kHz.
const WORDS: [[u16; 38]; 3] = [
    [
        64, 64, 80, 80, 96, 96, 112, 112, 128, 128, 160, 160, 192, 192, 224, 224, 256, 256, 320, 320, 384, 384, 448,
        448, 512, 512, 640, 640, 768, 768, 896, 896, 1024, 1024, 1152, 1152, 1280, 1280,
    ],
    [
        69, 70, 87, 88, 104, 105, 121, 122, 139, 140, 174, 175, 208, 209, 243, 244, 278, 279, 348, 349, 417, 418, 487,
        488, 557, 558, 696, 697, 835, 836, 975, 976, 1114, 1115, 1253, 1254, 1393, 1394,
    ],
    [
        96, 96, 120, 120, 144, 144, 168, 168, 192, 192, 240, 240, 288, 288, 336, 336, 384, 384, 480, 480, 576, 576,
        672, 672, 768, 768, 960, 960, 1152, 1152, 1344, 1344, 1536, 1536, 1728, 1728, 1920, 1920,
    ],
];
const ACMOD: [&str; 8] = [
    "1+1 (dual mono)",
    "1/0 (C)",
    "2/0 (L R)",
    "3/0 (L C R)",
    "2/1 (L R S)",
    "3/1 (L C R S)",
    "2/2 (L R Ls Rs)",
    "3/2 (L C R Ls Rs)",
];
const NFCH: [u32; 8] = [2, 1, 2, 3, 3, 4, 4, 5];
const BSMOD: [&str; 8] = [
    "complete main",
    "music and effects",
    "visually impaired",
    "hearing impaired",
    "dialogue",
    "commentary",
    "emergency",
    "voice-over / karaoke",
];

#[derive(Debug, Clone, PartialEq, Default)]
struct Header {
    rate: u32,
    acmod: u8,
    lfe: bool,
    bsid: u32,
    dialnorm: u32,
    kbps: u32,
    blocks: u32,
    strmtyp: u8,
    substream: u8,
}

/// What the walk through the frames found.
#[derive(Default)]
struct Tally {
    frames: u64,
    /// Access units: an independent frame together with the dependent substreams that follow it.
    access_units: u64,
    dependent: u64,
    bad_crc1: u64,
    bad_crc2: u64,
    bad_headers: u64,
    resyncs: u64,
    truncated: u64,
    param_changes: u64,
    size_changes: u64,
    /// Bytes skipped while looking for the next sync word.
    skipped_bytes: u64,
    /// Complexity index of Joint Object Coding (Dolby Atmos), and in how many access units it was signalled.
    joc: Option<u32>,
    joc_frames: u64,
    samples: u64,
    first: Option<Header>,
    first_dump: Vec<(String, String)>,
}

pub(crate) fn verify(w: &mut Window, c: &mut Collector, eac3: bool) -> Probe {
    let mut p = Probe::default();
    let tally = walk_frames(w, c, eac3, &mut p);
    let h0 = tally.first.clone().unwrap_or_default();
    p.sample_rate = h0.rate;
    p.channels = NFCH[h0.acmod as usize & 7] + u32::from(h0.lfe);
    p.duration_secs = if h0.rate > 0 {
        tally.samples as f64 / h0.rate as f64
    } else {
        0.0
    };
    report_facts(c, &p, &tally, &h0, w.len, eac3);
    report_checks(c, &p, &tally, &h0, eac3);
    if c.report.deep {
        c.section("First frame header", tally.first_dump);
    }
    p
}

/// The position of the next sync word after `pos`, looking at most a megabyte ahead.
fn next_sync(w: &mut Window, pos: u64) -> Option<u64> {
    let len = w.len;
    let from = pos + 1;
    let mut q = from;
    while q < len && q < from + (1 << 20) {
        let chunk = w.get(q, 1 << 16).to_vec();
        if chunk.len() < 2 {
            break;
        }
        if let Some(i) = chunk.windows(2).position(|pair| pair == [0x0B, 0x77]) {
            return Some(q + i as u64);
        }
        q += chunk.len() as u64 - 1;
    }
    None
}

/// Walks the frames from the start of the file, checking each one and filling in `p` and the tally.
fn walk_frames(w: &mut Window, c: &mut Collector, eac3: bool, p: &mut Probe) -> Tally {
    let mut t = Tally::default();
    let mut pos = 0u64;
    let len = w.len;
    let mut current: Option<Header> = None;
    while pos + 8 <= len {
        let head = w.get(pos, 16).to_vec();
        if head.len() < 8 || head[0] != 0x0B || head[1] != 0x77 {
            let found = next_sync(w, pos);
            let skipped = found.unwrap_or(len) - pos;
            t.resyncs += 1;
            c.finding(
                Status::Fail,
                Some(pos),
                format!("no sync word here; {skipped} bytes skipped to the next one"),
            );
            t.skipped_bytes += skipped;
            match found {
                Some(f) => {
                    pos = f;
                    continue;
                }
                None => break,
            }
        }
        let (h, fsize, header_ok) = if eac3 { parse_e(&head) } else { parse_a(&head) };
        if !header_ok || fsize == 0 {
            t.bad_headers += 1;
            c.finding(Status::Fail, Some(pos), "invalid frame header".to_string());
            pos += 2;
            continue;
        }
        if pos + fsize as u64 > len {
            c.finding(
                Status::Fail,
                Some(pos),
                format!("the last frame is cut short ({} of {} bytes)", len - pos, fsize),
            );
            t.truncated += 1;
            break;
        }
        let frame = w.get(pos, fsize).to_vec();
        check_crcs(c, &frame, pos, eac3, &mut t);
        if t.first.is_none() {
            t.first_dump = if eac3 { dump_e(&frame) } else { dump_a(&frame) };
            t.first = Some(h.clone());
        }
        if eac3 {
            if let Some(j) = joc_complexity(&frame) {
                t.joc_frames += 1;
                t.joc.get_or_insert(j);
            }
        }
        if eac3 && h.strmtyp == 1 {
            t.dependent += 1;
        } else {
            if current.as_ref().is_some_and(|prev| *prev != h) {
                t.param_changes += 1;
                if t.param_changes <= 5 {
                    c.finding(
                        Status::Warn,
                        Some(pos),
                        "stream parameters change here (rate, channels or bit rate)",
                    );
                }
            }
            current = Some(h.clone());
            t.access_units += 1;
            t.samples += h.blocks as u64 * 256;
            p.frame_samples.push(h.blocks * 256);
        }
        if eac3 && h.strmtyp == 1 && !p.frame_bytes.is_empty() {
            // a dependent substream belongs to the access unit before it: the bit rate chart counts them together
            *p.frame_bytes.last_mut().unwrap() += fsize as u32;
        } else {
            if p.frame_bytes.last().is_some_and(|last| *last != fsize as u32) {
                t.size_changes += 1;
            }
            p.frame_bytes.push(fsize as u32);
        }
        t.frames += 1;
        pos += fsize as u64;
    }
    t
}

/// AC-3 has two checksums (the first covers the first 5/8 of the frame), E-AC-3 one over the whole frame.
fn check_crcs(c: &mut Collector, frame: &[u8], pos: u64, eac3: bool, t: &mut Tally) {
    if !eac3 {
        let size = frame.len();
        let five_eighths = ((size >> 2) + (size >> 4)) << 1;
        if crc::ac3(&frame[2..five_eighths]) != 0 {
            t.bad_crc1 += 1;
            c.finding(
                Status::Fail,
                Some(pos),
                "first-part checksum (CRC1) mismatch".to_string(),
            );
        }
    }
    if crc::ac3(&frame[2..]) != 0 {
        t.bad_crc2 += 1;
        let name = if eac3 { "CRC" } else { "CRC2" };
        c.finding(Status::Fail, Some(pos), format!("frame checksum ({name}) mismatch"));
    }
}

/// The facts block: format, rate, channels, length, bit rate, dialogue level and Atmos signalling.
fn report_facts(c: &mut Collector, p: &Probe, t: &Tally, h0: &Header, file_len: u64, eac3: bool) {
    let family = if eac3 { "E-AC-3" } else { "AC-3" };
    c.kv("Format", format!("{family}, bit stream id {}", h0.bsid));
    c.kv("Sample rate", format!("{} Hz", h0.rate));
    c.kv(
        "Channels",
        format!(
            "{} + {}",
            ACMOD[h0.acmod as usize & 7],
            if h0.lfe { "LFE" } else { "no LFE" }
        ),
    );
    c.kv(
        "Frames",
        if eac3 && t.dependent > 0 {
            format!(
                "{} access units ({} frames, {} in dependent substreams)",
                t.access_units, t.frames, t.dependent
            )
        } else {
            format!("{}", t.frames)
        },
    );
    c.kv("Duration", format!("{:.3} s", p.duration_secs));
    if p.duration_secs > 0.0 {
        c.kv(
            "Average bit rate",
            format!(
                "{:.1} kbps",
                (file_len - t.skipped_bytes) as f64 * 8.0 / p.duration_secs / 1000.0
            ),
        );
    }
    c.kv("Dialogue level", format!("-{} dBFS", h0.dialnorm));
    if !eac3 {
        return;
    }
    let Some(j) = t.joc else {
        c.kv("Dolby Atmos", "no");
        return;
    };
    let (joc_frames, aus) = (t.joc_frames, t.access_units);
    c.kv(
        "Dolby Atmos",
        format!("yes: Joint Object Coding, complexity index {j} (in {joc_frames} of {aus} access units)"),
    );
    let every = joc_frames == aus;
    c.check(
        "Dolby Atmos signalling",
        if every { Status::Pass } else { Status::Warn },
        if every {
            format!("every access unit carries the JOC flag (complexity index {j})")
        } else {
            format!("only {joc_frames} of {aus} access units carry the JOC flag")
        },
    );
}

/// The pass/fail checks, one line each.
fn report_checks(c: &mut Collector, p: &Probe, t: &Tally, h0: &Header, eac3: bool) {
    let verdict = |ok: bool, bad: Status| if ok { Status::Pass } else { bad };
    c.check(
        "Sync words",
        verdict(t.resyncs == 0, Status::Fail),
        if t.resyncs == 0 {
            "every frame starts where the previous one ends".to_string()
        } else {
            format!("sync was lost {} time(s), {} bytes skipped", t.resyncs, t.skipped_bytes)
        },
    );
    c.check(
        "Frame headers",
        verdict(t.bad_headers == 0, Status::Fail),
        if t.bad_headers == 0 {
            "all headers hold legal values".to_string()
        } else {
            format!("{} invalid header(s)", t.bad_headers)
        },
    );
    let frames = t.frames;
    let crc_detail = |bad: u64| {
        if bad == 0 {
            format!("{frames} of {frames} frames")
        } else {
            format!("{bad} of {frames} frames fail")
        }
    };
    if !eac3 {
        c.check(
            "CRC1 (first 5/8 of each frame)",
            verdict(t.bad_crc1 == 0, Status::Fail),
            crc_detail(t.bad_crc1),
        );
    }
    c.check(
        "CRC (whole frame)",
        verdict(t.bad_crc2 == 0, Status::Fail),
        crc_detail(t.bad_crc2),
    );
    c.check(
        "Complete file",
        verdict(t.truncated == 0, Status::Fail),
        if t.truncated == 0 {
            "the file ends on a frame boundary".to_string()
        } else {
            "the last frame is truncated".to_string()
        },
    );
    c.check(
        "Stable parameters",
        verdict(t.param_changes == 0, Status::Warn),
        if t.param_changes == 0 {
            "rate, channel mode and bit rate never change".to_string()
        } else {
            format!("parameters change {} time(s)", t.param_changes)
        },
    );
    if !eac3 {
        let cbr = t.size_changes == 0 || h0.rate == 44_100;
        c.check(
            "Constant bit rate",
            verdict(cbr, Status::Warn),
            if cbr {
                format!(
                    "every frame is {} bytes{}",
                    p.frame_bytes.first().copied().unwrap_or(0),
                    if h0.rate == 44_100 {
                        " (44.1 kHz alternates by one word)"
                    } else {
                        ""
                    }
                )
            } else {
                format!("the frame size changes {} time(s)", t.size_changes)
            },
        );
    }
}

fn parse_a(h: &[u8]) -> (Header, usize, bool) {
    let fscod = (h[4] >> 6) as usize;
    let frm = (h[4] & 0x3F) as usize;
    if fscod == 3 || frm > 37 {
        return (Header::default(), 0, false);
    }
    let bsid = (h[5] >> 3) as u32;
    let acmod = h[6] >> 5;
    let mut b = Bits::new(&h[6..]);
    b.get(3);
    if acmod & 1 != 0 && acmod != 1 {
        b.get(2);
    }
    if acmod & 4 != 0 {
        b.get(2);
    }
    if acmod == 2 {
        b.get(2);
    }
    let lfe = b.flag();
    let dialnorm = b.get(5);
    let hd = Header {
        rate: RATES[fscod],
        acmod,
        lfe,
        bsid,
        dialnorm,
        kbps: KBPS[frm >> 1],
        blocks: 6,
        strmtyp: 0,
        substream: 0,
    };
    (hd, WORDS[fscod][frm] as usize * 2, bsid <= 10)
}

fn parse_e(h: &[u8]) -> (Header, usize, bool) {
    let mut b = Bits::new(&h[2..]);
    let strmtyp = b.get(2) as u8;
    let substream = b.get(3) as u8;
    let frmsiz = b.get(11) as usize;
    let fscod = b.get(2) as usize;
    let (rate, blocks) = if fscod == 3 {
        let fscod2 = b.get(2) as usize;
        if fscod2 == 3 {
            return (Header::default(), 0, false);
        }
        ([24_000, 22_050, 16_000][fscod2], 6)
    } else {
        (RATES[fscod], [1, 2, 3, 6][b.get(2) as usize])
    };
    let acmod = b.get(3) as u8;
    let lfe = b.flag();
    let bsid = b.get(5);
    let dialnorm = b.get(5);
    let size = (frmsiz + 1) * 2;
    let kbps = ((size as u64 * 8 * rate as u64) / (blocks as u64 * 256) / 1000) as u32;
    (
        Header {
            rate,
            acmod,
            lfe,
            bsid,
            dialnorm,
            kbps,
            blocks,
            strmtyp,
            substream,
        },
        size,
        strmtyp != 3 && (11..=16).contains(&bsid) && size >= 8,
    )
}

/// Walk the E-AC-3 bit stream information to its additional data and read the Dolby Atmos (JOC) flag:
/// `Some(complexity index)` when `flag_ec3_extension_type_a` is set.
fn joc_complexity(f: &[u8]) -> Option<u32> {
    let mut b = Bits::new(&f[2..]);
    let strmtyp = b.get(2);
    b.skip(3 + 11);
    let fscod = b.get(2);
    let numblkscod = if fscod == 3 {
        b.skip(2);
        3
    } else {
        b.get(2)
    };
    let acmod = b.get(3);
    let lfeon = b.flag();
    b.skip(5 + 5); // bsid, dialnorm
    if b.flag() {
        b.skip(8);
    }
    if acmod == 0 {
        b.skip(5);
        if b.flag() {
            b.skip(8);
        }
    }
    if strmtyp == 1 && b.flag() {
        b.skip(16);
    }
    if b.flag() {
        skip_mixing_metadata(
            &mut b,
            &Stream {
                strmtyp,
                acmod,
                lfeon,
                numblkscod,
            },
        );
    }
    if b.flag() {
        skip_info_metadata(&mut b, acmod, fscod);
    }
    if strmtyp == 0 && numblkscod != 3 {
        b.skip(1);
    }
    if strmtyp == 2 {
        let blkid = if numblkscod == 3 { true } else { b.flag() };
        if blkid {
            b.skip(6);
        }
    }
    if b.flag() {
        let len = b.get(6) as usize + 1;
        // the flag is the lowest bit of the first byte, the complexity index the second byte
        b.skip(7);
        if b.flag() && len >= 2 && !b.over {
            return Some(b.get(8));
        }
    }
    None
}

/// The fixed part of an E-AC-3 header that the metadata that follows depends on.
struct Stream {
    strmtyp: u32,
    acmod: u32,
    lfeon: bool,
    numblkscod: u32,
}

/// Skips the mixing metadata of a bit stream information block.
fn skip_mixing_metadata(b: &mut Bits, s: &Stream) {
    if s.acmod > 2 {
        b.skip(2);
    }
    if s.acmod & 1 != 0 && s.acmod > 2 {
        b.skip(6);
    }
    if s.acmod & 4 != 0 {
        b.skip(6);
    }
    if s.lfeon && b.flag() {
        b.skip(5);
    }
    if s.strmtyp == 0 {
        if b.flag() {
            b.skip(6);
        }
        if s.acmod == 0 && b.flag() {
            b.skip(6);
        }
        if b.flag() {
            b.skip(6);
        }
        match b.get(2) {
            1 => b.skip(5),
            2 => b.skip(12),
            3 => {
                let n = b.get(5) as usize;
                b.skip(8 * (n + 2));
            }
            _ => {}
        }
        if s.acmod < 2 {
            if b.flag() {
                b.skip(14);
            }
            if s.acmod == 0 && b.flag() {
                b.skip(14);
            }
        }
        if b.flag() {
            if s.numblkscod == 0 {
                b.skip(5);
            } else {
                for _ in 0..[1, 2, 3, 6][s.numblkscod as usize] {
                    if b.flag() {
                        b.skip(5);
                    }
                }
            }
        }
    }
}

/// Skips the informational metadata of a bit stream information block.
fn skip_info_metadata(b: &mut Bits, acmod: u32, fscod: u32) {
    b.skip(3 + 1 + 1);
    if acmod == 2 {
        b.skip(4);
    }
    if acmod >= 6 {
        b.skip(2);
    }
    if b.flag() {
        b.skip(5 + 2 + 1);
    }
    if acmod == 0 && b.flag() {
        b.skip(5 + 2 + 1);
    }
    if fscod < 3 {
        b.skip(1);
    }
}

fn dump_a(f: &[u8]) -> Vec<(String, String)> {
    let fscod = (f[4] >> 6) as usize;
    let frm = (f[4] & 0x3F) as usize;
    let bsid = f[5] >> 3;
    let bsmod = f[5] & 7;
    let mut b = Bits::new(&f[6..]);
    let acmod = b.get(3) as u8;
    let mut rows = vec![
        (
            "sample rate code (fscod)".to_string(),
            format!("{fscod} = {} Hz", RATES.get(fscod).copied().unwrap_or(0)),
        ),
        (
            "frame size code (frmsizecod)".to_string(),
            format!(
                "{frm} = {} kbps, {} bytes",
                KBPS[(frm >> 1).min(18)],
                WORDS[fscod.min(2)][frm.min(37)] as usize * 2
            ),
        ),
        ("bit stream id (bsid)".to_string(), bsid.to_string()),
        (
            "bit stream mode (bsmod)".to_string(),
            format!("{bsmod} = {}", BSMOD[bsmod as usize]),
        ),
        (
            "audio coding mode (acmod)".to_string(),
            format!("{acmod} = {}", ACMOD[acmod as usize]),
        ),
    ];
    if acmod & 1 != 0 && acmod != 1 {
        rows.push((
            "centre mix level (cmixlev)".to_string(),
            ["-3.0 dB", "-4.5 dB", "-6.0 dB", "reserved"][b.get(2) as usize].to_string(),
        ));
    }
    if acmod & 4 != 0 {
        rows.push((
            "surround mix level (surmixlev)".to_string(),
            ["-3 dB", "-6 dB", "0 dB", "reserved"][b.get(2) as usize].to_string(),
        ));
    }
    if acmod == 2 {
        rows.push((
            "Dolby Surround mode (dsurmod)".to_string(),
            ["not indicated", "not Dolby Surround", "Dolby Surround", "reserved"][b.get(2) as usize].to_string(),
        ));
    }
    rows.push(("LFE channel (lfeon)".to_string(), (b.get(1)).to_string()));
    rows.push((
        "dialogue normalization (dialnorm)".to_string(),
        format!("-{} dBFS", b.get(5)),
    ));
    if b.flag() {
        rows.push(("compression gain (compr)".to_string(), format!("0x{:02X}", b.get(8))));
    }
    if b.flag() {
        rows.push(("language code (langcod)".to_string(), format!("0x{:02X}", b.get(8))));
    }
    if b.flag() {
        rows.push(("mix level (mixlevel)".to_string(), format!("{} dB SPL", 80 + b.get(5))));
        rows.push((
            "room type (roomtyp)".to_string(),
            ["not indicated", "large room, X curve", "small room, flat", "reserved"][b.get(2) as usize].to_string(),
        ));
    }
    rows.push(("copyright (copyrightb)".to_string(), b.get(1).to_string()));
    rows.push(("original (origbs)".to_string(), b.get(1).to_string()));
    rows
}

fn dump_e(f: &[u8]) -> Vec<(String, String)> {
    let (h, size, _) = parse_e(f);
    vec![
        (
            "stream type (strmtyp)".to_string(),
            [
                "independent",
                "dependent",
                "independent, converted from AC-3",
                "reserved",
            ][h.strmtyp as usize]
                .to_string(),
        ),
        ("substream id".to_string(), h.substream.to_string()),
        ("frame size".to_string(), format!("{size} bytes")),
        ("sample rate".to_string(), format!("{} Hz", h.rate)),
        (
            "blocks per frame".to_string(),
            format!("{} ({} samples)", h.blocks, h.blocks * 256),
        ),
        (
            "audio coding mode (acmod)".to_string(),
            format!("{} = {}", h.acmod, ACMOD[h.acmod as usize]),
        ),
        ("LFE channel (lfeon)".to_string(), (h.lfe as u8).to_string()),
        ("bit stream id (bsid)".to_string(), h.bsid.to_string()),
        (
            "dialogue normalization (dialnorm)".to_string(),
            format!("-{} dBFS", h.dialnorm),
        ),
        ("bit rate of this frame".to_string(), format!("{} kbps", h.kbps)),
    ]
}
