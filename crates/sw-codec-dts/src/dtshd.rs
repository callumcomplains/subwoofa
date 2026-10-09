//! The `.dtshd` file container written around a DTS-HD stream.
//!
//! Layout, as the commercial encoder writes it (checked against its output for core-only, MA and Express
//! streams and against StreamTools, which refuses files without these chunks):
//!
//! ```text
//! DTSHDHDR  start time (samples at 48 kHz), frame rate code, which substreams are present
//! CORESSMD  core sample rate, bit rate, channel mask, frame payload       (when there is a core)
//! EXTSS_MD  extension substream average / peak bit rate                    (when there is one)
//! AUPR-HDR  audio presentation: rate, frame count, samples, channel mask
//! STRMDATA  the stream, every frame a whole number of 32-bit words
//! NAVI-TBL  start offset of every frame                                    (variable rate streams)
//! TIMECODE  clock, frame rate, start and reference time
//! BUILDVER  text, FILEINFO  flags
//! BLACKOUT  one silent frame (what StreamTools pastes for "add silence")  (when supplied)
//! ```

use std::io::{Seek, SeekFrom, Write};

/// Frame rate codes of the TIMECODE chunk.
pub const FPS_23_976: u8 = 1;
pub const FPS_24: u8 = 2;
pub const FPS_25: u8 = 3;
pub const FPS_29_97_DROP: u8 = 4;
pub const FPS_29_97: u8 = 5;
pub const FPS_30_DROP: u8 = 6;
pub const FPS_30: u8 = 7;

/// Which substreams the stream carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Core and extension substream (MA, HRA).
    CoreAndExtension,
    /// Core only.
    CoreOnly,
    /// Extension substream only, constant rate (Express).
    ExtensionOnly,
}

/// The rate a timecode frame runs at, as `numerator / denominator` frames per second (and whether the
/// counting drops frame numbers).
pub fn fps_exact(code: u8) -> (u64, u64) {
    match code {
        FPS_23_976 => (24_000, 1001),
        FPS_25 => (25, 1),
        FPS_29_97 | FPS_29_97_DROP => (30_000, 1001),
        FPS_30 | FPS_30_DROP => (30, 1),
        _ => (24, 1),
    }
}

/// Frame count of a `hh:mm:ss:ff` timecode (nominal frames per second, dropping numbers for the drop-frame
/// rates: two at the start of every minute except each tenth).
pub fn timecode_frames(code: u8, h: u32, m: u32, s: u32, f: u32) -> u64 {
    let nominal: u64 = match code {
        FPS_23_976 | FPS_24 => 24,
        FPS_25 => 25,
        _ => 30,
    };
    let total_minutes = (h * 60 + m) as u64;
    let frames = ((total_minutes * 60 + s as u64) * nominal) + f as u64;
    if code == FPS_29_97_DROP || code == FPS_30_DROP {
        frames - 2 * (total_minutes - total_minutes / 10)
    } else {
        frames
    }
}

/// A timecode position as the file stores it: whole samples at 48 kHz plus a remainder in units of
/// 1/(frame rate numerator) sample.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Position {
    pub samples: u32,
    pub fraction: u32,
}

/// Position of a frame count at a frame rate code.
pub fn position_of(code: u8, frames: u64) -> Position {
    let (num, den) = fps_exact(code);
    let scaled = frames as u128 * 48_000 * den as u128;
    Position { samples: (scaled / num as u128) as u32, fraction: (scaled % num as u128) as u32 }
}

#[derive(Debug, Clone)]
pub struct DtsHdInfo {
    pub sample_rate: u32,
    pub samples_per_frame: u16,
    /// Samples per channel in the programme.
    pub total_samples: u64,
    pub ch_mask: u16,
    pub kind: Kind,
    /// Core bit rate (kbit/s) and frame bytes, for CORESSMD.
    pub core_kbps: u32,
    pub core_payload: u32,
    /// Variable rate stream: write the frame offset table.
    pub vbr: bool,
    pub fps_code: u8,
    pub start: Position,
}

impl DtsHdInfo {
    pub fn new(sample_rate: u32, samples_per_frame: u16, total_samples: u64, ch_mask: u16, kind: Kind) -> DtsHdInfo {
        DtsHdInfo { sample_rate, samples_per_frame, total_samples, ch_mask, kind, core_kbps: 0, core_payload: 0, vbr: false, fps_code: FPS_24, start: Position::default() }
    }
}

fn chunk(out: &mut Vec<u8>, name: &[u8; 8], body: &[u8]) {
    out.extend_from_slice(name);
    out.extend_from_slice(&(body.len() as u64).to_be_bytes());
    out.extend_from_slice(body);
}

