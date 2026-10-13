//! Joining streams: append, and replacing or inserting a stretch of one stream with another.

use crate::container::{copy_frames, patch_container, write_trailer};
use crate::{Error, Format, Result, Stream};
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;

/// Append several streams into one file; they must be the same format at the same rate.
pub fn append(inputs: &[&Path], out: &Path) -> Result<()> {
    let streams: Vec<Stream> = inputs.iter().map(|p| Stream::open(p)).collect::<Result<_>>()?;
    let first = streams
        .first()
        .ok_or_else(|| Error::Format("nothing to append".into()))?;
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
        w.write_all(&patch_container(&first.header, frames, ticks, first.tick_rate, bytes))?;
    }
    for s in &streams {
        let mut src = BufReader::new(File::open(&s.path)?);
        copy_frames(&mut src, &mut w, s.data_start, &s.frames)?;
    }
    if first.is_container() {
        let sizes: Vec<u32> = streams.iter().flat_map(|s| s.frames.iter()).map(|f| f.len).collect();
        write_trailer(&mut w, &first.trailer, &sizes)?;
    }
    w.flush()?;
    Ok(())
}

pub(crate) fn check_compatible(a: &Stream, b: &Stream) -> Result<()> {
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
pub(crate) fn write_pieces(head: &Stream, pieces: &[(&Stream, usize, usize)], out: &Path) -> Result<()> {
    let frames: u32 = pieces.iter().map(|(_, a, b)| (b - a) as u32).sum();
    let ticks: u64 = pieces
        .iter()
        .flat_map(|(s, a, b)| s.frames[*a..*b].iter())
        .map(|f| f.ticks as u64)
        .sum();
    let bytes: u64 = pieces
        .iter()
        .flat_map(|(s, a, b)| s.frames[*a..*b].iter())
        .map(|f| f.len as u64)
        .sum();
    let mut w = BufWriter::new(File::create(out)?);
    if head.is_container() {
        w.write_all(&patch_container(&head.header, frames, ticks, head.tick_rate, bytes))?;
    }
    for (s, a, b) in pieces {
        let mut src = BufReader::new(File::open(&s.path)?);
        copy_frames(&mut src, &mut w, s.data_start, &s.frames[*a..*b])?;
    }
    if head.is_container() {
        let sizes: Vec<u32> = pieces
            .iter()
            .flat_map(|(s, a, b)| s.frames[*a..*b].iter())
            .map(|f| f.len)
            .collect();
        write_trailer(&mut w, &head.trailer, &sizes)?;
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
    Ok((
        a.frame_start_secs(first),
        a.frame_start_secs(last) - a.frame_start_secs(first),
    ))
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
