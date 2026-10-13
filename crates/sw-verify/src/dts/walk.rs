//! Walking a stream frame by frame: finding each core frame and extension substream, checking what can be checked
//! on the way and collecting the facts the report is made of.

use crate::dts::consts::{
    CORE_SYNC, EXSS_SYNC, SYNC_EXSS_CORE, SYNC_LBR, SYNC_X96, SYNC_XBR, SYNC_XCH, SYNC_XLL, SYNC_XXCH,
};
use crate::dts::core_header::{parse_core, Core};
use crate::dts::exss::{parse_exss, Exss};
use crate::dts::xll::{find_x_marker, parse_xll, parse_xll_runs_past, XMark, XllInfo};
use crate::{be32, Collector, Status, Window};

#[derive(Default)]
pub(crate) struct Walk {
    pub(crate) aus: u64,
    pub(crate) cores: u64,
    pub(crate) exss: u64,
    pub(crate) core_hdr: Option<Core>,
    pub(crate) core_changes: u64,
    pub(crate) core_bad: u64,
    pub(crate) core_hcrc_bad: u64,
    pub(crate) ext_missing: u64,
    pub(crate) ext_checked: u64,
    pub(crate) exss_first: Option<Exss>,
    pub(crate) exss_bad: u64,
    pub(crate) exss_hdr_crc_bad: u64,
    pub(crate) comp_missing: u64,
    pub(crate) comp_checked: u64,
    pub(crate) xll_first: Option<XllInfo>,
    pub(crate) xll_bad: u64,
    pub(crate) xll_checked: u64,
    pub(crate) band_total: Vec<u64>,
    /// per band: adaptive predictor order histogram (0..=15), fixed order histogram (0..=3), frames with decorrelation
    pub(crate) pred_adapt: Vec<[u64; 16]>,
    pub(crate) pred_fixed: Vec<[u64; 4]>,
    pub(crate) pred_decor: Vec<u64>,
    pub(crate) pred_frames: u64,
    pub(crate) x_frames: u64,
    pub(crate) imax_frames: u64,
    pub(crate) xll_chset_bad: u64,
    pub(crate) junk: u64,
    pub(crate) junk_events: u64,
    pub(crate) pad: u64,
    pub(crate) core_samples: u64,
    pub(crate) frame_bytes: Vec<u32>,
    pub(crate) frame_samples: Vec<u32>,
    pub(crate) frame_starts: Vec<u64>,
    pub(crate) hd_rate: u32,
    pub(crate) trailing: u64,
    pub(crate) saw_core: bool,
    pub(crate) saw_xll: bool,
    pub(crate) saw_lbr: bool,
    pub(crate) ext_kinds: std::collections::BTreeSet<&'static str>,
    pub(crate) secondary: bool,
}

