//! Walking a stream frame by frame: finding each core frame and extension substream, checking what can be checked
//! on the way and collecting the facts the report is made of.

use crate::dts::consts::{
    CORE_SYNC, EXSS_SYNC, SYNC_EXSS_CORE, SYNC_LBR, SYNC_X96, SYNC_XBR, SYNC_XCH, SYNC_XLL, SYNC_XXCH,
};
use crate::dts::core_header::{parse_core, Core};
use crate::dts::exss::{parse_exss, Exss};
use crate::dts::xll::{find_x_marker, parse_xll, parse_xll_runs_past, XMark, XllInfo};
use crate::{be32, Collector, Status, Window};

/// Everything a walk of a stream found: counts of frames and of the problems in them, the first header of each
/// kind (for the listings), and per-frame sizes.
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

/// The access unit being collected: its size and length so far.
#[derive(Default)]
struct Unit {
    bytes: u32,
    samples: u32,
    open: bool,
}

/// A walk in progress: the stream, where it is, and the access unit being collected.
struct Walker<'a> {
    w: &'a mut Window,
    c: &'a mut Collector,
    s: Walk,
    pos: u64,
    to: u64,
    /// Remember where every access unit starts (only the container's offset table needs it).
    record: bool,
    unit: Unit,
    /// The previous frame was a core frame: an extension substream after it is part of its access unit.
    prev_was_core: bool,
    /// Samples a lossless frame holds, which sets the length of extension-only access units.
    exss_frame_samples: Option<u32>,
}

/// Walks the stream `from..to` frame by frame.
pub(crate) fn walk(w: &mut Window, from: u64, to: u64, c: &mut Collector, record: bool) -> Walk {
    let mut walker = Walker {
        w,
        c,
        s: Walk::default(),
        pos: from,
        to,
        record,
        unit: Unit::default(),
        prev_was_core: false,
        exss_frame_samples: None,
    };
    while walker.pos + 4 <= walker.to {
        let head = walker.w.get(walker.pos, 64).to_vec();
        if head.len() < 4 {
            break;
        }
        let sync = be32(&head);
        let keep_going = if sync == CORE_SYNC {
            walker.core_frame(&head)
        } else if sync == EXSS_SYNC {
            walker.extension_substream()
        } else if head[0] == 0 && walker.padding() {
            true
        } else {
            walker.junk();
            true
        };
        if !keep_going {
            break;
        }
    }
    walker.close_unit();
    walker.s
}

