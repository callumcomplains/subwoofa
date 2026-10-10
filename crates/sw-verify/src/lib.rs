//! Native stream verification: reads an encoded file the way a decoder would parse it and reports what is wrong
//! with it, without needing any other program.
//!
//! * **Simple** verification checks the structure of the whole file: sync words, frame sizes and continuity,
//!   checksums, header fields against each other and against the container, duration.
//! * **Deep** verification adds everything that needs the audio: a full decode (with FFmpeg when it is found),
//!   statistics of the decoded signal, bit-rate statistics, the commercial verifier when it is installed, and a
//!   dump of every header the stream carries.

mod ac3;
mod bits;
mod crc;
mod deep;
mod dts;
mod thd;

use serde::Serialize;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pass,
    Warn,
    Fail,
    Skipped,
    Info,
}

/// One named check and how it came out.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: String,
    pub status: Status,
    pub detail: String,
}

/// A problem found at a place in the file.
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub status: Status,
    pub message: String,
    pub offset: Option<u64>,
}

/// A titled list of facts (a header dump, statistics...).
#[derive(Debug, Clone, Serialize)]
pub struct Section {
    pub title: String,
    pub rows: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    pub file: String,
    pub bytes: u64,
    pub format: String,
    pub deep: bool,
    pub passed: bool,
    pub duration_secs: f64,
    pub summary: Vec<(String, String)>,
    pub checks: Vec<Check>,
    pub findings: Vec<Finding>,
    /// Findings beyond the ones listed (the list is capped).
    pub findings_omitted: u64,
    pub sections: Vec<Section>,
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub deep: bool,
    /// FFmpeg, for the decode of a deep verification (looked up on the path when `None`).
    pub ffmpeg: Option<PathBuf>,
    /// DTS's own `DTSHDVerify`, used as a second opinion in a deep verification when present.
    pub reference_verifier: Option<PathBuf>,
}

const MAX_FINDINGS: usize = 200;

/// Collects the outcome of a verification.
pub(crate) struct Collector {
    pub report: Report,
}

impl Collector {
    fn new(path: &Path, bytes: u64, deep: bool) -> Collector {
        Collector { report: Report { file: path.display().to_string(), bytes, deep, ..Default::default() } }
    }
    pub fn kv(&mut self, k: &str, v: impl Into<String>) {
        self.report.summary.push((k.to_string(), v.into()));
    }
    pub fn check(&mut self, name: &str, status: Status, detail: impl Into<String>) {
        self.report.checks.push(Check { name: name.to_string(), status, detail: detail.into() });
    }
    pub fn finding(&mut self, status: Status, offset: Option<u64>, message: impl Into<String>) {
        if self.report.findings.len() < MAX_FINDINGS {
            self.report.findings.push(Finding { status, message: message.into(), offset });
        } else {
            self.report.findings_omitted += 1;
        }
    }
    pub fn section(&mut self, title: &str, rows: Vec<(String, String)>) {
        self.report.sections.push(Section { title: title.to_string(), rows });
    }
    fn finish(mut self) -> Report {
        self.report.passed = !self.report.checks.iter().any(|c| c.status == Status::Fail) && !self.report.findings.iter().any(|f| f.status == Status::Fail);
        self.report
    }
}

/// Random access to a file through a window, so a film-length stream never has to be in memory.
pub(crate) struct Window {
    f: File,
    pub len: u64,
    buf: Vec<u8>,
    base: u64,
}

impl Window {
    fn open(path: &Path) -> std::io::Result<Window> {
        let f = File::open(path)?;
        let len = f.metadata()?.len();
        Ok(Window { f, len, buf: Vec::new(), base: 0 })
    }
    /// `n` bytes at `pos` (fewer at the end of the file).
    pub fn get(&mut self, pos: u64, n: usize) -> &[u8] {
        let end = (pos + n as u64).min(self.len);
        if pos >= self.base && end <= self.base + self.buf.len() as u64 && !self.buf.is_empty() {
            let a = (pos - self.base) as usize;
            return &self.buf[a..a + (end - pos) as usize];
        }
        let want = n.max(1 << 20);
        let size = (want as u64).min(self.len.saturating_sub(pos)) as usize;
        self.buf.resize(size, 0);
        let ok = self.f.seek(SeekFrom::Start(pos)).and_then(|_| self.f.read_exact(&mut self.buf)).is_ok();
        if !ok {
            self.buf.clear();
        }
        self.base = pos;
        let end = (pos + n as u64).min(self.base + self.buf.len() as u64);
        &self.buf[..(end - pos) as usize]
    }
}

pub(crate) fn be32(d: &[u8]) -> u32 {
    u32::from_be_bytes([d[0], d[1], d[2], d[3]])
}

