//! AC-3 and E-AC-3 (A/52 and its Annex E): sync, sizes, checksums, header fields, continuity.

use crate::bits::Bits;
use crate::{crc, Collector, Probe, Status, Window};

const RATES: [u32; 3] = [48_000, 44_100, 32_000];
const KBPS: [u32; 19] = [32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 576, 640];
/// Frame size in 16-bit words per `frmsizecod`, for 48, 44.1 and 32 kHz.
const WORDS: [[u16; 38]; 3] = [
    [64, 64, 80, 80, 96, 96, 112, 112, 128, 128, 160, 160, 192, 192, 224, 224, 256, 256, 320, 320, 384, 384, 448, 448, 512, 512, 640, 640, 768, 768, 896, 896, 1024, 1024, 1152, 1152, 1280, 1280],
    [69, 70, 87, 88, 104, 105, 121, 122, 139, 140, 174, 175, 208, 209, 243, 244, 278, 279, 348, 349, 417, 418, 487, 488, 557, 558, 696, 697, 835, 836, 975, 976, 1114, 1115, 1253, 1254, 1393, 1394],
    [96, 96, 120, 120, 144, 144, 168, 168, 192, 192, 240, 240, 288, 288, 336, 336, 384, 384, 480, 480, 576, 576, 672, 672, 768, 768, 960, 960, 1152, 1152, 1344, 1344, 1536, 1536, 1728, 1728, 1920, 1920],
];
const ACMOD: [&str; 8] = ["1+1 (dual mono)", "1/0 (C)", "2/0 (L R)", "3/0 (L C R)", "2/1 (L R S)", "3/1 (L C R S)", "2/2 (L R Ls Rs)", "3/2 (L C R Ls Rs)"];
const NFCH: [u32; 8] = [2, 1, 2, 3, 3, 4, 4, 5];
const BSMOD: [&str; 8] = ["complete main", "music and effects", "visually impaired", "hearing impaired", "dialogue", "commentary", "emergency", "voice-over / karaoke"];

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

