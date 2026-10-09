//! DTS-HD High Resolution Audio for 7.1 / 7.0: a 5.1 (5.0) core plus an XXCH extension that carries the rear
//! pair, in the extension substream. The core's surround channels hold the sides with the rears folded in
//! (embedded downmix, signalled in the XXCH channel set header), so a decoder without XXCH plays a proper
//! 5.1 mix and an XXCH decoder subtracts the rears again.

use crate::encoder::XxchParams;
use crate::exss::{exss_frame_xbr_xxch, exss_frame_xxch, ExssParams};
use crate::xbr::XbrEncoder;
use crate::native::{NativeError, Request, Stats};
use crate::{Config, Encoder};
use std::sync::atomic::{AtomicBool, Ordering};
use sw_core::Channel;
use sw_pcm::StemSet;

/// Where the channels of a 7.1 / 7.0 layout go.
pub struct XxchMap {
    /// Layout index of C, L, R, Ls (side), Rs (side) in core order.
    pub core: [usize; 5],
    pub lfe: Option<usize>,
    /// Layout index of the rear pair, in XXCH coding order.
    pub rear: [usize; 2],
}

/// `Some` for the 7.1 / 7.0 layouts with side and rear surround pairs.
pub fn xxch_map(layout: &sw_core::ChannelLayout) -> Option<XxchMap> {
    use Channel::*;
    let pos = |list: &[Channel]| layout.channels.iter().position(|c| list.contains(c));
    let map = XxchMap {
        core: [pos(&[C])?, pos(&[L])?, pos(&[R])?, pos(&[Ls, Lss])?, pos(&[Rs, Rss])?],
        lfe: pos(&[Lfe]),
        rear: [pos(&[Lsr])?, pos(&[Rsr])?],
    };
    (layout.channel_count() == 7 + usize::from(map.lfe.is_some())).then_some(map)
}

/// Pre-scaling of every core channel (the decoder multiplies by `SCALE_INV` code 55) and the rear coefficient
/// (dmix table entry 217, 23170 / 32768).
const SCALE: f64 = 65536.0 / 92682.0;
const REAR_COEFF: f64 = 23170.0 / 32768.0;

