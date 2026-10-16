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

/// What the walk through the access units found.
struct Tally {
    aus: u64,
    majors: u64,
    bad_len: u64,
    bad_nibble: u64,
    bad_major: u64,
    bad_par: u64,
    bad_sum: u64,
    bad_dir: u64,
    timing_jumps: u64,
    first: Option<Major>,
    current: Option<Major>,
    changes: u64,
    prev_timing: Option<u16>,
    /// Samples in one access unit, from the latest major sync.
    au_samples: u32,
    sync_loss: u64,
    last_major_au: u64,
    /// Access units between one major sync and the next.
    intervals: Vec<u64>,
    first_is_major: bool,
    /// Bytes of an unfinished access unit at the end of the file.
    trailing: u64,
    size_min: u32,
    size_max: u32,
    bytes_total: u64,
    checked_subs: u64,
}

impl Tally {
    fn new() -> Tally {
        Tally {
            aus: 0,
            majors: 0,
            bad_len: 0,
            bad_nibble: 0,
            bad_major: 0,
            bad_par: 0,
            bad_sum: 0,
            bad_dir: 0,
            timing_jumps: 0,
            first: None,
            current: None,
            changes: 0,
            prev_timing: None,
            au_samples: 0,
            sync_loss: 0,
            last_major_au: 0,
            intervals: Vec::new(),
            first_is_major: true,
            trailing: 0,
            size_min: u32::MAX,
            size_max: 0,
            bytes_total: 0,
            checked_subs: 0,
        }
    }
}

pub(crate) fn verify(w: &mut Window, c: &mut Collector, mlp: bool) -> Probe {
    let mut p = Probe {
        lossless: true,
        ..Default::default()
    };
    let t = walk_access_units(w, c, mlp, &mut p);
    let f = t.first.clone().unwrap_or_default();
    let total_samples: u64 = p.frame_samples.iter().map(|x| *x as u64).sum();
    p.sample_rate = f.rate;
    p.duration_secs = if f.rate > 0 {
        total_samples as f64 / f.rate as f64
    } else {
        0.0
    };
    report_facts(c, &mut p, &t, &f, w.len, mlp);
    report_checks(c, &t);
    report_peak(c, &p, &f, mlp);
    if c.report.deep {
        report_deep(w, c, &t, &f);
    }
    p
}

