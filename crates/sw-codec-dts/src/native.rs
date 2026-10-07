//! File-level driver: stems in, raw `.dts` core stream out.

use crate::{Config, Encoder, Error};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use sw_core::{Channel, ChannelLayout};
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

/// How a layout maps onto the DTS core's channel arrangement.
#[derive(Debug, Clone)]
pub struct LayoutMap {
    pub amode: u8,
    /// Index into the layout's channels for each primary channel, in DTS stream order.
    pub order: Vec<usize>,
    pub lfe: Option<usize>,
}

pub fn map_layout(layout: &ChannelLayout) -> Result<LayoutMap, Error> {
    use Channel::*;
    let pos = |c: Channel| layout.channels.iter().position(|x| *x == c);
    let lfe = pos(Lfe);
    let main: Vec<Channel> = layout.channels.iter().copied().filter(|c| *c != Lfe).collect();
    let mut has = main.clone();
    has.sort_by_key(|c| *c as u8);
    let want = |list: &[Channel]| {
        let mut w = list.to_vec();
        w.sort_by_key(|c| *c as u8);
        w == has
    };
    let (amode, order): (u8, &[Channel]) = if want(&[C]) {
        (0, &[C])
    } else if want(&[L, R]) {
        (2, &[L, R])
    } else if want(&[Lt, Rt]) {
        (4, &[Lt, Rt])
    } else if want(&[L, C, R]) {
        (5, &[C, L, R])
    } else if want(&[L, R, Cs]) {
        (6, &[L, R, Cs])
    } else if want(&[L, C, R, Cs]) {
        (7, &[C, L, R, Cs])
    } else if want(&[L, R, Ls, Rs]) {
        (8, &[L, R, Ls, Rs])
    } else if want(&[L, R, C, Ls, Rs]) {
        (9, &[C, L, R, Ls, Rs])
    } else {
        return Err(Error::Unsupported(format!("layout '{}' needs the DTS extension channels (not implemented yet)", layout.id)));
    };
    Ok(LayoutMap { amode, order: order.iter().map(|c| pos(*c).unwrap()).collect(), lfe })
}

pub struct Request<'a> {
    pub layout: &'static ChannelLayout,
    pub bitrate_kbps: u32,
    pub dialnorm: i32,
    pub pcm_bits: u32,
    pub start_secs: f64,
    pub duration_secs: Option<f64>,
    pub output: &'a Path,
}

pub struct Stats {
    pub frames: u64,
    pub bytes: u64,
}

pub fn encode_to_file(stems: &mut StemSet, req: &Request, on_progress: &mut dyn FnMut(f32), cancel: &AtomicBool) -> Result<Stats, NativeError> {
    let map = map_layout(req.layout)?;
    let rate = stems.sample_rate;
    let nch = map.order.len();
    let bitrate = req.bitrate_kbps * 1000;
    // 512-sample frames up to 1.5 Mbit/s are what players expect; longer frames for low rates
    let blocks = if bitrate >= 700_000 || rate > 48_000 { 16 } else { 32 };
    let mut enc = Encoder::new(Config { sample_rate: rate, amode: map.amode, channels: nch, lfe: map.lfe.is_some(), bitrate, dialnorm: req.dialnorm, pcm_bits: req.pcm_bits, es: false, blocks })?;
    let first = ((req.start_secs * rate as f64).round() as u64).min(stems.frames);
    stems.seek_frame(first)?;
    let avail = stems.frames - first;
    let want = req.duration_secs.map(|d| (d * rate as f64).round() as u64).unwrap_or(avail).min(avail);
    if want == 0 {
        return Err(NativeError::Empty);
    }
    let n = enc.frame_samples();
    let mut out = std::io::BufWriter::new(std::fs::File::create(req.output)?);
    let mut planar: Vec<Vec<f32>> = Vec::new();
    let mut remaining = want;
    let mut frames = 0u64;
    let mut bytes = 0u64;
    // one extra frame flushes the filter bank delay
    let total_frames = want.div_ceil(n as u64) + 1;
    let mut pcm: Vec<Vec<f64>> = vec![vec![0.0; n]; nch + usize::from(map.lfe.is_some())];
    for _ in 0..total_frames {
        if cancel.load(Ordering::Relaxed) {
            return Err(NativeError::Canceled);
        }
        let got = (remaining as usize).min(n);
        if got > 0 {
            stems.read_planar(&mut planar, got)?;
        }
        for (slot, src) in map.order.iter().copied().chain(map.lfe).enumerate() {
            let dst = &mut pcm[slot];
            dst.iter_mut().for_each(|v| *v = 0.0);
            for i in 0..got {
                dst[i] = (planar[src][i] as f64 * 8_388_608.0).clamp(-8_388_608.0, 8_388_607.0);
            }
        }
        remaining -= got as u64;
        let data = enc.encode_frame(&pcm);
        out.write_all(&data)?;
        frames += 1;
        bytes += data.len() as u64;
        on_progress(1.0 - remaining as f32 / want as f32);
    }
    out.flush()?;
    on_progress(1.0);
    Ok(Stats { frames, bytes })
}
