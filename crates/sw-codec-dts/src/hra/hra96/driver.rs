//! File-level driver: 96 kHz stems in, `.dtshd` (or raw) out.

use crate::container::exss::ExssParams;
use crate::dca::native::{NativeError, Request, Stats};
use crate::dca::native_xxch::XxchMix;
use crate::hra::hra96::encoder::{Hra96Config, Hra96Encoder};
use crate::hra::hra96::map::{Hra96Kind, Hra96Map};
use crate::hra::hra96::CS_MIX;
use crate::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use sw_pcm::StemSet;

/// What the request and the layout fix before encoding starts.
pub(crate) struct Plan {
    pub(crate) rate: u32,
    pub(crate) map: Hra96Map,
    pub(crate) kind: Hra96Kind,
    pub(crate) total_kbps: u32,
    /// Primary channels in the core.
    pub(crate) nprim: usize,
    pub(crate) has_lfe: bool,
    /// Speakers the extension header describes: the core's and what is coded beside them.
    pub(crate) exss_mask: u32,
    /// Input buffers of the encoder: core channels, LFE and the extra channels it takes.
    pub(crate) buffers: usize,
}

impl Plan {
    fn new(req: &Request, map: Hra96Map, rate: u32) -> Result<Plan, NativeError> {
        if rate != 96_000 {
            return Err(Error::Unsupported(
                "HRA above 5.1 channels, and HRA 6.x, are implemented at 96 kHz (and 7.x at 48 kHz)".into(),
            )
            .into());
        }
        if req.es.is_some_and(|e| e.phase_shift || e.rear_minus_3db) {
            return Err(Error::Unsupported(
                "the ES phase shift and the rear -3 dB trim are not available for DTS-HD High Resolution at 96 kHz"
                    .into(),
            )
            .into());
        }
        let kind = map.kind;
        let total_kbps = req
            .hra_kbps
            .unwrap_or(if kind == Hra96Kind::Rear { 5376 } else { 4224 });
        let has_lfe = map.lfe.is_some();
        let nprim = map.core.len();
        let exss_mask: u32 = req
            .layout
            .channels
            .iter()
            .enumerate()
            .filter(|(i, _)| !(kind == Hra96Kind::Matrix && map.extra.contains(i)))
            .map(|(_, c)| crate::lossless::ma::exss_bit(*c))
            .fold(0, |m, b| m | b);
        Ok(Plan {
            rate,
            kind,
            total_kbps,
            nprim,
            has_lfe,
            exss_mask,
            buffers: nprim + usize::from(has_lfe) + kind.extra_channels(),
            map,
        })
    }

    /// The channels the extension header describes: the core channels, and what is coded beside them.
    fn exss(&self, req: &Request) -> ExssParams {
        let coded_extra = if self.kind == Hra96Kind::Matrix {
            0
        } else {
            self.map.extra.len()
        };
        ExssParams {
            sample_rate: self.rate,
            channels: self.nprim + usize::from(self.has_lfe) + coded_extra,
            bit_res: req.pcm_bits,
            spk_mask: self.exss_mask,
            with_core: true,
            dialnorm: (-req.dialnorm) as u32,
            frame_duration_code: 0,
            x96_in_core: false,
            xch_in_core: self.kind == Hra96Kind::Discrete,
            secondary: false,
            extras: crate::container::exss::ExssExtras {
                min_header: 28,
                repr_type: (req.layout.kind == sw_core::layout::LayoutKind::LtRt).then_some(2),
                ..Default::default()
            },
        }
    }

