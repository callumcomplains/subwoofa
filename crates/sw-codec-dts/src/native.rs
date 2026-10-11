//! File-level driver: stems in, raw `.dts` core stream out.

use crate::es::{EsMixer, EsOptions};
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
    map_channels(layout.channels, layout.id)
}

/// Like `map_layout` for a plain list of channels (side surrounds count as surrounds; the indices of the
/// result refer to `channels`).
pub fn map_channels(channels: &[Channel], name: &str) -> Result<LayoutMap, Error> {
    use Channel::*;
    let canon: Vec<Channel> = channels.iter().map(|c| match c { Lss => Ls, Rss => Rs, c => *c }).collect();
    let pos = |c: Channel| canon.iter().position(|x| *x == c);
    let lfe = pos(Lfe);
    let main: Vec<Channel> = canon.iter().copied().filter(|c| *c != Lfe).collect();
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
        return Err(Error::Unsupported(format!("layout '{}' needs the DTS extension channels (not implemented yet)", name)));
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
    /// DTS-ES (matrix): flag the stream and, for layouts with a separate Cs, mix it into the surrounds.
    pub es: Option<EsOptions>,
    /// DTS-HD High Resolution: total bit rate (core plus XBR extension) in kbit/s.
    pub hra_kbps: Option<u32>,
    /// DTS 96/24: the stems are 96 kHz and an X96 extension follows the core.
    pub x96: bool,
    /// Bitstream-tab metadata (programme information, seamless branching, wide 7.1 remapping).
    pub bitstream: sw_core::BitstreamOptions,
    /// DVD-Video: the two legal rates are 754.5 and 1509.75 kbit/s (frames of 1006 and 2013 bytes), not 754 and 1509.
    pub dvd: bool,
}

pub struct Stats {
    pub frames: u64,
    pub bytes: u64,
}

/// The core's bit rate in bit/s for the request: DVD rates are a quarter percent above the nominal ones.
pub fn core_bitrate(req: &Request) -> u32 {
    match (req.dvd, req.bitrate_kbps) {
        (true, 1509) => 1_509_750,
        (true, 754) => 754_500,
        (_, k) => k * 1000,
    }
}