/// Verify a file. Simple verification reads it once; deep verification also decodes it.
pub fn verify(path: &Path, opt: &Options) -> Result<Report, Error> {
    let mut w = Window::open(path)?;
    let mut c = Collector::new(path, w.len, opt.deep);
    let head: Vec<u8> = w.get(0, 64).to_vec();
    let kind = sniff(&head);
    c.report.format = kind.name().to_string();
    let mut probe = None;
    match kind {
        Kind::Ac3 | Kind::Eac3 => probe = Some(ac3::verify(&mut w, &mut c, kind == Kind::Eac3)),
        Kind::Dts | Kind::DtsHd => probe = Some(dts::verify(&mut w, &mut c, kind == Kind::DtsHd)),
        Kind::TrueHd | Kind::Mlp => probe = Some(thd::verify(&mut w, &mut c, kind == Kind::Mlp)),
        Kind::DtsPacked => {
            c.check("Recognised stream", Status::Pass, "DTS packed into 14-bit or little-endian 16-bit words (as on a DTS CD or in a WAV file)");
            c.check("Frame structure", Status::Skipped, "the structure check reads the plain byte-stream form; unpack the stream first, or use deep verification, which decodes it");
        }
        Kind::Unknown => {
            c.check("Recognised stream", Status::Fail, "the file does not start with a known sync word (AC-3, E-AC-3, DTS, DTS-HD, TrueHD, MLP)");
        }
    }
    if let Some(p) = &probe {
        c.report.duration_secs = p.duration_secs;
    }
    if opt.deep {
        deep::run(path, opt, &mut c, probe.as_ref());
    }
    Ok(c.finish())
}

/// What the structure pass learned that the deep pass can use.
#[derive(Debug, Clone, Default)]
pub(crate) struct Probe {
    pub duration_secs: f64,
    pub sample_rate: u32,
    pub channels: u32,
    /// Bytes and samples of every access unit, in order.
    pub frame_bytes: Vec<u32>,
    pub frame_samples: Vec<u32>,
    /// A lossless stream: the decoded audio is the original, so its MD5 is the result.
    pub lossless: bool,
    /// The stream has a DTS core under its extension (so a core-only decode is possible).
    pub has_core: bool,
    /// Sample rate of the core (differs from `sample_rate` when the lossless data runs at twice the core's rate).
    pub core_rate: u32,
    pub dts: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Ac3,
    Eac3,
    Dts,
    DtsHd,
    /// DTS stored as 16-bit little-endian words or 14-bit words (DTS CD / DTS-in-WAV packing).
    DtsPacked,
    TrueHd,
    Mlp,
    Unknown,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Ac3 => "Dolby Digital (AC-3)",
            Kind::Eac3 => "Dolby Digital Plus (E-AC-3)",
            Kind::Dts => "DTS elementary stream",
            Kind::DtsHd => "DTS-HD (.dtshd container)",
            Kind::DtsPacked => "DTS in 14-bit or little-endian words",
            Kind::TrueHd => "Dolby TrueHD",
            Kind::Mlp => "MLP (Meridian Lossless Packing)",
            Kind::Unknown => "unknown",
        }
    }
}

fn sniff(h: &[u8]) -> Kind {
    if h.len() >= 8 {
        if h.starts_with(b"DTSHDHDR") {
            return Kind::DtsHd;
        }
        if h[4..8] == [0xF8, 0x72, 0x6F, 0xBA] {
            return Kind::TrueHd;
        }
        if h[4..8] == [0xF8, 0x72, 0x6F, 0xBB] {
            return Kind::Mlp;
        }
    }
    if h.len() >= 4 {
        let w = be32(h);
        if w == 0x7FFE_8001 || w == 0x6458_2025 {
            return Kind::Dts;
        }
        if w == 0xFE7F_0180 || w == 0x1FFF_E800 || w == 0xFF1F_00E8 || w == 0x7FFE_0180 {
            return Kind::DtsPacked;
        }
    }
    if h.len() >= 6 && h[0] == 0x0B && h[1] == 0x77 {
        // both syntaxes carry the bit stream id in the top five bits of byte 5
        let bsid = h[5] >> 3;
        return if bsid > 10 { Kind::Eac3 } else { Kind::Ac3 };
    }
    Kind::Unknown
}

impl Report {
    /// The report as plain text (what the command line prints).
    pub fn to_text(&self) -> String {
        let mut o = String::new();
        let mark = |s: Status| match s {
            Status::Pass => "[ OK ]",
            Status::Warn => "[WARN]",
            Status::Fail => "[FAIL]",
            Status::Skipped => "[skip]",
            Status::Info => "[info]",
        };
        o.push_str(&format!("{}\n{} - {} bytes{}\n\n", self.file, self.format, self.bytes, if self.deep { " - deep verification" } else { "" }));
        for (k, v) in &self.summary {
            o.push_str(&format!("  {k:<32} {v}\n"));
        }
        o.push_str("\nChecks\n");
        for c in &self.checks {
            o.push_str(&format!("  {} {:<38} {}\n", mark(c.status), c.name, c.detail));
        }
        if !self.findings.is_empty() {
            o.push_str("\nFindings\n");
            for f in &self.findings {
                let at = f.offset.map(|a| format!(" at byte {a}")).unwrap_or_default();
                o.push_str(&format!("  {} {}{}\n", mark(f.status), f.message, at));
            }
            if self.findings_omitted > 0 {
                o.push_str(&format!("  ... and {} more\n", self.findings_omitted));
            }
        }
        for s in &self.sections {
            o.push_str(&format!("\n{}\n", s.title));
            for (k, v) in &s.rows {
                o.push_str(&format!("  {k:<38} {v}\n"));
            }
        }
        o.push_str(&format!("\n{}\n", if self.passed { "PASSED: the stream shows no errors." } else { "FAILED: see the checks and findings above." }));
        o
    }
}
