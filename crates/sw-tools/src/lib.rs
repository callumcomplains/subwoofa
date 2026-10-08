//! StreamTools: look inside, cut and join DTS / DTS-HD and TrueHD / MLP elementary streams.
//!
//! Streams are indexed frame by frame (access unit by access unit) without loading them into memory; cuts
//! happen on frame boundaries, copying the bytes untouched.

use serde::Serialize;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Format(String),
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Format {
    /// DTS core and/or DTS-HD extension substream frames (raw `.dts` or `.dtshd`).
    Dts,
    TrueHd,
    Mlp,
}

/// One frame (access unit) of the stream.
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    /// Offset of the frame from the start of the elementary stream data.
    pub off: u64,
    pub len: u32,
    /// Duration in ticks of the stream clock.
    pub ticks: u32,
    /// Decoding can start here (first frame / major sync).
    pub restart: bool,
}

pub struct Stream {
    pub format: Format,
    /// Container bytes in front of the elementary stream (`.dtshd` header chunks); empty for raw streams.
    pub header: Vec<u8>,
    /// File offset of the first elementary stream byte.
    pub data_start: u64,
    pub data_len: u64,
    pub frames: Vec<Frame>,
    /// Ticks per second of the frame durations.
    pub tick_rate: u32,
    pub sample_rate: u32,
    pub channels: u32,
    pub description: String,
    path: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct Info {
    pub format: String,
    pub description: String,
    pub sample_rate: u32,
    pub channels: u32,
    pub frames: usize,
    pub duration_secs: f64,
    pub bytes: u64,
    pub average_kbps: f64,
    pub peak_kbps: f64,
    pub restart_points: usize,
    pub container: bool,
}

#[derive(Debug, Serialize)]
pub struct PbrPoint {
    pub secs: f64,
    pub mean_kbps: f64,
    pub peak_kbps: f64,
}

// ------------------------------------------------------------------------------------------------
// reading

struct Bits<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Bits<'a> {
    fn new(d: &'a [u8]) -> Self {
        Bits { d, p: 0 }
    }
    fn get(&mut self, n: usize) -> u32 {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = self.d.get(self.p >> 3).copied().unwrap_or(0);
            v = (v << 1) | ((byte >> (7 - (self.p & 7))) & 1) as u32;
            self.p += 1;
        }
        v
    }
}

const CORE_SYNC: u32 = 0x7FFE_8001;
const EXSS_SYNC: u32 = 0x6458_2025;
const SFREQ: [u32; 16] = [0, 8000, 16000, 32000, 0, 0, 11025, 22050, 44100, 0, 0, 12000, 24000, 48000, 0, 0];
const FS_INDEX: [u32; 16] = [8000, 16000, 32000, 64000, 128000, 22050, 44100, 88200, 176400, 352800, 12000, 24000, 48000, 96000, 192000, 384000];
const AMODE_CHANNELS: [u32; 16] = [1, 2, 2, 2, 2, 3, 3, 4, 4, 5, 6, 6, 6, 7, 8, 8];

fn be32(d: &[u8]) -> u32 {
    u32::from_be_bytes([d[0], d[1], d[2], d[3]])
}

struct CoreInfo {
    size: usize,
    samples: u32,
    rate: u32,
    channels: u32,
    ext_x96: bool,
    xch: bool,
}

fn parse_core(d: &[u8]) -> Option<CoreInfo> {
    if d.len() < 16 || be32(d) != CORE_SYNC {
        return None;
    }
    let mut b = Bits::new(&d[4..]);
    b.get(1);
    b.get(5);
    b.get(1);
    let nblks = b.get(7);
    let fsize = b.get(14);
    let amode = b.get(6) as usize;
    let sfreq = b.get(4) as usize;
    b.get(5);
    b.get(5); // mix, dynf, timef, auxf, hdcd
    let ext_id = b.get(3);
    let ext = b.get(1);
    b.get(1);
    let lff = b.get(2);
    if nblks < 4 || fsize < 94 {
        return None;
    }
    let channels = AMODE_CHANNELS.get(amode).copied().unwrap_or(0) + u32::from(lff != 0);
    Some(CoreInfo { size: fsize as usize + 1, samples: (nblks + 1) * 32, rate: SFREQ[sfreq & 15], channels, ext_x96: ext == 1 && ext_id == 2, xch: ext == 1 && ext_id == 0 })
}