    fn encoder(&self, req: &Request) -> Result<Hra96Encoder, Error> {
        Hra96Encoder::new(Hra96Config {
            amode: self.map.amode,
            nprim: self.nprim,
            has_lfe: self.has_lfe,
            kind: self.kind,
            total_kbps: self.total_kbps,
            dialnorm: req.dialnorm,
            pcm_bits: req.pcm_bits,
            exss: self.exss(req),
            core_es: req.layout.id == "7.1-ls-rs-lsr-rsr",
            xxch: crate::dca::native_xxch::xxch_map(req.layout).map(|m| m.params(&req.bitstream.downmix)),
            aux: crate::lossless::ma::core_aux(
                &req.bitstream.downmix,
                req.layout,
                self.map.amode,
                self.has_lfe,
                self.kind == Hra96Kind::Discrete,
            ),
        })
    }

    /// What the core carries: the 5.1 downmix of a 7.1 programme, or the core channels alone.
    fn core_mask(&self, req: &Request) -> u32 {
        if self.kind != Hra96Kind::Rear {
            return self.exss_mask;
        }
        let speakers = self
            .map
            .core
            .iter()
            .map(|&i| crate::lossless::ma::exss_bit(req.layout.channels[i]))
            .fold(0, |m, b| m | b);
        speakers | if self.has_lfe { 0x8 } else { 0 }
    }

    /// The header of the `.dtshd` file.
    fn container_info(&self, req: &Request, n: usize, want: u64) -> crate::container::dtshd::DtsHdInfo {
        let mut info = crate::container::dtshd::DtsHdInfo::new(
            self.rate,
            n as u16,
            want,
            self.exss_mask as u16,
            crate::container::dtshd::Kind::CoreAndExtension,
        );
        let secs = want as f64 / self.rate as f64;
        crate::container::mixmeta::apply_container_options(&mut info, &req.bitstream, secs);
        info.core_kbps = 1509;
        info.core_payload = CORE_PAYLOAD;
        info.codec_delay = CODEC_DELAY as u16;
        info.core_rate = Some(48_000);
        info.core_mask = Some(self.core_mask(req) as u16);
        info
    }

    /// The silent frame StreamTools' Add silence inserts: a fresh encoder fed zeros until its filters have settled.
    fn silent_frame(&self, req: &Request, n: usize) -> Option<Vec<u8>> {
        let mut enc = self.encoder(req).ok()?;
        let zeros = vec![vec![0.0f64; 6 * n]; self.buffers];
        enc.push(&zeros).pop()
    }

    /// One block of `got` samples shaped for the encoder: `[buffer][sample]`, scaled to 24 bits.
    fn shape_block(&self, mix: Option<&XxchMix>, planar: &[Vec<f32>], got: usize) -> Vec<Vec<f64>> {
        let (map, kind) = (&self.map, self.kind);
        let to24 = |v: f32| v as f64 * 8_388_608.0;
        let clamp = |v: f64| v.clamp(-8_388_608.0, 8_388_607.0);
        let lfe_slot = self.nprim;
        let extra_start = self.nprim + usize::from(self.has_lfe);
        let mut block: Vec<Vec<f64>> = vec![vec![0.0; got]; self.buffers];
        for i in 0..got {
            for (slot, &src) in map.core.iter().enumerate() {
                let value = match kind {
                    // 7.x: scaled, with the rear on the same side added to the sides
                    Hra96Kind::Rear => {
                        let m = mix.expect("7.x mix");
                        let mut v = to24(planar[src][i]) * m.scale;
                        for e in 0..2 {
                            if m.extra[e][slot] != 0.0 {
                                v += to24(planar[map.extra[e]][i]) * m.extra[e][slot];
                            }
                        }
                        v
                    }
                    // 6.x: the centre surround at -3 dB on both surrounds
                    Hra96Kind::Matrix | Hra96Kind::Discrete if slot >= 3 => {
                        to24(planar[src][i]) + to24(planar[map.extra[0]][i]) * CS_MIX
                    }
                    _ => to24(planar[src][i]),
                };
                block[slot][i] = clamp(value);
            }
            if let Some(l) = map.lfe {
                let v = to24(planar[l][i]);
                block[lfe_slot][i] = clamp(match mix {
                    Some(m) if kind == Hra96Kind::Rear => {
                        v * m.scale
                            + (0..2)
                                .map(|e| to24(planar[map.extra[e]][i]) * m.extra[e][5])
                                .sum::<f64>()
                    }
                    _ => v,
                });
            }
            match kind {
                Hra96Kind::Rear => {
                    for (j, &src) in map.extra.iter().enumerate() {
                        block[extra_start + j][i] = clamp(to24(planar[src][i]));
                    }
                }
                Hra96Kind::Discrete => block[extra_start][i] = clamp(to24(planar[map.extra[0]][i])),
                _ => {}
            }
        }
        block
    }
}