fn be24(v: u32) -> [u8; 3] {
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

/// The chunks in front of the stream data, ending with the STRMDATA chunk header.
fn leading(info: &DtsHdInfo, frames: u32, bytes: u64, avg_kbps: u32, peak_kbps: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let flags: u8 = match info.kind {
        Kind::CoreAndExtension => 0x08 | 0x10 | if info.vbr { 0x05 } else { 0 },
        Kind::CoreOnly => 0x08,
        Kind::ExtensionOnly => 0x10 | if info.vbr { 0x05 } else { 0 },
    };
    let mut hdr = vec![0u8, 0, 0, 0, 0x80];
    hdr.extend_from_slice(&info.start.samples.to_be_bytes());
    hdr.extend_from_slice(&[info.fps_code, 0, flags, 1, 0, 0, 0]);
    chunk(&mut out, b"DTSHDHDR", &hdr);
    if info.kind != Kind::ExtensionOnly {
        let mut c = Vec::new();
        c.extend_from_slice(&be24(info.sample_rate));
        c.extend_from_slice(&(info.core_kbps as u16).to_be_bytes());
        c.extend_from_slice(&info.ch_mask.to_be_bytes());
        c.push(0);
        c.extend_from_slice(&be24(info.core_payload));
        c.push(0);
        chunk(&mut out, b"CORESSMD", &c);
    }
    if info.kind != Kind::CoreOnly {
        let mut e = Vec::new();
        e.extend_from_slice(&be24(avg_kbps));
        e.push(0);
        if info.kind == Kind::ExtensionOnly && !info.vbr {
            e.extend_from_slice(&[0x00, 0x0a, 0xa8, 0x00]);
        } else {
            e.extend_from_slice(&(peak_kbps.min(0xFFFF) as u16).to_be_bytes());
            e.extend_from_slice(&128u16.to_be_bytes());
        }
        chunk(&mut out, b"EXTSS_MD", &e);
    }
    let kind_byte = match info.kind {
        Kind::CoreAndExtension => 5,
        Kind::CoreOnly => 1,
        Kind::ExtensionOnly if info.vbr => 5, // lossless data without a core
        Kind::ExtensionOnly => 8,
    };
    let mut a = vec![0u8, 0, kind_byte];
    a.extend_from_slice(&be24(info.sample_rate));
    a.extend_from_slice(&frames.to_be_bytes());
    a.extend_from_slice(&info.samples_per_frame.to_be_bytes());
    a.extend_from_slice(&((info.total_samples >> 8) as u32).to_be_bytes());
    a.push(info.total_samples as u8);
    a.extend_from_slice(&info.ch_mask.to_be_bytes());
    a.extend_from_slice(&[0, 0]); // codec delay
    a.extend_from_slice(&[0, 0, 0]);
    chunk(&mut out, b"AUPR-HDR", &a);
    out.extend_from_slice(b"STRMDATA");
    out.extend_from_slice(&bytes.to_be_bytes());
    out
}

/// Writer for a `.dtshd` file: header up front (rewritten at the end with the real counts), frames, trailer.
pub struct DtsHdSink {
    out: std::io::BufWriter<std::fs::File>,
    info: DtsHdInfo,
    header_len: u64,
    sizes: Vec<u32>,
    bytes: u64,
    ext_peak: u64,
    ext_total: u64,
}

impl DtsHdSink {
    pub fn create(path: &std::path::Path, info: DtsHdInfo) -> std::io::Result<DtsHdSink> {
        let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
        let h = leading(&info, 0, 0, 0, 0);
        out.write_all(&h)?;
        Ok(DtsHdSink { out, info, header_len: h.len() as u64, sizes: Vec::new(), bytes: 0, ext_peak: 0, ext_total: 0 })
    }

    /// Append one frame (`ext_len` bytes of it belong to the extension substream).
    pub fn write_frame(&mut self, data: &[u8], ext_len: usize) -> std::io::Result<()> {
        debug_assert_eq!(data.len() % 4, 0, "DTS-HD frames are whole words");
        self.out.write_all(data)?;
        self.sizes.push(data.len() as u32);
        self.bytes += data.len() as u64;
        self.ext_peak = self.ext_peak.max(ext_len as u64);
        self.ext_total += ext_len as u64;
        Ok(())
    }

    /// Write the trailer and the final header. `blackout` is one silent frame, if the caller can make one.
    pub fn finish(mut self, blackout: Option<&[u8]>) -> std::io::Result<()> {
        let info = self.info.clone();
        let mut t = Vec::new();
        if info.vbr {
            let mut navi = Vec::new();
            navi.extend_from_slice(&(self.sizes.len() as u32).to_be_bytes());
            let wide = self.bytes > 0xFF_FFFF;
            navi.extend_from_slice(&[0, 1, if wide { 4 } else { 3 }]);
            let mut off = 0u64;
            for s in &self.sizes {
                if wide {
                    navi.extend_from_slice(&(off as u32).to_be_bytes());
                } else {
                    navi.extend_from_slice(&be24(off as u32));
                }
                off += *s as u64;
            }
            chunk(&mut t, b"NAVI-TBL", &navi);
        }
        // clock, frame rate code, start position and reference position (see `put_position`)
        let mut tcb = vec![0u8; 32];
        tcb[..4].copy_from_slice(&48_000u32.to_be_bytes());
        tcb[4] = info.fps_code;
        put_position(&mut tcb[8..], info.start);
        put_position(&mut tcb[20..], info.start);
        chunk(&mut t, b"TIMECODE", &tcb);
        let mut build = b"BUILD DATE = Subwoofa
BUILD TIME = 00:00:00
VERSION = 1
REVISION = 0
".to_vec();
        build.resize(76, 0);
        chunk(&mut t, b"BUILDVER", &build);
        chunk(&mut t, b"FILEINFO", &[0x09, 0x0a, 0, 0]);
        if let Some(b) = blackout {
            chunk(&mut t, b"BLACKOUT", b);
        }
        self.out.write_all(&t)?;
        self.out.flush()?;
        let frames = self.sizes.len() as u32;
        let secs = frames as f64 * info.samples_per_frame as f64 / info.sample_rate as f64;
        let avg = if info.kind == Kind::CoreAndExtension || info.vbr { (self.ext_total as f64 * 8.0 / secs.max(1e-9) / 1000.0) as u32 } else { (self.bytes as f64 * 8.0 / secs.max(1e-9) / 1000.0) as u32 };
        let peak = (self.ext_peak as f64 * 8.0 * info.sample_rate as f64 / info.samples_per_frame as f64 / 1000.0).ceil() as u32;
        let h = leading(&info, frames, self.bytes, avg, peak);
        debug_assert_eq!(h.len() as u64, self.header_len);
        let mut inner = self.out.into_inner().map_err(|e| e.into_error())?;
        inner.seek(SeekFrom::Start(0))?;
        inner.write_all(&h)?;
        inner.flush()
    }
}

/// `0, samples(4), remainder(4)` as it sits in the TIMECODE chunk (nine bytes per position).
fn put_position(dst: &mut [u8], p: Position) {
    dst[0] = 0;
    dst[1..5].copy_from_slice(&p.samples.to_be_bytes());
    dst[5..9].copy_from_slice(&p.fraction.to_be_bytes());
}

/// Output of a stream encoder: a raw stream file, or a `.dtshd` container around it.
pub enum StreamOut {
    Raw(std::io::BufWriter<std::fs::File>),
    Hd(DtsHdSink),
}

impl StreamOut {
    pub fn create(path: &std::path::Path, info: Option<DtsHdInfo>) -> std::io::Result<StreamOut> {
        Ok(match info {
            Some(i) => StreamOut::Hd(DtsHdSink::create(path, i)?),
            None => StreamOut::Raw(std::io::BufWriter::new(std::fs::File::create(path)?)),
        })
    }

    pub fn write_frame(&mut self, data: &[u8], ext_len: usize) -> std::io::Result<()> {
        match self {
            StreamOut::Raw(w) => w.write_all(data),
            StreamOut::Hd(s) => s.write_frame(data, ext_len),
        }
    }

    pub fn finish(self, blackout: Option<&[u8]>) -> std::io::Result<()> {
        match self {
            StreamOut::Raw(mut w) => w.flush(),
            StreamOut::Hd(s) => s.finish(blackout),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timecode_positions_match_the_commercial_tools() {
        // values StreamTools wrote when restriping the same file
        assert_eq!(position_of(FPS_24, timecode_frames(FPS_24, 0, 0, 1, 0)).samples, 48_000);
        assert_eq!(position_of(FPS_23_976, timecode_frames(FPS_23_976, 0, 0, 10, 0)).samples, 480_480);
        assert_eq!(position_of(FPS_23_976, timecode_frames(FPS_23_976, 1, 2, 3, 4)).samples, 178_890_712);
        assert_eq!(position_of(FPS_25, timecode_frames(FPS_25, 1, 0, 0, 0)).samples, 172_800_000);
        assert_eq!(position_of(FPS_30, timecode_frames(FPS_30, 1, 0, 0, 0)).samples, 172_800_000);
        assert_eq!(position_of(FPS_29_97, timecode_frames(FPS_29_97, 1, 0, 0, 0)).samples, 172_972_800);
        let p = position_of(FPS_29_97_DROP, timecode_frames(FPS_29_97_DROP, 1, 0, 0, 0));
        assert_eq!((p.samples, p.fraction), (172_799_827, 6000));
        assert_eq!(position_of(FPS_30_DROP, timecode_frames(FPS_30_DROP, 1, 0, 0, 0)).samples, 172_627_200);
    }
}
