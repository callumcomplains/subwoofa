//! Native PCM input: WAV / Broadcast WAV / RF64 and AIFF / AIFF-C, read as `f32`.
//!
//! The reader is streaming (never loads a whole file), exposes format info including the
//! Broadcast-WAV time reference, and `StemSet` maps per-channel mono/stereo files onto a
//! channel layout with the same pairing rules professional encoders use.

use serde::Serialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use sw_core::{Channel, ChannelLayout};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}: {1}")]
    Io(String, #[source] std::io::Error),
    #[error("{0}: not a WAV or AIFF file")]
    NotPcm(String),
    #[error("{0}: unsupported format ({1})")]
    Unsupported(String, String),
    #[error("{0}: file is truncated or corrupt")]
    Corrupt(String),
    #[error("{0}")]
    Mismatch(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Container {
    Wav,
    Rf64,
    Aiff,
}

#[derive(Debug, Clone, Serialize)]
pub struct PcmInfo {
    pub container: Container,
    pub channels: u16,
    pub sample_rate: u32,
    /// Bits per sample as stored (8/16/24/32/64).
    pub bits: u16,
    pub float: bool,
    pub frames: u64,
    /// Broadcast-WAV `TimeReference`: samples since midnight, if the file has a `bext` chunk.
    pub time_reference: Option<u64>,
}

impl PcmInfo {
    pub fn duration_secs(&self) -> f64 {
        self.frames as f64 / self.sample_rate as f64
    }
    /// Broadcast time reference as seconds since midnight.
    pub fn start_secs(&self) -> Option<f64> {
        self.time_reference.map(|t| t as f64 / self.sample_rate as f64)
    }
}

pub struct PcmReader {
    path: String,
    file: BufReader<File>,
    info: PcmInfo,
    data_start: u64,
    big_endian: bool,
    frame_bytes: usize,
    pos: u64,
    scratch: Vec<u8>,
}

fn io_err(path: &str) -> impl Fn(std::io::Error) -> Error + '_ {
    move |e| if e.kind() == std::io::ErrorKind::UnexpectedEof { Error::Corrupt(path.to_string()) } else { Error::Io(path.to_string(), e) }
}

fn rd<const N: usize>(f: &mut impl Read, path: &str) -> Result<[u8; N], Error> {
    let mut b = [0u8; N];
    f.read_exact(&mut b).map_err(io_err(path))?;
    Ok(b)
}

/// 80-bit IEEE extended precision (AIFF sample rate) to f64.
fn extended_to_f64(b: [u8; 10]) -> f64 {
    let sign = if b[0] & 0x80 != 0 { -1.0 } else { 1.0 };
    let exp = (((b[0] & 0x7f) as i32) << 8) | b[1] as i32;
    let mant = u64::from_be_bytes([b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9]]);
    if exp == 0 && mant == 0 {
        return 0.0;
    }
    sign * (mant as f64) * 2f64.powi(exp - 16383 - 63)
}

impl PcmReader {
    pub fn open(path: impl AsRef<Path>) -> Result<PcmReader, Error> {
        let p = path.as_ref();
        let name = p.display().to_string();
        let file = File::open(p).map_err(|e| Error::Io(name.clone(), e))?;
        let len = file.metadata().map_err(|e| Error::Io(name.clone(), e))?.len();
        let mut f = BufReader::with_capacity(1 << 20, file);
        let tag = rd::<4>(&mut f, &name)?;
        match &tag {
            b"RIFF" | b"RF64" => Self::open_wav(f, name, len, &tag == b"RF64"),
            b"FORM" => Self::open_aiff(f, name, len),
            _ => Err(Error::NotPcm(name)),
        }
    }