/// Walks the access units from the start of the file, checking each and filling in the tally and the probe.
fn walk_access_units(w: &mut Window, c: &mut Collector, mlp: bool, p: &mut Probe) -> Tally {
    let mut t = Tally::new();
    let len = w.len;
    let mut pos = 0u64;
    while pos + 4 <= len {
        let head = w.get(pos, 4096.min((len - pos) as usize)).to_vec();
        let words = (((head[0] & 0x0F) as usize) << 8) | head[1] as usize;
        let au_len = words * 2;
        if au_len < 8 {
            t.bad_len += 1;
            c.finding(
                Status::Fail,
                Some(pos),
                format!("access unit length {au_len} is too small"),
            );
            t.sync_loss += 1;
            break;
        }
        if pos + au_len as u64 > len {
            t.trailing = len - pos;
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
        let is_major = au.len() >= 8 && au[4..7] == [0xF8, 0x72, 0x6F] && (au[7] == 0xBA || au[7] == 0xBB);
        if t.aus == 0 {
            t.first_is_major = is_major;
        }
        if check_access_unit(&au, is_major, mlp, pos, c, &mut t) {
            record_access_unit(&au, p, &mut t);
        }
        t.aus += 1;
        pos += au_len as u64;
    }
    t
}

/// Checks one access unit. Returns false when it was too damaged to go on with (its size is then not recorded).
fn check_access_unit(au: &[u8], is_major: bool, mlp: bool, pos: u64, c: &mut Collector, t: &mut Tally) -> bool {
    let mut off = 4usize;
    if is_major {
        t.majors += 1;
        if t.majors > 1 {
            t.intervals.push(t.aus - t.last_major_au);
        }
        t.last_major_au = t.aus;
        match check_major_sync(au, mlp, pos, c, t) {
            Some(end) => off = end,
            None => return false,
        }
    }
    let nsub = t.current.as_ref().map(|m| m.nsub as usize).unwrap_or(0);
    if nsub == 0 || nsub > 4 {
        t.bad_dir += 1;
        c.finding(
            Status::Fail,
            Some(pos),
            "no valid major sync has been seen, the substream layout is unknown",
        );
        return false;
    }
    let dir_start = off;
    let Some(directory) = read_directory(au, off, nsub) else {
        t.bad_dir += 1;
        c.finding(Status::Fail, Some(pos), "substream directory runs past the access unit");
        return false;
    };
    off = directory.end;
    // parity nibble of the access unit header
    let x = parity(&au[..4]) ^ parity(&au[dir_start..off]);
    if ((x >> 4) ^ x) & 0xF != 0xF {
        t.bad_nibble += 1;
        c.finding(Status::Fail, Some(pos), "access unit header parity nibble mismatch");
    }
    if !check_substreams(au, off, &directory, pos, c, t) {
        t.bad_dir += 1;
        c.finding(
            Status::Fail,
            Some(pos),
            "substream sizes do not add up to the access unit",
        );
    }
    let timing = u16::from_be_bytes([au[2], au[3]]);
    if let Some(prev) = t.prev_timing {
        if prev.wrapping_add(t.au_samples as u16) != timing {
            t.timing_jumps += 1;
            if t.timing_jumps <= 3 {
                c.finding(
                    Status::Info,
                    Some(pos),
                    format!("input timing jumps from {prev} to {timing}"),
                );
            }
        }
    }
    t.prev_timing = Some(timing);
    true
}

fn record_access_unit(au: &[u8], p: &mut Probe, t: &mut Tally) {
    let size = au.len() as u32;
    p.frame_bytes.push(size);
    p.frame_samples.push(t.au_samples);
    t.size_min = t.size_min.min(size);
    t.size_max = t.size_max.max(size);
    t.bytes_total += size as u64;
}

/// Reads and checks the major sync of an access unit. Returns where the substream directory starts, or `None` when
/// the sync runs past the access unit.
fn check_major_sync(au: &[u8], mlp: bool, pos: u64, c: &mut Collector, t: &mut Tally) -> Option<usize> {
    let ext = au.len() >= 30 && au[7] == 0xBA && au[4 + 25] & 1 == 1;
    let msize = if ext {
        28 + 2 + (au[4 + 26] >> 4) as usize * 2
    } else {
        28
    };
    let end = 4 + msize;
    if au.len() < end {
        t.bad_major += 1;
        c.finding(Status::Fail, Some(pos), "major sync runs past the access unit");
        return None;
    }
    if !crate::crc::mlp_major_sync_ok(&au[4..end]) {
        t.bad_major += 1;
        c.finding(Status::Fail, Some(pos), "major sync checksum mismatch");
    }
    let mut b = Bits::new(&au[8..end]);
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
        t.bad_major += 1;
        c.finding(Status::Fail, Some(pos), "invalid sampling rate code in the major sync");
    }
    t.au_samples = 40u32 << (ratebits & 7);
    if t.first.is_none() {
        t.first = Some(mj.clone());
    }
    if let Some(prev) = &t.current {
        if prev.rate != mj.rate || prev.nsub != mj.nsub || prev.arr6 != mj.arr6 || prev.arr8 != mj.arr8 {
            t.changes += 1;
            if t.changes <= 5 {
                c.finding(Status::Warn, Some(pos), "stream parameters change at this major sync");
            }
        }
    }
    t.current = Some(mj);
    Some(end)
}

/// The substream directory: where each substream ends, whether it carries check bytes, and where the directory ends.
struct Directory {
    ends: Vec<usize>,
    checked: Vec<bool>,
    end: usize,
}

fn read_directory(au: &[u8], start: usize, nsub: usize) -> Option<Directory> {
    let mut off = start;
    let (mut ends, mut checked) = (Vec::new(), Vec::new());
    for _ in 0..nsub {
        if off + 2 > au.len() {
            break;
        }
        let hdr = u16::from_be_bytes([au[off], au[off + 1]]);
        off += 2;
        if hdr >> 15 == 1 {
            off += 2;
        }
        checked.push(hdr >> 13 & 1 == 1);
        ends.push((hdr & 0x0FFF) as usize * 2);
    }
    (ends.len() == nsub && off <= au.len()).then_some(Directory {
        ends,
        checked,
        end: off,
    })
}

/// Checks the parity and checksum bytes of the substreams that carry them. Returns false when the sizes in the
/// directory do not add up to the access unit.
fn check_substreams(au: &[u8], data_start: usize, dir: &Directory, pos: u64, c: &mut Collector, t: &mut Tally) -> bool {
    let nsub = dir.ends.len();
    let mut start = data_start;
    let mut layout_ok = true;
    for si in 0..nsub {
        let end = data_start + dir.ends[si];
        if end > au.len() || end < start + 2 {
            layout_ok = false;
            break;
        }
        if dir.checked[si] {
            t.checked_subs += 1;
            let sub = &au[start..end];
            let body = &sub[..sub.len() - 2];
            if (sub[sub.len() - 2] ^ parity(body)) != 0xA9 {
                t.bad_par += 1;
                c.finding(
                    Status::Fail,
                    Some(pos + start as u64),
                    format!("substream {si} parity byte mismatch"),
                );
            }
            if !body.is_empty() && sub[sub.len() - 1] != checksum8(body) {
                t.bad_sum += 1;
                c.finding(
                    Status::Fail,
                    Some(pos + start as u64),
                    format!("substream {si} checksum byte mismatch"),
                );
            }
        }
        start = end;
    }
    layout_ok && start == au.len().min(data_start + dir.ends[nsub - 1])
}

