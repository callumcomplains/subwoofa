//! File-level driver: stems in, raw `.dts` core stream out.

use crate::container::dtshd::DtsHdSink;
use crate::container::exss::{ExssExtras, ExssParams};
use crate::dca::encoder::AuxDownmix;
use crate::dca::es::{EsMixer, EsOptions};
use crate::dca::wav_out::{pack_14bit, words_to_samples, WavOut};
use crate::hra::stage::HraStage;
use crate::lossless::ma::exss_bit;
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
    let canon: Vec<Channel> = channels
        .iter()
        .map(|c| match c {
            Lss => Ls,
            Rss => Rs,
            c => *c,
        })
        .collect();
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
        return Err(Error::Unsupported(format!(
            "layout '{}' needs the DTS extension channels (not implemented yet)",
            name
        )));
    };
    Ok(LayoutMap {
        amode,
        order: order.iter().map(|c| pos(*c).unwrap()).collect(),
        lfe,
    })
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

/// Frames encoded together; they are spread over the cores.
const BATCH: usize = 32;

/// The drivers for the cases that have their own: 96 kHz High Resolution, 96/24 and 7.1 High Resolution.
fn other_driver(
    stems: &mut StemSet,
    req: &Request,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Option<Result<Stats, NativeError>> {
    let high_rate = req.hra_kbps.is_some() && stems.sample_rate > 48_000;
    if high_rate {
        if let Some(m) = crate::hra::hra96::hra96_map(req.layout) {
            return Some(crate::hra::hra96::encode_to_file(stems, req, m, on_progress, cancel));
        }
    }
    if req.x96 || high_rate {
        return Some(crate::dca::native96::encode_to_file(stems, req, on_progress, cancel));
    }
    if req.hra_kbps.is_some() {
        if let Some(m) = crate::dca::native_xxch::xxch_map(req.layout) {
            return Some(crate::dca::native_xxch::encode_to_file(
                stems,
                req,
                m,
                on_progress,
                cancel,
            ));
        }
    }
    None
}

/// Encodes the stems as a DTS core stream (with an XBR extension for High Resolution), into a raw `.dts` file, a
/// `.dtshd` file or a WAV file. The 96 kHz, XXCH and 96 kHz High Resolution variants have their own drivers.
pub fn encode_to_file(
    stems: &mut StemSet,
    req: &Request,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Result<Stats, NativeError> {
    if let Some(done) = other_driver(stems, req, on_progress, cancel) {
        return done;
    }
    let mut plan = Plan::new(req, stems.sample_rate)?;
    let mut enc = Encoder::new(plan.config(req))?;
    let aux = plan.aux_downmix(req);
    enc.set_aux_downmix(aux.clone());
    let want = stems.select_span(req.start_secs, req.duration_secs)?;
    if want == 0 {
        return Err(NativeError::Empty);
    }
    let n = enc.frame_samples();
    let mut hra = req.hra_kbps.map(|kbps| {
        enc.keep_residual = true;
        let mut h = plan.hra_stage(req, kbps, n);
        if plan.xch {
            // the XCH channel's refinement is a channel set of its own
            h.set_split(vec![plan.nch, 1]);
        }
        h
    });
    let container = hra.is_some() && has_extension(req.output, "dtshd");
    // The decoder's filter bank delays its output. In a .dtshd file the stream starts with silence so the delay
    // comes to 1024 samples (what the commercial encoder does) and the file says so: decoders that honour the
    // field drop exactly that many samples and every source sample is kept. A raw stream has nowhere to say it,
    // so there the encoder starts that many samples into the source and the decoded audio lines up (losing
    // the first few milliseconds, which fall in the decoder's start-up).
    let mix_delay = req.es.map_or(0, |e| e.delay());
    let delay = enc.delay() + mix_delay;
    let (skip, lead) = if container {
        (0usize, 1024usize.saturating_sub(delay))
    } else {
        (delay.min(want as usize), 0)
    };
    let core_payload = (plan.bitrate * 512 / 48_000 / 8) & !3;
    let mut output = Output::open(req, &plan, container, n, want, delay + lead, core_payload)?;
    let mut feeder = Feeder::new(&mut plan, req, n, want - skip as u64, lead);
    feeder.skip(stems, skip as u64)?;
    // one extra frame flushes the filter bank delay
    let total_frames = (feeder.remaining + (mix_delay + lead) as u64).div_ceil(n as u64) + 1;
    let (mut frames, mut bytes, mut coded) = (0u64, 0u64, 0u64);
    while coded < total_frames {
        if cancel.load(Ordering::Relaxed) {
            return Err(NativeError::Canceled);
        }
        // the input of the next frames, then all of them are encoded together
        let count = BATCH.min((total_frames - coded) as usize);
        let chunk = (0..count)
            .map(|_| feeder.next_frame(stems))
            .collect::<Result<Vec<_>, _>>()?;
        coded += count as u64;
        let refs: Vec<&[Vec<f64>]> = chunk.iter().map(|f| f.as_slice()).collect();
        let encoded = enc.encode_batch(&refs);
        let finished: Vec<Vec<u8>> = match hra.as_mut() {
            Some(h) => h.wrap_batch(
                encoded
                    .iter()
                    .map(|(d, left)| (d.clone(), &left.residual[..]))
                    .collect(),
            ),
            None => encoded.into_iter().map(|(d, _)| d).collect(),
        };
        for data in finished {
            bytes += output.write_frame(&data)?;
            frames += 1;
        }
        on_progress(1.0 - feeder.remaining as f32 / want as f32);
    }
    let blackout = match (&output, req.hra_kbps) {
        (Output::Container(..), Some(kbps)) => silent_frame(&plan, req, kbps, n, &aux),
        _ => None,
    };
    output.finish(blackout.as_deref())?;
    on_progress(1.0);
    Ok(Stats { frames, bytes })
}

fn has_extension(path: &Path, ext: &str) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

/// What the request fixes before any audio is read: which input feeds which core channel, the frame size and the
/// optional ES mixer.
struct Plan {
    map: LayoutMap,
    /// Where the separate centre surround is in the layout when an XCH extension carries it.
    cs_slot: Option<usize>,
    xch: bool,
    mixer: Option<EsMixer>,
    rate: u32,
    /// Primary channels in the core (without LFE).
    nch: usize,
    bitrate: u32,
    blocks: usize,
    wav: bool,
    /// A DTS CD: the stream packed into 14-bit words (16-bit containers, so a player that does not know DTS plays
    /// quiet noise), 1024-sample frames of 3584 bytes, 1234.8 kbit/s of stream in the 1411.2 kbit/s of the disc.
    cd14: bool,
}

impl Plan {
    fn new(req: &Request, rate: u32) -> Result<Plan, NativeError> {
        // channels that go into the core (a separate Cs is folded into the surrounds first)
        let mix_cs = req.es.is_some_and(|e| e.mix_cs);
        let core_idx: Vec<usize> = (0..req.layout.channels.len())
            .filter(|&i| !(mix_cs && req.layout.channels[i] == Channel::Cs))
            .collect();
        let core_chs: Vec<Channel> = core_idx.iter().map(|&i| req.layout.channels[i]).collect();
        let core_map = map_channels(&core_chs, req.layout.id)?;
        let map = LayoutMap {
            amode: core_map.amode,
            order: core_map.order.iter().map(|&i| core_idx[i]).collect(),
            lfe: core_map.lfe.map(|i| core_idx[i]),
        };
        let xch = req.es.is_some_and(|e| e.xch);
        let cs_slot = req
            .layout
            .channels
            .iter()
            .position(|c| *c == Channel::Cs)
            .filter(|_| xch);
        let mixer = match req.es {
            Some(o) if o.mix_cs || o.phase_shift || o.rear_minus_3db => Some(
                EsMixer::new(o, req.layout.channels)
                    .ok_or_else(|| Error::Unsupported("the ES options need a layout with surround channels".into()))?,
            ),
            _ => None,
        };
        let wav = has_extension(req.output, "wav");
        let cd14 = wav && rate == 44_100 && matches!(req.bitrate_kbps, 1411 | 1234 | 1235);
        let bitrate = if cd14 { 1_234_800 } else { core_bitrate(req) };
        // 512-sample frames up to 1.5 Mbit/s are what players expect; longer frames for low rates
        let blocks = if (bitrate >= 700_000 || rate > 48_000) && !cd14 {
            16
        } else {
            32
        };
        Ok(Plan {
            nch: map.order.len(),
            map,
            cs_slot,
            xch,
            mixer,
            rate,
            bitrate,
            blocks,
            wav,
            cd14,
        })
    }

    /// Input slots of the encoder: the primary channels, the XCH channel and the LFE.
    fn slots(&self) -> usize {
        self.nch + usize::from(self.xch) + usize::from(self.map.lfe.is_some())
    }

    fn config(&self, req: &Request) -> Config {
        Config {
            sample_rate: self.rate,
            amode: self.map.amode,
            channels: self.nch,
            lfe: self.map.lfe.is_some(),
            bitrate: self.bitrate,
            dialnorm: req.dialnorm,
            pcm_bits: req.pcm_bits,
            es: req.es.is_some_and(|e| e.flag),
            xch: self.xch,
            perfect: false,
            blocks: self.blocks,
        }
    }

    fn aux_downmix(&self, req: &Request) -> Option<AuxDownmix> {
        crate::lossless::ma::core_aux(
            &req.bitstream.downmix,
            req.layout,
            self.map.amode,
            self.map.lfe.is_some(),
            self.xch,
        )
    }

    /// The High Resolution stage that wraps each core frame with its extension substream.
    fn hra_stage(&self, req: &Request, kbps: u32, frame_samples: usize) -> HraStage {
        let mix_cs_only = req.es.is_some_and(|e| e.mix_cs && !e.xch);
        let hidden_cs = if mix_cs_only { exss_bit(Channel::Cs) } else { 0 };
        let params = ExssParams {
            sample_rate: self.rate,
            channels: self.slots(),
            bit_res: req.pcm_bits,
            spk_mask: crate::lossless::ma::map_layout(req.layout).exss_mask & !hidden_cs,
            with_core: true,
            dialnorm: (-req.dialnorm) as u32,
            frame_duration_code: 0,
            x96_in_core: false,
            xch_in_core: self.xch,
            secondary: false,
            extras: ExssExtras {
                min_header: 28,
                ..Default::default()
            },
        };
        HraStage::new(kbps, self.rate, frame_samples, self.blocks, params)
    }
}

/// Reads the source frame by frame and shapes it into what the encoder takes: planar samples in the core's channel
/// order, scaled to 24 bits.
struct Feeder {
    mixer: Option<EsMixer>,
    planar: Vec<Vec<f32>>,
    /// Layout index of the source for each encoder slot.
    sources: Vec<usize>,
    layout_channels: usize,
    frame_samples: usize,
    /// Samples of the programme not read yet, and silence still to put in front of it.
    remaining: u64,
    lead_left: usize,
}

impl Feeder {
    fn new(plan: &mut Plan, req: &Request, frame_samples: usize, remaining: u64, lead: usize) -> Feeder {
        Feeder {
            mixer: plan.mixer.take(),
            planar: Vec::new(),
            sources: plan
                .map
                .order
                .iter()
                .copied()
                .chain(plan.cs_slot)
                .chain(plan.map.lfe)
                .collect(),
            layout_channels: req.layout.channels.len(),
            frame_samples,
            remaining,
            lead_left: lead,
        }
    }

    /// Reads and drops `count` samples (the start of the programme that the decoder's delay would cut anyway).
    fn skip(&mut self, stems: &mut StemSet, count: u64) -> Result<(), NativeError> {
        let mut left = count;
        while left > 0 {
            let k = left.min(self.frame_samples as u64) as usize;
            stems.read_planar(&mut self.planar, k)?;
            left -= k as u64;
        }
        Ok(())
    }

    /// The input of the next frame: `[slot][sample]`.
    fn next_frame(&mut self, stems: &mut StemSet) -> Result<Vec<Vec<f64>>, NativeError> {
        let n = self.frame_samples;
        let zeros = self.lead_left.min(n);
        self.lead_left -= zeros;
        let got = (self.remaining as usize).min(n - zeros);
        if got > 0 {
            stems.read_planar(&mut self.planar, got)?;
        }
        self.remaining -= got.min(self.remaining as usize) as u64;
        // with a mixer the block already holds the lead, so nothing is added in front of it
        let (zeros, got) = match self.mixer.as_mut() {
            Some(mixer) => {
                // the mixer works on whole blocks (silence past the end of the programme)
                let block: Vec<Vec<f32>> = (0..self.layout_channels)
                    .map(|c| {
                        // the lead of silence (a container's codec delay) first, then the source
                        let mut v = vec![0.0f32; zeros];
                        if got > 0 {
                            v.extend_from_slice(&self.planar[c][..got]);
                        }
                        v.resize(n, 0.0);
                        v
                    })
                    .collect();
                self.planar = mixer.process(&block);
                (0, n)
            }
            None => (zeros, got),
        };
        let mut pcm: Vec<Vec<f64>> = vec![vec![0.0; n]; self.sources.len()];
        for (dst, &src) in pcm.iter_mut().zip(&self.sources) {
            for i in 0..got {
                dst[zeros + i] = (self.planar[src][i] as f64 * 8_388_608.0).clamp(-8_388_608.0, 8_388_607.0);
            }
        }
        Ok(pcm)
    }
}

/// Where the finished frames go.
enum Output {
    Raw(std::io::BufWriter<std::fs::File>),
    /// A WAV file; the flag says the stream is packed into 14-bit words (a DTS CD).
    Wav(WavOut, bool),
    /// A `.dtshd` file, and how many bytes of each frame belong to the core.
    Container(DtsHdSink, u32),
}

impl Output {
    /// Opens the file: a `.dtshd` container (stating the codec delay), a WAV file or the raw stream.
    fn open(
        req: &Request,
        plan: &Plan,
        container: bool,
        frame_samples: usize,
        want: u64,
        codec_delay: usize,
        core_payload: u32,
    ) -> Result<Output, NativeError> {
        Ok(if container {
            let mut info = crate::container::dtshd::DtsHdInfo::new(
                plan.rate,
                frame_samples as u16,
                want,
                crate::lossless::ma::map_layout(req.layout).exss_mask as u16,
                crate::container::dtshd::Kind::CoreAndExtension,
            );
            let secs = want as f64 / plan.rate as f64;
            crate::container::mixmeta::apply_container_options(&mut info, &req.bitstream, secs);
            info.core_kbps = plan.bitrate / 1000;
            info.core_payload = core_payload;
            info.codec_delay = codec_delay as u16;
            Output::Container(
                crate::container::dtshd::DtsHdSink::create(req.output, info)?,
                core_payload,
            )
        } else if plan.wav {
            Output::Wav(WavOut::create(req.output, plan.rate)?, plan.cd14)
        } else {
            Output::Raw(std::io::BufWriter::new(std::fs::File::create(req.output)?))
        })
    }

    /// Writes one frame and returns how many bytes it took.
    fn write_frame(&mut self, data: &[u8]) -> std::io::Result<u64> {
        match self {
            Output::Wav(wav, cd14) => {
                // DTS in PCM: either the stream packed into 14-bit words (a DTS CD) or its 16-bit words
                // byte-swapped into little-endian samples
                let samples = if *cd14 {
                    pack_14bit(data)
                } else {
                    words_to_samples(data)
                };
                wav.write(&samples)?;
                Ok(samples.len() as u64)
            }
            Output::Container(sink, core_payload) => {
                sink.write_frame(data, data.len().saturating_sub(*core_payload as usize))?;
                Ok(data.len() as u64)
            }
            Output::Raw(out) => {
                out.write_all(data)?;
                Ok(data.len() as u64)
            }
        }
    }

    /// Closes the file. `blackout` is the silent frame StreamTools' Add silence inserts, for the container's trailer.
    fn finish(self, blackout: Option<&[u8]>) -> std::io::Result<()> {
        match self {
            Output::Raw(mut out) => out.flush(),
            Output::Wav(wav, _) => wav.finish(),
            Output::Container(sink, _) => sink.finish(blackout),
        }
    }
}

/// A silent frame for the container's trailer: a fresh encoder fed zeros (several frames, so the filter bank has
/// settled).
fn silent_frame(
    plan: &Plan,
    req: &Request,
    kbps: u32,
    frame_samples: usize,
    aux: &Option<AuxDownmix>,
) -> Option<Vec<u8>> {
    let mut enc = Encoder::new(plan.config(req)).ok()?;
    enc.set_aux_downmix(aux.clone());
    enc.keep_residual = true;
    let mut stage = plan.hra_stage(req, kbps, frame_samples);
    let zeros = vec![vec![0.0; frame_samples]; plan.slots()];
    let mut last = None;
    for _ in 0..4 {
        let data = enc.encode_frame(&zeros);
        last = Some(stage.wrap(data, &enc.last_residual));
    }
    last
}