impl Walker<'_> {
    /// Ends the access unit being collected, if there is one.
    fn close_unit(&mut self) {
        if self.unit.open {
            self.s.frame_bytes.push(self.unit.bytes);
            self.s.frame_samples.push(self.unit.samples);
            self.unit.open = false;
        }
    }

    /// Starts a new access unit at the current position.
    fn open_unit(&mut self, bytes: u32, samples: u32) {
        self.close_unit();
        self.s.aus += 1;
        self.unit = Unit {
            bytes,
            samples,
            open: true,
        };
        if self.record {
            self.s.frame_starts.push(self.pos);
        }
    }

    /// A core frame at the current position. Returns false when the stream ends in the middle of it.
    fn core_frame(&mut self, head: &[u8]) -> bool {
        let pos = self.pos;
        let h = match parse_core(head) {
            Ok(h) => h,
            Err(e) => {
                self.s.core_bad += 1;
                self.c
                    .finding(Status::Fail, Some(pos), format!("core frame header: {e}"));
                self.pos += 4;
                return true;
            }
        };
        if pos + h.fsize as u64 > self.to {
            self.s.trailing = self.to - pos;
            self.c.finding(
                Status::Fail,
                Some(pos),
                format!("core frame is cut short ({} of {} bytes)", self.to - pos, h.fsize),
            );
            return false;
        }
        self.open_unit(h.fsize as u32, h.nblks * 32);
        self.s.cores += 1;
        self.s.saw_core = true;
        self.s.core_samples += h.nblks as u64 * 32;
        if h.crc_present && !h.hcrc_ok {
            self.s.core_hcrc_bad += 1;
        }
        if h.ext_present {
            self.check_core_extension(&h);
        }
        self.note_core_parameters(h.clone());
        self.pos += h.fsize as u64;
        self.prev_was_core = true;
        true
    }

    /// The extension a core header announces (XCH, X96, XXCH) must be in the frame.
    fn check_core_extension(&mut self, h: &Core) {
        let pos = self.pos;
        let frame = self.w.get(pos, h.fsize).to_vec();
        let sync = match h.ext_id {
            0 | 3 => Some((SYNC_XCH, "XCH")),
            2 => Some((SYNC_X96, "X96")),
            6 => Some((SYNC_XXCH, "XXCH")),
            _ => None,
        };
        let Some((word, name)) = sync else { return };
        self.s.ext_checked += 1;
        let needle = word.to_be_bytes();
        let found = frame.windows(4).skip(16).any(|win| win == needle);
        let found = found || (h.ext_id == 3 && frame.windows(4).any(|win| win == SYNC_X96.to_be_bytes()));
        if found {
            self.s.ext_kinds.insert(name);
            return;
        }
        self.s.ext_missing += 1;
        if self.s.ext_missing <= 3 {
            self.c.finding(
                Status::Fail,
                Some(pos),
                format!("the core header announces {name} but its sync word is not in the frame"),
            );
        }
    }

    /// Keeps the first core header, and notes where later ones differ from it.
    fn note_core_parameters(&mut self, h: Core) {
        let Some(first) = &self.s.core_hdr else {
            self.s.core_hdr = Some(h);
            return;
        };
        let changed = first.rate != h.rate
            || first.amode != h.amode
            || first.lff != h.lff
            || first.nblks != h.nblks
            || first.ext_id != h.ext_id
            || first.ext_present != h.ext_present
            || first.pcmr != h.pcmr;
        if changed {
            self.s.core_changes += 1;
            if self.s.core_changes <= 5 {
                self.c.finding(
                    Status::Warn,
                    Some(self.pos),
                    "core parameters change here (rate, channels, frame length or extension)",
                );
            }
        }
    }

    /// An extension substream at the current position. Returns false when the stream ends in the middle of it.
    fn extension_substream(&mut self) -> bool {
        let pos = self.pos;
        let hdr = self.w.get(pos, 8192.min((self.to - pos) as usize)).to_vec();
        let e = match parse_exss(&hdr) {
            Ok(e) => e,
            Err(msg) => {
                self.s.exss_bad += 1;
                self.c
                    .finding(Status::Fail, Some(pos), format!("extension substream header: {msg}"));
                self.pos += 4;
                return true;
            }
        };
        if !e.hdr_crc_ok {
            self.s.exss_hdr_crc_bad += 1;
            self.c
                .finding(Status::Fail, Some(pos), "extension substream header checksum mismatch");
        }
        if pos + e.size as u64 > self.to {
            self.s.trailing = self.to - pos;
            self.c.finding(
                Status::Fail,
                Some(pos),
                format!(
                    "extension substream is cut short ({} of {} bytes)",
                    self.to - pos,
                    e.size
                ),
            );
            return false;
        }
        // a substream after a core belongs to the same access unit; otherwise it starts one
        if !self.prev_was_core {
            self.open_unit(0, 0);
        }
        self.s.exss += 1;
        self.unit.bytes += e.size as u32;
        if e.index == 2 {
            self.s.secondary = true;
        }
        let body = self.w.get(pos, e.size).to_vec();
        self.check_components(&e, &body);
        self.note_extensions(&e);
        // with the peak-rate smoothing buffer a lossless frame is spread over several substreams: only a component
        // that says it holds the start of a frame (sync present) has a header to check, and the frame may run past
        // the end of the component
        if e.ext_mask & 0x200 != 0 && !e.xll_sync_present {
            if self.s.exss_first.is_none() {
                self.s.exss_first = Some(e.clone());
            }
        } else {
            if e.ext_mask & 0x200 != 0 {
                self.check_lossless_frame(&e, &body);
            }
            if self.s.exss_first.is_none() {
                if e.static_fields {
                    self.s.hd_rate = e.max_rate;
                }
                self.s.exss_first = Some(e.clone());
            }
        }
        if !self.prev_was_core {
            self.unit.samples = self.exss_frame_samples.unwrap_or(0);
        }
        self.pos += e.size as u64;
        self.prev_was_core = false;
        true
    }

    /// Each component the substream announces must start with its sync word.
    fn check_components(&mut self, e: &Exss, body: &[u8]) {
        let pos = self.pos;
        let mut check = |present: bool, off: usize, sync: u32, name: &str| {
            if !present {
                return;
            }
            self.s.comp_checked += 1;
            if body.len() >= off + 4 && be32(&body[off..]) == sync {
                return;
            }
            self.s.comp_missing += 1;
            if self.s.comp_missing <= 3 {
                self.c.finding(
                    Status::Fail,
                    Some(pos + off as u64),
                    format!("the {name} component does not start with its sync word"),
                );
            }
        };
        check(e.ext_mask & 0x10 != 0, e.core_offset, SYNC_EXSS_CORE, "core");
        check(e.ext_mask & 0x20 != 0, e.xbr_offset, SYNC_XBR, "XBR");
        check(e.ext_mask & 0x40 != 0, e.xxch_offset, SYNC_XXCH, "XXCH");
        check(e.ext_mask & 0x80 != 0, e.x96_offset, SYNC_X96, "X96");
        check(e.ext_mask & 0x100 != 0, e.lbr_offset, SYNC_LBR, "LBR");
    }

    /// Which kinds of extension the stream has.
    fn note_extensions(&mut self, e: &Exss) {
        if e.ext_mask & 0x40 != 0 {
            self.s.ext_kinds.insert("XXCH");
        }
        if e.ext_mask & 0x20 != 0 {
            self.s.ext_kinds.insert("XBR");
        }
        if e.ext_mask & 0x80 != 0 {
            self.s.ext_kinds.insert("X96");
        }
        if e.ext_mask & 0x100 != 0 {
            self.s.saw_lbr = true;
        }
        if e.ext_mask & 0x200 != 0 {
            self.s.saw_xll = true;
        }
    }

    /// The lossless (XLL) frame in the substream's body: its headers and checksums, and what its channel sets use.
    fn check_lossless_frame(&mut self, e: &Exss, body: &[u8]) {
        let pos = self.pos;
        let xoff = e.xll_offset + e.xll_sync_offset;
        self.s.xll_checked += 1;
        match body.get(xoff..).map(parse_xll) {
            Some(Ok(x)) => {
                if !x.common_crc_ok {
                    self.s.xll_bad += 1;
                    self.c.finding(
                        Status::Fail,
                        Some(pos + xoff as u64),
                        "XLL common header checksum mismatch",
                    );
                }
                if x.chset_header_bad > 0 {
                    self.s.xll_chset_bad += 1;
                    self.c.finding(
                        Status::Fail,
                        Some(pos + xoff as u64),
                        format!("{} XLL channel set header checksum(s) mismatch", x.chset_header_bad),
                    );
                }
                if e.xll_pbr == 0 && x.frame_size > e.xll_size {
                    self.s.xll_bad += 1;
                    self.c.finding(
                        Status::Fail,
                        Some(pos + xoff as u64),
                        format!(
                            "XLL frame ({} bytes) is larger than its component ({} bytes)",
                            x.frame_size, e.xll_size
                        ),
                    );
                }
                self.tally_lossless_frame(&x);
                if let Some(frame) = body.get(xoff..xoff + x.frame_size.min(body.len() - xoff)) {
                    match find_x_marker(frame) {
                        Some(XMark::X) => self.s.x_frames += 1,
                        Some(XMark::Imax) => self.s.imax_frames += 1,
                        None => {}
                    }
                }
                // lossless-only frames set the access unit length
                if self.exss_frame_samples.is_none() {
                    self.exss_frame_samples = Some(x.nsegs * x.seg_samples);
                }
                if self.s.xll_first.is_none() {
                    self.s.xll_first = Some(x);
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
                self.s.xll_bad += 1;
                let avail = body.len().saturating_sub(xoff);
                let first = body
                    .get(xoff..xoff + 4.min(avail))
                    .map(|b| b.iter().map(|x| format!("{x:02X}")).collect::<String>())
                    .unwrap_or_default();
                self.c.finding(Status::Fail, Some(pos + xoff as u64), format!("XLL frame: {msg} ({avail} bytes left in the substream, first word {first}, sync offset {}, component {} bytes at {})", e.xll_sync_offset, e.xll_size, e.xll_offset));
            }
            None => {
                self.s.xll_bad += 1;
                self.c.finding(
                    Status::Fail,
                    Some(pos + xoff as u64),
                    "the XLL component lies outside the substream",
                );
            }
        }
    }

    /// Adds a lossless frame to the totals: bytes per band, and how the predictors were used.
    fn tally_lossless_frame(&mut self, x: &XllInfo) {
        let s = &mut self.s;
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
            for (i, band) in x.bands.iter().enumerate() {
                for a in &band.adapt {
                    s.pred_adapt[i][(*a as usize).min(15)] += 1;
                }
                for f in &band.fixed {
                    s.pred_fixed[i][(*f as usize).min(3)] += 1;
                }
                s.pred_decor[i] += u64::from(band.decor);
            }
        }
        for (total, bytes) in s.band_total.iter_mut().zip(&x.band_bytes) {
            *total += *bytes as u64;
        }
    }

    /// Zero bytes up to a word boundary between frames. Returns whether there were any.
    fn padding(&mut self) -> bool {
        let mut n = 0u64;
        while self.pos + n < self.to {
            let chunk = self.w.get(self.pos + n, 4096);
            if chunk.is_empty() {
                break;
            }
            let zeros = chunk.iter().take_while(|b| **b == 0).count();
            n += zeros as u64;
            if zeros < chunk.len() {
                break;
            }
        }
        if n == 0 {
            return false;
        }
        self.s.pad += n;
        self.unit.bytes += n as u32;
        self.pos += n;
        true
    }

    /// Bytes that are not a frame: skips ahead to the next sync word.
    fn junk(&mut self) {
        let pos = self.pos;
        self.s.junk_events += 1;
        let mut q = pos + 1;
        let mut found = None;
        'find: while q < self.to && q < pos + (1 << 20) {
            let chunk = self.w.get(q, 1 << 16).to_vec();
            if chunk.len() < 4 {
                break;
            }
            for i in 0..chunk.len() - 3 {
                let word = be32(&chunk[i..]);
                if word == CORE_SYNC || word == EXSS_SYNC {
                    found = Some(q + i as u64);
                    break 'find;
                }
            }
            q += chunk.len() as u64 - 3;
        }
        let end = found.unwrap_or(self.to.min(pos + (1 << 20)));
        self.s.junk += end - pos;
        self.c.finding(
            Status::Fail,
            Some(pos),
            format!("{} bytes that are not a DTS frame", end - pos),
        );
        self.pos = end;
        self.prev_was_core = false;
    }
}
