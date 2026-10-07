//! File-level driver: stems in, `.ac3` out.

use crate::{map_layout, Config, Encoder, Error, SAMPLES_PER_FRAME};
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
    #[error("canceled")]
    Canceled,
}

pub struct Request<'a> {
    pub layout: &'static ChannelLayout,
    pub bitrate_kbps: u32,
    pub dialnorm: u8,
    pub start_secs: f64,
    pub duration_secs: Option<f64>,
    pub output: &'a Path,
    /// Write E-AC-3 instead of AC-3.
    pub eac3: bool,
}

pub struct Stats {
    pub frames: u64,
    pub input_frames: u64,
    pub bytes: u64,
}

pub fn encode_to_file(
    stems: &mut StemSet,
    req: &Request,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Result<Stats, NativeError> {
    let map = map_layout(req.layout)?;
    let rate = stems.sample_rate;
    let mut enc = Encoder::new(Config {
        acmod: map.acmod,
        lfe: map.lfe,
        sample_rate: rate,
        bitrate_kbps: req.bitrate_kbps,
        dialnorm: req.dialnorm,
        dolby_surround: map.dolby_surround,
        bandwidth_hz: None,
        eac3: req.eac3,
    })?;
    let first = ((req.start_secs * rate as f64) as u64).min(stems.frames);
    stems.seek_frame(first)?;
    let avail = stems.frames - first;
    let want = req.duration_secs.map(|d| (d * rate as f64) as u64).unwrap_or(avail).min(avail);
    // one extra block of zeros flushes the transform delay
    let total_frames = (want + 256).div_ceil(SAMPLES_PER_FRAME as u64);
    let mut out = std::io::BufWriter::new(std::fs::File::create(req.output)?);

    let nch = enc.channels();
    let mut planar: Vec<Vec<f32>> = Vec::new();
    let mut remaining = want;
    let mut bytes = 0u64;
    let mut buf: Vec<Vec<f32>> = vec![vec![0.0; SAMPLES_PER_FRAME]; nch];
    for f in 0..total_frames {
        if cancel.load(Ordering::Relaxed) {
            return Err(NativeError::Canceled);
        }
        let n = (remaining as usize).min(SAMPLES_PER_FRAME);
        if n > 0 {
            stems.read_planar(&mut planar, n)?;
        }
        for (ac3_ch, &src) in map.source_index.iter().enumerate() {
            let dst = &mut buf[ac3_ch];
            dst.fill(0.0);
            if n > 0 {
                dst[..n].copy_from_slice(&planar[src][..n]);
            }
        }
        remaining -= n as u64;
        let refs: Vec<&[f32]> = buf.iter().map(|v| v.as_slice()).collect();
        let frame = enc.encode_frame(&refs);
        out.write_all(&frame)?;
        bytes += frame.len() as u64;
        if f % 64 == 0 {
            on_progress(f as f32 / total_frames as f32);
        }
    }
    out.flush()?;
    on_progress(1.0);
    Ok(Stats { frames: total_frames, input_frames: want, bytes })
}
