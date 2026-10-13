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
    /// Speaker positions from a WAVE_FORMAT_EXTENSIBLE header (Windows channel-mask bits).
    pub channel_mask: Option<u32>,
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
    move |e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            Error::Corrupt(path.to_string())
        } else {
            Error::Io(path.to_string(), e)
        }
    }
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
        let mut channel_mask: Option<u32> = None;
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
                        let mask = u32::from_le_bytes([b[20], b[21], b[22], b[23]]);
                        channel_mask = (mask != 0).then_some(mask);
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
            channel_mask,
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
                            other => {
                                return Err(Error::Unsupported(
                                    name,
                                    format!("AIFF-C compression {}", String::from_utf8_lossy(other)),
                                ))
                            }
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
        let stored = bits.div_ceil(8) * 8;
        check_depth(&name, stored, false)?;
        let frame_bytes = channels as usize * (stored as usize / 8);
        let info = PcmInfo {
            container: Container::Aiff,
            channels,
            sample_rate,
            bits: stored,
            float: false,
            frames,
            time_reference: None,
            channel_mask: None,
        };
        Self::finish(f, name, info, data_start, !little, frame_bytes)
    }

    fn finish(
        mut f: BufReader<File>,
        path: String,
        info: PcmInfo,
        data_start: u64,
        big_endian: bool,
        frame_bytes: usize,
    ) -> Result<PcmReader, Error> {
        f.seek(SeekFrom::Start(data_start))
            .map_err(|e| Error::Io(path.clone(), e))?;
        Ok(PcmReader {
            path,
            file: f,
            info,
            data_start,
            big_endian,
            frame_bytes,
            pos: 0,
            scratch: Vec::new(),
        })
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
        let start = out.len();
        out.resize(start + want * self.info.channels as usize, 0.0);
        convert(&self.scratch, &mut out[start..], bits, float, be);
        self.pos += want as u64;
        Ok(want)
    }
}

fn check_depth(name: &str, bits: u16, float: bool) -> Result<(), Error> {
    let ok = if float {
        matches!(bits, 32 | 64)
    } else {
        matches!(bits, 8 | 16 | 24 | 32)
    };
    if ok {
        Ok(())
    } else {
        Err(Error::Unsupported(
            name.to_string(),
            format!("{bits}-bit {}", if float { "float" } else { "integer" }),
        ))
    }
}

/// Turns a block of raw samples into `f32`. The common cases are plain loops over fixed-size chunks, which the
/// compiler turns into vector code; the rest go sample by sample.
fn convert(raw: &[u8], out: &mut [f32], bits: u16, float: bool, be: bool) {
    match (bits, float, be) {
        (16, false, false) => {
            for (o, s) in out.iter_mut().zip(raw.chunks_exact(2)) {
                *o = i16::from_le_bytes([s[0], s[1]]) as f32 * (1.0 / 32768.0);
            }
        }
        (24, false, false) => {
            for (o, s) in out.iter_mut().zip(raw.chunks_exact(3)) {
                *o = (i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8) as f32 * (1.0 / 8_388_608.0);
            }
        }
        (32, false, false) => {
            for (o, s) in out.iter_mut().zip(raw.chunks_exact(4)) {
                *o = i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f32 * (1.0 / 2_147_483_648.0);
            }
        }
        _ => {
            for (o, s) in out.iter_mut().zip(raw.chunks_exact(bits as usize / 8)) {
                *o = sample_to_f32(s, bits, float, be);
            }
        }
    }
}

