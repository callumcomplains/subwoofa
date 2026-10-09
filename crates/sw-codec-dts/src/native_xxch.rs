//! DTS-HD High Resolution Audio for 7.1 / 7.0: a 5.1 (5.0) core plus an XXCH extension that carries the rear
//! pair, in the extension substream. The core's surround channels hold the sides with the rears folded in
//! (embedded downmix, signalled in the XXCH channel set header), so a decoder without XXCH plays a proper
//! 5.1 mix and an XXCH decoder subtracts the rears again.

use crate::encoder::XxchParams;
use crate::exss::{dtshd_header, exss_frame_xxch, DtsHdHeader, ExssParams};
use crate::native::{NativeError, Request, Stats};
use crate::{Config, Encoder};
use std::io::{Seek, SeekFrom, Write};
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
    let exss = ExssParams { sample_rate: rate, channels: nch_total, bit_res: req.pcm_bits, spk_mask: spk, with_core: true, dialnorm: 0, frame_duration_code: 0 };
    // speakers: core C L R LFE Lss Rss, extension Lsr Rsr
    let core_mask = 0x1 | 0x2 | 0x4 | (if has_lfe { 0x20 } else { 0 }) | (1 << 9) | (1 << 10);
    let params = XxchParams { core_mask, spk_mask: (1 << 7) | (1 << 8), mask_bits: 11, dmix: Some((55, vec![vec![(9, 119)], vec![(10, 119)]])) };
    let overhead = exss_frame_xxch(&exss, &[0u8]).len() - 1;
    let (bits_num, bits_den) = (total_kbps as u64 * 1000 * n as u64, rate as u64);
    let upto = |k: u64| (k * bits_num) / (bits_den * 8);

    let container = req.output.extension().map_or(false, |e| e.eq_ignore_ascii_case("dtshd"));
    let mut out = std::io::BufWriter::new(std::fs::File::create(req.output)?);
    let header = |frames: u32, bytes: u64| dtshd_header(&DtsHdHeader { sample_rate: rate, frames, samples_per_frame: n as u16, total_samples: want, ch_mask: spk as u16, initial_padding: 0 }, bytes);
    let header_len = if container {
        let h = header(0, 0);
        out.write_all(&h)?;
        h.len() as u64
    } else {
        0
    };

    let mut planar: Vec<Vec<f32>> = Vec::new();
    let mut remaining = want;
    let (mut frames, mut bytes) = (0u64, 0u64);
    let total_frames = want.div_ceil(n as u64) + 1;
    let mut core_pcm: Vec<Vec<f64>> = vec![vec![0.0; n]; 5 + usize::from(has_lfe)];
    let mut ext_pcm: Vec<Vec<f64>> = vec![vec![0.0; n]; 2];
    let to24 = |v: f32| v as f64 * 8_388_608.0;
    for f in 0..total_frames {
        if cancel.load(Ordering::Relaxed) {
            return Err(NativeError::Canceled);
        }
        let got = (remaining as usize).min(n);
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
                core_pcm[slot][i] = v.clamp(-8_388_608.0, 8_388_607.0);
            }
            if let Some(l) = map.lfe {
                core_pcm[5][i] = (to24(planar[l][i]) * SCALE).clamp(-8_388_608.0, 8_388_607.0);
            }
            for (slot, &src) in map.rear.iter().enumerate() {
                ext_pcm[slot][i] = to24(planar[src][i]).clamp(-8_388_608.0, 8_388_607.0);
            }
        }
        remaining -= got as u64;
        let mut data = core.encode_frame(&core_pcm);
        let pad = (4 - data.len() % 4) % 4;
        data.resize(data.len() + pad, 0);
        let total = (upto(f + 1) - upto(f)) as usize;
        let budget = total.saturating_sub(data.len() + overhead).clamp(64, 16_000);
        let x = ext.encode_xxch(&ext_pcm, budget, &params);
        data.extend_from_slice(&exss_frame_xxch(&exss, &x));
        out.write_all(&data)?;
        frames += 1;
        bytes += data.len() as u64;
        on_progress(1.0 - remaining as f32 / want as f32);
    }
    out.flush()?;
    if container {
        let h = header(frames as u32, bytes);
        debug_assert_eq!(h.len() as u64, header_len);
        let mut inner = out.into_inner().map_err(|e| e.into_error())?;
        inner.seek(SeekFrom::Start(0))?;
        inner.write_all(&h)?;
        inner.flush()?;
    }
    on_progress(1.0);
    Ok(Stats { frames, bytes })
}