pub fn encode_to_file(stems: &mut StemSet, req: &Request, map: XxchMap, on_progress: &mut dyn FnMut(f32), cancel: &AtomicBool) -> Result<Stats, NativeError> {
    let total_kbps = req.hra_kbps.unwrap_or(2046);
    let rate = stems.sample_rate;
    if rate != 48_000 {
        return Err(crate::Error::Unsupported("HRA 7.1 is implemented at 48 kHz".into()).into());
    }
    let has_lfe = map.lfe.is_some();
    // the core keeps at most 1509 kbps; the XXCH extension gets the rest
    let core_kbps = (total_kbps * 3 / 4).min(1509);
    let blocks = 16;
    let mk = |amode: u8, channels: usize, lfe: bool, bitrate: u32| Config {
        sample_rate: rate,
        amode,
        channels,
        lfe,
        bitrate,
        dialnorm: req.dialnorm,
        pcm_bits: req.pcm_bits,
        es: false,
        xch: false,
        perfect: true,
        blocks,
    };
    let mut core = Encoder::new(mk(9, 5, has_lfe, core_kbps * 1000))?;
    core.keep_residual = true;
    let xbr = XbrEncoder::new(blocks);
    let mut ext = Encoder::new(mk(2, 2, false, 1_536_000))?;
    let first = ((req.start_secs * rate as f64).round() as u64).min(stems.frames);
    stems.seek_frame(first)?;
    let avail = stems.frames - first;
    let want = req.duration_secs.map(|d| (d * rate as f64).round() as u64).unwrap_or(avail).min(avail);
    if want == 0 {
        return Err(NativeError::Empty);
    }
    let n = core.frame_samples();
    let nch_total = 7 + usize::from(has_lfe);
    let spk = crate::ma::map_layout(req.layout).exss_mask;
    let exss = ExssParams { sample_rate: rate, channels: nch_total, bit_res: req.pcm_bits, spk_mask: spk, with_core: true, dialnorm: 0, frame_duration_code: 0, x96_in_core: false, secondary: false };
    // speakers: core C L R LFE Lss Rss, extension Lsr Rsr
    let core_mask = 0x1 | 0x2 | 0x4 | (if has_lfe { 0x20 } else { 0 }) | (1 << 9) | (1 << 10);
    let params = XxchParams { core_mask, spk_mask: (1 << 7) | (1 << 8), mask_bits: 11, dmix: Some((55, vec![vec![(9, 119)], vec![(10, 119)]])) };
    let overhead = exss_frame_xxch(&exss, &[0u8]).len() - 1;
    let overhead_both = exss_frame_xbr_xxch(&exss, &[0u8], &[0u8]).len() - 2;
    let (bits_num, bits_den) = (total_kbps as u64 * 1000 * n as u64, rate as u64);
    let upto = |k: u64| (k * bits_num) / (bits_den * 8);

    let container = req.output.extension().map_or(false, |e| e.eq_ignore_ascii_case("dtshd"));
    let core_payload = (1509u32 * 1000 * 512 / 48_000 / 8) & !3;
    let info = container.then(|| {
        let mut i = crate::dtshd::DtsHdInfo::new(rate, n as u16, want, spk as u16, crate::dtshd::Kind::CoreAndExtension);
        i.core_kbps = 1509;
        i.core_payload = core_payload;
        i.codec_delay = 1024;
        i
    });
    let mut out = crate::dtshd::StreamOut::create(req.output, info)?;

    let mut planar: Vec<Vec<f32>> = Vec::new();
    // filter bank delay: in a .dtshd the stream starts with silence so the delay is 1024 samples, which the
    // file states; a raw stream starts that many samples into the source instead (see `native.rs`)
    let delay = core.delay();
    let (skip, lead) = if container { (0usize, 1024usize.saturating_sub(delay)) } else { (delay.min(want as usize), 0) };
    let mut left = skip as u64;
    while left > 0 {
        let k = left.min(n as u64) as usize;
        stems.read_planar(&mut planar, k)?;
        left -= k as u64;
    }
    let mut remaining = want - skip as u64;
    let mut lead_left = lead;
    let (mut frames, mut bytes) = (0u64, 0u64);
    let total_frames = (remaining + lead as u64).div_ceil(n as u64) + 1;
    let mut core_pcm: Vec<Vec<f64>> = vec![vec![0.0; n]; 5 + usize::from(has_lfe)];
    let mut ext_pcm: Vec<Vec<f64>> = vec![vec![0.0; n]; 2];
    let to24 = |v: f32| v as f64 * 8_388_608.0;
    for f in 0..total_frames {
        if cancel.load(Ordering::Relaxed) {
            return Err(NativeError::Canceled);
        }
        let zeros = lead_left.min(n);
        lead_left -= zeros;
        let got = (remaining as usize).min(n - zeros);
        if got > 0 {
            stems.read_planar(&mut planar, got)?;
        }
        for v in core_pcm.iter_mut().chain(ext_pcm.iter_mut()) {
            v.iter_mut().for_each(|x| *x = 0.0);
        }
        for i in 0..got {
            for (slot, &src) in map.core.iter().enumerate() {
                let mut v = to24(planar[src][i]) * SCALE;
                if slot >= 3 {
                    // the sides carry the rear on the same side
                    v += to24(planar[map.rear[slot - 3]][i]) * REAR_COEFF;
                }
                core_pcm[slot][zeros + i] = v.clamp(-8_388_608.0, 8_388_607.0);
            }
            if let Some(l) = map.lfe {
                core_pcm[5][zeros + i] = (to24(planar[l][i]) * SCALE).clamp(-8_388_608.0, 8_388_607.0);
            }
            for (slot, &src) in map.rear.iter().enumerate() {
                ext_pcm[slot][zeros + i] = to24(planar[src][i]).clamp(-8_388_608.0, 8_388_607.0);
            }
        }
        remaining -= got as u64;
        let mut data = core.encode_frame(&core_pcm);
        let pad = (4 - data.len() % 4) % 4;
        data.resize(data.len() + pad, 0);
        let total = (upto(f + 1) - upto(f)) as usize;
        // the rate beyond what the extra pair needs at the core's per-channel rate goes to XBR for the core
        // channels and XXCH in the ratio of their channel counts (a commercial 5376 kbps stream splits it that way)
        let rest = total.saturating_sub(data.len());
        let per_channel = (data.len() / (5 + usize::from(has_lfe))).max(64);
        let want_xxch = 2 * per_channel;
        let (xbr_bytes, xxch_bytes) = if rest > overhead + want_xxch + 600 {
            let extra = rest - overhead_both - want_xxch;
            let xxch = want_xxch + extra * 2 / 7;
            (rest - overhead_both - xxch, xxch)
        } else {
            (0, rest.saturating_sub(overhead).clamp(64, 16_000))
        };
        let x = ext.encode_xxch(&ext_pcm, xxch_bytes.min(16_000), &params);
        if xbr_bytes > 0 {
            let mut b = xbr.encode(&core.last_residual, xbr_bytes);
            b.resize(xbr_bytes, 0);
            data.extend_from_slice(&exss_frame_xbr_xxch(&exss, &b, &x));
        } else {
            data.extend_from_slice(&exss_frame_xxch(&exss, &x));
        }
        out.write_frame(&data, data.len().saturating_sub(core_payload as usize))?;
        frames += 1;
        bytes += data.len() as u64;
        on_progress(1.0 - remaining as f32 / want as f32);
    }
    out.finish(None)?;
    on_progress(1.0);
    Ok(Stats { frames, bytes })
}