#[inline]
fn sample_to_f32(s: &[u8], bits: u16, float: bool, be: bool) -> f32 {
    match (bits, float) {
        (8, _) => (s[0] as i32 - 128) as f32 / 128.0, // WAV 8-bit is unsigned
        (16, _) => {
            let v = if be {
                i16::from_be_bytes([s[0], s[1]])
            } else {
                i16::from_le_bytes([s[0], s[1]])
            };
            v as f32 / 32768.0
        }
        (24, _) => {
            let v = if be {
                i32::from_be_bytes([s[0], s[1], s[2], 0]) >> 8
            } else {
                i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8
            };
            v as f32 / 8_388_608.0
        }
        (32, false) => {
            let v = if be {
                i32::from_be_bytes([s[0], s[1], s[2], s[3]])
            } else {
                i32::from_le_bytes([s[0], s[1], s[2], s[3]])
            };
            v as f32 / 2_147_483_648.0
        }
        (32, true) => f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
        (64, true) => f64::from_le_bytes(s.try_into().unwrap()) as f32,
        _ => 0.0,
    }
}

/* ------------------------------------------------------------------------------------ */

/// How channels inside a multichannel file are assigned to the layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChannelOrder {
    /// Follow the file's speaker mask, except for the standard 7.1 mask (0x63F), which many tools
    /// write over film-ordered audio (L R C LFE Ls Rs Lrs Rrs): that is read in file order.
    #[default]
    Auto,
    /// Always follow the speaker mask (the Windows convention: ..., BL BR, SL SR).
    Mask,
    /// File channel `i` is layout channel `i`.
    File,
}

impl ChannelOrder {
    pub fn parse(s: &str) -> Option<ChannelOrder> {
        match s.to_ascii_lowercase().as_str() {
            "" | "auto" => Some(ChannelOrder::Auto),
            "mask" => Some(ChannelOrder::Mask),
            "file" | "film" => Some(ChannelOrder::File),
            _ => None,
        }
    }
}

/// Windows speaker bit positions used by WAVE_FORMAT_EXTENSIBLE channel masks.
mod spk {
    pub const FL: u32 = 0;
    pub const FR: u32 = 1;
    pub const FC: u32 = 2;
    pub const LFE: u32 = 3;
    pub const BL: u32 = 4;
    pub const BR: u32 = 5;
    pub const FLC: u32 = 6;
    pub const FRC: u32 = 7;
    pub const BC: u32 = 8;
    pub const SL: u32 = 9;
    pub const SR: u32 = 10;
    pub const TC: u32 = 11;
    pub const TFL: u32 = 12;
    pub const TFC: u32 = 13;
    pub const TFR: u32 = 14;
}

/// Channel mask assumed for a multichannel file that does not declare one.
fn default_mask(channels: usize) -> Option<u32> {
    use spk::*;
    let bits: &[u32] = match channels {
        3 => &[FL, FR, FC],
        4 => &[FL, FR, BL, BR],
        5 => &[FL, FR, FC, BL, BR],
        6 => &[FL, FR, FC, LFE, BL, BR],
        7 => &[FL, FR, FC, LFE, BC, SL, SR],
        8 => &[FL, FR, FC, LFE, BL, BR, SL, SR],
        _ => return None,
    };
    Some(bits.iter().fold(0, |m, b| m | 1 << b))
}