struct ExssInfo {
    size: usize,
    /// (frame duration in reference clock ticks, reference clock rate)
    ticks: Option<(u32, u32)>,
    rate: u32,
    channels: u32,
}

fn parse_exss(d: &[u8], carry: Option<(u32, u32)>) -> Option<ExssInfo> {
    if d.len() < 16 || be32(d) != EXSS_SYNC {
        return None;
    }
    let mut b = Bits::new(&d[4..]);
    b.get(8);
    let idx = b.get(2);
    let wide = b.get(1) as usize;
    let _hsize = b.get(8 + 4 * wide);
    let fsize = b.get(16 + 4 * wide) as usize + 1;
    let nb4f = 16 + 4 * wide;
    let static_fields = b.get(1);
    let mut ticks = carry;
    let (mut rate, mut channels) = (0, 0);
    if static_fields == 1 {
        let clk = b.get(2) as usize;
        let dur = 512 * (b.get(3) + 1);
        ticks = Some((dur, [32000, 44100, 48000, 48000][clk]));
        if b.get(1) == 1 {
            b.get(36);
        }
        let npres = b.get(3) as usize + 1;
        let nass = b.get(3) as usize + 1;
        let mut masks = Vec::new();
        for _ in 0..npres {
            masks.push(b.get(idx as usize + 1));
        }
        for m in &masks {
            for ss in 0..=idx as usize {
                if m >> ss & 1 == 1 {
                    b.get(8);
                }
            }
        }
        if b.get(1) == 1 {
            b.get(2);
            let nb = (b.get(2) as usize + 1) << 2;
            let n = b.get(2) as usize + 1;
            for _ in 0..n {
                b.get(nb);
            }
        }
        for _ in 0..nass {
            b.get(nb4f);
        }
        // first asset descriptor
        b.get(9);
        b.get(3);
        if b.get(1) == 1 {
            b.get(4);
        }
        if b.get(1) == 1 {
            b.get(24);
        }
        if b.get(1) == 1 {
            let n = b.get(10) as usize + 1;
            b.p += n * 8;
        }
        b.get(5);
        rate = FS_INDEX[b.get(4) as usize & 15];
        channels = b.get(8) + 1;
    }
    Some(ExssInfo { size: fsize, ticks, rate, channels })
}

/// Sliding window over a file: indexing reads forward in big blocks instead of seeking per frame.
struct Window {
    base: u64,
    start: u64,
    data: Vec<u8>,
    total: u64,
}

impl Window {
    fn new(base: u64, total: u64) -> Window {
        Window { base, start: 0, data: Vec::new(), total }
    }

    /// Up to `n` bytes at stream position `pos`.
    fn get<R: Read + Seek>(&mut self, r: &mut R, pos: u64, n: usize) -> Result<&[u8]> {
        let end = (pos + n as u64).min(self.total);
        if pos < self.start || end > self.start + self.data.len() as u64 {
            r.seek(SeekFrom::Start(self.base + pos))?;
            let want = ((self.total - pos) as usize).min(8 << 20);
            self.data.resize(want, 0);
            let mut got = 0;
            while got < want {
                let k = r.read(&mut self.data[got..])?;
                if k == 0 {
                    break;
                }
                got += k;
            }
            self.data.truncate(got);
            self.start = pos;
        }
        let off = (pos - self.start) as usize;
        Ok(&self.data[off..(off + n).min(self.data.len())])
    }
}

