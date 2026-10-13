//! The `.dtshd` container: its chunks, the frame index it ends with, and copying frames into a new file.

use crate::{Chunk, Frame, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// The chunks between `pos` and the end of the file.
pub(crate) fn read_chunks<R: Read + Seek>(r: &mut R, mut pos: u64, total: u64) -> Result<Vec<Chunk>> {
    let mut out = Vec::new();
    while pos + 16 <= total {
        r.seek(SeekFrom::Start(pos))?;
        let mut h = [0u8; 16];
        r.read_exact(&mut h)?;
        if !h[..8]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b'-' || *c == b'_')
        {
            break;
        }
        let size = u64::from_be_bytes(h[8..16].try_into().unwrap());
        if size > total - pos - 16 {
            break;
        }
        let mut body = vec![0u8; size as usize];
        r.read_exact(&mut body)?;
        out.push(Chunk {
            name: h[..8].try_into().unwrap(),
            body,
        });
        pos += 16 + size;
    }
    Ok(out)
}

/// The frame offset table for frames of the given sizes: count, 1, bytes per entry, entries.
pub(crate) fn navi_table(sizes: &[u32]) -> Vec<u8> {
    let total: u64 = sizes.iter().map(|s| *s as u64).sum();
    let wide = total > 0xFF_FFFF;
    let mut navi = Vec::with_capacity(7 + sizes.len() * 4);
    navi.extend_from_slice(&(sizes.len() as u32).to_be_bytes());
    navi.extend_from_slice(&[0, 1, if wide { 4 } else { 3 }]);
    let mut off = 0u64;
    for s in sizes {
        if wide {
            navi.extend_from_slice(&(off as u32).to_be_bytes());
        } else {
            navi.extend_from_slice(&(off as u32).to_be_bytes()[1..]);
        }
        off += *s as u64;
    }
    navi
}

/// Write the chunks that follow the stream, rebuilding the frame offset table for the frames just written.
pub(crate) fn write_trailer<W: Write>(w: &mut W, trailer: &[Chunk], sizes: &[u32]) -> Result<()> {
    for c in trailer {
        let body = if &c.name == b"NAVI-TBL" {
            navi_table(sizes)
        } else {
            c.body.clone()
        };
        w.write_all(&c.name)?;
        w.write_all(&(body.len() as u64).to_be_bytes())?;
        w.write_all(&body)?;
    }
    Ok(())
}

pub(crate) fn copy_frames<R: Read + Seek, W: Write>(
    src: &mut R,
    w: &mut W,
    data_start: u64,
    frames: &[Frame],
) -> Result<()> {
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
/// Rewrite the frame count, programme length and stream size in a container header. `ticks` count at
/// `tick_rate` (the core's rate); the programme length is stored in samples at the presentation's rate, which is
/// higher for 96 kHz streams with a 48 kHz core.
pub(crate) fn patch_container(header: &[u8], frames: u32, ticks: u64, tick_rate: u32, bytes: u64) -> Vec<u8> {
    let mut h = header.to_vec();
    let mut pos = 0usize;
    while pos + 16 <= h.len() {
        let size = u64::from_be_bytes(h[pos + 8..pos + 16].try_into().unwrap()) as usize;
        let name = h[pos..pos + 8].to_vec();
        if name == b"AUPR-HDR" && size >= 19 {
            let p = pos + 16;
            h[p + 6..p + 10].copy_from_slice(&frames.to_be_bytes());
            let rate = ((h[p + 3] as u64) << 16) | ((h[p + 4] as u64) << 8) | h[p + 5] as u64;
            let samples = if tick_rate > 0 && rate > 0 {
                ticks * rate / tick_rate as u64
            } else {
                ticks
            };
            h[p + 12..p + 16].copy_from_slice(&((samples >> 8) as u32).to_be_bytes());
            h[p + 16] = samples as u8;
        } else if name == b"STRMDATA" {
            h[pos + 8..pos + 16].copy_from_slice(&bytes.to_be_bytes());
            break;
        }
        pos += 16 + size;
    }
    h
}

/// Chunks of a `.dtshd` file as (name, payload offset, payload size).
pub(crate) fn chunk_offsets(path: &Path) -> Result<Vec<(Vec<u8>, u64, u64)>> {
    let mut f = File::open(path)?;
    let total = f.metadata()?.len();
    let mut pos = 0u64;
    let mut out = Vec::new();
    while pos + 16 <= total {
        f.seek(SeekFrom::Start(pos))?;
        let mut h = [0u8; 16];
        f.read_exact(&mut h)?;
        let size = u64::from_be_bytes(h[8..16].try_into().unwrap());
        if !h[..8]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b'-' || *c == b'_')
            || size > total - pos - 16
        {
            break;
        }
        out.push((h[..8].to_vec(), pos + 16, size));
        pos += 16 + size;
    }
    Ok(out)
}