    fn open_wav(mut f: BufReader<File>, name: String, file_len: u64, rf64: bool) -> Result<PcmReader, Error> {
        let _riff_size = rd::<4>(&mut f, &name)?;
        if &rd::<4>(&mut f, &name)? != b"WAVE" {
            return Err(Error::NotPcm(name));
        }
        let (mut fmt, mut data, mut bext_ref, mut ds64_data) = (None, None, None, None::<u64>);
        loop {
            let Ok(id) = rd::<4>(&mut f, &name) else { break };
            let Ok(sz) = rd::<4>(&mut f, &name) else { break };
            let mut size = u32::from_le_bytes(sz) as u64;
            let body = f.stream_position().map_err(|e| Error::Io(name.clone(), e))?;
            match &id {
                b"ds64" => {
                    let b = rd::<24>(&mut f, &name)?;
                    ds64_data = Some(u64::from_le_bytes(b[8..16].try_into().unwrap()));
                }
                b"fmt " => {
                    let mut b = vec![0u8; size.min(64) as usize];
                    f.read_exact(&mut b).map_err(io_err(&name))?;
                    if b.len() < 16 {
                        return Err(Error::Corrupt(name));
                    }
                    let mut tag = u16::from_le_bytes([b[0], b[1]]);
                    let channels = u16::from_le_bytes([b[2], b[3]]);
                    let rate = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
                    let bits = u16::from_le_bytes([b[14], b[15]]);
                    if tag == 0xFFFE && b.len() >= 26 {
                        tag = u16::from_le_bytes([b[24], b[25]]); // sub-format GUID starts with the tag
                    }
                    fmt = Some((tag, channels, rate, bits));
                }
                b"bext" => {
                    f.seek(SeekFrom::Current(338)).map_err(|e| Error::Io(name.clone(), e))?;
                    let b = rd::<8>(&mut f, &name)?;
                    bext_ref = Some(u64::from_le_bytes(b));
                }
                b"data" => {
                    if rf64 && size == 0xFFFF_FFFF {
                        size = ds64_data.ok_or_else(|| Error::Corrupt(name.clone()))?;
                    }
                    data = Some((body, size));
                    if fmt.is_some() {
                        break;
                    }
                }
                _ => {}
            }
            let next = body + size + (size & 1);
            if next >= file_len {
                break;
            }
            f.seek(SeekFrom::Start(next)).map_err(|e| Error::Io(name.clone(), e))?;
        }
        let (tag, channels, sample_rate, bits) = fmt.ok_or_else(|| Error::Corrupt(name.clone()))?;
        let (data_start, mut data_len) = data.ok_or_else(|| Error::Corrupt(name.clone()))?;
        let float = match tag {
            1 => false,
            3 => true,
            t => return Err(Error::Unsupported(name, format!("WAV format tag {t}"))),
        };
        check_depth(&name, bits, float)?;
        // Streaming writers leave the size at 0 or too large: trust the file length instead.
        let avail = file_len.saturating_sub(data_start);
        if data_len == 0 || data_len > avail {
            data_len = avail;
        }
        let frame_bytes = channels as usize * (bits as usize / 8);
        if channels == 0 || frame_bytes == 0 {
            return Err(Error::Corrupt(name));
        }
        let info = PcmInfo {
            container: if rf64 { Container::Rf64 } else { Container::Wav },
            channels,
            sample_rate,
            bits,
            float,
            frames: data_len / frame_bytes as u64,
            time_reference: bext_ref,
        };
        Self::finish(f, name, info, data_start, false, frame_bytes)
    }

    fn open_aiff(mut f: BufReader<File>, name: String, file_len: u64) -> Result<PcmReader, Error> {
        let _size = rd::<4>(&mut f, &name)?;
        let form = rd::<4>(&mut f, &name)?;
        let aifc = &form == b"AIFC";
        if !aifc && &form != b"AIFF" {
            return Err(Error::NotPcm(name));
        }
        let (mut comm, mut ssnd) = (None, None);
        let mut little = false;
        loop {
            let Ok(id) = rd::<4>(&mut f, &name) else { break };
            let Ok(sz) = rd::<4>(&mut f, &name) else { break };
            let size = u32::from_be_bytes(sz) as u64;
            let body = f.stream_position().map_err(|e| Error::Io(name.clone(), e))?;
            match &id {
                b"COMM" => {
                    let b = rd::<18>(&mut f, &name)?;
                    let channels = u16::from_be_bytes([b[0], b[1]]);
                    let frames = u32::from_be_bytes([b[2], b[3], b[4], b[5]]) as u64;
                    let bits = u16::from_be_bytes([b[6], b[7]]);
                    let rate = extended_to_f64(b[8..18].try_into().unwrap()).round() as u32;
                    if aifc {
                        let c = rd::<4>(&mut f, &name)?;
                        match &c {
                            b"NONE" | b"twos" => {}
                            b"sowt" => little = true,
                            other => return Err(Error::Unsupported(name, format!("AIFF-C compression {}", String::from_utf8_lossy(other)))),
                        }
                    }
                    comm = Some((channels, frames, bits, rate));
                }
                b"SSND" => {
                    let b = rd::<8>(&mut f, &name)?;
                    let offset = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64;
                    ssnd = Some(body + 8 + offset);
                }
                _ => {}
            }
            let next = body + size + (size & 1);
            if next >= file_len {
                break;
            }
            f.seek(SeekFrom::Start(next)).map_err(|e| Error::Io(name.clone(), e))?;
        }
        let (channels, frames, bits, sample_rate) = comm.ok_or_else(|| Error::Corrupt(name.clone()))?;
        let data_start = ssnd.ok_or_else(|| Error::Corrupt(name.clone()))?;
        // AIFF stores odd depths (e.g. 20) in the next whole byte count.
        let stored = ((bits + 7) / 8) * 8;
        check_depth(&name, stored, false)?;
        let frame_bytes = channels as usize * (stored as usize / 8);
        let info = PcmInfo { container: Container::Aiff, channels, sample_rate, bits: stored, float: false, frames, time_reference: None };
        Self::finish(f, name, info, data_start, !little, frame_bytes)
    }