/// For each layout channel, its index inside a multichannel file with the given speaker mask.
/// Surround positions are matched flexibly: a layout with both side and rear pairs needs side and
/// back speakers in the file; otherwise Ls/Rs accept either.
pub fn map_channel_mask(layout: &ChannelLayout, mask: u32) -> Option<Vec<usize>> {
    use spk::*;
    let has_rear = layout.channels.iter().any(|c| matches!(c, Channel::Lsr | Channel::Rsr));
    let wants = |c: Channel| -> Vec<u32> {
        match c {
            Channel::L | Channel::Lt => vec![FL],
            Channel::R | Channel::Rt => vec![FR],
            Channel::C => vec![FC],
            Channel::Lfe => vec![LFE],
            Channel::Ls => {
                if has_rear {
                    vec![SL]
                } else {
                    vec![BL, SL]
                }
            }
            Channel::Rs => {
                if has_rear {
                    vec![SR]
                } else {
                    vec![BR, SR]
                }
            }
            Channel::Lss => vec![SL],
            Channel::Rss => vec![SR],
            Channel::Lsr => vec![BL],
            Channel::Rsr => vec![BR],
            Channel::Cs => vec![BC],
            Channel::Lh | Channel::Lhs => vec![TFL],
            Channel::Rh | Channel::Rhs => vec![TFR],
            Channel::Ch => vec![TFC],
            Channel::Oh => vec![TC],
            Channel::Lw => vec![FLC],
            Channel::Rw => vec![FRC],
            Channel::Tfl => vec![TFL],
            Channel::Tfr => vec![TFR],
            Channel::Tfc => vec![TFC],
            Channel::Tc => vec![TC],
            Channel::Lfe2
            | Channel::Tsl
            | Channel::Tsr
            | Channel::Tbl
            | Channel::Tbr
            | Channel::Tbc
            | Channel::Bfc
            | Channel::Bfl
            | Channel::Bfr => vec![],
        }
    };
    let present: Vec<u32> = (0..32).filter(|b| mask >> b & 1 == 1).collect();
    let mut used = Vec::new();
    let mut out = Vec::new();
    for &ch in layout.channels {
        let bit = wants(ch)
            .into_iter()
            .find(|b| present.contains(b) && !used.contains(b))?;
        used.push(bit);
        out.push(present.iter().position(|p| *p == bit)?);
    }
    (used.len() == present.len()).then_some(out)
}

/// Resolved source of each layout channel: file index and channel inside the file.
fn resolve_sources(
    layout: &'static ChannelLayout,
    inputs: &HashMap<Channel, PathBuf>,
    order: ChannelOrder,
) -> Result<(Vec<PcmReader>, Vec<Slot>, Vec<PathBuf>), Error> {
    let mut readers: Vec<PcmReader> = Vec::new();
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut by_path: HashMap<PathBuf, usize> = HashMap::new();
    let mut slots = Vec::new();
    let mut multi: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, ch) in layout.channels.iter().enumerate() {
        let path = inputs
            .get(ch)
            .ok_or_else(|| Error::Mismatch(format!("no input file for channel {}", ch.short())))?;
        let idx = match by_path.get(path) {
            Some(&i) => i,
            None => {
                let r = PcmReader::open(path)?;
                readers.push(r);
                paths.push(path.clone());
                by_path.insert(path.clone(), readers.len() - 1);
                readers.len() - 1
            }
        };
        let n = readers[idx].info.channels as usize;
        let ch_in_file = match n {
            1 => 0,
            2 => i % 2,
            _ => {
                if n != layout.channel_count() {
                    return Err(Error::Mismatch(format!(
                        "{}: has {n} channels but layout '{}' needs {}",
                        path.display(),
                        layout.id,
                        layout.channel_count()
                    )));
                }
                if let std::collections::hash_map::Entry::Vacant(e) = multi.entry(idx) {
                    let info = &readers[idx].info;
                    let mask = info.channel_mask.or_else(|| default_mask(n));
                    let positional = || (0..n).collect::<Vec<usize>>();
                    let map = match (order, mask) {
                        (ChannelOrder::File, _) => positional(),
                        (ChannelOrder::Auto, None) => positional(),
                        // the standard 7.1 mask is routinely written over film-ordered audio
                        (ChannelOrder::Auto, Some(0x63F)) | (ChannelOrder::Auto, Some(0x3F)) => positional(),
                        (_, Some(mask)) => map_channel_mask(layout, mask).ok_or_else(|| {
                            Error::Mismatch(format!(
                                "{}: its speaker layout (mask {mask:#x}) does not fit '{}'",
                                path.display(),
                                layout.id
                            ))
                        })?,
                        (ChannelOrder::Mask, None) => {
                            return Err(Error::Mismatch(format!("{}: has no speaker mask", path.display())))
                        }
                    };
                    e.insert(map);
                }
                multi[&idx][i]
            }
        };
        slots.push(Slot {
            file: idx,
            ch: ch_in_file,
        });
    }
    Ok((readers, slots, paths))
}

