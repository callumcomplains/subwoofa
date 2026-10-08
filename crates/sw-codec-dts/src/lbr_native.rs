//! File-level driver for DTS Express: stems in, `.dtshd` out (extension substream frames only).

use crate::exss::{dtshd_header_lbr as dtshd_header, exss_frame_lbr, DtsHdHeader, ExssParams};
use crate::lbr::{LbrConfig, LbrEncoder};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use sw_core::{Channel, ChannelLayout};
use sw_pcm::StemSet;

#[derive(Debug, thiserror::Error)]
pub enum LbrError {
    #[error(transparent)]
    Encoder(#[from] crate::Error),
    #[error(transparent)]
    Pcm(#[from] sw_pcm::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("nothing to encode: the selected span is empty")]
    Empty,
    #[error("canceled")]
    Canceled,
}

pub struct LbrRequest<'a> {
    pub layout: &'static ChannelLayout,
    pub bitrate_kbps: u32,
    pub start_secs: f64,
    pub duration_secs: Option<f64>,
    pub output: &'a Path,
}

pub struct LbrStats {
    pub frames: u64,
    pub bytes: u64,
}

/// How a layout maps onto the channels of an LBR stream.
#[derive(Debug, Clone)]
pub struct LbrLayout {
    /// Speaker activity mask (C = 1, L/R = 2, Ls/Rs = 4) of the full-band channels, plus LFE (8).
    pub mask: u16,
    /// Layout channel index of every coded full-band channel.
    pub order: Vec<usize>,
    pub lfe: Option<usize>,
}

pub fn map_layout(layout: &ChannelLayout) -> Result<LbrLayout, crate::Error> {
    use Channel::*;
    let pos = |c: Channel| layout.channels.iter().position(|x| *x == c);
    let has = |c: Channel| pos(c).is_some();
    let side = (has(Ls) && has(Rs)) || (has(Lss) && has(Rss));
    let (ls, rs) = if has(Ls) { (Ls, Rs) } else { (Lss, Rss) };
    let known = layout.channels.iter().all(|c| matches!(c, C | L | R | Lfe | Ls | Rs | Lss | Rss));
    if !known || (has(L) != has(R)) || (has(Ls) && !has(Rs)) || (has(Lss) && !has(Rss)) || (has(Ls) && has(Lss)) {
        return Err(crate::Error::Unsupported(format!("layout '{}' is not one of the DTS Express arrangements (mono, stereo, with centre, with surrounds, with LFE)", layout.id)));
    }
    let mut mask = 0u16;
    let mut order = Vec::new();
    if has(L) {
        mask |= 2;
        order.push(pos(L).unwrap());
        order.push(pos(R).unwrap());
    }
    if side {
        mask |= 4;
        order.push(pos(ls).unwrap());
        order.push(pos(rs).unwrap());
    }
    if has(C) {
        mask |= 1;
        order.push(pos(C).unwrap());
    }
    if has(Lfe) {
        mask |= 8;
    }
    if mask & 7 == 0 {
        return Err(crate::Error::Unsupported("DTS Express needs at least one full-band channel".into()));
    }
    Ok(LbrLayout { mask, order, lfe: pos(Lfe) })
}

pub fn encode_to_file(stems: &mut StemSet, req: &LbrRequest, on_progress: &mut dyn FnMut(f32), cancel: &AtomicBool) -> Result<LbrStats, LbrError> {
    let rate = stems.sample_rate;
    let map = map_layout(req.layout)?;
    let nch = map.order.len();
    let mut enc = LbrEncoder::new(LbrConfig { sample_rate: rate, channels: nch, speaker_mask: map.mask, lfe: map.lfe.is_some(), bitrate: req.bitrate_kbps * 1000 })?;
    let first = ((req.start_secs * rate as f64).round() as u64).min(stems.frames);
    stems.seek_frame(first)?;
    let avail = stems.frames - first;
    let want = req.duration_secs.map(|d| (d * rate as f64).round() as u64).unwrap_or(avail).min(avail);
    if want == 0 {
        return Err(LbrError::Empty);
    }
    let n = enc.frame_samples();
    let total_channels = nch + usize::from(map.lfe.is_some());
    let exss = ExssParams {
        sample_rate: rate,
        channels: total_channels,
        bit_res: 16,
        spk_mask: map.mask as u32,
        with_core: false,
        dialnorm: 0,
        frame_duration_code: (n / 512 - 1) as u32,
    };
    let frames_total = want.div_ceil(n as u64);
    let mut file = std::io::BufWriter::new(std::fs::File::create(req.output)?);
    let header = |frames: u32, bytes: u64| dtshd_header(&DtsHdHeader { sample_rate: rate, frames, samples_per_frame: n as u16, total_samples: want, ch_mask: map.mask, initial_padding: 0 }, bytes);
    let header_len = {
        let h = header(frames_total as u32, 0);
        file.write_all(&h)?;
        h.len() as u64
    };
    let slots: Vec<usize> = map.order.iter().copied().chain(map.lfe).collect();
    let mut planar: Vec<Vec<f32>> = Vec::new();
    let mut remaining = want;
    let (mut frames, mut bytes) = (0u64, 0u64);
    let chunk = n;
    while remaining > 0 {
        if cancel.load(Ordering::Relaxed) {
            return Err(LbrError::Canceled);
        }
        let got = (remaining as usize).min(chunk);
        stems.read_planar(&mut planar, got)?;
        let block: Vec<Vec<f32>> = slots.iter().map(|&s| planar[s][..got].to_vec()).collect();
        remaining -= got as u64;
        for p in enc.push(&block) {
            let f = exss_frame_lbr(&exss, &p);
            file.write_all(&f)?;
            bytes += f.len() as u64;
            frames += 1;
        }
        on_progress(1.0 - remaining as f32 / want as f32);
    }
    for p in enc.finish() {
        let f = exss_frame_lbr(&exss, &p);
        file.write_all(&f)?;
        bytes += f.len() as u64;
        frames += 1;
    }
    file.flush()?;
    let h = header(frames as u32, bytes);
    debug_assert_eq!(h.len() as u64, header_len);
    let mut inner = file.into_inner().map_err(|e| e.into_error())?;
    inner.seek(SeekFrom::Start(0))?;
    inner.write_all(&h)?;
    inner.flush()?;
    on_progress(1.0);
    Ok(LbrStats { frames, bytes })
}