pub fn encode_to_file(stems: &mut StemSet, req: &Request, on_progress: &mut dyn FnMut(f32), cancel: &AtomicBool) -> Result<Stats, NativeError> {
    if req.hra_kbps.is_some() && stems.sample_rate > 48_000 {
        if let Some(m) = crate::hra96::hra96_map(req.layout) {
            return crate::hra96::encode_to_file(stems, req, m, on_progress, cancel);
        }
    }
    if req.x96 || (req.hra_kbps.is_some() && stems.sample_rate > 48_000) {
        return crate::native96::encode_to_file(stems, req, on_progress, cancel);
    }
    if req.hra_kbps.is_some() {
        if let Some(m) = crate::native_xxch::xxch_map(req.layout) {
            return crate::native_xxch::encode_to_file(stems, req, m, on_progress, cancel);
        }
    }
    // channels that go into the core (a separate Cs is folded into the surrounds first)
    let mix_cs = req.es.map_or(false, |e| e.mix_cs);
    let core_idx: Vec<usize> = (0..req.layout.channels.len()).filter(|&i| !(mix_cs && req.layout.channels[i] == Channel::Cs)).collect();
    let core_chs: Vec<Channel> = core_idx.iter().map(|&i| req.layout.channels[i]).collect();
    let cmap = map_channels(&core_chs, req.layout.id)?;
    let map = LayoutMap { amode: cmap.amode, order: cmap.order.iter().map(|&i| core_idx[i]).collect(), lfe: cmap.lfe.map(|i| core_idx[i]) };
    let xch = req.es.map_or(false, |e| e.xch);
    let cs_slot = req.layout.channels.iter().position(|c| *c == Channel::Cs).filter(|_| xch);
    let mut mixer = match req.es {
        Some(o) if o.mix_cs || o.phase_shift || o.rear_minus_3db => Some(EsMixer::new(o, req.layout.channels).ok_or_else(|| Error::Unsupported("the ES options need a layout with surround channels".into()))?),
        _ => None,
    };
    let mix_delay = req.es.map_or(0, |e| e.delay());
    let rate = stems.sample_rate;
    let nch = map.order.len();
    let wav = req.output.extension().map_or(false, |e| e.eq_ignore_ascii_case("wav"));
    // A DTS CD packs the stream into 14-bit words (16-bit containers, so a player that does not know DTS plays
    // quiet noise): 1024-sample frames of 3584 bytes, 1234.8 kbit/s of stream in the 1411.2 kbit/s of the disc.
    let cd14 = wav && rate == 44_100 && matches!(req.bitrate_kbps, 1411 | 1234 | 1235);
    let bitrate = if cd14 { 1_234_800 } else { core_bitrate(req) };
    // 512-sample frames up to 1.5 Mbit/s are what players expect; longer frames for low rates
    let blocks = if (bitrate >= 700_000 || rate > 48_000) && !cd14 { 16 } else { 32 };
    let make_config = || Config { sample_rate: rate, amode: map.amode, channels: nch, lfe: map.lfe.is_some(), bitrate, dialnorm: req.dialnorm, pcm_bits: req.pcm_bits, es: req.es.map_or(false, |e| e.flag), xch, perfect: false, blocks };
    let aux = crate::ma::core_aux(&req.bitstream.downmix, req.layout, map.amode, map.lfe.is_some(), xch);
    let mut enc = Encoder::new(make_config())?;
    enc.set_aux_downmix(aux.clone());
    let first = ((req.start_secs * rate as f64).round() as u64).min(stems.frames);
    stems.seek_frame(first)?;
    let avail = stems.frames - first;
    let want = req.duration_secs.map(|d| (d * rate as f64).round() as u64).unwrap_or(avail).min(avail);
    if want == 0 {
        return Err(NativeError::Empty);
    }
    let n = enc.frame_samples();
    let make_hra = |kbps: u32| {
        let ex = crate::exss::ExssParams { sample_rate: rate, channels: nch + usize::from(map.lfe.is_some()) + usize::from(xch), bit_res: req.pcm_bits, spk_mask: crate::ma::map_layout(req.layout).exss_mask & !(if req.es.map_or(false, |e| e.mix_cs && !e.xch) { crate::ma::exss_bit(sw_core::Channel::Cs) } else { 0 }), with_core: true, dialnorm: (-req.dialnorm) as u32, frame_duration_code: 0, x96_in_core: false, xch_in_core: xch, secondary: false, extras: crate::exss::ExssExtras { min_header: 28, ..Default::default() } };
        crate::hra::HraStage::new(kbps, rate, n, blocks, ex)
    };
    let mut hra = req.hra_kbps.map(|kbps| {
        enc.keep_residual = true;
        let mut h = make_hra(kbps);
        if xch {
            // the XCH channel's refinement is a channel set of its own
            h.set_split(vec![nch, 1]);
        }
        h
    });
    let container = hra.is_some() && req.output.extension().map_or(false, |e| e.eq_ignore_ascii_case("dtshd"));
    // The decoder's filter bank delays its output. In a .dtshd file the stream starts with silence so the delay
    // comes to 1024 samples (what the commercial encoder does) and the file says so: decoders that honour the
    // field drop exactly that many samples and every source sample is kept. A raw stream has nowhere to say it,
    // so there the encoder starts that many samples into the source and the decoded audio lines up (losing
    // the first few milliseconds, which fall in the decoder's start-up).
    let delay = enc.delay() + mix_delay;
    let (skip, lead) = if container { (0usize, 1024usize.saturating_sub(delay)) } else { (delay.min(want as usize), 0) };
    let core_payload = (bitrate * 512 / 48_000 / 8) & !3;
    let mut hd = if container {
        let mut i = crate::dtshd::DtsHdInfo::new(rate, n as u16, want, crate::ma::map_layout(req.layout).exss_mask as u16, crate::dtshd::Kind::CoreAndExtension);
        crate::mixmeta::apply_container_options(&mut i, &req.bitstream, want as f64 / rate as f64);
        i.core_kbps = bitrate / 1000;
        i.core_payload = core_payload;
        i.codec_delay = (delay + lead) as u16;
        Some(crate::dtshd::DtsHdSink::create(req.output, i)?)
    } else {
        None
    };
    let mut out = if container { None } else { Some(std::io::BufWriter::new(std::fs::File::create(req.output)?)) };
    if wav {
        out.as_mut().unwrap().write_all(&wav_header(rate, 0))?;
    }
    let mut planar: Vec<Vec<f32>> = Vec::new();
    let mut left = skip as u64;
    while left > 0 {
        let k = left.min(n as u64) as usize;
        stems.read_planar(&mut planar, k)?;
        left -= k as u64;
    }
    let mut remaining = want - skip as u64;
    let mut lead_left = lead;
    let mut frames = 0u64;
    let mut bytes = 0u64;
    // one extra frame flushes the filter bank delay
    let total_frames = (remaining + (mix_delay + lead) as u64).div_ceil(n as u64) + 1;
    let mut pcm: Vec<Vec<f64>> = vec![vec![0.0; n]; nch + usize::from(xch) + usize::from(map.lfe.is_some())];
    for _ in 0..total_frames {
        if cancel.load(Ordering::Relaxed) {
            return Err(NativeError::Canceled);
        }
        let zeros = lead_left.min(n);
        lead_left -= zeros;
        let got = (remaining as usize).min(n - zeros);
        if got > 0 {
            stems.read_planar(&mut planar, got)?;
        }
        let consumed = got;
        if let Some(m) = mixer.as_mut() {
            // the mixer works on whole blocks (silence past the end of the programme)
            let block: Vec<Vec<f32>> = (0..req.layout.channels.len())
                .map(|c| {
                    // the lead of silence (a container's codec delay) first, then the source
                    let mut v = vec![0.0f32; zeros];
                    if got > 0 {
                        v.extend_from_slice(&planar[c][..got]);
                    }
                    v.resize(n, 0.0);
                    v
                })
                .collect();
            planar = m.process(&block);
        }
        // with a mixer the block already holds the lead, so nothing is added in front of it
        let zeros = if mixer.is_some() { 0 } else { zeros };
        let got = if mixer.is_some() { n } else { got };
        for (slot, src) in map.order.iter().copied().chain(cs_slot).chain(map.lfe).enumerate() {
            let dst = &mut pcm[slot];
            dst.iter_mut().for_each(|v| *v = 0.0);
            for i in 0..got {
                dst[zeros + i] = (planar[src][i] as f64 * 8_388_608.0).clamp(-8_388_608.0, 8_388_607.0);
            }
        }
        remaining -= consumed.min(remaining as usize) as u64;
        let mut data = enc.encode_frame(&pcm);
        if let Some(h) = hra.as_mut() {
            data = h.wrap(data, &enc.last_residual);
        }
        if wav {
            // DTS in PCM: either the stream packed into 14-bit words (a DTS CD) or its 16-bit words
            // byte-swapped into little-endian samples
            let w = if cd14 {
                pack_14bit(&data)
            } else {
                let mut w = data.clone();
                if w.len() % 2 == 1 {
                    w.push(0);
                }
                for pair in w.chunks_exact_mut(2) {
                    pair.swap(0, 1);
                }
                w
            };
            out.as_mut().unwrap().write_all(&w)?;
            bytes += w.len() as u64;
        } else if let Some(h) = hd.as_mut() {
            h.write_frame(&data, data.len().saturating_sub(core_payload as usize))?;
            bytes += data.len() as u64;
        } else {
            out.as_mut().unwrap().write_all(&data)?;
            bytes += data.len() as u64;
        }
        frames += 1;
        on_progress(1.0 - remaining as f32 / want as f32);
    }
    if let Some(o) = out.as_mut() {
        o.flush()?;
    }
    if let Some(h) = hd.take() {
        // the silent frame StreamTools' Add silence inserts: a fresh encoder fed zeros (several frames, so the
        // filter bank has settled)
        let blackout = req.hra_kbps.and_then(|kbps| {
            let mut e = Encoder::new(make_config()).ok()?;
            e.set_aux_downmix(aux.clone());
            e.keep_residual = true;
            let mut st = make_hra(kbps);
            let zeros = vec![vec![0.0; n]; nch + usize::from(xch) + usize::from(map.lfe.is_some())];
            let mut last = None;
            for _ in 0..4 {
                let d = e.encode_frame(&zeros);
                last = Some(st.wrap(d, &e.last_residual));
            }
            last
        });
        h.finish(blackout.as_deref())?;
    } else if wav {
        use std::io::{Seek, SeekFrom};
        let pad = (4 - bytes % 4) % 4;
        let mut out = out.take().unwrap();
        out.write_all(&vec![0u8; pad as usize])?;
        out.flush()?;
        let mut inner = out.into_inner().map_err(|e| e.into_error())?;
        inner.seek(SeekFrom::Start(0))?;
        inner.write_all(&wav_header(rate, bytes + pad))?;
        inner.flush()?;
    }
    on_progress(1.0);
    Ok(Stats { frames, bytes })
}