pub(crate) fn verify(w: &mut Window, c: &mut Collector, eac3: bool) -> Probe {
    let mut p = Probe::default();
    let (mut pos, len) = (0u64, w.len);
    let (mut frames, mut aus) = (0u64, 0u64);
    let (mut bad_crc1, mut bad_crc2, mut bad_hdr, mut resync, mut gaps) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut first: Option<Header> = None;
    let mut changes = 0u64;
    let mut cur: Option<Header> = None;
    let mut sizes: Vec<u32> = Vec::new();
    let mut size_changes = 0u64;
    let mut dependent = 0u64;
    let mut dump: Vec<(String, String)> = Vec::new();
    let mut extra_bytes = 0u64;
    let mut joc: Option<u32> = None;
    let mut joc_frames = 0u64;
    let mut sample_total = 0u64;
    while pos + 8 <= len {
        let head = w.get(pos, 16).to_vec();
        if head.len() < 8 || head[0] != 0x0B || head[1] != 0x77 {
            // lost sync: look for the next one
            let from = pos + 1;
            let mut found = None;
            let mut q = from;
            'scan: while q < len && q < from + (1 << 20) {
                let chunk = w.get(q, 1 << 16).to_vec();
                if chunk.len() < 2 {
                    break;
                }
                for i in 0..chunk.len() - 1 {
                    if chunk[i] == 0x0B && chunk[i + 1] == 0x77 {
                        found = Some(q + i as u64);
                        break 'scan;
                    }
                }
                q += chunk.len() as u64 - 1;
            }
            resync += 1;
            let skipped = found.unwrap_or(len) - pos;
            c.finding(Status::Fail, Some(pos), format!("no sync word here; {skipped} bytes skipped to the next one"));
            extra_bytes += skipped;
            match found {
                Some(f) => {
                    pos = f;
                    continue;
                }
                None => break,
            }
        }
        // header
        let (h, fsize, hdr_ok) = if eac3 { parse_e(&head) } else { parse_a(&head) };
        if !hdr_ok || fsize == 0 {
            bad_hdr += 1;
            c.finding(Status::Fail, Some(pos), "invalid frame header".to_string());
            pos += 2;
            continue;
        }
        if pos + fsize as u64 > len {
            c.finding(Status::Fail, Some(pos), format!("the last frame is cut short ({} of {} bytes)", len - pos, fsize));
            gaps += 1;
            break;
        }
        let frame = w.get(pos, fsize).to_vec();
        // checksums
        if eac3 {
            if crc::ac3(&frame[2..]) != 0 {
                bad_crc2 += 1;
                c.finding(Status::Fail, Some(pos), "frame checksum (CRC) mismatch".to_string());
            }
        } else {
            let f58 = ((fsize >> 2) + (fsize >> 4)) << 1;
            if crc::ac3(&frame[2..f58]) != 0 {
                bad_crc1 += 1;
                c.finding(Status::Fail, Some(pos), "first-part checksum (CRC1) mismatch".to_string());
            }
            if crc::ac3(&frame[2..]) != 0 {
                bad_crc2 += 1;
                c.finding(Status::Fail, Some(pos), "frame checksum (CRC2) mismatch".to_string());
            }
        }
        if first.is_none() {
            let f = match eac3 {
                true => dump_e(&frame),
                false => dump_a(&frame),
            };
            dump = f;
            first = Some(h.clone());
        }
        if eac3 {
            if let Some(j) = joc_complexity(&frame) {
                joc_frames += 1;
                joc.get_or_insert(j);
            }
        }
        if eac3 && h.strmtyp == 1 {
            dependent += 1;
        } else {
            if let Some(prev) = &cur {
                if *prev != h {
                    changes += 1;
                    if changes <= 5 {
                        c.finding(Status::Warn, Some(pos), "stream parameters change here (rate, channels or bit rate)");
                    }
                }
            }
            cur = Some(h.clone());
            aus += 1;
            sample_total += h.blocks as u64 * 256;
            p.frame_samples.push(h.blocks * 256);
        }
        if sizes.last().map_or(false, |l| *l != fsize as u32) {
            size_changes += 1;
        }
        sizes.push(fsize as u32);
        p.frame_bytes.push(fsize as u32);
        frames += 1;
        pos += fsize as u64;
    }
    let h0 = first.unwrap_or_default();
    p.sample_rate = h0.rate;
    p.channels = if eac3 { NFCH[h0.acmod as usize & 7] + u32::from(h0.lfe) } else { NFCH[h0.acmod as usize & 7] + u32::from(h0.lfe) };
    p.duration_secs = if h0.rate > 0 { sample_total as f64 / h0.rate as f64 } else { 0.0 };
    c.kv("Format", if eac3 { format!("E-AC-3, bit stream id {}", h0.bsid) } else { format!("AC-3, bit stream id {}", h0.bsid) });
    c.kv("Sample rate", format!("{} Hz", h0.rate));
    c.kv("Channels", format!("{} + {}", ACMOD[h0.acmod as usize & 7], if h0.lfe { "LFE" } else { "no LFE" }));
    c.kv("Frames", if eac3 && dependent > 0 { format!("{aus} access units ({frames} frames, {dependent} in dependent substreams)") } else { format!("{frames}") });
    c.kv("Duration", format!("{:.3} s", p.duration_secs));
    if p.duration_secs > 0.0 {
        c.kv("Average bit rate", format!("{:.1} kbps", (len - extra_bytes) as f64 * 8.0 / p.duration_secs / 1000.0));
    }
    c.kv("Dialogue level", format!("-{} dBFS", h0.dialnorm));
    if eac3 {
        match joc {
            Some(j) => {
                c.kv("Dolby Atmos", format!("yes: Joint Object Coding, complexity index {j} (in {joc_frames} of {aus} access units)"));
                c.check("Dolby Atmos signalling", if joc_frames == aus { Status::Pass } else { Status::Warn }, if joc_frames == aus { format!("every access unit carries the JOC flag (complexity index {j})") } else { format!("only {joc_frames} of {aus} access units carry the JOC flag") });
            }
            None => c.kv("Dolby Atmos", "no"),
        }
    }

    c.check("Sync words", if resync == 0 { Status::Pass } else { Status::Fail }, if resync == 0 { "every frame starts where the previous one ends".to_string() } else { format!("sync was lost {resync} time(s), {extra_bytes} bytes skipped") });
    c.check("Frame headers", if bad_hdr == 0 { Status::Pass } else { Status::Fail }, if bad_hdr == 0 { "all headers hold legal values".to_string() } else { format!("{bad_hdr} invalid header(s)") });
    if !eac3 {
        c.check("CRC1 (first 5/8 of each frame)", if bad_crc1 == 0 { Status::Pass } else { Status::Fail }, if bad_crc1 == 0 { format!("{frames} of {frames} frames") } else { format!("{bad_crc1} of {frames} frames fail") });
    }
    c.check("CRC (whole frame)", if bad_crc2 == 0 { Status::Pass } else { Status::Fail }, if bad_crc2 == 0 { format!("{frames} of {frames} frames") } else { format!("{bad_crc2} of {frames} frames fail") });
    c.check("Complete file", if gaps == 0 { Status::Pass } else { Status::Fail }, if gaps == 0 { "the file ends on a frame boundary".to_string() } else { "the last frame is truncated".to_string() });
    let steady = changes == 0;
    c.check("Stable parameters", if steady { Status::Pass } else { Status::Warn }, if steady { "rate, channel mode and bit rate never change".to_string() } else { format!("parameters change {changes} time(s)") });
    if !eac3 {
        let cbr = size_changes == 0 || h0.rate == 44_100;
        c.check("Constant bit rate", if cbr { Status::Pass } else { Status::Warn }, if cbr { format!("every frame is {} bytes{}", sizes.first().copied().unwrap_or(0), if h0.rate == 44_100 { " (44.1 kHz alternates by one word)" } else { "" }) } else { format!("the frame size changes {size_changes} time(s)") });
    }
    if c.report.deep {
        c.section("First frame header", dump);
        if eac3 {
            let streams: std::collections::BTreeSet<u8> = std::collections::BTreeSet::new();
            let _ = streams;
        }
    }
    p
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
    let hd = Header { rate: RATES[fscod], acmod, lfe, bsid, dialnorm, kbps: KBPS[frm >> 1], blocks: 6, strmtyp: 0, substream: 0 };
    (hd, WORDS[fscod][frm] as usize * 2, bsid <= 10)
}

