//! Dolby TrueHD and MLP: access unit framing, the nibble parity, the major sync checksum and the
//! per-substream parity and checksum bytes.

use crate::bits::Bits;
use crate::{Collector, Probe, Status, Window};
use sw_codec_truehd::bits::{checksum8, parity};

const RATES: [u32; 16] = [
    48_000, 96_000, 192_000, 0, 0, 0, 0, 0, 44_100, 88_200, 176_400, 0, 0, 0, 0, 0,
];

const GROUPS: [(&str, u32); 13] = [
    ("L R", 2),
    ("C", 1),
    ("LFE", 1),
    ("Ls Rs", 2),
    ("Lvh Rvh", 2),
    ("Lc Rc", 2),
    ("Lrs Rrs", 2),
    ("Cs", 1),
    ("Ts", 1),
    ("Lsd Rsd", 2),
    ("Lw Rw", 2),
    ("Cvh", 1),
    ("LFE2", 1),
];

/// The 13-bit channel assignment of the full presentation as names and a channel count.
fn assignment(bits: u32) -> (String, u32) {
    let names: Vec<&str> = (0..13).filter(|g| bits >> g & 1 == 1).map(|g| GROUPS[g].0).collect();
    let n = (0..13).filter(|g| bits >> g & 1 == 1).map(|g| GROUPS[g].1).sum();
    (
        if names.is_empty() {
            "none".to_string()
        } else {
            names.join(" + ")
        },
        n,
    )
}

#[derive(Default, Clone, PartialEq)]
struct Major {
    rate: u32,
    nsub: u32,
    arr6: u32,
    arr8: u32,
    mods: (u32, u32, u32),
    vbr: bool,
    peak: u32,
    info: u32,
    size: usize,
}

