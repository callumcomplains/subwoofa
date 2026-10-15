//! A set of per-channel files read together as one multichannel source.

use crate::channels::{resolve_sources, ChannelOrder, Slot};
use crate::reader::PcmReader;
use crate::Error;
use std::collections::HashMap;
use std::path::PathBuf;
use sw_core::{Channel, ChannelLayout};

/// A set of per-channel files mapped onto a layout, read in lock step as planar `f32`.
pub struct StemSet {
    pub(crate) layout: &'static ChannelLayout,
    pub(crate) readers: Vec<PcmReader>,
    pub(crate) slots: Vec<Slot>,
    pub sample_rate: u32,
    pub bits: u16,
    /// Length of the longest file; shorter files are zero-padded.
    pub frames: u64,
    pub time_reference: Option<u64>,
    pub(crate) scratch: Vec<Vec<f32>>,
    pub(crate) mix: Option<sw_core::downmix::Matrix>,
    /// Linear gain applied to everything read (1.0 leaves the samples as they are).
    pub(crate) gain: f32,
    /// Length of the files before any delay, and the delay: `lead` frames of silence in front of the audio, or
    /// `skip` frames of its start left out. `pos` is where the next read starts, counting the silence.
    pub(crate) file_frames: u64,
    pub(crate) lead: u64,
    pub(crate) skip: u64,
    pub(crate) pos: u64,
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
            gain: 1.0,
            file_frames: frames,
            lead: 0,
            skip: 0,
            pos: 0,
        })
    }

    /// Mix the layout channels down through `matrix` (inputs in layout order) before they are returned, so
    /// the set reads as the matrix's output channels.
    pub fn set_downmix(&mut self, matrix: sw_core::downmix::Matrix) {
        self.mix = Some(matrix);
    }

    /// Scales everything read from now on by `db` decibels (after any downmix).
    pub fn set_gain_db(&mut self, db: f64) {
        self.gain = 10f64.powf(db / 20.0) as f32;
    }

    pub fn layout(&self) -> &'static ChannelLayout {
        self.layout
    }

    /// Delays the audio by `frames` (positive: that much silence first) or advances it (negative: that much of the
    /// start is left out). Everything after reads as if the files were that much longer or shorter.
    pub fn set_delay_frames(&mut self, frames: i64) {
        self.lead = frames.max(0) as u64;
        self.skip = (-frames).max(0) as u64;
        self.frames = (self.file_frames + self.lead).saturating_sub(self.skip);
    }

    pub fn seek_frame(&mut self, frame: u64) -> Result<(), Error> {
        self.pos = frame;
        let in_files = (frame + self.skip).saturating_sub(self.lead);
        for r in &mut self.readers {
            r.seek_frame(in_files)?;
        }
        Ok(())
    }

    /// The frame `secs` seconds in, rounded to the nearest, and no later than the end of the files.
    pub fn frame_at(&self, secs: f64) -> u64 {
        ((secs * self.sample_rate as f64).round() as u64).min(self.frames)
    }

    /// Moves to `start_secs` and returns how many frames to encode from there: `duration_secs` worth, or to the end of
    /// the files, and never more than they hold. Seconds are rounded to the nearest frame. Zero means nothing is left.
    pub fn select_span(&mut self, start_secs: f64, duration_secs: Option<f64>) -> Result<u64, Error> {
        let rate = self.sample_rate as f64;
        let first = self.frame_at(start_secs);
        self.seek_frame(first)?;
        let available = self.frames - first;
        Ok(duration_secs
            .map(|d| (d * rate).round() as u64)
            .unwrap_or(available)
            .min(available))
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
        let n = (max_frames as u64).min(self.frames.saturating_sub(self.pos)) as usize;
        planar.resize(self.slots.len(), Vec::new());
        for p in planar.iter_mut() {
            p.clear();
        }
        if n == 0 {
            return Ok(0);
        }
        // a delay puts silence first; the files are read for the rest
        let silent = self.lead.saturating_sub(self.pos).min(n as u64) as usize;
        let data = n - silent;
        let mut got = Vec::with_capacity(self.readers.len());
        for (i, r) in self.readers.iter_mut().enumerate() {
            self.scratch[i].clear();
            got.push(if data > 0 {
                r.read_f32(&mut self.scratch[i], data)?
            } else {
                0
            });
        }
        for (s, out) in self.slots.iter().zip(planar.iter_mut()) {
            let nch = self.readers[s.file].info.channels as usize;
            let have = got[s.file];
            out.resize(silent, 0.0);
            out.extend((0..have).map(|f| self.scratch[s.file][f * nch + s.ch]));
            out.resize(n, 0.0);
        }
        // Readers of shorter files stop advancing; keep all positions aligned for the next call.
        for (r, g) in self.readers.iter_mut().zip(&got) {
            if *g < data {
                r.pos = r.info.frames;
            }
        }
        self.pos += n as u64;
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
        if self.gain != 1.0 {
            for plane in planar.iter_mut() {
                for x in plane.iter_mut() {
                    *x *= self.gain;
                }
            }
        }
        Ok(n)
    }
}
