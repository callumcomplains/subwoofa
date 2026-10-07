//! File-level driver: stems in, raw `.thd` elementary stream out.

use crate::{map_layout, Config, Encoder, Error, DEFAULT_RESTART_INTERVAL};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use sw_core::ChannelLayout;
use sw_pcm::StemSet;

#[derive(Debug, thiserror::Error)]
pub enum NativeError {
    #[error(transparent)]
    Encoder(#[from] Error),
    #[error(transparent)]
    Pcm(#[from] sw_pcm::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("nothing to encode: the selected span is empty")]
    Empty,
    #[error("canceled")]
    Canceled,
}

pub struct Request<'a> {
    pub layout: &'static ChannelLayout,
    pub start_secs: f64,
    pub duration_secs: Option<f64>,
    pub output: &'a Path,
}

pub struct Stats {
    pub frames: u64,
    pub bytes: u64,
}

#[inline]
fn to_i24(v: f32) -> i32 {
    ((v * 8_388_608.0).round() as i32).clamp(-8_388_608, 8_388_607)
}

pub fn encode_to_file(
    stems: &mut StemSet,
    req: &Request,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Result<Stats, NativeError> {
    let map = map_layout(req.layout)?;
    let rate = stems.sample_rate;
    let mut enc = Encoder::new(Config { sample_rate: rate, chanmap: map.chanmap, channels: map.source_index.len(), restart_interval: DEFAULT_RESTART_INTERVAL })?;
    let first = ((req.start_secs * rate as f64).round() as u64).min(stems.frames);
    stems.seek_frame(first)?;
    let avail = stems.frames - first;
    let want = req.duration_secs.map(|d| (d * rate as f64).round() as u64).unwrap_or(avail).min(avail);
    if want == 0 {
        return Err(NativeError::Empty);
    }
    let chunk = enc.interval_samples();
    let mut out = std::io::BufWriter::new(std::fs::File::create(req.output)?);

    let mut planar: Vec<Vec<f32>> = Vec::new();
    let mut remaining = want;
    let mut bytes = 0u64;
    let nch = map.source_index.len();
    let mut chans: Vec<Vec<i32>> = vec![vec![0; chunk]; nch];
    while remaining > 0 {
        if cancel.load(Ordering::Relaxed) {
            return Err(NativeError::Canceled);
        }
        let n = (remaining as usize).min(chunk);
        stems.read_planar(&mut planar, n)?;
        let padded = n.div_ceil(enc.au_samples()) * enc.au_samples();
        for (coded, &src) in map.source_index.iter().enumerate() {
            let dst = &mut chans[coded];
            dst.iter_mut().for_each(|v| *v = 0);
            for (d, s) in dst[..n].iter_mut().zip(&planar[src][..n]) {
                *d = to_i24(*s);
            }
            debug_assert!(padded <= dst.len());
        }
        remaining -= n as u64;
        let last = remaining == 0;
        let data = enc.encode_interval(&chans, n, last);
        out.write_all(&data)?;
        bytes += data.len() as u64;
        on_progress(1.0 - remaining as f32 / want as f32);
    }
    out.flush()?;
    on_progress(1.0);
    Ok(Stats { frames: want, bytes })
}