/// The facts block: format, rate, channels, substreams, length and bit rates.
fn report_facts(c: &mut Collector, p: &mut Probe, t: &Tally, f: &Major, file_len: u64, mlp: bool) {
    c.kv("Format", if mlp { "MLP" } else { "Dolby TrueHD" }.to_string());
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
            c.check(
                "Dolby Atmos signalling",
                Status::Info,
                "4 substreams with the spatial-presentation flag set; the object substream is not decoded by FFmpeg, \
                 so the audio check covers the 7.1 bed",
            );
        }
    }
    c.kv("Access units", t.aus.to_string());
    c.kv("Duration", format!("{:.3} s", p.duration_secs));
    if p.duration_secs > 0.0 {
        c.kv(
            "Average bit rate",
            format!("{:.1} kbps", file_len as f64 * 8.0 / p.duration_secs / 1000.0),
        );
    }
    c.kv(
        "Peak bit rate (stated)",
        format!("{:.0} kbps", ((f.peak as u64 * f.rate as u64 + 8) >> 4) as f64 / 1000.0),
    );
}

/// Highest data rate a disc allows the stream, in kbps: Blu-ray's 18 Mbit/s for TrueHD, DVD-Audio's 9.6 Mbit/s for MLP.
const TRUEHD_DISC_KBPS: f64 = 18_000.0;
const MLP_DISC_KBPS: f64 = 9_600.0;
/// How far a one-second peak may run over the stated peak before it is flagged: Dolby's encoder states a peak from its
/// own, longer buffer model, which its files exceed by about 1% when measured over one second.
const STATED_SLACK: f64 = 1.05;

/// The measured peak bit rate against the one the major syncs state and the disc limit. A decoder sizes its input
/// buffer by the stated peak, so a stream that runs faster than it says may stutter on strict players. (Like FFmpeg's
/// encoder, Subwoofa states 9.6 Mbit/s; Dolby's states a peak it measures with its own buffer model.)
fn report_peak(c: &mut Collector, p: &Probe, f: &Major, mlp: bool) {
    if f.rate == 0 || p.frame_bytes.is_empty() {
        return;
    }
    let measured = crate::peak_one_second(&p.frame_bytes, &p.frame_samples, f.rate);
    let stated = ((f.peak as u64 * f.rate as u64 + 8) >> 4) as f64 / 1000.0;
    c.kv("Peak bit rate (any 1 s)", format!("{measured:.0} kbps"));
    let (limit, disc) = if mlp {
        (MLP_DISC_KBPS, "DVD-Audio")
    } else {
        (TRUEHD_DISC_KBPS, "Blu-ray")
    };
    let (status, detail) = if measured > limit {
        (
            Status::Warn,
            format!("{measured:.0} kbps in the busiest second, above the {limit:.0} kbps {disc} allows"),
        )
    } else if measured > stated * STATED_SLACK {
        (
            Status::Warn,
            format!("{measured:.0} kbps in the busiest second, above the {stated:.0} kbps the stream states"),
        )
    } else {
        (
            Status::Pass,
            format!("{measured:.0} kbps in the busiest second, in line with the stated {stated:.0} kbps"),
        )
    };
    c.check("Peak data rate", status, detail);
}

