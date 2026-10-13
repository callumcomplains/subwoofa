//! File-level driver for DTS 96/24: 96 kHz stems in, raw `.dts` (core plus X96 extension) out. A DTS CD holds the
//! stream at 88.2 kHz, stereo, in 14-bit words in a WAV file (1234.8 kbit/s of stream in the CD's 1411.2).

use crate::container::dtshd::{DtsHdInfo, Kind, StreamOut};
use crate::container::exss::{ExssExtras, ExssParams};
use crate::dca::dts96::{Dts96Config, Dts96Encoder};
use crate::dca::native::{map_channels, map_layout, LayoutMap, NativeError, Request, Stats};
use crate::dca::wav_out::{pack_14bit, WavOut};
use crate::hra::stage::HraStage;
use std::sync::atomic::{AtomicBool, Ordering};
use sw_core::Channel;
use sw_pcm::StemSet;

/// What the request and the layout fix before encoding starts.
struct Plan {
    rate: u32,
    map: LayoutMap,
    /// Where the centre surround is in the layout when it is folded into the surrounds of the core.
    cs_src: Option<usize>,
    wav: bool,
    /// A DTS CD: 44.1 kHz core of 1024-sample frames, the stream packed into 14-bit words of a WAV file.
    cd: bool,
    core_rate: u32,
    blocks: usize,
    bitrate: u32,
    /// Bytes of the X96 extension in each frame: a fifth of the frame, at least 96.
    x96_bytes: usize,
}