fn parse_e(h: &[u8]) -> (Header, usize, bool) {
    let mut b = Bits::new(&h[2..]);
    let strmtyp = b.get(2) as u8;
    let substream = b.get(3) as u8;
    let frmsiz = b.get(11) as usize;
    let fscod = b.get(2) as usize;
    let blocks;
    let rate;
    if fscod == 3 {
        let fscod2 = b.get(2) as usize;
        if fscod2 == 3 {
            return (Header::default(), 0, false);
        }
        rate = [24_000, 22_050, 16_000][fscod2];
        blocks = 6;
    } else {
        rate = RATES[fscod];
        blocks = [1, 2, 3, 6][b.get(2) as usize];
    }
    let acmod = b.get(3) as u8;
    let lfe = b.flag();
    let bsid = b.get(5);
    let dialnorm = b.get(5);
    let size = (frmsiz + 1) * 2;
    let kbps = ((size as u64 * 8 * rate as u64) / (blocks as u64 * 256) / 1000) as u32;
    (Header { rate, acmod, lfe, bsid, dialnorm, kbps, blocks, strmtyp, substream }, size, strmtyp != 3 && (11..=16).contains(&bsid) && size >= 8)
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
        // mixing metadata
        if acmod > 2 {
            b.skip(2);
        }
        if acmod & 1 != 0 && acmod > 2 {
            b.skip(6);
        }
        if acmod & 4 != 0 {
            b.skip(6);
        }
        if lfeon && b.flag() {
            b.skip(5);
        }
        if strmtyp == 0 {
            if b.flag() {
                b.skip(6);
            }
            if acmod == 0 && b.flag() {
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
            if acmod < 2 {
                if b.flag() {
                    b.skip(14);
                }
                if acmod == 0 && b.flag() {
                    b.skip(14);
                }
            }
            if b.flag() {
                if numblkscod == 0 {
                    b.skip(5);
                } else {
                    for _ in 0..[1, 2, 3, 6][numblkscod as usize] {
                        if b.flag() {
                            b.skip(5);
                        }
                    }
                }
            }
        }
    }
    if b.flag() {
        // informational metadata
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

fn dump_a(f: &[u8]) -> Vec<(String, String)> {
    let fscod = (f[4] >> 6) as usize;
    let frm = (f[4] & 0x3F) as usize;
    let bsid = f[5] >> 3;
    let bsmod = f[5] & 7;
    let mut b = Bits::new(&f[6..]);
    let acmod = b.get(3) as u8;
    let mut rows = vec![
        ("sample rate code (fscod)".to_string(), format!("{fscod} = {} Hz", RATES.get(fscod).copied().unwrap_or(0))),
        ("frame size code (frmsizecod)".to_string(), format!("{frm} = {} kbps, {} bytes", KBPS[(frm >> 1).min(18)], WORDS[fscod.min(2)][frm.min(37)] as usize * 2)),
        ("bit stream id (bsid)".to_string(), bsid.to_string()),
        ("bit stream mode (bsmod)".to_string(), format!("{bsmod} = {}", BSMOD[bsmod as usize])),
        ("audio coding mode (acmod)".to_string(), format!("{acmod} = {}", ACMOD[acmod as usize])),
    ];
    if acmod & 1 != 0 && acmod != 1 {
        rows.push(("centre mix level (cmixlev)".to_string(), ["-3.0 dB", "-4.5 dB", "-6.0 dB", "reserved"][b.get(2) as usize].to_string()));
    }
    if acmod & 4 != 0 {
        rows.push(("surround mix level (surmixlev)".to_string(), ["-3 dB", "-6 dB", "0 dB", "reserved"][b.get(2) as usize].to_string()));
    }
    if acmod == 2 {
        rows.push(("Dolby Surround mode (dsurmod)".to_string(), ["not indicated", "not Dolby Surround", "Dolby Surround", "reserved"][b.get(2) as usize].to_string()));
    }
    rows.push(("LFE channel (lfeon)".to_string(), (b.get(1)).to_string()));
    rows.push(("dialogue normalization (dialnorm)".to_string(), format!("-{} dBFS", b.get(5))));
    if b.flag() {
        rows.push(("compression gain (compr)".to_string(), format!("0x{:02X}", b.get(8))));
    }
    if b.flag() {
        rows.push(("language code (langcod)".to_string(), format!("0x{:02X}", b.get(8))));
    }
    if b.flag() {
        rows.push(("mix level (mixlevel)".to_string(), format!("{} dB SPL", 80 + b.get(5))));
        rows.push(("room type (roomtyp)".to_string(), ["not indicated", "large room, X curve", "small room, flat", "reserved"][b.get(2) as usize].to_string()));
    }
    rows.push(("copyright (copyrightb)".to_string(), b.get(1).to_string()));
    rows.push(("original (origbs)".to_string(), b.get(1).to_string()));
    rows
}

fn dump_e(f: &[u8]) -> Vec<(String, String)> {
    let (h, size, _) = parse_e(f);
    vec![
        ("stream type (strmtyp)".to_string(), ["independent", "dependent", "independent, converted from AC-3", "reserved"][h.strmtyp as usize].to_string()),
        ("substream id".to_string(), h.substream.to_string()),
        ("frame size".to_string(), format!("{size} bytes")),
        ("sample rate".to_string(), format!("{} Hz", h.rate)),
        ("blocks per frame".to_string(), format!("{} ({} samples)", h.blocks, h.blocks * 256)),
        ("audio coding mode (acmod)".to_string(), format!("{} = {}", h.acmod, ACMOD[h.acmod as usize])),
        ("LFE channel (lfeon)".to_string(), (h.lfe as u8).to_string()),
        ("bit stream id (bsid)".to_string(), h.bsid.to_string()),
        ("dialogue normalization (dialnorm)".to_string(), format!("-{} dBFS", h.dialnorm)),
        ("bit rate of this frame".to_string(), format!("{} kbps", h.kbps)),
    ]
}
