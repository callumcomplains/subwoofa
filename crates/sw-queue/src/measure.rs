//! Measuring the loudness of the input files of a job before it is encoded: the same files, channel order and span
//! the encode would use, so what is measured is what goes into the encoder.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use sw_core::{Channel, ChannelLayout};
use sw_loudness::{Loudness, Meter};
use sw_pcm::{ChannelOrder, StemSet};

/// What to measure.
pub struct Measure<'a> {
    /// The layout of the input files (the source layout when the encode mixes down).
    pub layout: &'static ChannelLayout,
    pub inputs: &'a HashMap<Channel, PathBuf>,
    pub order: ChannelOrder,
    pub start: Option<f64>,
    pub duration: Option<f64>,
}

/// Why a measurement stopped.
#[derive(Debug)]
pub enum MeasureError {
    Pcm(sw_pcm::Error),
    Canceled,
    Empty,
}

impl From<sw_pcm::Error> for MeasureError {
    fn from(e: sw_pcm::Error) -> Self {
        MeasureError::Pcm(e)
    }
}

impl std::fmt::Display for MeasureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MeasureError::Pcm(e) => write!(f, "{e}"),
            MeasureError::Canceled => write!(f, "canceled"),
            MeasureError::Empty => write!(f, "nothing to measure: the span is empty"),
        }
    }
}

/// Reads the inputs once and measures them. `on_progress` gets the fraction done.
pub fn loudness(m: &Measure, cancel: &AtomicBool, on_progress: &mut dyn FnMut(f32)) -> Result<Loudness, MeasureError> {
    let mut stems = StemSet::open_ordered(m.layout, m.inputs, m.order)?;
    let total = stems.select_span(m.start.unwrap_or(0.0), m.duration)?;
    if total == 0 {
        return Err(MeasureError::Empty);
    }
    let mut meter = Meter::new(stems.sample_rate, m.layout.channels);
    let mut done = 0u64;
    stems.read_ahead(1 << 16, total, |planar, frames| {
        if cancel.load(Ordering::Relaxed) {
            return Err(MeasureError::Canceled);
        }
        meter.push(planar, frames);
        done += frames as u64;
        on_progress(done as f32 / total as f32);
        Ok(())
    })?;
    Ok(meter.finish())
}