/// If `path` is a single file holding every channel of `layout` (more than two), return the input
/// map that points each layout channel at it. The channels are separated by `StemSet`/`slot_map`
/// using the file's speaker mask.
pub fn multichannel_inputs(layout: &'static ChannelLayout, path: &Path) -> Option<HashMap<Channel, PathBuf>> {
    let r = PcmReader::open(path).ok()?;
    let n = r.info().channels as usize;
    (n > 2 && n == layout.channel_count()).then(|| layout.channels.iter().map(|c| (*c, path.to_path_buf())).collect())
}

/// For one multichannel file: its channel count and speaker mask, and the first of `candidates` whose
/// channels the file can fill (same channel count, and a speaker mask the layout fits; files with no mask or
/// the common film-order masks are read in file order, so the layout with the film order wins there).
pub fn suggest_layout(
    path: &Path,
    candidates: &[&'static ChannelLayout],
) -> Result<(usize, Option<u32>, Option<&'static ChannelLayout>), Error> {
    let r = PcmReader::open(path)?;
    let n = r.info().channels as usize;
    let mask = r.info().channel_mask.or_else(|| default_mask(n));
    let positional = n <= 2 || matches!(mask, None | Some(0x63F) | Some(0x3F));
    let mut fits: Vec<&'static ChannelLayout> = candidates
        .iter()
        .copied()
        .filter(|l| l.channel_count() == n && (positional || mask.is_some_and(|m| map_channel_mask(l, m).is_some())))
        .collect();
    // the plain layouts before the ES matrix ones that list the same channels
    fits.sort_by_key(|l| l.id.contains("premixed") || l.id.contains("matrix"));
    let film: &[Channel] = match n {
        2 => &[Channel::L, Channel::R],
        3 => &[Channel::L, Channel::R, Channel::C],
        4 => &[Channel::L, Channel::R, Channel::Ls, Channel::Rs],
        5 => &[Channel::L, Channel::R, Channel::C, Channel::Ls, Channel::Rs],
        6 => &[
            Channel::L,
            Channel::R,
            Channel::C,
            Channel::Lfe,
            Channel::Ls,
            Channel::Rs,
        ],
        8 => &[
            Channel::L,
            Channel::R,
            Channel::C,
            Channel::Lfe,
            Channel::Ls,
            Channel::Rs,
            Channel::Lsr,
            Channel::Rsr,
        ],
        _ => &[],
    };
    let pick = if positional {
        fits.iter()
            .copied()
            .find(|l| l.channels == film)
            .or_else(|| fits.first().copied())
    } else {
        fits.first().copied()
    };
    Ok((n, mask, pick))
}

/// Per layout channel: the file and channel index inside it that supplies the channel.
pub fn slot_map(
    layout: &'static ChannelLayout,
    inputs: &HashMap<Channel, PathBuf>,
    order: ChannelOrder,
) -> Result<Vec<(PathBuf, usize)>, Error> {
    let (_, slots, paths) = resolve_sources(layout, inputs, order)?;
    Ok(slots.into_iter().map(|s| (paths[s.file].clone(), s.ch)).collect())
}

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
    mix: Option<sw_core::downmix::Matrix>,
}

impl StemSet {
    /// Open the files for `layout`. Mono files feed one channel. A stereo file in a pair's first
    /// slot (L, C, Ls, ...) supplies its left channel there and its right to the next slot; a
    /// stereo file in the second slot supplies its right channel.
    pub fn open(layout: &'static ChannelLayout, inputs: &HashMap<Channel, PathBuf>) -> Result<StemSet, Error> {
        Self::open_ordered(layout, inputs, ChannelOrder::Auto)
    }