    fn finish(mut f: BufReader<File>, path: String, info: PcmInfo, data_start: u64, big_endian: bool, frame_bytes: usize) -> Result<PcmReader, Error> {
        f.seek(SeekFrom::Start(data_start)).map_err(|e| Error::Io(path.clone(), e))?;
        Ok(PcmReader { path, file: f, info, data_start, big_endian, frame_bytes, pos: 0, scratch: Vec::new() })
    }

    pub fn info(&self) -> &PcmInfo {
        &self.info
    }
    pub fn position(&self) -> u64 {
        self.pos
    }

    pub fn seek_frame(&mut self, frame: u64) -> Result<(), Error> {
        let frame = frame.min(self.info.frames);
        self.file
            .seek(SeekFrom::Start(self.data_start + frame * self.frame_bytes as u64))
            .map_err(|e| Error::Io(self.path.clone(), e))?;
        self.pos = frame;
        Ok(())
    }

    /// Read up to `max_frames` interleaved frames, appending `f32` samples to `out`.
    /// Returns the number of frames read (0 at end of data).
    pub fn read_f32(&mut self, out: &mut Vec<f32>, max_frames: usize) -> Result<usize, Error> {
        let want = (max_frames as u64).min(self.info.frames - self.pos) as usize;
        if want == 0 {
            return Ok(0);
        }
        self.scratch.resize(want * self.frame_bytes, 0);
        self.file.read_exact(&mut self.scratch).map_err(io_err(&self.path))?;
        let (bits, float, be) = (self.info.bits, self.info.float, self.big_endian);
        let bytes = bits as usize / 8;
        out.reserve(want * self.info.channels as usize);
        for s in self.scratch.chunks_exact(bytes) {
            out.push(sample_to_f32(s, bits, float, be));
        }
        self.pos += want as u64;
        Ok(want)
    }
}

fn check_depth(name: &str, bits: u16, float: bool) -> Result<(), Error> {
    let ok = if float { matches!(bits, 32 | 64) } else { matches!(bits, 8 | 16 | 24 | 32) };
    if ok { Ok(()) } else { Err(Error::Unsupported(name.to_string(), format!("{bits}-bit {}", if float { "float" } else { "integer" }))) }
}

#[inline]
fn sample_to_f32(s: &[u8], bits: u16, float: bool, be: bool) -> f32 {
    match (bits, float) {
        (8, _) => (s[0] as i32 - 128) as f32 / 128.0, // WAV 8-bit is unsigned
        (16, _) => {
            let v = if be { i16::from_be_bytes([s[0], s[1]]) } else { i16::from_le_bytes([s[0], s[1]]) };
            v as f32 / 32768.0
        }
        (24, _) => {
            let v = if be { i32::from_be_bytes([s[0], s[1], s[2], 0]) >> 8 } else { i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8 };
            v as f32 / 8_388_608.0
        }
        (32, false) => {
            let v = if be { i32::from_be_bytes([s[0], s[1], s[2], s[3]]) } else { i32::from_le_bytes([s[0], s[1], s[2], s[3]]) };
            v as f32 / 2_147_483_648.0
        }
        (32, true) => f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
        (64, true) => f64::from_le_bytes(s.try_into().unwrap()) as f32,
        _ => 0.0,
    }
}

/* ------------------------------------------------------------------------------------ */

/// One layout channel's source: which file and which channel inside it.
#[derive(Debug, Clone)]
struct Slot {
    file: usize,
    ch: usize,
}

