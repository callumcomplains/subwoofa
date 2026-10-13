//! Cutting a stream: keeping a stretch of it, or splitting it at given times.

use crate::container::{copy_frames, patch_container, write_trailer};
use crate::{Error, Format, Result, Stream};
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

impl Stream {
    /// Snap a start frame back to one decoding can begin at.
    pub(crate) fn restart_at_or_before(&self, mut i: usize) -> usize {
        while i > 0 && !self.frames[i].restart {
            i -= 1;
        }
        i
    }

    /// First frame at or after `i` that decoding can begin at (the frame count when there is none).
    pub(crate) fn restart_at_or_after(&self, mut i: usize) -> usize {
        while i < self.frames.len() && !self.frames[i].restart {
            i += 1;
        }
        i
    }

    /// Write frames `[first, last)` of this stream into `out` (container header patched).
    pub(crate) fn write_range(&self, out: &Path, first: usize, last: usize) -> Result<()> {
        let mut src = BufReader::new(File::open(&self.path)?);
        let mut w = BufWriter::new(File::create(out)?);
        let frames = &self.frames[first..last];
        let bytes: u64 = frames.iter().map(|f| f.len as u64).sum();
        let ticks: u64 = frames.iter().map(|f| f.ticks as u64).sum();
        if self.is_container() {
            w.write_all(&patch_container(
                &self.header,
                frames.len() as u32,
                ticks,
                self.tick_rate,
                bytes,
            ))?;
        }
        copy_frames(&mut src, &mut w, self.data_start, frames)?;
        if self.is_container() {
            let sizes: Vec<u32> = frames.iter().map(|f| f.len).collect();
            write_trailer(&mut w, &self.trailer, &sizes)?;
        }
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