    /// Like [`StemSet::open`] with an explicit multichannel-file channel order policy.
    pub fn open_ordered(
        layout: &'static ChannelLayout,
        inputs: &HashMap<Channel, PathBuf>,
        order: ChannelOrder,
    ) -> Result<StemSet, Error> {
        let (readers, slots, _) = resolve_sources(layout, inputs, order)?;
        let (rate, bits) = (readers[0].info.sample_rate, readers[0].info.bits);
        for r in &readers {
            if r.info.sample_rate != rate {
                return Err(Error::Mismatch(format!(
                    "sample rate mismatch: {} Hz vs {} Hz ({})",
                    rate, r.info.sample_rate, r.path
                )));
            }
            if r.info.bits != bits || r.info.float != readers[0].info.float {
                return Err(Error::Mismatch(format!(
                    "bit depth mismatch between input files ({})",
                    r.path
                )));
            }
        }
        let frames = readers.iter().map(|r| r.info.frames).max().unwrap_or(0);
        let time_reference = readers.iter().find_map(|r| r.info.time_reference);
        let nfiles = readers.len();
        Ok(StemSet {
            layout,
            readers,
            slots,
            sample_rate: rate,
            bits,
            frames,
            time_reference,
            scratch: vec![Vec::new(); nfiles],
            mix: None,
        })
    }