/// Bytes of the 1509 kbps core in each frame.
pub(crate) const CORE_PAYLOAD: u32 = (1509u32 * 1000 * 512 / 48_000 / 8) & !3;

/// How far the decoder's output lags the source; a container states it and the stream starts with silence to match.
pub(crate) const CODEC_DELAY: usize = 2048;

/// The output file and a count of what went into it.
pub(crate) struct Writer {
    pub(crate) file: crate::container::dtshd::StreamOut,
    pub(crate) frames: u64,
    pub(crate) bytes: u64,
}

impl Writer {
    fn write(&mut self, frames: Vec<Vec<u8>>) -> std::io::Result<()> {
        for f in frames {
            self.file
                .write_frame(&f, f.len().saturating_sub(CORE_PAYLOAD as usize))?;
            self.frames += 1;
            self.bytes += f.len() as u64;
        }
        Ok(())
    }
}

/// File-level driver: 96 kHz stems in, `.dtshd` (or raw) out.
pub fn encode_to_file(
    stems: &mut StemSet,
    req: &Request,
    map: Hra96Map,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Result<Stats, NativeError> {
    let plan = Plan::new(req, map, stems.sample_rate)?;
    let mix = crate::dca::native_xxch::xxch_map(req.layout).map(|m| m.mix(&req.bitstream.downmix));
    let mut enc = plan.encoder(req)?;
    let want = stems.select_span(req.start_secs, req.duration_secs)?;
    if want == 0 {
        return Err(NativeError::Empty);
    }
    let n = enc.frame_samples();
    let container = req.output.extension().is_some_and(|e| e.eq_ignore_ascii_case("dtshd"));
    let info = container.then(|| plan.container_info(req, n, want));
    let mut out = Writer {
        file: crate::container::dtshd::StreamOut::create(req.output, info)?,
        frames: 0,
        bytes: 0,
    };
    // the decoder lags the source by `delay`: a container states `CODEC_DELAY` and the stream starts with silence
    // to make up the difference; a raw stream starts that many samples into the source instead
    let delay = enc.delay();
    let (skip, lead) = if container {
        (0usize, CODEC_DELAY.saturating_sub(delay))
    } else {
        (delay.min(want as usize), 0)
    };
    let mut planar: Vec<Vec<f32>> = Vec::new();
    let mut left = skip as u64;
    while left > 0 {
        let k = left.min(n as u64) as usize;
        stems.read_planar(&mut planar, k)?;
        left -= k as u64;
    }
    let mut remaining = want - skip as u64;
    if lead > 0 {
        out.write(enc.push(&vec![vec![0.0f64; lead]; plan.buffers]))?;
    }
    let chunk = n * 16;
    while remaining > 0 {
        if cancel.load(Ordering::Relaxed) {
            return Err(NativeError::Canceled);
        }
        let got = (remaining as usize).min(chunk);
        stems.read_planar(&mut planar, got)?;
        let block = plan.shape_block(mix.as_ref(), &planar, got);
        remaining -= got as u64;
        out.write(enc.push(&block))?;
        on_progress(1.0 - remaining as f32 / want as f32);
    }
    out.write(enc.finish())?;
    let blackout = if container { plan.silent_frame(req, n) } else { None };
    out.file.finish(blackout.as_deref())?;
    on_progress(1.0);
    Ok(Stats {
        frames: out.frames,
        bytes: out.bytes,
    })
}