/// Index a raw DTS / DTS-HD elementary stream (positions relative to `base`).
fn index_dts<R: Read + Seek>(r: &mut R, base: u64, len: u64) -> Result<(Vec<Frame>, u32, u32, u32, String)> {
    let mut frames: Vec<Frame> = Vec::new();
    let mut pos = 0u64;
    let (mut rate, mut chans, mut tick_rate) = (0u32, 0u32, 0u32);
    let mut carry: Option<(u32, u32)> = None;
    let (mut has_core, mut has_exss, mut x96, mut xch) = (false, false, false, false);
    let mut win = Window::new(base, len);
    let mut cur: Option<Frame> = None;
    while pos + 16 <= len {
        let buf = win.get(r, pos, 256)?.to_vec();
        let n = buf.len();
        if n < 16 {
            break;
        }
        let w = be32(&buf);
        if w == CORE_SYNC {
            if let Some(c) = parse_core(&buf[..n]) {
                if let Some(f) = cur.take() {
                    frames.push(f);
                }
                has_core = true;
                x96 |= c.ext_x96;
                xch |= c.xch;
                rate = c.rate * if c.ext_x96 { 2 } else { 1 };
                chans = chans.max(c.channels + u32::from(c.xch));
                tick_rate = c.rate;
                cur = Some(Frame { off: pos, len: c.size as u32, ticks: c.samples, restart: true });
                pos += c.size as u64;
                continue;
            }
        } else if w == EXSS_SYNC {
            if let Some(e) = parse_exss(&buf[..n], carry) {
                carry = e.ticks;
                has_exss = true;
                if e.rate > 0 {
                    rate = e.rate;
                    chans = chans.max(e.channels);
                }
                match cur.as_mut() {
                    Some(f) if has_core && f.off + f.len as u64 == pos => f.len += e.size as u32,
                    _ => {
                        if let Some(f) = cur.take() {
                            frames.push(f);
                        }
                        let (dur, clk) = e.ticks.unwrap_or((512, 48000));
                        tick_rate = clk;
                        cur = Some(Frame { off: pos, len: e.size as u32, ticks: dur, restart: true });
                    }
                }
                pos += e.size as u64;
                continue;
            }
        }
        // not at a frame start: look for the next sync word
        pos += 4;
    }
    if let Some(f) = cur.take() {
        frames.push(f);
    }
    if frames.is_empty() {
        return Err(Error::Format("no DTS frames found".into()));
    }
    if tick_rate == 0 {
        tick_rate = rate.max(1);
    }
    let mut parts = vec![if has_exss { "DTS-HD".to_string() } else { "DTS".to_string() }];
    if has_core && has_exss {
        parts.push("core + extension substream".into());
    } else if has_core {
        parts.push("core only".into());
    } else {
        parts.push("extension substream only (lossless)".into());
    }
    if x96 {
        parts.push("X96".into());
    }
    if xch {
        parts.push("XCH".into());
    }
    Ok((frames, tick_rate, rate, chans, parts.join(", ")))
}

type TrueHdIndex = (Vec<Frame>, u32, u32, u32, String, Format);