/// Canonical 44-byte WAV header for 16-bit stereo PCM holding `data_len` bytes (a DTS stream in PCM).
pub(crate) fn wav_header(rate: u32, data_len: u64) -> Vec<u8> {
    let mut h = Vec::with_capacity(44);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&((36 + data_len).min(u32::MAX as u64) as u32).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes());
    h.extend_from_slice(&2u16.to_le_bytes());
    h.extend_from_slice(&rate.to_le_bytes());
    h.extend_from_slice(&(rate * 4).to_le_bytes());
    h.extend_from_slice(&4u16.to_le_bytes());
    h.extend_from_slice(&16u16.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&(data_len.min(u32::MAX as u64) as u32).to_le_bytes());
    h
}


/// The 14-bit DTS stream format: the bit stream is cut into 14-bit groups, each stored sign-extended in a 16-bit
/// little-endian word (a frame whose size is a multiple of 14 bytes fills whole words).
pub(crate) fn pack_14bit(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 8 / 7 + 2);
    let (mut acc, mut n) = (0u32, 0u32);
    for &b in data {
        acc = (acc << 8) | b as u32;
        n += 8;
        while n >= 14 {
            n -= 14;
            let w = ((acc >> n) & 0x3FFF) as u16;
            let signed = if w & 0x2000 != 0 { w | 0xC000 } else { w };
            out.extend_from_slice(&signed.to_le_bytes());
        }
        acc &= (1 << n) - 1;
    }
    if n > 0 {
        let w = ((acc << (14 - n)) & 0x3FFF) as u16;
        let signed = if w & 0x2000 != 0 { w | 0xC000 } else { w };
        out.extend_from_slice(&signed.to_le_bytes());
    }
    out
}
