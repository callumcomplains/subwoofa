//! Adding silence to the head and tail of a stream.

use crate::container::{copy_frames, patch_container, write_trailer};
use crate::{Error, Result, Stream};
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;

/// Add silence to the head and/or tail of a `.dtshd` stream: whole frames of the stored silent frame, as many
/// as fit in the given time (so the length is right to within one frame, as in the commercial tool).
/// Returns (frames added at the head, frames added at the tail).
pub fn add_silence(input: &Path, out: &Path, head_secs: f64, tail_secs: f64) -> Result<(usize, usize)> {
    let s = Stream::open(input)?;
    if !s.is_container() {
        return Err(Error::Format("Add Silence needs a .dtshd file".into()));
    }
    let silent = s.blackout().ok_or_else(|| {
        Error::Format("this file has no silent frame (BLACKOUT chunk), so silence cannot be added to it".into())
    })?;
    let per_frame = s.frames[0].ticks as f64 / s.tick_rate.max(1) as f64;
    let nh = (head_secs.max(0.0) / per_frame + 1e-9).floor() as usize;
    let nt = (tail_secs.max(0.0) / per_frame + 1e-9).floor() as usize;
    let ticks = s.total_ticks() + (nh + nt) as u64 * s.frames[0].ticks as u64;
    let frames = (s.frames.len() + nh + nt) as u32;
    let bytes = s.data_len + (nh + nt) as u64 * silent.len() as u64;
    let mut w = BufWriter::new(File::create(out)?);
    w.write_all(&patch_container(&s.header, frames, ticks, s.tick_rate, bytes))?;
    for _ in 0..nh {
        w.write_all(&silent)?;
    }
    let mut src = BufReader::new(File::open(&s.path)?);
    copy_frames(&mut src, &mut w, s.data_start, &s.frames)?;
    for _ in 0..nt {
        w.write_all(&silent)?;
    }
    let mut sizes: Vec<u32> = vec![silent.len() as u32; nh];
    sizes.extend(s.frames.iter().map(|f| f.len));
    sizes.extend(std::iter::repeat_n(silent.len() as u32, nt));
    write_trailer(&mut w, &s.trailer, &sizes)?;
    w.flush()?;
    Ok((nh, nt))
}