fn index_truehd<R: Read + Seek>(r: &mut R, base: u64, len: u64) -> Result<TrueHdIndex> {
    let mut frames = Vec::new();
    let mut pos = 0u64;
    let (mut rate, mut chans, mut au_ticks) = (0u32, 0u32, 40u32);
    let mut fmt = Format::TrueHd;
    let mut win = Window::new(base, len);
    while pos + 4 <= len {
        let buf = win.get(r, pos, 40)?.to_vec();
        let n = buf.len();
        if n < 4 {
            break;
        }
        let size = ((((buf[0] & 0x0F) as usize) << 8) | buf[1] as usize) * 2;
        if size < 4 || pos + size as u64 > len {
            break;
        }
        let mut restart = false;
        if n >= 28 && buf[4] == 0xF8 && buf[5] == 0x72 && buf[6] == 0x6F && (buf[7] == 0xBA || buf[7] == 0xBB) {
            restart = true;
            let mut b = Bits::new(&buf[8..]);
            if buf[7] == 0xBA {
                let code = b.get(4);
                b.get(1);
                b.get(1);
                b.get(2);
                b.get(2);
                b.get(2);
                b.get(5);
                b.get(2);
                let map = b.get(13);
                const GROUP: [u32; 13] = [2, 1, 1, 2, 2, 2, 2, 1, 1, 2, 2, 1, 1];
                chans = (0..13).filter(|g| map >> g & 1 == 1).map(|g| GROUP[g]).sum();
                rate = (if code & 8 != 0 { 44100 } else { 48000 }) << (code & 7);
                au_ticks = 40 << (code & 7);
            } else {
                fmt = Format::Mlp;
                b.get(8);
                let code = b.get(4);
                b.get(4);
                b.get(4);
                b.get(4);
                b.get(3);
                let arr = b.get(5);
                const OCC: [u32; 13] = [1, 2, 3, 4, 3, 4, 5, 3, 4, 5, 4, 5, 6];
                chans = OCC.get(arr as usize).copied().unwrap_or(0);
                rate = (if code & 8 != 0 { 44100 } else { 48000 }) << (code & 7);
                au_ticks = 40 << (code & 7);
            }
        }
        frames.push(Frame { off: pos, len: size as u32, ticks: au_ticks, restart });
        pos += size as u64;
    }
    if frames.is_empty() {
        return Err(Error::Format("no access units found".into()));
    }
    let desc = if fmt == Format::Mlp { "MLP (DVD-Audio)" } else { "Dolby TrueHD" }.to_string();
    Ok((frames, rate, rate, chans, desc, fmt))
}

impl Stream {
    pub fn open(path: &Path) -> Result<Stream> {
        let f = File::open(path)?;
        let total = f.metadata()?.len();
        let mut r = BufReader::new(f);
        let mut head = [0u8; 16];
        let n = r.read(&mut head)?;
        if n < 16 {
            return Err(Error::Format("file too short".into()));
        }
        r.seek(SeekFrom::Start(0))?;
        let (header, data_start, data_len) = if &head[..8] == b"DTSHDHDR" {
            // chunks: 8-byte name, 8-byte big-endian size, payload; the stream data chunk is STRMDATA
            let mut pos = 0u64;
            loop {
                r.seek(SeekFrom::Start(pos))?;
                let mut ch = [0u8; 16];
                if r.read(&mut ch)? < 16 {
                    return Err(Error::Format("no STRMDATA chunk".into()));
                }
                let size = u64::from_be_bytes(ch[8..16].try_into().unwrap());
                if &ch[..8] == b"STRMDATA" {
                    let start = pos + 16;
                    let len = size.min(total - start);
                    r.seek(SeekFrom::Start(0))?;
                    let mut header = vec![0u8; start as usize];
                    r.read_exact(&mut header)?;
                    break (header, start, len);
                }
                pos += 16 + size;
            }
        } else {
            (Vec::new(), 0, total)
        };
        let word = {
            r.seek(SeekFrom::Start(data_start))?;
            let mut w = [0u8; 8];
            r.read_exact(&mut w)?;
            w
        };
        let looks_truehd = word[4] == 0xF8 && word[5] == 0x72 && word[6] == 0x6F;
        let (format, frames, tick_rate, sample_rate, channels, description) = if looks_truehd {
            let (fr, tr, sr, ch, d, f) = index_truehd(&mut r, data_start, data_len)?;
            (f, fr, tr, sr, ch, d)
        } else {
            let (fr, tr, sr, ch, d) = index_dts(&mut r, data_start, data_len)?;
            (Format::Dts, fr, tr, sr, ch, d)
        };
        Ok(Stream { format, header, data_start, data_len, frames, tick_rate, sample_rate, channels, description, path: path.to_path_buf() })
    }

    pub fn is_container(&self) -> bool {
        !self.header.is_empty()
    }

    pub fn total_ticks(&self) -> u64 {
        self.frames.iter().map(|f| f.ticks as u64).sum()
    }

    pub fn duration_secs(&self) -> f64 {
        self.total_ticks() as f64 / self.tick_rate.max(1) as f64
    }