/// The pass/fail checks, one line each.
fn report_checks(c: &mut Collector, t: &Tally) {
    let aus = t.aus;
    let ok = |n: u64| if n == 0 { Status::Pass } else { Status::Fail };
    c.check(
        "Access unit framing",
        ok(t.bad_len + t.sync_loss),
        if t.bad_len + t.sync_loss == 0 {
            format!("{aus} access units, each starts where the previous one ends")
        } else {
            "an access unit has an impossible length".to_string()
        },
    );
    c.check(
        "Starts with a major sync",
        if t.first_is_major { Status::Pass } else { Status::Warn },
        if t.first_is_major {
            "decoding can start at the first access unit".to_string()
        } else {
            "the first access unit has no major sync; a player must skip to the next".to_string()
        },
    );
    c.check(
        "Major sync checksums",
        ok(t.bad_major),
        if t.bad_major == 0 {
            format!("{} major syncs", t.majors)
        } else {
            format!("{} bad", t.bad_major)
        },
    );
    c.check(
        "Access unit parity",
        ok(t.bad_nibble),
        if t.bad_nibble == 0 {
            format!("{aus} of {aus} headers")
        } else {
            format!("{} of {aus} headers fail", t.bad_nibble)
        },
    );
    c.check(
        "Substream directory",
        ok(t.bad_dir),
        if t.bad_dir == 0 {
            "sizes add up in every access unit".to_string()
        } else {
            format!("{} access unit(s) with an inconsistent directory", t.bad_dir)
        },
    );
    c.check(
        "Substream parity and checksum",
        ok(t.bad_par + t.bad_sum),
        if t.bad_par + t.bad_sum == 0 {
            format!("{} substreams checked", t.checked_subs)
        } else {
            format!(
                "{} parity and {} checksum errors in {} substreams",
                t.bad_par, t.bad_sum, t.checked_subs
            )
        },
    );
    // Dolby's own encoder lets this timestamp jump (it follows the variable data rate), so it is reported, not judged
    c.check(
        "Input timing",
        if t.timing_jumps == 0 {
            Status::Pass
        } else {
            Status::Info
        },
        if t.timing_jumps == 0 {
            "advances by one access unit each time".to_string()
        } else {
            format!("{} jump(s) (normal for streams from Dolby's encoder)", t.timing_jumps)
        },
    );
    let regular = t.intervals.windows(2).all(|w| w[0] == w[1]) || t.intervals.len() < 2;
    c.check(
        "Restart points",
        Status::Info,
        if t.intervals.is_empty() {
            "a single major sync".to_string()
        } else {
            format!(
                "{} major syncs, every {} access units{}",
                t.majors,
                t.intervals[0],
                if regular { "" } else { " (varies)" }
            )
        },
    );
    c.check(
        "Stable parameters",
        if t.changes == 0 { Status::Pass } else { Status::Warn },
        if t.changes == 0 {
            "rate and channels never change".to_string()
        } else {
            format!("{} change(s)", t.changes)
        },
    );
    c.check(
        "Complete file",
        if t.trailing == 0 { Status::Pass } else { Status::Fail },
        if t.trailing == 0 {
            "the file ends on an access unit boundary".to_string()
        } else {
            format!("{} bytes of an unfinished access unit at the end", t.trailing)
        },
    );
}

/// The `--deep` sections: the first major sync, and the substream analysis of the first restart intervals.
fn report_deep(w: &mut Window, c: &mut Collector, t: &Tally, f: &Major) {
    c.section(
        "Major sync (first)",
        vec![
            ("sample rate".to_string(), format!("{} Hz", f.rate)),
            ("access unit length".to_string(), format!("{} samples", t.au_samples)),
            ("major sync size".to_string(), format!("{} bytes", f.size)),
            ("variable bit rate".to_string(), f.vbr.to_string()),
            ("substreams".to_string(), f.nsub.to_string()),
            ("substream info".to_string(), format!("0x{:02X}", f.info)),
            (
                "access unit size (min / max)".to_string(),
                format!(
                    "{} / {} bytes",
                    if t.size_min == u32::MAX { 0 } else { t.size_min },
                    t.size_max
                ),
            ),
            (
                "average access unit".to_string(),
                format!("{:.0} bytes", t.bytes_total as f64 / t.aus.max(1) as f64),
            ),
        ],
    );
    let data = w.get(0, 64 << 20).to_vec();
    let Ok(analysis) = std::panic::catch_unwind(|| sw_codec_truehd::analyze::analyze_range(&data, 0, 400)) else {
        c.finding(
            Status::Warn,
            None,
            "the substream analysis could not follow this stream (it stopped on an unexpected layout)",
        );
        return;
    };
    let mut rows = vec![(
        "scope".to_string(),
        "the first 400 restart intervals (at most 64 MB)".to_string(),
    )];
    for (i, l) in analysis.summary().lines().enumerate() {
        if !l.trim().is_empty() {
            rows.push((format!("{:02}", i + 1), l.trim().to_string()));
        }
    }
    if !analysis.errors.is_empty() {
        let mut kinds: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for e in &analysis.errors {
            *kinds.entry(e.as_str()).or_default() += 1;
        }
        for (e, n) in kinds.iter().take(5) {
            rows.push(("analysis stopped".to_string(), format!("{e} ({n} times)")));
        }
        // this analyser does not know every coding tool of Dolby's encoder; the decoder check is the verdict
        c.finding(
            Status::Warn,
            None,
            format!(
                "the substream analysis could not follow part of this stream ({} stops, e.g. '{}'); the decoder \
                 check above is the verdict",
                analysis.errors.len(),
                analysis.errors[0]
            ),
        );
    }
    c.section("Substream analysis", rows);
}