    /// Mix the layout channels down through `matrix` (inputs in layout order) before they are returned, so
    /// the set reads as the matrix's output channels.
    pub fn set_downmix(&mut self, matrix: sw_core::downmix::Matrix) {
        self.mix = Some(matrix);
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

    /// Reads the next `total` frames in blocks of `block` and hands each to `use_block(planar, frames)`. The next
    /// block is read on a second thread while the current one is used, so slow input and slow encoding overlap.
    /// Stops at the first error from either side.
    pub fn read_ahead<E: From<Error>>(
        &mut self,
        block: usize,
        total: u64,
        mut use_block: impl FnMut(&[Vec<f32>], usize) -> Result<(), E>,
    ) -> Result<(), E> {
        let (send, receive) = std::sync::mpsc::sync_channel::<Result<(Vec<Vec<f32>>, usize), Error>>(2);
        std::thread::scope(|scope| {
            scope.spawn(move || {
                let mut left = total;
                while left > 0 {
                    let mut planar = Vec::new();
                    let item = self.read_planar(&mut planar, block.min(left as usize));
                    let (stop, got) = match &item {
                        Ok(0) | Err(_) => (true, 0),
                        Ok(n) => (false, *n as u64),
                    };
                    if send.send(item.map(|n| (planar, n))).is_err() || stop {
                        return;
                    }
                    left -= got;
                }
            });
            for item in receive {
                let (planar, got) = item?;
                if got > 0 {
                    use_block(&planar, got)?;
                }
            }
            Ok(())
        })
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
        if let Some(m) = &self.mix {
            let nin = planar.len();
            let mut mixed: Vec<Vec<f32>> = vec![vec![0.0; n]; m.outputs.len()];
            for (o, row) in m.cells.iter().enumerate() {
                for (i, cell) in row.iter().enumerate().take(nin) {
                    let g = cell.linear();
                    if g != 0.0 {
                        for (acc, x) in mixed[o].iter_mut().zip(&planar[i]) {
                            *acc += g * x;
                        }
                    }
                }
            }
            *planar = mixed;
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn mask_mapping_for_common_layouts() {
        let five_one = ChannelLayout::find("5.1").unwrap();
        // 5.1 with back speakers (0x3F) and with side speakers (0x60F) both map in file order
        assert_eq!(map_channel_mask(five_one, 0x3F), Some(vec![0, 1, 2, 3, 4, 5]));
        assert_eq!(map_channel_mask(five_one, 0x60F), Some(vec![0, 1, 2, 3, 4, 5]));
        // 7.1 with side + rear speakers: file order is FL FR FC LFE BL BR SL SR
        let seven = ChannelLayout::find("7.1-side-rear").unwrap(); // L R C LFE Lss Rss Lsr Rsr
        assert_eq!(map_channel_mask(seven, 0x63F), Some(vec![0, 1, 2, 3, 6, 7, 4, 5]));
        // a mask with a speaker the layout lacks is rejected
        assert_eq!(map_channel_mask(five_one, 0x63F), None);
    }

    #[test]
    fn eight_channel_extensible_wav_is_split_by_mask() {
        let seven = ChannelLayout::find("7.1-side-rear").unwrap();
        let dir = std::env::temp_dir().join(format!("sw_pcm_mc_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("eight.wav");
        // one frame, eight channels with distinct values 1..=8, mask 0x63F, 16-bit
        let mut f = File::create(&p).unwrap();
        let data: Vec<u8> = (1..=8i16).flat_map(|v| (v * 256).to_le_bytes()).collect();
        f.write_all(b"RIFF").unwrap();
        f.write_all(&((4 + 8 + 40 + 8 + data.len()) as u32).to_le_bytes())
            .unwrap();
        f.write_all(b"WAVEfmt ").unwrap();
        f.write_all(&40u32.to_le_bytes()).unwrap();
        f.write_all(&0xFFFEu16.to_le_bytes()).unwrap();
        f.write_all(&8u16.to_le_bytes()).unwrap();
        f.write_all(&48_000u32.to_le_bytes()).unwrap();
        f.write_all(&(48_000u32 * 16).to_le_bytes()).unwrap();
        f.write_all(&16u16.to_le_bytes()).unwrap();
        f.write_all(&16u16.to_le_bytes()).unwrap();
        f.write_all(&22u16.to_le_bytes()).unwrap();
        f.write_all(&16u16.to_le_bytes()).unwrap();
        f.write_all(&0x63Fu32.to_le_bytes()).unwrap();
        f.write_all(&[0x01, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71])
            .unwrap(); // PCM sub-format GUID
        f.write_all(b"data").unwrap();
        f.write_all(&(data.len() as u32).to_le_bytes()).unwrap();
        f.write_all(&data).unwrap();
        drop(f);

        let r = PcmReader::open(&p).unwrap();
        assert_eq!(r.info().channels, 8);
        assert_eq!(r.info().channel_mask, Some(0x63F));
        let inputs = multichannel_inputs(seven, &p).expect("one file for all channels");
        let read = |order| {
            let mut s = StemSet::open_ordered(seven, &inputs, order).unwrap();
            let mut planar = Vec::new();
            assert_eq!(s.read_planar(&mut planar, 4).unwrap(), 1);
            planar
                .iter()
                .map(|c| (c[0] * 32768.0).round() as i32 / 256)
                .collect::<Vec<i32>>()
        };
        // Windows convention: file order FL FR FC LFE BL BR SL SR  ->  layout L R C LFE Lss Rss Lsr Rsr
        assert_eq!(read(ChannelOrder::Mask), vec![1, 2, 3, 4, 7, 8, 5, 6]);
        // the standard 7.1 mask is read in file (film) order by default
        assert_eq!(read(ChannelOrder::Auto), vec![1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(read(ChannelOrder::File), vec![1, 2, 3, 4, 5, 6, 7, 8]);
        let slots = slot_map(seven, &inputs, ChannelOrder::Mask).unwrap();
        assert_eq!(
            slots.iter().map(|(_, c)| *c).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 6, 7, 4, 5]
        );
    }

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
        f.write_all(&(rate * channels as u32 * bytes as u32).to_le_bytes())
            .unwrap();
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
        assert_eq!(
            (r.info().channels, r.info().sample_rate, r.info().bits, r.info().frames),
            (1, 48_000, 24, 4)
        );
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
        b[0] = 0x40;
        b[1] = 0x0E; // 44100 = 0xAC44 * 2^(14-... )
        b[2] = 0xAC;
        b[3] = 0x44;
        assert_eq!(extended_to_f64(b), 44100.0);
    }

    #[test]
    fn rejects_garbage() {
        let p = tmp("junk.wav");
        std::fs::write(&p, b"not audio at all, just text").unwrap();
        assert!(matches!(PcmReader::open(&p), Err(Error::NotPcm(_))));
    }
}