pub(crate) fn walk(w: &mut Window, from: u64, to: u64, c: &mut Collector, record: bool) -> Walk {
    let mut s = Walk::default();
    let mut pos = from;
    let mut prev_was_core = false;
    let mut unit_bytes = 0u32;
    let mut unit_samples = 0u32;
    let mut have_unit = false;
    let mut exss_frame_samples: Option<u32> = None;
    let flush = |s: &mut Walk, have: &mut bool, bytes: &mut u32, samples: &mut u32| {
        if *have {
            s.frame_bytes.push(*bytes);
            s.frame_samples.push(*samples);
            *have = false;
        }
    };
    while pos + 4 <= to {
        let head = w.get(pos, 64).to_vec();
        if head.len() < 4 {
            break;
        }
        let sync = be32(&head);
        if sync == CORE_SYNC {
            let h = match parse_core(&head) {
                Ok(h) => h,
                Err(e) => {
                    s.core_bad += 1;
                    c.finding(Status::Fail, Some(pos), format!("core frame header: {e}"));
                    pos += 4;
                    continue;
                }
            };
            if pos + h.fsize as u64 > to {
                s.trailing = to - pos;
                c.finding(
                    Status::Fail,
                    Some(pos),
                    format!("core frame is cut short ({} of {} bytes)", to - pos, h.fsize),
                );
                break;
            }
            flush(&mut s, &mut have_unit, &mut unit_bytes, &mut unit_samples);
            s.aus += 1;
            s.cores += 1;
            s.saw_core = true;
            have_unit = true;
            unit_bytes = h.fsize as u32;
            unit_samples = h.nblks
                * 32
                * if h.ext_present && (h.ext_id == 2 || h.ext_id == 3) {
                    1
                } else {
                    1
                };
            if record {
                s.frame_starts.push(pos);
            }
            s.core_samples += h.nblks as u64 * 32;
            if h.crc_present && !h.hcrc_ok {
                s.core_hcrc_bad += 1;
            }
            // extension inside the core
            if h.ext_present {
                let frame = w.get(pos, h.fsize).to_vec();
                let sync = match h.ext_id {
                    0 | 3 => Some((SYNC_XCH, "XCH")),
                    2 => Some((SYNC_X96, "X96")),
                    6 => Some((SYNC_XXCH, "XXCH")),
                    _ => None,
                };
                if let Some((word, name)) = sync {
                    s.ext_checked += 1;
                    let needle = word.to_be_bytes();
                    let found = frame.windows(4).skip(16).any(|win| win == needle);
                    let found = found || (h.ext_id == 3 && frame.windows(4).any(|win| win == SYNC_X96.to_be_bytes()));
                    if !found {
                        s.ext_missing += 1;
                        if s.ext_missing <= 3 {
                            c.finding(
                                Status::Fail,
                                Some(pos),
                                format!("the core header announces {name} but its sync word is not in the frame"),
                            );
                        }
                    } else {
                        s.ext_kinds.insert(name);
                    }
                }
            }
            match &s.core_hdr {
                None => s.core_hdr = Some(h.clone()),
                Some(f) => {
                    if f.rate != h.rate
                        || f.amode != h.amode
                        || f.lff != h.lff
                        || f.nblks != h.nblks
                        || f.ext_id != h.ext_id
                        || f.ext_present != h.ext_present
                        || f.pcmr != h.pcmr
                    {
                        s.core_changes += 1;
                        if s.core_changes <= 5 {
                            c.finding(
                                Status::Warn,
                                Some(pos),
                                "core parameters change here (rate, channels, frame length or extension)",
                            );
                        }
                    }
                }
            }
            pos += h.fsize as u64;
            prev_was_core = true;
            continue;
        }
        if sync == EXSS_SYNC {
            let hdr = w.get(pos, 8192.min((to - pos) as usize)).to_vec();
            let e = match parse_exss(&hdr) {
                Ok(e) => e,
                Err(msg) => {
                    s.exss_bad += 1;
                    c.finding(Status::Fail, Some(pos), format!("extension substream header: {msg}"));
                    pos += 4;
                    continue;
                }
            };
            if !e.hdr_crc_ok {
                s.exss_hdr_crc_bad += 1;
                c.finding(Status::Fail, Some(pos), "extension substream header checksum mismatch");
            }
            if pos + e.size as u64 > to {
                s.trailing = to - pos;
                c.finding(
                    Status::Fail,
                    Some(pos),
                    format!("extension substream is cut short ({} of {} bytes)", to - pos, e.size),
                );
                break;
            }
            // a substream after a core belongs to the same access unit; otherwise it starts one
            if !prev_was_core {
                flush(&mut s, &mut have_unit, &mut unit_bytes, &mut unit_samples);
                s.aus += 1;
                have_unit = true;
                unit_bytes = 0;
                unit_samples = 0;
                if record {
                    s.frame_starts.push(pos);
                }
            }
            s.exss += 1;
            unit_bytes += e.size as u32;
            if e.index == 2 {
                s.secondary = true;
            }
            // components
            let body = w.get(pos, e.size).to_vec();
            let check =
                |s: &mut Walk, c: &mut Collector, present: bool, off: usize, sync: u32, name: &str, extra: usize| {
                    if !present {
                        return;
                    }
                    s.comp_checked += 1;
                    let at = off + extra;
                    if body.len() >= at + 4 && be32(&body[at..]) == sync {
                        return;
                    }
                    s.comp_missing += 1;
                    if s.comp_missing <= 3 {
                        c.finding(
                            Status::Fail,
                            Some(pos + at as u64),
                            format!("the {name} component does not start with its sync word"),
                        );
                    }
                };
            check(
                &mut s,
                c,
                e.ext_mask & 0x10 != 0,
                e.core_offset,
                SYNC_EXSS_CORE,
                "core",
                0,
            );
            check(&mut s, c, e.ext_mask & 0x20 != 0, e.xbr_offset, SYNC_XBR, "XBR", 0);
            check(&mut s, c, e.ext_mask & 0x40 != 0, e.xxch_offset, SYNC_XXCH, "XXCH", 0);
            check(&mut s, c, e.ext_mask & 0x80 != 0, e.x96_offset, SYNC_X96, "X96", 0);
            check(&mut s, c, e.ext_mask & 0x100 != 0, e.lbr_offset, SYNC_LBR, "LBR", 0);
            if e.ext_mask & 0x40 != 0 {
                s.ext_kinds.insert("XXCH");
            }
            if e.ext_mask & 0x20 != 0 {
                s.ext_kinds.insert("XBR");
            }
            if e.ext_mask & 0x80 != 0 {
                s.ext_kinds.insert("X96");
            }
            if e.ext_mask & 0x100 != 0 {
                s.saw_lbr = true;
            }
            if e.ext_mask & 0x200 != 0 {
                s.saw_xll = true;
                // with the peak-rate smoothing buffer a lossless frame is spread over several substreams: only a
                // component that says it holds the start of a frame (sync present) has a header to check, and the
                // frame may run past the end of the component
                if !e.xll_sync_present {
                    if s.exss_first.is_none() {
                        s.exss_first = Some(e.clone());
                    }
                    if !prev_was_core {
                        unit_samples = exss_frame_samples.unwrap_or(0);
                    }
                    pos += e.size as u64;
                    prev_was_core = false;
                    continue;
                }
                let xoff = e.xll_offset + e.xll_sync_offset;
                s.xll_checked += 1;
                match body.get(xoff..).map(parse_xll) {
                    Some(Ok(x)) => {
                        if !x.common_crc_ok {
                            s.xll_bad += 1;
                            c.finding(
                                Status::Fail,
                                Some(pos + xoff as u64),
                                "XLL common header checksum mismatch",
                            );
                        }
                        if x.chset_header_bad > 0 {
                            s.xll_chset_bad += 1;
                            c.finding(
                                Status::Fail,
                                Some(pos + xoff as u64),
                                format!("{} XLL channel set header checksum(s) mismatch", x.chset_header_bad),
                            );
                        }
                        if e.xll_pbr == 0 && x.frame_size > e.xll_size {
                            s.xll_bad += 1;
                            c.finding(
                                Status::Fail,
                                Some(pos + xoff as u64),
                                format!(
                                    "XLL frame ({} bytes) is larger than its component ({} bytes)",
                                    x.frame_size, e.xll_size
                                ),
                            );
                        }
                        if s.band_total.len() < x.band_bytes.len() {
                            s.band_total.resize(x.band_bytes.len(), 0);
                        }
                        if !x.bands.is_empty() {
                            s.pred_frames += 1;
                            if s.pred_adapt.len() < x.bands.len() {
                                s.pred_adapt.resize(x.bands.len(), [0; 16]);
                                s.pred_fixed.resize(x.bands.len(), [0; 4]);
                                s.pred_decor.resize(x.bands.len(), 0);
                            }
                            for (i, bh) in x.bands.iter().enumerate() {
                                for a in &bh.adapt {
                                    s.pred_adapt[i][(*a as usize).min(15)] += 1;
                                }
                                for f in &bh.fixed {
                                    s.pred_fixed[i][(*f as usize).min(3)] += 1;
                                }
                                s.pred_decor[i] += u64::from(bh.decor);
                            }
                        }
                        for (t, b) in s.band_total.iter_mut().zip(&x.band_bytes) {
                            *t += *b as u64;
                        }
                        if let Some(frame) = body.get(xoff..xoff + x.frame_size.min(body.len() - xoff)) {
                            match find_x_marker(frame) {
                                Some(XMark::X) => s.x_frames += 1,
                                Some(XMark::Imax) => s.imax_frames += 1,
                                None => {}
                            }
                        }
                        // lossless-only frames set the access unit length
                        if exss_frame_samples.is_none() {
                            exss_frame_samples = Some(x.nsegs * x.seg_samples);
                        }
                        if s.xll_first.is_none() {
                            s.xll_first = Some(x);
                        }
                    }
                    Some(Err(_))
                        if body.len().saturating_sub(xoff) >= 4
                            && be32(&body[xoff..]) == SYNC_XLL
                            && (body.len() - xoff < 16 || parse_xll_runs_past(&body[xoff..])) =>
                    {
                        // the frame starts here with its sync word, but its header continues in the next substream
                    }
                    Some(Err(msg)) => {
                        s.xll_bad += 1;
                        let avail = body.len().saturating_sub(xoff);
                        let first = body
                            .get(xoff..xoff + 4.min(avail))
                            .map(|b| b.iter().map(|x| format!("{x:02X}")).collect::<String>())
                            .unwrap_or_default();
                        c.finding(Status::Fail, Some(pos + xoff as u64), format!("XLL frame: {msg} ({avail} bytes left in the substream, first word {first}, sync offset {}, component {} bytes at {})", e.xll_sync_offset, e.xll_size, e.xll_offset));
                    }
                    None => {
                        s.xll_bad += 1;
                        c.finding(
                            Status::Fail,
                            Some(pos + xoff as u64),
                            "the XLL component lies outside the substream",
                        );
                    }
                }
            }
            if s.exss_first.is_none() {
                if e.static_fields {
                    s.hd_rate = e.max_rate;
                }
                s.exss_first = Some(e.clone());
            }
            if !prev_was_core {
                unit_samples = exss_frame_samples.unwrap_or(0);
            }
            pos += e.size as u64;
            prev_was_core = false;
            // keep access-unit grouping: the next substream may still belong to this unit only after a core
            continue;
        }
        // padding (zero bytes up to a word boundary)
        if head[0] == 0 {
            let mut n = 0u64;
            while pos + n < to {
                let chunk = w.get(pos + n, 4096);
                if chunk.is_empty() {
                    break;
                }
                let z = chunk.iter().take_while(|b| **b == 0).count();
                n += z as u64;
                if z < chunk.len() {
                    break;
                }
            }
            if n > 0 {
                s.pad += n;
                unit_bytes += n as u32;
                pos += n;
                continue;
            }
        }
        // junk: find the next sync word
        s.junk_events += 1;
        let mut q = pos + 1;
        let mut found = None;
        'find: while q < to && q < pos + (1 << 20) {
            let chunk = w.get(q, 1 << 16).to_vec();
            if chunk.len() < 4 {
                break;
            }
            for i in 0..chunk.len() - 3 {
                let wd = be32(&chunk[i..]);
                if wd == CORE_SYNC || wd == EXSS_SYNC {
                    found = Some(q + i as u64);
                    break 'find;
                }
            }
            q += chunk.len() as u64 - 3;
        }
        let end = found.unwrap_or(to.min(pos + (1 << 20)));
        s.junk += end - pos;
        c.finding(
            Status::Fail,
            Some(pos),
            format!("{} bytes that are not a DTS frame", end - pos),
        );
        pos = end;
        prev_was_core = false;
    }
    flush(&mut s, &mut have_unit, &mut unit_bytes, &mut unit_samples);
    s
}