impl Plan {
    fn new(req: &Request, rate: u32) -> Result<Plan, NativeError> {
        if rate != 96_000 && rate != 88_200 {
            return Err(
                crate::Error::Unsupported(format!("DTS 96/24 needs 88.2 or 96 kHz input, not {rate} Hz")).into(),
            );
        }
        // 6.x with a matrixed centre surround: the core is the 5.x downmix (Cs at -3 dB on both surrounds), flagged ES
        let mix_cs = req.es.is_some_and(|e| e.mix_cs && !e.xch) && req.layout.channels.contains(&Channel::Cs);
        let cs_src = req
            .layout
            .channels
            .iter()
            .position(|c| *c == Channel::Cs)
            .filter(|_| mix_cs);
        let mut map = if mix_cs {
            let idx: Vec<usize> = (0..req.layout.channels.len())
                .filter(|&i| req.layout.channels[i] != Channel::Cs)
                .collect();
            let chans: Vec<Channel> = idx.iter().map(|&i| req.layout.channels[i]).collect();
            let core_map = map_channels(&chans, req.layout.id)?;
            LayoutMap {
                amode: core_map.amode,
                order: core_map.order.iter().map(|&i| idx[i]).collect(),
                lfe: core_map.lfe.map(|i| idx[i]),
            }
        } else {
            map_layout(req.layout)?
        };
        let wav = req.output.extension().is_some_and(|e| e.eq_ignore_ascii_case("wav"));
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
            crate::dca::native::core_bitrate(req)
        };
        let frame_bytes = (bitrate as usize * (blocks * 32) / core_rate as usize / 8) & !3;
        Ok(Plan {
            rate,
            map,
            cs_src,
            wav,
            cd,
            core_rate,
            blocks,
            bitrate,
            x96_bytes: ((frame_bytes / 5).max(96)) & !3,
        })
    }

    fn encoder(&self, req: &Request) -> Result<Dts96Encoder, NativeError> {
        Ok(Dts96Encoder::new(Dts96Config {
            map: self.map.clone(),
            bitrate: self.bitrate,
            dialnorm: req.dialnorm,
            pcm_bits: req.pcm_bits,
            x96_bytes: self.x96_bytes,
            core_rate: self.core_rate,
            blocks: self.blocks,
            es: req.es.is_some_and(|e| e.flag),
            aux: crate::lossless::ma::core_aux(
                &req.bitstream.downmix,
                req.layout,
                self.map.amode,
                self.map.lfe.is_some(),
                false,
            ),
        })?)
    }

    /// Encoder input slots: the core channels, then the LFE.
    fn slots(&self) -> Vec<usize> {
        self.map.order.iter().copied().chain(self.map.lfe).collect()
    }

    /// The High Resolution stage that wraps each core frame with its extension substream.
    fn hra_stage(&self, req: &Request, kbps: u32, frame_samples: usize) -> HraStage {
        let params = ExssParams {
            sample_rate: self.rate,
            channels: self.map.order.len() + usize::from(self.map.lfe.is_some()),
            bit_res: req.pcm_bits,
            spk_mask: crate::lossless::ma::map_layout(req.layout).exss_mask,
            with_core: true,
            dialnorm: (-req.dialnorm) as u32,
            frame_duration_code: 0,
            x96_in_core: true,
            xch_in_core: false,
            secondary: false,
            extras: ExssExtras {
                min_header: 28,
                ..Default::default()
            },
        };
        HraStage::new(kbps, self.rate, frame_samples, 16, params)
    }

    /// The silent frame StreamTools' Add silence inserts: a fresh encoder fed zeros until the filter bank has settled.
    fn silent_frame(&self, req: &Request, n: usize) -> Option<Vec<u8>> {
        let mut enc = self.encoder(req).ok()?;
        if let Some(kbps) = req.hra_kbps {
            enc.set_hra(self.hra_stage(req, kbps, n));
        }
        let zeros = vec![vec![0.0f64; n]; self.slots().len()];
        let mut out = enc.push(&zeros);
        out.extend(enc.push(&zeros));
        out.extend(enc.push(&zeros));
        out.extend(enc.push(&zeros));
        out.pop()
    }

    /// One block of `got` samples shaped for the encoder: `[slot][sample]`, scaled to 24 bits, with the centre
    /// surround added to the surrounds when it is folded in.
    fn shape_block(&self, req: &Request, planar: &[Vec<f32>], got: usize) -> Vec<Vec<f64>> {
        let surround = |c: Channel| matches!(c, Channel::Ls | Channel::Rs | Channel::Lss | Channel::Rss);
        self.slots()
            .iter()
            .map(|&s| {
                let add_cs = self.cs_src.filter(|_| surround(req.layout.channels[s]));
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
            .collect()
    }
}

/// Where the finished frames go.
enum Output {
    /// A `.dtshd` file or a raw stream, and how many bytes of each frame belong to the core.
    Stream(StreamOut, u32),
    /// A WAV file; the flag says the stream is packed into 14-bit words (a DTS CD).
    Wav(WavOut, bool),
}

impl Output {
    fn write_frame(&mut self, frame: &[u8]) -> std::io::Result<()> {
        match self {
            Output::Wav(wav, cd) => wav.write(&if *cd { pack_14bit(frame) } else { frame.to_vec() }),
            Output::Stream(out, core_payload) => {
                out.write_frame(frame, frame.len().saturating_sub(*core_payload as usize))
            }
        }
    }
}

pub fn encode_to_file(
    stems: &mut StemSet,
    req: &Request,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Result<Stats, NativeError> {
    let plan = Plan::new(req, stems.sample_rate)?;
    let mut enc = plan.encoder(req)?;
    let want = stems.select_span(req.start_secs, req.duration_secs)?;
    if want == 0 {
        return Err(NativeError::Empty);
    }
    let n = enc.frame_samples();
    if let Some(kbps) = req.hra_kbps {
        enc.set_hra(plan.hra_stage(req, kbps, n));
    }
    let container = req.hra_kbps.is_some() && req.output.extension().is_some_and(|e| e.eq_ignore_ascii_case("dtshd"));
    let core_payload = (plan.bitrate * 512 / 48_000 / 8) & !3;
    let mut output = if plan.wav {
        Output::Wav(WavOut::create(req.output, 44_100)?, plan.cd)
    } else {
        let info = container.then(|| {
            let mut i = DtsHdInfo::new(
                plan.rate,
                n as u16,
                want,
                crate::lossless::ma::map_layout(req.layout).exss_mask as u16,
                Kind::CoreAndExtension,
            );
            let secs = want as f64 / plan.rate as f64;
            crate::container::mixmeta::apply_container_options(&mut i, &req.bitstream, secs);
            i.core_kbps = req.bitrate_kbps;
            i.core_payload = core_payload;
            i
        });
        Output::Stream(StreamOut::create(req.output, info)?, core_payload)
    };
    let mut planar: Vec<Vec<f32>> = Vec::new();
    let mut remaining = want;
    let (mut frames, mut bytes) = (0u64, 0u64);
    let chunk = n * 16;
    while remaining > 0 {
        if cancel.load(Ordering::Relaxed) {
            return Err(NativeError::Canceled);
        }
        let got = (remaining as usize).min(chunk);
        stems.read_planar(&mut planar, got)?;
        let block = plan.shape_block(req, &planar, got);
        remaining -= got as u64;
        for f in enc.push(&block) {
            output.write_frame(&f)?;
            frames += 1;
            bytes += f.len() as u64;
        }
        on_progress(1.0 - remaining as f32 / want as f32);
    }
    for f in enc.finish() {
        output.write_frame(&f)?;
        frames += 1;
        bytes += f.len() as u64;
    }
    match output {
        Output::Stream(out, _) => {
            let blackout = if container { plan.silent_frame(req, n) } else { None };
            out.finish(blackout.as_deref())?;
        }
        Output::Wav(wav, _) => wav.finish()?,
    }
    on_progress(1.0);
    Ok(Stats { frames, bytes })
}
