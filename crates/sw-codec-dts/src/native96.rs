//! File-level driver for DTS 96/24: 96 kHz stems in, raw `.dts` (core plus X96 extension) out.

use crate::dts96::{Dts96Config, Dts96Encoder};
use crate::native::{map_layout, NativeError, Request, Stats};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use sw_pcm::StemSet;

pub fn encode_to_file(stems: &mut StemSet, req: &Request, on_progress: &mut dyn FnMut(f32), cancel: &AtomicBool) -> Result<Stats, NativeError> {
    let rate = stems.sample_rate;
    if rate != 96_000 && rate != 88_200 {
        return Err(crate::Error::Unsupported(format!("DTS 96/24 needs 88.2 or 96 kHz input, not {rate} Hz")).into());
    }
    let map = map_layout(req.layout)?;
    let nprim = map.order.len();
    let bitrate = req.bitrate_kbps * 1000;
    // a fifth of the frame for the extension, at least 96 bytes
    let frame_bytes = (bitrate as usize * 512 / 48_000 / 8) & !3;
    let x96_bytes = ((frame_bytes / 5).max(96)) & !3;
    let mut enc = Dts96Encoder::new(Dts96Config { map: map.clone(), bitrate, dialnorm: req.dialnorm, pcm_bits: req.pcm_bits, x96_bytes })?;
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
    let (mut frames, mut bytes) = (0u64, 0u64);
    let slots: Vec<usize> = map.order.iter().copied().chain(map.lfe).collect();
    let chunk = n * 4;
    while remaining > 0 {
        if cancel.load(Ordering::Relaxed) {
            return Err(NativeError::Canceled);
        }
        let got = (remaining as usize).min(chunk);
        stems.read_planar(&mut planar, got)?;
        let block: Vec<Vec<f64>> = slots.iter().map(|&s| planar[s][..got].iter().map(|v| (*v as f64 * 8_388_608.0).clamp(-8_388_608.0, 8_388_607.0)).collect()).collect();
        remaining -= got as u64;
        for f in enc.push(&block) {
            out.write_all(&f)?;
            frames += 1;
            bytes += f.len() as u64;
        }
        on_progress(1.0 - remaining as f32 / want as f32);
    }
    for f in enc.finish() {
        out.write_all(&f)?;
        frames += 1;
        bytes += f.len() as u64;
    }
    let _ = nprim;
    out.flush()?;
    on_progress(1.0);
    Ok(Stats { frames, bytes })
}
