//! File-level driver for DTS 96/24: 96 kHz stems in, raw `.dts` (core plus X96 extension) out. A DTS CD holds the
//! stream at 88.2 kHz, stereo, in 14-bit words in a WAV file (1234.8 kbit/s of stream in the CD's 1411.2).

use crate::dts96::{Dts96Config, Dts96Encoder};
use crate::native::{map_layout, NativeError, Request, Stats};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use sw_pcm::StemSet;

pub fn encode_to_file(
    stems: &mut StemSet,
    req: &Request,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Result<Stats, NativeError> {
    let rate = stems.sample_rate;
    if rate != 96_000 && rate != 88_200 {
        return Err(crate::Error::Unsupported(format!("DTS 96/24 needs 88.2 or 96 kHz input, not {rate} Hz")).into());
    }
    // 6.x with a matrixed centre surround: the core is the 5.x downmix (Cs at -3 dB on both surrounds), flagged ES
    let mix_cs = req.es.is_some_and(|e| e.mix_cs && !e.xch) && req.layout.channels.contains(&sw_core::Channel::Cs);
    let cs_src = req
        .layout
        .channels
        .iter()
        .position(|c| *c == sw_core::Channel::Cs)
        .filter(|_| mix_cs);
    let mut map = if mix_cs {
        let idx: Vec<usize> = (0..req.layout.channels.len())
            .filter(|&i| req.layout.channels[i] != sw_core::Channel::Cs)
            .collect();
        let chans: Vec<sw_core::Channel> = idx.iter().map(|&i| req.layout.channels[i]).collect();
        let cm = crate::native::map_channels(&chans, req.layout.id)?;
        crate::native::LayoutMap {
            amode: cm.amode,
            order: cm.order.iter().map(|&i| idx[i]).collect(),
            lfe: cm.lfe.map(|i| idx[i]),
        }
    } else {
        map_layout(req.layout)?
    };
    let nprim = map.order.len();
    let wav = req.output.extension().is_some_and(|e| e.eq_ignore_ascii_case("wav"));
    // a DTS CD: 44.1 kHz core of 1024-sample frames, the stream packed into 14-bit words of a WAV file
    let cd = wav && rate == 88_200 && matches!(req.bitrate_kbps, 1411 | 1234 | 1235);
    let (core_rate, blocks) = if rate == 88_200 {
        (44_100u32, if cd { 32usize } else { 16 })
    } else {
        (48_000, 16)
    };
    if cd && map.amode == 4 {
        map.amode = 2; // a DTS CD at 88.2 kHz does not flag matrix-encoded stereo
    }
    let bitrate = if cd {
        1_234_800
    } else {
        crate::native::core_bitrate(req)
    };
    // a fifth of the frame for the extension, at least 96 bytes
    let frame_bytes = (bitrate as usize * (blocks * 32) / core_rate as usize / 8) & !3;
    let x96_bytes = ((frame_bytes / 5).max(96)) & !3;
    let make_enc = || {
        Dts96Encoder::new(Dts96Config {
            map: map.clone(),
            bitrate,
            dialnorm: req.dialnorm,
            pcm_bits: req.pcm_bits,
            x96_bytes,
            core_rate,
            blocks,
            es: req.es.is_some_and(|e| e.flag),
            aux: crate::ma::core_aux(&req.bitstream.downmix, req.layout, map.amode, map.lfe.is_some(), false),
        })
    };
    let mut enc = make_enc()?;
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
    let n = enc.frame_samples();
    if let Some(kbps) = req.hra_kbps {
        let ex = crate::exss::ExssParams {
            sample_rate: rate,
            channels: nprim + usize::from(map.lfe.is_some()),
            bit_res: req.pcm_bits,
            spk_mask: crate::ma::map_layout(req.layout).exss_mask,
            with_core: true,
            dialnorm: (-req.dialnorm) as u32,
            frame_duration_code: 0,
            x96_in_core: true,
            xch_in_core: false,
            secondary: false,
            extras: crate::exss::ExssExtras {
                min_header: 28,
                ..Default::default()
            },
        };
        enc.set_hra(crate::hra::HraStage::new(kbps, rate, n, 16, ex));
    }
    let container = req.hra_kbps.is_some() && req.output.extension().is_some_and(|e| e.eq_ignore_ascii_case("dtshd"));
    let core_payload = (bitrate * 512 / 48_000 / 8) & !3;
    let info = container.then(|| {
        let mut i = crate::dtshd::DtsHdInfo::new(
            rate,
            n as u16,
            want,
            crate::ma::map_layout(req.layout).exss_mask as u16,
            crate::dtshd::Kind::CoreAndExtension,
        );
        crate::mixmeta::apply_container_options(&mut i, &req.bitstream, want as f64 / rate as f64);
        i.core_kbps = req.bitrate_kbps;
        i.core_payload = core_payload;
        i
    });
    let mut wav_out = if wav {
        let mut w = std::io::BufWriter::new(std::fs::File::create(req.output)?);
        w.write_all(&crate::native::wav_header(44_100, 0))?;
        Some(w)
    } else {
        None
    };
    let mut out = if wav {
        None
    } else {
        Some(crate::dtshd::StreamOut::create(req.output, info)?)
    };
    let mut written = 0u64;
    let mut planar: Vec<Vec<f32>> = Vec::new();
    let mut remaining = want;
    let (mut frames, mut bytes) = (0u64, 0u64);
    let slots: Vec<usize> = map.order.iter().copied().chain(map.lfe).collect();
    let chunk = n * 16;
    while remaining > 0 {
        if cancel.load(Ordering::Relaxed) {
            return Err(NativeError::Canceled);
        }
        let got = (remaining as usize).min(chunk);
        stems.read_planar(&mut planar, got)?;
        let surround = |c: sw_core::Channel| {
            matches!(
                c,
                sw_core::Channel::Ls | sw_core::Channel::Rs | sw_core::Channel::Lss | sw_core::Channel::Rss
            )
        };
        let block: Vec<Vec<f64>> = slots
            .iter()
            .map(|&s| {
                let add_cs = cs_src.filter(|_| surround(req.layout.channels[s]));
                (0..got)
                    .map(|i| {
                        let mut v = planar[s][i] as f64 * 8_388_608.0;
                        if let Some(cs) = add_cs {
                            v += planar[cs][i] as f64 * 8_388_608.0 * std::f64::consts::FRAC_1_SQRT_2;
                        }
                        v.clamp(-8_388_608.0, 8_388_607.0)
                    })
                    .collect()
            })
            .collect();
        remaining -= got as u64;
        for f in enc.push(&block) {
            put(&mut out, &mut wav_out, &mut written, &f, core_payload, cd)?;
            frames += 1;
            bytes += f.len() as u64;
        }
        on_progress(1.0 - remaining as f32 / want as f32);
    }
    for f in enc.finish() {
        put(&mut out, &mut wav_out, &mut written, &f, core_payload, cd)?;
        frames += 1;
        bytes += f.len() as u64;
    }
    // the silent frame StreamTools' Add silence inserts: a fresh encoder fed zeros until the filter bank has settled
    let blackout = if container {
        make_enc().ok().and_then(|mut e| {
            if let Some(kbps) = req.hra_kbps {
                let ex = crate::exss::ExssParams {
                    sample_rate: rate,
                    channels: nprim + usize::from(map.lfe.is_some()),
                    bit_res: req.pcm_bits,
                    spk_mask: crate::ma::map_layout(req.layout).exss_mask,
                    with_core: true,
                    dialnorm: (-req.dialnorm) as u32,
                    frame_duration_code: 0,
                    x96_in_core: true,
                    xch_in_core: false,
                    secondary: false,
                    extras: crate::exss::ExssExtras {
                        min_header: 28,
                        ..Default::default()
                    },
                };
                e.set_hra(crate::hra::HraStage::new(kbps, rate, n, 16, ex));
            }
            let zeros = vec![vec![0.0f64; n]; slots.len()];
            let mut out = e.push(&zeros);
            out.extend(e.push(&zeros));
            out.extend(e.push(&zeros));
            out.extend(e.push(&zeros));
            out.pop()
        })
    } else {
        None
    };
    if let Some(o) = out {
        o.finish(blackout.as_deref())?;
    }
    if let Some(mut w) = wav_out {
        use std::io::{Seek, SeekFrom};
        let pad = (4 - written % 4) % 4;
        w.write_all(&vec![0u8; pad as usize])?;
        w.flush()?;
        let mut inner = w.into_inner().map_err(|e| e.into_error())?;
        inner.seek(SeekFrom::Start(0))?;
        inner.write_all(&crate::native::wav_header(44_100, written + pad))?;
        inner.flush()?;
    }
    on_progress(1.0);
    Ok(Stats { frames, bytes })
}

/// Write one frame: into the `.dtshd` container or raw file, or (a DTS CD) packed into 14-bit words in the WAV file.
fn put(
    out: &mut Option<crate::dtshd::StreamOut>,
    wav: &mut Option<std::io::BufWriter<std::fs::File>>,
    written: &mut u64,
    frame: &[u8],
    core_payload: u32,
    cd: bool,
) -> std::io::Result<()> {
    if let Some(w) = wav {
        let data = if cd {
            crate::native::pack_14bit(frame)
        } else {
            frame.to_vec()
        };
        w.write_all(&data)?;
        *written += data.len() as u64;
    } else if let Some(o) = out {
        o.write_frame(frame, frame.len().saturating_sub(core_payload as usize))?;
    }
    Ok(())
}