    /// Frame whose span contains `secs` (the frame count when past the end).
    pub fn frame_at(&self, secs: f64) -> usize {
        let target = (secs.max(0.0) * self.tick_rate as f64).round() as u64;
        let mut t = 0u64;
        for (i, f) in self.frames.iter().enumerate() {
            if t + f.ticks as u64 > target {
                return i;
            }
            t += f.ticks as u64;
        }
        self.frames.len()
    }

    pub fn frame_start_secs(&self, index: usize) -> f64 {
        self.frames[..index.min(self.frames.len())].iter().map(|f| f.ticks as u64).sum::<u64>() as f64 / self.tick_rate.max(1) as f64
    }

    pub fn info(&self) -> Info {
        let secs = self.duration_secs();
        let bytes: u64 = self.frames.iter().map(|f| f.len as u64).sum();
        let peak = self.frames.iter().map(|f| f.len as f64 * 8.0 * self.tick_rate as f64 / f.ticks.max(1) as f64 / 1000.0).fold(0.0, f64::max);
        Info {
            format: format!("{:?}", self.format),
            description: self.description.clone(),
            sample_rate: self.sample_rate,
            channels: self.channels,
            frames: self.frames.len(),
            duration_secs: secs,
            bytes,
            average_kbps: if secs > 0.0 { bytes as f64 * 8.0 / secs / 1000.0 } else { 0.0 },
            peak_kbps: peak,
            restart_points: self.frames.iter().filter(|f| f.restart).count(),
            container: self.is_container(),
        }
    }

    /// Mean and peak bit rate in windows of `window` seconds.
    pub fn pbr(&self, window: f64) -> Vec<PbrPoint> {
        let mut out = Vec::new();
        let (mut t, mut wstart, mut bits, mut peak) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
        for f in &self.frames {
            let d = f.ticks as f64 / self.tick_rate.max(1) as f64;
            let kbps = f.len as f64 * 8.0 / d / 1000.0;
            peak = peak.max(kbps);
            bits += f.len as f64 * 8.0;
            t += d;
            if t - wstart >= window {
                out.push(PbrPoint { secs: wstart, mean_kbps: bits / (t - wstart) / 1000.0, peak_kbps: peak });
                wstart = t;
                bits = 0.0;
                peak = 0.0;
            }
        }
        if t > wstart + 1e-9 {
            out.push(PbrPoint { secs: wstart, mean_kbps: bits / (t - wstart) / 1000.0, peak_kbps: peak });
        }
        out
    }

    /// Snap a start frame back to one decoding can begin at.
    fn restart_at_or_before(&self, mut i: usize) -> usize {
        while i > 0 && !self.frames[i].restart {
            i -= 1;
        }
        i
    }

    /// Write frames `[first, last)` of this stream into `out` (container header patched).
    fn write_range(&self, out: &Path, first: usize, last: usize) -> Result<()> {
        let mut src = BufReader::new(File::open(&self.path)?);
        let mut w = BufWriter::new(File::create(out)?);
        let frames = &self.frames[first..last];
        let bytes: u64 = frames.iter().map(|f| f.len as u64).sum();
        let ticks: u64 = frames.iter().map(|f| f.ticks as u64).sum();
        if self.is_container() {
            w.write_all(&patch_container(&self.header, frames.len() as u32, ticks, bytes))?;
        }
        copy_frames(&mut src, &mut w, self.data_start, frames)?;
        w.flush()?;
        Ok(())
    }

    /// Cut `[start, end)` (seconds; `end` None = to the end) on frame boundaries. Returns (first frame,
    /// frame count, start seconds) actually written.
    pub fn trim(&self, out: &Path, start: f64, end: Option<f64>) -> Result<(usize, usize, f64)> {
        let mut first = self.frame_at(start);
        if first >= self.frames.len() {
            return Err(Error::Format("the start is past the end of the stream".into()));
        }
        if self.format != Format::Dts {
            first = self.restart_at_or_before(first);
        }
        let last = match end {
            Some(e) => (self.frame_at(e) + 1).min(self.frames.len()).max(first + 1),
            None => self.frames.len(),
        };
        self.write_range(out, first, last)?;
        Ok((first, last - first, self.frame_start_secs(first)))
    }

