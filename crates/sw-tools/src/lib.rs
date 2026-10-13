//! StreamTools: look inside, cut and join DTS / DTS-HD and TrueHD / MLP elementary streams.
//!
//! Streams are indexed frame by frame (access unit by access unit) without loading them into memory; cuts
//! happen on frame boundaries, copying the bytes untouched.
//!
//! Where things are: `dts_index` and `truehd_index` read a stream into frames; `cut` keeps or splits stretches of it;
//! `join` appends, replaces and inserts; `container` handles the `.dtshd` wrapper; `timecode` relabels its frame rate
//! and start time; `silence` pads it; `check` decodes the result with FFmpeg.

use serde::Serialize;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

mod blackout;
mod check;
mod container;
mod cut;
mod dts_index;
mod join;
mod reader;
mod silence;
mod timecode;
mod truehd_index;

use container::read_chunks;
use dts_index::index_dts;
use truehd_index::index_truehd;

pub use check::{verify, VerifyReport};
pub use join::{append, insert, replace};
pub use silence::add_silence;
pub use timecode::{
    fps_code, parse_timecode, read_stripe, restripe, timecode_position, Stripe, FPS_23_976, FPS_24, FPS_25, FPS_29_97,
    FPS_29_97_DROP, FPS_30, FPS_30_DROP,
};

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

/// A chunk of a `.dtshd` container.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub name: [u8; 8],
    pub body: Vec<u8>,
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
    /// Chunks after the stream data (frame offsets, timecode, silent frame...); empty for raw streams.
    pub trailer: Vec<Chunk>,
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
        let trailer = if header.is_empty() {
            Vec::new()
        } else {
            read_chunks(&mut r, data_start + data_len, total)?
        };
        Ok(Stream {
            format,
            header,
            data_start,
            data_len,
            frames,
            tick_rate,
            sample_rate,
            channels,
            description,
            trailer,
            path: path.to_path_buf(),
        })
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
        self.frames[..index.min(self.frames.len())]
            .iter()
            .map(|f| f.ticks as u64)
            .sum::<u64>() as f64
            / self.tick_rate.max(1) as f64
    }

    pub fn info(&self) -> Info {
        let secs = self.duration_secs();
        let bytes: u64 = self.frames.iter().map(|f| f.len as u64).sum();
        let peak = self
            .frames
            .iter()
            .map(|f| f.len as f64 * 8.0 * self.tick_rate as f64 / f.ticks.max(1) as f64 / 1000.0)
            .fold(0.0, f64::max);
        Info {
            format: format!("{:?}", self.format),
            description: self.description.clone(),
            sample_rate: self.sample_rate,
            channels: self.channels,
            frames: self.frames.len(),
            duration_secs: secs,
            bytes,
            average_kbps: if secs > 0.0 {
                bytes as f64 * 8.0 / secs / 1000.0
            } else {
                0.0
            },
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
                out.push(PbrPoint {
                    secs: wstart,
                    mean_kbps: bits / (t - wstart) / 1000.0,
                    peak_kbps: peak,
                });
                wstart = t;
                bits = 0.0;
                peak = 0.0;
            }
        }
        if t > wstart + 1e-9 {
            out.push(PbrPoint {
                secs: wstart,
                mean_kbps: bits / (t - wstart) / 1000.0,
                peak_kbps: peak,
            });
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------------
// Restripe and Add Silence

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

    #[test]
    fn timecode_positions_match_the_commercial_tool() {
        // sample positions StreamTools wrote for the same timecodes
        assert_eq!(timecode_position(FPS_24, 0, 0, 1, 0), (48_000, 0));
        assert_eq!(timecode_position(FPS_23_976, 0, 0, 10, 0), (480_480, 0));
        assert_eq!(timecode_position(FPS_23_976, 1, 2, 3, 4), (178_890_712, 0));
        assert_eq!(timecode_position(FPS_25, 1, 0, 0, 0), (172_800_000, 0));
        assert_eq!(timecode_position(FPS_30, 1, 0, 0, 0), (172_800_000, 0));
        assert_eq!(timecode_position(FPS_29_97, 1, 0, 0, 0), (172_972_800, 0));
        assert_eq!(timecode_position(FPS_29_97_DROP, 1, 0, 0, 0), (172_799_827, 6000));
        assert_eq!(timecode_position(FPS_30_DROP, 1, 0, 0, 0), (172_627_200, 0));
        assert_eq!(parse_timecode("01:02:03:04"), Some((1, 2, 3, 4)));
        assert_eq!(fps_code("29.97 DROP"), Some(FPS_29_97_DROP));
    }

    #[test]
    fn restripe_patches_header_and_timecode_chunk() {
        let dir = std::env::temp_dir().join(format!("sw_tools_rs_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut f = Vec::new();
        let chunk = |f: &mut Vec<u8>, n: &[u8; 8], b: &[u8]| {
            f.extend_from_slice(n);
            f.extend_from_slice(&(b.len() as u64).to_be_bytes());
            f.extend_from_slice(b);
        };
        chunk(
            &mut f,
            b"DTSHDHDR",
            &[0, 0, 0, 0, 0x80, 0, 0, 0, 0, 2, 0, 0x1d, 1, 0, 0, 0],
        );
        chunk(&mut f, b"STRMDATA", &[0u8; 64]);
        let mut tc = vec![0u8; 32];
        tc[..4].copy_from_slice(&48_000u32.to_be_bytes());
        tc[4] = 2;
        chunk(&mut f, b"TIMECODE", &tc);
        let (a, b) = (dir.join("a.dtshd"), dir.join("b.dtshd"));
        std::fs::write(&a, &f).unwrap();
        restripe(&a, &b, FPS_29_97_DROP, (1, 0, 0, 0)).unwrap();
        let s = read_stripe(&b).unwrap();
        assert_eq!(
            (s.fps_code, s.start_samples, s.reference_samples),
            (FPS_29_97_DROP, 172_799_827, 0)
        );
        let out = std::fs::read(&b).unwrap();
        assert_eq!(&out[16 + 5..16 + 9], &172_799_827u32.to_be_bytes());
        assert_eq!(out[16 + 9], FPS_29_97_DROP);
        assert_eq!(out.len(), f.len());
    }
}
