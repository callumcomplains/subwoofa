//! File-level driver: stems in, raw `.thd` elementary stream out.

use crate::{
    default_downmix, map_layout, map_layout_mlp, Config, Encoder, Error, Interval, LayoutMap, Mlp,
    DEFAULT_RESTART_INTERVAL,
};
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
    /// Include the stereo downmix substream (recommended; ignored for mono/stereo layouts).
    pub downmix: bool,
    /// Write MLP (DVD-Audio) instead of TrueHD.
    pub mlp: bool,
    /// How hard the encoder searches for a smaller file.
    pub effort: sw_core::Effort,
}

pub struct Stats {
    pub frames: u64,
    pub bytes: u64,
    /// Samples of the stereo presentation that exceed 24 bits.
    pub downmix_overflows: u64,
}

/// Restart intervals coded together.
const BATCH: usize = 8;

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
    let rate = stems.sample_rate;
    let (map, mlp) = if req.mlp {
        let (arrangement, source_index) = map_layout_mlp(req.layout)?;
        let bits = match stems.bits as u32 {
            0..=16 => 16,
            17..=20 => 20,
            _ => 24,
        };
        (
            LayoutMap {
                chanmap: 0,
                source_index,
            },
            Some(Mlp { arrangement, bits }),
        )
    } else {
        (map_layout(req.layout)?, None)
    };
    let mut enc = Encoder::new(Config {
        sample_rate: rate,
        chanmap: map.chanmap,
        channels: map.source_index.len(),
        restart_interval: DEFAULT_RESTART_INTERVAL,
        downmix: if req.downmix && mlp.is_none() {
            default_downmix(req.layout, &map)
        } else {
            None
        },
        decorrelate: true,
        effort: req.effort,
        mlp,
    })?;
    let first = ((req.start_secs * rate as f64).round() as u64).min(stems.frames);
    stems.seek_frame(first)?;
    let avail = stems.frames - first;
    let want = req
        .duration_secs
        .map(|d| (d * rate as f64).round() as u64)
        .unwrap_or(avail)
        .min(avail);
    if want == 0 {
        return Err(NativeError::Empty);
    }
    let chunk = enc.interval_samples();
    let mut out = std::io::BufWriter::new(std::fs::File::create(req.output)?);

    let mut planar: Vec<Vec<f32>> = Vec::new();
    let mut remaining = want;
    let mut bytes = 0u64;
    let nch = map.source_index.len();
    while remaining > 0 {
        if cancel.load(Ordering::Relaxed) {
            return Err(NativeError::Canceled);
        }
        // several intervals at a time: they are coded on all cores
        let mut batch: Vec<(Vec<Vec<i32>>, usize, bool)> = Vec::with_capacity(BATCH);
        while batch.len() < BATCH && remaining > 0 {
            let n = (remaining as usize).min(chunk);
            stems.read_planar(&mut planar, n)?;
            let mut chans: Vec<Vec<i32>> = vec![vec![0; chunk]; nch];
            for (dst, &src) in chans.iter_mut().zip(&map.source_index) {
                for (d, s) in dst[..n].iter_mut().zip(&planar[src][..n]) {
                    *d = to_i24(*s);
                }
            }
            remaining -= n as u64;
            batch.push((chans, n, remaining == 0));
        }
        let intervals: Vec<Interval> = batch
            .iter()
            .map(|(chans, valid, last)| Interval {
                chans,
                valid: *valid,
                last: *last,
            })
            .collect();
        for data in enc.encode_intervals(&intervals) {
            out.write_all(&data)?;
            bytes += data.len() as u64;
        }
        on_progress(1.0 - remaining as f32 / want as f32);
    }
    out.flush()?;
    on_progress(1.0);
    Ok(Stats {
        frames: want,
        bytes,
        downmix_overflows: enc.downmix_overflows,
    })
}