pub(crate) fn verify(w: &mut Window, c: &mut Collector, mlp: bool) -> Probe {
    let mut p = Probe {
        lossless: true,
        ..Default::default()
    };
    let len = w.len;
    let mut pos = 0u64;
    let (mut aus, mut majors) = (0u64, 0u64);
    let (mut bad_len, mut bad_nibble, mut bad_major, mut bad_par, mut bad_sum, mut bad_dir, mut timing_jumps) =
        (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let mut first: Option<Major> = None;
    let mut cur: Option<Major> = None;
    let mut changes = 0u64;
    let mut prev_timing: Option<u16> = None;
    let mut au_samples = 0u32;
    let mut sync_loss = 0u64;
    let mut last_major_au = 0u64;
    let mut intervals: Vec<u64> = Vec::new();
    let mut first_is_major = true;
    let mut trailing = 0u64;
    let mut sizes_min = u32::MAX;
    let mut sizes_max = 0u32;
    let mut bytes_total = 0u64;
    let mut checked_subs = 0u64;
    while pos + 4 <= len {
        let head = w.get(pos, 4096.min((len - pos) as usize)).to_vec();
        let words = (((head[0] & 0x0F) as usize) << 8) | head[1] as usize;
        let au_len = words * 2;
        if au_len < 8 {
            bad_len += 1;
            c.finding(
                Status::Fail,
                Some(pos),
                format!("access unit length {au_len} is too small"),
            );
            sync_loss += 1;
            break;
        }
        if pos + au_len as u64 > len {
            trailing = len - pos;
            c.finding(
                Status::Fail,
                Some(pos),
                format!("the last access unit is cut short ({} of {au_len} bytes)", len - pos),
            );
            break;
        }
        let au = if head.len() >= au_len {
            head[..au_len].to_vec()
        } else {
            w.get(pos, au_len).to_vec()
        };
        let major = au.len() >= 8 && au[4..7] == [0xF8, 0x72, 0x6F] && (au[7] == 0xBA || au[7] == 0xBB);
        if aus == 0 {
            first_is_major = major;
        }
        let mut off = 4usize;
        let mut m: Option<Major> = None;
        if major {
            majors += 1;
            if majors > 1 {
                intervals.push(aus - last_major_au);
            }
            last_major_au = aus;
            let ext = au.len() >= 30 && au[7] == 0xBA && au[4 + 25] & 1 == 1;
            let msize = if ext {
                28 + 2 + (au[4 + 26] >> 4) as usize * 2
            } else {
                28
            };
            off += msize;
            if au.len() < off {
                bad_major += 1;
                c.finding(Status::Fail, Some(pos), "major sync runs past the access unit");
                pos += au_len as u64;
                aus += 1;
                continue;
            }
            if !crate::crc::mlp_major_sync_ok(&au[4..4 + msize]) {
                bad_major += 1;
                c.finding(Status::Fail, Some(pos), "major sync checksum mismatch");
            }
            let mut b = Bits::new(&au[8..4 + msize]);
            let mut ratebits = b.get(4);
            let mut mj = Major {
                rate: RATES[ratebits as usize],
                size: msize,
                ..Default::default()
            };
            if mlp {
                // MLP: two quantiser fields come first, then the two group rates
                b.pos = 0;
                b.get(8);
                ratebits = b.get(4);
                mj.rate = RATES[ratebits as usize];
                b.get(4);
                b.get(11);
                mj.arr6 = b.get(5);
            } else {
                b.get(2);
                b.get(2);
                mj.mods.0 = b.get(2);
                mj.mods.1 = b.get(2);
                mj.arr6 = b.get(5);
                mj.mods.2 = b.get(2);
                mj.arr8 = b.get(13);
            }
            b.get(48);
            mj.vbr = b.flag();
            mj.peak = b.get(15);
            mj.nsub = b.get(4);
            b.get(4);
            mj.info = b.get(8);
            if mj.rate == 0 {
                bad_major += 1;
                c.finding(Status::Fail, Some(pos), "invalid sampling rate code in the major sync");
            }
            au_samples = 40u32 << (ratebits & 7);
            if first.is_none() {
                first = Some(mj.clone());
            }
            if let Some(prev) = &cur {
                if prev.rate != mj.rate || prev.nsub != mj.nsub || prev.arr6 != mj.arr6 || prev.arr8 != mj.arr8 {
                    changes += 1;
                    if changes <= 5 {
                        c.finding(Status::Warn, Some(pos), "stream parameters change at this major sync");
                    }
                }
            }
            cur = Some(mj.clone());
            m = Some(mj);
        }
        let nsub = m.as_ref().or(cur.as_ref()).map(|m| m.nsub as usize).unwrap_or(0);
        if nsub == 0 || nsub > 4 {
            bad_dir += 1;
            c.finding(
                Status::Fail,
                Some(pos),
                "no valid major sync has been seen, the substream layout is unknown",
            );
            pos += au_len as u64;
            aus += 1;
            continue;
        }
        // substream directory
        let dir_start = off;
        let mut ends = Vec::new();
        let mut checkdata = Vec::new();
        for _ in 0..nsub {
            if off + 2 > au.len() {
                break;
            }
            let hdr = u16::from_be_bytes([au[off], au[off + 1]]);
            off += 2;
            if hdr >> 15 == 1 {
                off += 2;
            }
            checkdata.push(hdr >> 13 & 1 == 1);
            ends.push((hdr & 0x0FFF) as usize * 2);
        }
        if ends.len() != nsub || off > au.len() {
            bad_dir += 1;
            c.finding(Status::Fail, Some(pos), "substream directory runs past the access unit");
            pos += au_len as u64;
            aus += 1;
            continue;
        }
        // parity nibble of the access unit header
        let x = parity(&au[..4]) ^ parity(&au[dir_start..off]);
        if ((x >> 4) ^ x) & 0xF != 0xF {
            bad_nibble += 1;
            c.finding(Status::Fail, Some(pos), "access unit header parity nibble mismatch");
        }
        // substreams
        let mut start = off;
        let mut layout_ok = true;
        for si in 0..nsub {
            let end = off + ends[si];
            if end > au.len() || end < start + 2 {
                layout_ok = false;
                break;
            }
            if checkdata[si] {
                checked_subs += 1;
                let sub = &au[start..end];
                let body = &sub[..sub.len() - 2];
                if (sub[sub.len() - 2] ^ parity(body)) != 0xA9 {
                    bad_par += 1;
                    c.finding(
                        Status::Fail,
                        Some(pos + start as u64),
                        format!("substream {si} parity byte mismatch"),
                    );
                }
                if !body.is_empty() && sub[sub.len() - 1] != checksum8(body) {
                    bad_sum += 1;
                    c.finding(
                        Status::Fail,
                        Some(pos + start as u64),
                        format!("substream {si} checksum byte mismatch"),
                    );
                }
            }
            start = end;
        }
        if !layout_ok || start != au.len().min(off + ends[nsub - 1]) {
            bad_dir += 1;
            c.finding(
                Status::Fail,
                Some(pos),
                "substream sizes do not add up to the access unit",
            );
        }
        // timing
        let timing = u16::from_be_bytes([au[2], au[3]]);
        if let Some(pt) = prev_timing {
            if pt.wrapping_add(au_samples as u16) != timing {
                timing_jumps += 1;
                if timing_jumps <= 3 {
                    c.finding(
                        Status::Info,
                        Some(pos),
                        format!("input timing jumps from {pt} to {timing}"),
                    );
                }
            }
        }
        prev_timing = Some(timing);
        p.frame_bytes.push(au_len as u32);
        p.frame_samples.push(au_samples);
        sizes_min = sizes_min.min(au_len as u32);
        sizes_max = sizes_max.max(au_len as u32);
        bytes_total += au_len as u64;
        aus += 1;
        pos += au_len as u64;
    }
    let f = first.unwrap_or_default();
    let total_samples: u64 = p.frame_samples.iter().map(|x| *x as u64).sum();
    p.sample_rate = f.rate;
    p.duration_secs = if f.rate > 0 {
        total_samples as f64 / f.rate as f64
    } else {
        0.0
    };
    c.kv(
        "Format",
        if mlp {
            "MLP".to_string()
        } else {
            "Dolby TrueHD".to_string()
        },
    );
    c.kv("Sample rate", format!("{} Hz", f.rate));
    if mlp {
        c.kv("Channel arrangement", format!("code {}", f.arr6));
    } else {
        let (names, n) = assignment(f.arr8);
        c.kv("Channels (full presentation)", format!("{n}: {names}"));
        c.kv(
            "Stereo / 6-channel presentations",
            format!(
                "6-channel arrangement code {} (modifiers {}/{}/{})",
                f.arr6, f.mods.0, f.mods.1, f.mods.2
            ),
        );
        p.channels = n;
    }
    c.kv("Substreams", f.nsub.to_string());
    if !mlp {
        let atmos = f.nsub == 4 && f.info >> 7 == 1;
        c.kv(
            "Dolby Atmos",
            if atmos {
                "yes: 16-channel spatial presentation (the 4th substream carries the object data)".to_string()
            } else {
                "no".to_string()
            },
        );
        if atmos {
            c.check("Dolby Atmos signalling", Status::Info, "4 substreams with the spatial-presentation flag set; the object substream is not decoded by FFmpeg, so the audio check covers the 7.1 bed");
        }
    }
    c.kv("Access units", aus.to_string());
    c.kv("Duration", format!("{:.3} s", p.duration_secs));
    if p.duration_secs > 0.0 {
        c.kv(
            "Average bit rate",
            format!("{:.1} kbps", len as f64 * 8.0 / p.duration_secs / 1000.0),
        );
    }
    c.kv(
        "Peak bit rate (stated)",
        format!("{:.0} kbps", ((f.peak as u64 * f.rate as u64 + 8) >> 4) as f64 / 1000.0),
    );
    let ok = |n: u64| if n == 0 { Status::Pass } else { Status::Fail };
    c.check(
        "Access unit framing",
        ok(bad_len + sync_loss),
        if bad_len + sync_loss == 0 {
            format!("{aus} access units, each starts where the previous one ends")
        } else {
            "an access unit has an impossible length".to_string()
        },
    );
    c.check(
        "Starts with a major sync",
        if first_is_major { Status::Pass } else { Status::Warn },
        if first_is_major {
            "decoding can start at the first access unit".to_string()
        } else {
            "the first access unit has no major sync; a player must skip to the next".to_string()
        },
    );
    c.check(
        "Major sync checksums",
        ok(bad_major),
        if bad_major == 0 {
            format!("{majors} major syncs")
        } else {
            format!("{bad_major} bad")
        },
    );
    c.check(
        "Access unit parity",
        ok(bad_nibble),
        if bad_nibble == 0 {
            format!("{aus} of {aus} headers")
        } else {
            format!("{bad_nibble} of {aus} headers fail")
        },
    );
    c.check(
        "Substream directory",
        ok(bad_dir),
        if bad_dir == 0 {
            "sizes add up in every access unit".to_string()
        } else {
            format!("{bad_dir} access unit(s) with an inconsistent directory")
        },
    );
    c.check(
        "Substream parity and checksum",
        ok(bad_par + bad_sum),
        if bad_par + bad_sum == 0 {
            format!("{checked_subs} substreams checked")
        } else {
            format!("{bad_par} parity and {bad_sum} checksum errors in {checked_subs} substreams")
        },
    );
    // Dolby's own encoder lets this timestamp jump (it follows the variable data rate), so it is reported, not judged
    c.check(
        "Input timing",
        if timing_jumps == 0 { Status::Pass } else { Status::Info },
        if timing_jumps == 0 {
            "advances by one access unit each time".to_string()
        } else {
            format!("{timing_jumps} jump(s) (normal for streams from Dolby's encoder)")
        },
    );
    let regular = intervals.windows(2).all(|w| w[0] == w[1]) || intervals.len() < 2;
    c.check(
        "Restart points",
        Status::Info,
        if intervals.is_empty() {
            "a single major sync".to_string()
        } else {
            format!(
                "{majors} major syncs, every {} access units{}",
                intervals[0],
                if regular { "" } else { " (varies)" }
            )
        },
    );
    c.check(
        "Stable parameters",
        if changes == 0 { Status::Pass } else { Status::Warn },
        if changes == 0 {
            "rate and channels never change".to_string()
        } else {
            format!("{changes} change(s)")
        },
    );
    c.check(
        "Complete file",
        if trailing == 0 { Status::Pass } else { Status::Fail },
        if trailing == 0 {
            "the file ends on an access unit boundary".to_string()
        } else {
            format!("{trailing} bytes of an unfinished access unit at the end")
        },
    );
    if c.report.deep {
        c.section(
            "Major sync (first)",
            vec![
                ("sample rate".to_string(), format!("{} Hz", f.rate)),
                ("access unit length".to_string(), format!("{au_samples} samples")),
                ("major sync size".to_string(), format!("{} bytes", f.size)),
                ("variable bit rate".to_string(), f.vbr.to_string()),
                ("substreams".to_string(), f.nsub.to_string()),
                ("substream info".to_string(), format!("0x{:02X}", f.info)),
                (
                    "access unit size (min / max)".to_string(),
                    format!(
                        "{} / {} bytes",
                        if sizes_min == u32::MAX { 0 } else { sizes_min },
                        sizes_max
                    ),
                ),
                (
                    "average access unit".to_string(),
                    format!("{:.0} bytes", bytes_total as f64 / aus.max(1) as f64),
                ),
            ],
        );
        let data = w.get(0, 64 << 20).to_vec();
        let r = match std::panic::catch_unwind(|| sw_codec_truehd::analyze::analyze_range(&data, 0, 400)) {
            Ok(r) => r,
            Err(_) => {
                c.finding(
                    Status::Warn,
                    None,
                    "the substream analysis could not follow this stream (it stopped on an unexpected layout)",
                );
                return p;
            }
        };
        let mut rows = vec![(
            "scope".to_string(),
            "the first 400 restart intervals (at most 64 MB)".to_string(),
        )];
        for (i, l) in r.summary().lines().enumerate() {
            if !l.trim().is_empty() {
                rows.push((format!("{:02}", i + 1), l.trim().to_string()));
            }
        }
        if !r.errors.is_empty() {
            let mut kinds: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
            for e in &r.errors {
                *kinds.entry(e.as_str()).or_default() += 1;
            }
            for (e, n) in kinds.iter().take(5) {
                rows.push(("analysis stopped".to_string(), format!("{e} ({n} times)")));
            }
            // this analyser does not know every coding tool of Dolby's encoder; the decoder check is the verdict
            c.finding(Status::Warn, None, format!("the substream analysis could not follow part of this stream ({} stops, e.g. '{}'); the decoder check above is the verdict", r.errors.len(), r.errors[0]));
        }
        c.section("Substream analysis", rows);
    }
    p
}