/// A set of per-channel files mapped onto a layout, read in lock step as planar `f32`.
pub struct StemSet {
    layout: &'static ChannelLayout,
    readers: Vec<PcmReader>,
    slots: Vec<Slot>,
    pub sample_rate: u32,
    pub bits: u16,
    /// Length of the longest file; shorter files are zero-padded.
    pub frames: u64,
    pub time_reference: Option<u64>,
    scratch: Vec<Vec<f32>>,
}

impl StemSet {
    /// Open the files for `layout`. Mono files feed one channel. A stereo file in a pair's first
    /// slot (L, C, Ls, ...) supplies its left channel there and its right to the next slot; a
    /// stereo file in the second slot supplies its right channel.
    pub fn open(layout: &'static ChannelLayout, inputs: &HashMap<Channel, PathBuf>) -> Result<StemSet, Error> {
        let mut readers: Vec<PcmReader> = Vec::new();
        let mut by_path: HashMap<PathBuf, usize> = HashMap::new();
        let mut slots = Vec::new();
        for (i, ch) in layout.channels.iter().enumerate() {
            let path = inputs.get(ch).ok_or_else(|| Error::Mismatch(format!("no input file for channel {}", ch.short())))?;
            let idx = match by_path.get(path) {
                Some(&i) => i,
                None => {
                    let r = PcmReader::open(path)?;
                    readers.push(r);
                    by_path.insert(path.clone(), readers.len() - 1);
                    readers.len() - 1
                }
            };
            let n = readers[idx].info.channels as usize;
            let ch_in_file = match n {
                1 => 0,
                2 => i % 2,
                _ => return Err(Error::Mismatch(format!("{}: only mono or stereo files are supported ({n} channels)", path.display()))),
            };
            slots.push(Slot { file: idx, ch: ch_in_file });
        }
        let (rate, bits) = (readers[0].info.sample_rate, readers[0].info.bits);
        for r in &readers {
            if r.info.sample_rate != rate {
                return Err(Error::Mismatch(format!("sample rate mismatch: {} Hz vs {} Hz ({})", rate, r.info.sample_rate, r.path)));
            }
            if r.info.bits != bits || r.info.float != readers[0].info.float {
                return Err(Error::Mismatch(format!("bit depth mismatch between input files ({})", r.path)));
            }
        }
        let frames = readers.iter().map(|r| r.info.frames).max().unwrap_or(0);
        let time_reference = readers.iter().find_map(|r| r.info.time_reference);
        let nfiles = readers.len();
        Ok(StemSet { layout, readers, slots, sample_rate: rate, bits, frames, time_reference, scratch: vec![Vec::new(); nfiles] })
    }

