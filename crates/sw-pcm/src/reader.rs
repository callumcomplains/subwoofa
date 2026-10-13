//! Streaming reader for WAV / RF64 and AIFF: format info, seeking and conversion to `f32`.

use crate::Error;
use serde::Serialize;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

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
    pub(crate) path: String,
    pub(crate) file: BufReader<File>,
    pub(crate) info: PcmInfo,
    pub(crate) data_start: u64,
    pub(crate) big_endian: bool,
    pub(crate) frame_bytes: usize,
    pub(crate) pos: u64,
    pub(crate) scratch: Vec<u8>,
}

pub(crate) fn io_err(path: &str) -> impl Fn(std::io::Error) -> Error + '_ {
    move |e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            Error::Corrupt(path.to_string())
        } else {
            Error::Io(path.to_string(), e)
        }
    }
}

pub(crate) fn rd<const N: usize>(f: &mut impl Read, path: &str) -> Result<[u8; N], Error> {
    let mut b = [0u8; N];
    f.read_exact(&mut b).map_err(io_err(path))?;
    Ok(b)
}

/// 80-bit IEEE extended precision (AIFF sample rate) to f64.
pub(crate) fn extended_to_f64(b: [u8; 10]) -> f64 {
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

pub(crate) fn check_depth(name: &str, bits: u16, float: bool) -> Result<(), Error> {
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
pub(crate) fn convert(raw: &[u8], out: &mut [f32], bits: u16, float: bool, be: bool) {
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
pub(crate) fn sample_to_f32(s: &[u8], bits: u16, float: bool, be: bool) -> f32 {
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