    /// Cut at the given times into `<prefix>_001.<ext>`, `<prefix>_002.<ext>`... Returns the written paths.
    pub fn split(&self, out_prefix: &Path, ext: &str, at: &[f64]) -> Result<Vec<PathBuf>> {
        let mut cuts: Vec<usize> = vec![0];
        for &t in at {
            let mut i = self.frame_at(t);
            if self.format != Format::Dts {
                i = self.restart_at_or_before(i);
            }
            if i > *cuts.last().unwrap() && i < self.frames.len() {
                cuts.push(i);
            }
        }
        cuts.push(self.frames.len());
        let mut paths = Vec::new();
        for (n, w) in cuts.windows(2).enumerate() {
            let name = out_prefix.file_name().and_then(|s| s.to_str()).unwrap_or("part");
            let p = out_prefix.with_file_name(format!("{name}_{:03}.{ext}", n + 1));
            self.write_range(&p, w[0], w[1])?;
            paths.push(p);
        }
        Ok(paths)
    }
}

/// Append several streams into one file; they must be the same format at the same rate.
pub fn append(inputs: &[&Path], out: &Path) -> Result<()> {
    let streams: Vec<Stream> = inputs.iter().map(|p| Stream::open(p)).collect::<Result<_>>()?;
    let first = streams.first().ok_or_else(|| Error::Format("nothing to append".into()))?;
    for s in &streams {
        if s.format != first.format || s.sample_rate != first.sample_rate || s.channels != first.channels {
            return Err(Error::Format(format!(
                "{} ({} Hz, {} ch) does not match {} ({} Hz, {} ch)",
                s.path.display(),
                s.sample_rate,
                s.channels,
                first.path.display(),
                first.sample_rate,
                first.channels
            )));
        }
    }
    let frames: u32 = streams.iter().map(|s| s.frames.len() as u32).sum();
    let ticks: u64 = streams.iter().map(|s| s.total_ticks()).sum();
    let bytes: u64 = streams.iter().flat_map(|s| s.frames.iter()).map(|f| f.len as u64).sum();
    let mut w = BufWriter::new(File::create(out)?);
    if first.is_container() {
        w.write_all(&patch_container(&first.header, frames, ticks, bytes))?;
    }
    for s in &streams {
        let mut src = BufReader::new(File::open(&s.path)?);
        copy_frames(&mut src, &mut w, s.data_start, &s.frames)?;
    }
    w.flush()?;
    Ok(())
}


impl Stream {
    /// First frame at or after `i` that decoding can begin at (the frame count when there is none).
    fn restart_at_or_after(&self, mut i: usize) -> usize {
        while i < self.frames.len() && !self.frames[i].restart {
            i += 1;
        }
        i
    }
}

fn check_compatible(a: &Stream, b: &Stream) -> Result<()> {
    if a.format != b.format || a.sample_rate != b.sample_rate || a.channels != b.channels {
        return Err(Error::Format(format!(
            "{} ({} Hz, {} ch) does not match {} ({} Hz, {} ch)",
            b.path.display(),
            b.sample_rate,
            b.channels,
            a.path.display(),
            a.sample_rate,
            a.channels
        )));
    }
    Ok(())
}

/// Write the given frame ranges of several streams, one after the other, into one file with the header of
/// `head` patched for the new totals.
fn write_pieces(head: &Stream, pieces: &[(&Stream, usize, usize)], out: &Path) -> Result<()> {
    let frames: u32 = pieces.iter().map(|(_, a, b)| (b - a) as u32).sum();
    let ticks: u64 = pieces.iter().flat_map(|(s, a, b)| s.frames[*a..*b].iter()).map(|f| f.ticks as u64).sum();
    let bytes: u64 = pieces.iter().flat_map(|(s, a, b)| s.frames[*a..*b].iter()).map(|f| f.len as u64).sum();
    let mut w = BufWriter::new(File::create(out)?);
    if head.is_container() {
        w.write_all(&patch_container(&head.header, frames, ticks, bytes))?;
    }
    for (s, a, b) in pieces {
        let mut src = BufReader::new(File::open(&s.path)?);
        copy_frames(&mut src, &mut w, s.data_start, &s.frames[*a..*b])?;
    }
    w.flush()?;
    Ok(())
}

