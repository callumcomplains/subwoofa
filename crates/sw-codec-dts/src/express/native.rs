//! File-level driver for DTS Express: stems in, `.dtshd` out (extension substream frames only).

use crate::container::dtshd::{DtsHdInfo, Kind, StreamOut};
use crate::container::exss::{exss_frame_lbr, ExssExtras, ExssParams};
use crate::express::encoder::{LbrConfig, LbrEncoder};
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
    /// Blu-ray secondary audio: extension substream 2, mixing metadata, 24-bit asset.
    pub secondary: bool,
    /// Bitstream-tab metadata: the mixing metadata of secondary audio, Express dialog mode.
    pub bitstream: sw_core::BitstreamOptions,
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
    let known = layout
        .channels
        .iter()
        .all(|c| matches!(c, C | L | R | Lfe | Ls | Rs | Lss | Rss));
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
        return Err(crate::Error::Unsupported(
            "DTS Express needs at least one full-band channel".into(),
        ));
    }
    Ok(LbrLayout {
        mask,
        order,
        lfe: pos(Lfe),
    })
}

pub fn encode_to_file(
    stems: &mut StemSet,
    req: &LbrRequest,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Result<LbrStats, LbrError> {
    let rate = stems.sample_rate;
    let map = map_layout(req.layout)?;
    let nch = map.order.len();
    let first = ((req.start_secs * rate as f64).round() as u64).min(stems.frames);
    stems.seek_frame(first)?;
    let avail = stems.frames - first;
    let want = req
        .duration_secs
        .map(|d| (d * rate as f64).round() as u64)
        .unwrap_or(avail)
        .min(avail);
    if want == 0 {
        return Err(LbrError::Empty);
    }
    let n = 4096usize; // samples per Express frame
    let total_channels = nch + usize::from(map.lfe.is_some());
    let frames_total = want.div_ceil(n as u64) as usize;
    // the mixing metadata of every frame: the primary audio's attenuation (with its fades) and the mono mapping
    let plan = req.secondary.then(|| {
        crate::container::mixmeta::SecondaryPlan::new(
            &req.bitstream.secondary,
            n as f64 / rate as f64,
            want as f64 / rate as f64,
            frames_total + 1,
        )
    });
    let mut exss = ExssParams {
        sample_rate: rate,
        channels: total_channels,
        bit_res: (stems.bits as u32).clamp(16, 24),
        spk_mask: map.mask as u32,
        with_core: false,
        dialnorm: 0,
        frame_duration_code: (n / 512 - 1) as u32,
        x96_in_core: false,
        xch_in_core: false,
        secondary: req.secondary,
        extras: ExssExtras {
            min_header: 28,
            emb_stereo: false,
            mix: plan.as_ref().map(|p| p.frames[0].clone()),
            ..Default::default()
        },
    };
    // the stream's bit rate counts the whole extension substream frame, so the LBR payload gives up the header's share
    let overhead = exss_frame_lbr(&exss, &[0u8; 4]).len() - 4;
    // a whole extension substream frame is a multiple of four bytes: the original rounds the exact size down
    let total = ((req.bitrate_kbps as u64 * 1000 * n as u64 / rate as u64 / 8) & !3) as usize;
    let payload = total.saturating_sub(overhead);
    let bitrate = (payload as u64 * 8 * rate as u64 / n as u64) as u32;
    let mut enc = LbrEncoder::new(LbrConfig {
        sample_rate: rate,
        channels: nch,
        speaker_mask: map.mask,
        lfe: map.lfe.is_some(),
        bitrate,
        dialog_mode: req.bitstream.secondary.dialog_mode,
        payload_bytes: Some(payload),
    })?;
    let mut info = DtsHdInfo::new(rate, n as u16, want, map.mask, Kind::ExtensionOnly);
    crate::container::mixmeta::apply_container_options(&mut info, &req.bitstream, want as f64 / rate as f64);
    let mut file = StreamOut::create(req.output, Some(info))?;
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
            exss.extras.mix = plan
                .as_ref()
                .map(|pl| pl.frames[(frames as usize).min(pl.frames.len() - 1)].clone());
            let f = exss_frame_lbr(&exss, &p);
            file.write_frame(&f, f.len())?;
            bytes += f.len() as u64;
            frames += 1;
        }
        on_progress(1.0 - remaining as f32 / want as f32);
    }
    for p in enc.finish() {
        exss.extras.mix = plan
            .as_ref()
            .map(|pl| pl.frames[(frames as usize).min(pl.frames.len() - 1)].clone());
        let f = exss_frame_lbr(&exss, &p);
        file.write_frame(&f, f.len())?;
        bytes += f.len() as u64;
        frames += 1;
    }
    file.finish(None)?;
    on_progress(1.0);
    Ok(LbrStats { frames, bytes })
}