    pub fn layout(&self) -> &'static ChannelLayout {
        self.layout
    }

    pub fn seek_frame(&mut self, frame: u64) -> Result<(), Error> {
        for r in &mut self.readers {
            r.seek_frame(frame)?;
        }
        Ok(())
    }

    /// Read up to `max_frames` into `planar` (one Vec per layout channel, replaced).
    /// Returns frames produced; shorter files are padded with zeros up to the longest.
    pub fn read_planar(&mut self, planar: &mut Vec<Vec<f32>>, max_frames: usize) -> Result<usize, Error> {
        let pos = self.readers[0].position();
        let n = (max_frames as u64).min(self.frames.saturating_sub(pos)) as usize;
        planar.resize(self.slots.len(), Vec::new());
        for p in planar.iter_mut() {
            p.clear();
        }
        if n == 0 {
            return Ok(0);
        }
        let mut got = Vec::with_capacity(self.readers.len());
        for (i, r) in self.readers.iter_mut().enumerate() {
            self.scratch[i].clear();
            got.push(r.read_f32(&mut self.scratch[i], n)?);
        }
        for (s, out) in self.slots.iter().zip(planar.iter_mut()) {
            let nch = self.readers[s.file].info.channels as usize;
            let have = got[s.file];
            out.extend((0..have).map(|f| self.scratch[s.file][f * nch + s.ch]));
            out.resize(n, 0.0);
        }
        // Readers of shorter files stop advancing; keep all positions aligned for the next call.
        for (r, g) in self.readers.iter_mut().zip(&got) {
            if *g < n {
                r.pos = r.info.frames;
            }
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_wav(path: &Path, channels: u16, rate: u32, bits: u16, samples: &[i32]) {
        let bytes = bits as usize / 8;
        let data_len = samples.len() * bytes;
        let mut f = File::create(path).unwrap();
        f.write_all(b"RIFF").unwrap();
        f.write_all(&((36 + data_len) as u32).to_le_bytes()).unwrap();
        f.write_all(b"WAVEfmt ").unwrap();
        f.write_all(&16u32.to_le_bytes()).unwrap();
        f.write_all(&1u16.to_le_bytes()).unwrap();
        f.write_all(&channels.to_le_bytes()).unwrap();
        f.write_all(&rate.to_le_bytes()).unwrap();
        f.write_all(&(rate * channels as u32 * bytes as u32).to_le_bytes()).unwrap();
        f.write_all(&(channels * bytes as u16).to_le_bytes()).unwrap();
        f.write_all(&bits.to_le_bytes()).unwrap();
        f.write_all(b"data").unwrap();
        f.write_all(&(data_len as u32).to_le_bytes()).unwrap();
        for s in samples {
            f.write_all(&s.to_le_bytes()[..bytes]).unwrap();
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sw_pcm_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join(name)
    }

    #[test]
    fn reads_24_bit_wav() {
        let p = tmp("a.wav");
        write_wav(&p, 1, 48_000, 24, &[0, 4_194_304, -4_194_304, 8_388_607]);
        let mut r = PcmReader::open(&p).unwrap();
        assert_eq!((r.info().channels, r.info().sample_rate, r.info().bits, r.info().frames), (1, 48_000, 24, 4));
        let mut v = Vec::new();
        assert_eq!(r.read_f32(&mut v, 100).unwrap(), 4);
        assert_eq!(&v[..3], &[0.0, 0.5, -0.5]);
        assert!((v[3] - 1.0).abs() < 1e-6);
        assert_eq!(r.read_f32(&mut v, 10).unwrap(), 0);
    }

    #[test]
    fn stereo_file_pairs_into_adjacent_slots() {
        let layout = ChannelLayout::find("2.0").unwrap();
        let p = tmp("st.wav");
        write_wav(&p, 2, 48_000, 16, &[1000, -1000, 2000, -2000]);
        let mut inputs = HashMap::new();
        inputs.insert(Channel::L, p.clone());
        inputs.insert(Channel::R, p);
        let mut s = StemSet::open(layout, &inputs).unwrap();
        let mut planar = Vec::new();
        assert_eq!(s.read_planar(&mut planar, 10).unwrap(), 2);
        assert!(planar[0][0] > 0.0 && planar[1][0] < 0.0);
        assert!((planar[0][1] - 2000.0 / 32768.0).abs() < 1e-6);
    }

    #[test]
    fn short_files_are_zero_padded() {
        let layout = ChannelLayout::find("2.0").unwrap();
        let (a, b) = (tmp("long.wav"), tmp("short.wav"));
        write_wav(&a, 1, 48_000, 16, &[100, 200, 300, 400]);
        write_wav(&b, 1, 48_000, 16, &[500]);
        let inputs = HashMap::from([(Channel::L, a), (Channel::R, b)]);
        let mut s = StemSet::open(layout, &inputs).unwrap();
        assert_eq!(s.frames, 4);
        let mut planar = Vec::new();
        assert_eq!(s.read_planar(&mut planar, 4).unwrap(), 4);
        assert_eq!(planar[1][1..], [0.0, 0.0, 0.0]);
    }

    #[test]
    fn sample_rate_mismatch_is_rejected() {
        let layout = ChannelLayout::find("2.0").unwrap();
        let (a, b) = (tmp("r48.wav"), tmp("r44.wav"));
        write_wav(&a, 1, 48_000, 16, &[1]);
        write_wav(&b, 1, 44_100, 16, &[1]);
        let inputs = HashMap::from([(Channel::L, a), (Channel::R, b)]);
        assert!(matches!(StemSet::open(layout, &inputs), Err(Error::Mismatch(_))));
    }

    #[test]
    fn aiff_extended_rate() {
        let mut b = [0u8; 10];
        b[0] = 0x40; b[1] = 0x0E; // 44100 = 0xAC44 * 2^(14-... )
        b[2] = 0xAC; b[3] = 0x44;
        assert_eq!(extended_to_f64(b), 44100.0);
    }

    #[test]
    fn rejects_garbage() {
        let p = tmp("junk.wav");
        std::fs::write(&p, b"not audio at all, just text").unwrap();
        assert!(matches!(PcmReader::open(&p), Err(Error::NotPcm(_))));
    }
}