/// Replace the part of `base` that `insert` covers, starting at `at` seconds, with `insert` (the Join/Replace
/// tool). Cut points move to frames decoding can start at. Returns (seconds where the insert begins, seconds
/// of base removed).
pub fn replace(base: &Path, insert: &Path, at: f64, out: &Path) -> Result<(f64, f64)> {
    let (a, b) = (Stream::open(base)?, Stream::open(insert)?);
    check_compatible(&a, &b)?;
    let mut first = a.frame_at(at);
    if a.format != Format::Dts {
        first = a.restart_at_or_before(first);
    }
    let end_secs = a.frame_start_secs(first) + b.duration_secs();
    let mut last = a.frame_at(end_secs);
    if a.format != Format::Dts {
        last = a.restart_at_or_after(last);
    }
    let last = last.max(first);
    let pieces = [(&a, 0, first), (&b, 0, b.frames.len()), (&a, last, a.frames.len())];
    write_pieces(&a, &pieces, out)?;
    Ok((a.frame_start_secs(first), a.frame_start_secs(last) - a.frame_start_secs(first)))
}

/// Insert all of `insert` into `base` at `at` seconds without removing anything.
pub fn insert(base: &Path, insert: &Path, at: f64, out: &Path) -> Result<f64> {
    let (a, b) = (Stream::open(base)?, Stream::open(insert)?);
    check_compatible(&a, &b)?;
    let mut first = a.frame_at(at);
    if a.format != Format::Dts {
        first = a.restart_at_or_before(first);
    }
    let pieces = [(&a, 0, first), (&b, 0, b.frames.len()), (&a, first, a.frames.len())];
    write_pieces(&a, &pieces, out)?;
    Ok(a.frame_start_secs(first))
}

fn copy_frames<R: Read + Seek, W: Write>(src: &mut R, w: &mut W, data_start: u64, frames: &[Frame]) -> Result<()> {
    // frames are contiguous in practice: copy runs in large reads
    let mut i = 0;
    let mut buf = vec![0u8; 1 << 20];
    while i < frames.len() {
        let start = frames[i].off;
        let mut end = start + frames[i].len as u64;
        let mut j = i + 1;
        while j < frames.len() && frames[j].off == end {
            end += frames[j].len as u64;
            j += 1;
        }
        src.seek(SeekFrom::Start(data_start + start))?;
        let mut left = end - start;
        while left > 0 {
            let n = left.min(buf.len() as u64) as usize;
            src.read_exact(&mut buf[..n])?;
            w.write_all(&buf[..n])?;
            left -= n as u64;
        }
        i = j;
    }
    Ok(())
}

/// Rewrite the counts in a `.dtshd` header: AUPR-HDR frame count and total samples, STRMDATA size.
fn patch_container(header: &[u8], frames: u32, ticks: u64, bytes: u64) -> Vec<u8> {
    let mut h = header.to_vec();
    let mut pos = 0usize;
    while pos + 16 <= h.len() {
        let size = u64::from_be_bytes(h[pos + 8..pos + 16].try_into().unwrap()) as usize;
        let name = h[pos..pos + 8].to_vec();
        if name == b"AUPR-HDR" && size >= 19 {
            let p = pos + 16;
            h[p + 6..p + 10].copy_from_slice(&frames.to_be_bytes());
            h[p + 12..p + 16].copy_from_slice(&((ticks >> 8) as u32).to_be_bytes());
            h[p + 16] = ticks as u8;
        } else if name == b"STRMDATA" {
            h[pos + 8..pos + 16].copy_from_slice(&bytes.to_be_bytes());
            break;
        }
        pos += 16 + size;
    }
    h
}

#[derive(Debug, Serialize)]
pub struct VerifyReport {
    pub passed: bool,
    /// Decoder complaints (empty when the stream decodes cleanly).
    pub messages: Vec<String>,
    /// MD5 of the decoded 24-bit PCM, so two encodes of the same source can be compared.
    pub pcm_md5: Option<String>,
    pub frames: usize,
    pub duration_secs: f64,
}

/// Structural check plus a full decode with FFmpeg (errors on stderr fail the check).
pub fn verify(path: &Path, ffmpeg: &Path) -> Result<VerifyReport> {
    let s = Stream::open(path)?;
    let out = std::process::Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(path)
        .args(["-c:a", "pcm_s24le", "-f", "md5", "-"])
        .output()?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    let messages: Vec<String> = stderr.lines().filter(|l| !l.trim().is_empty()).map(|l| l.to_string()).collect();
    let md5 = String::from_utf8_lossy(&out.stdout).lines().find_map(|l| l.strip_prefix("MD5=").map(|m| m.to_string()));
    Ok(VerifyReport { passed: out.status.success() && messages.is_empty(), messages, pcm_md5: md5, frames: s.frames.len(), duration_secs: s.duration_secs() })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny synthetic TrueHD-like stream: access units of fixed size, a major sync every 4th.
    fn fake_thd(units: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for i in 0..units {
            let size = 64usize;
            let mut au = vec![0u8; size];
            au[0] = ((size / 2) >> 8) as u8 & 0x0F;
            au[1] = (size / 2) as u8;
            if i % 4 == 0 {
                au[4..8].copy_from_slice(&[0xF8, 0x72, 0x6F, 0xBA]);
                au[8] = 0x00; // 48 kHz
                au[9] = 0x00;
                au[10] = 0x00;
            }
            out.extend(au);
        }
        out
    }

    #[test]
    fn thd_trim_snaps_to_restart_and_append_restores() {
        let dir = std::env::temp_dir().join(format!("sw_tools_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("a.thd");
        std::fs::write(&src, fake_thd(40)).unwrap();
        let s = Stream::open(&src).unwrap();
        assert_eq!(s.frames.len(), 40);
        assert_eq!(s.format, Format::TrueHd);
        assert_eq!(s.sample_rate, 48_000);
        // frame 10 is 10 * 40 samples in; the nearest restart at or before is frame 8
        let t = 10.0 * 40.0 / 48_000.0;
        let out = dir.join("b.thd");
        let (first, n, _) = s.trim(&out, t, Some(20.0 * 40.0 / 48_000.0)).unwrap();
        assert_eq!(first, 8);
        assert_eq!(n, 13); // frames 8 ..= 20
        let parts = s.split(&dir.join("part"), "thd", &[t]).unwrap();
        assert_eq!(parts.len(), 2);
        let joined = dir.join("joined.thd");
        append(&[&parts[0], &parts[1]], &joined).unwrap();
        assert_eq!(std::fs::read(&joined).unwrap(), std::fs::read(&src).unwrap());
    }

    #[test]
    fn replace_and_insert_swap_whole_restart_groups() {
        let dir = std::env::temp_dir().join(format!("sw_tools_rep_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("base.thd");
        std::fs::write(&base, fake_thd(40)).unwrap();
        let mut marked = fake_thd(8);
        for i in 0..8 {
            marked[i * 64 + 20] = 0xAA;
        }
        let ins = dir.join("ins.thd");
        std::fs::write(&ins, &marked).unwrap();
        let out = dir.join("out.thd");
        // 12 frames in (a restart): replacing eight frames removes frames 12..20
        let at = 12.0 * 40.0 / 48_000.0;
        let (start, removed) = replace(&base, &ins, at, &out).unwrap();
        assert!((start - at).abs() < 1e-9 && (removed - 8.0 * 40.0 / 48_000.0).abs() < 1e-9);
        let data = std::fs::read(&out).unwrap();
        assert_eq!(data.len(), 40 * 64);
        for i in 0..40 {
            assert_eq!(data[i * 64 + 20] == 0xAA, (12..20).contains(&i), "frame {i}");
        }
        insert(&base, &ins, at, &out).unwrap();
        assert_eq!(std::fs::read(&out).unwrap().len(), 48 * 64);
    }
}
