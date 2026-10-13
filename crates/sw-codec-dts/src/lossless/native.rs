//! File-level driver for DTS-HD Master Audio: stems in, `.dtshd` (or raw) stream out.

use crate::container::dtshd::{DtsHdInfo, Kind, StreamOut};
use crate::lossless::ma::{map_layout_with, MaConfig, MaEncoder, MaMap};
use crate::lossless::ma_core::{MaCoreConfig, MaCoreEncoder};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use sw_core::ChannelLayout;
use sw_pcm::StemSet;

#[derive(Debug, thiserror::Error)]
pub enum MaError {
    #[error(transparent)]
    Encoder(#[from] crate::Error),
    #[error(transparent)]
    Pcm(#[from] sw_pcm::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Unsupported(String),
    #[error("nothing to encode: the selected span is empty")]
    Empty,
    #[error("canceled")]
    Canceled,
}

pub struct MaRequest<'a> {
    pub layout: &'static ChannelLayout,
    /// Core bit rate in kbit/s; `None` writes the lossless extension only (no core).
    pub core_bitrate_kbps: Option<u32>,
    pub dialnorm: i32,
    pub start_secs: f64,
    pub duration_secs: Option<f64>,
    pub output: &'a Path,
    /// Write the `.dtshd` container (otherwise the raw stream).
    pub container: bool,
    /// 96 kHz: give the core an X96 extension (a 96/24 core).
    pub x96: bool,
    /// Bitstream-tab metadata (programme information, seamless branching, wide 7.1 remapping).
    pub bitstream: sw_core::BitstreamOptions,
    /// How hard the lossless stage searches for a smaller file.
    pub effort: sw_core::Effort,
}

pub struct MaStats {
    pub frames: u64,
    pub bytes: u64,
}

/// The lossless stage on its own, or with a DTS core underneath it.
enum Enc {
    Core(MaCoreEncoder),
    Lossless(MaEncoder),
}

impl Enc {
    fn frame_samples(&self) -> usize {
        match self {
            Enc::Core(e) => e.frame_samples(),
            Enc::Lossless(e) => e.frame_samples(),
        }
    }

    /// Feeds `got` samples per channel; returns the frames that are ready.
    fn push(&mut self, pcm: &[Vec<i32>], got: usize) -> Vec<Vec<u8>> {
        match self {
            Enc::Core(e) => {
                let part: Vec<Vec<i32>> = pcm.iter().map(|c| c[..got].to_vec()).collect();
                e.push(&part)
            }
            Enc::Lossless(e) => {
                let n = e.frame_samples();
                lossless_frames(e, pcm, got, n)
            }
        }
    }

    /// The frames still held back when the input ends (only a core holds any).
    fn finish(&mut self) -> Vec<Vec<u8>> {
        match self {
            Enc::Core(e) => e.finish(),
            Enc::Lossless(_) => Vec::new(),
        }
    }
}

/// Cuts `got` samples per channel into frames of `n` and encodes each; the last is padded with zeros.
fn lossless_frames(e: &mut MaEncoder, pcm: &[Vec<i32>], got: usize, n: usize) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < got {
        let part: Vec<Vec<i32>> = pcm
            .iter()
            .map(|c| {
                let mut v = c[pos..(pos + n).min(c.len())].to_vec();
                v.resize(n, 0);
                v
            })
            .collect();
        out.push(e.encode_frame(&part));
        pos += n;
    }
    out
}

/// What the request and the source fix before encoding starts.
struct Plan {
    rate: u32,
    bit_res: u32,
    downmix: sw_core::Downmix,
    map: MaMap,
    /// `None` when the stream has no core (or cannot have one).
    core_kbps: Option<u32>,
    wide_remap: bool,
}

impl Plan {
    fn new(stems: &StemSet, req: &MaRequest) -> Result<Plan, MaError> {
        let rate = stems.sample_rate;
        if !matches!(rate, 44_100 | 48_000 | 88_200 | 96_000 | 176_400 | 192_000) {
            return Err(MaError::Unsupported(format!(
                "{rate} Hz: only 44.1 to 192 kHz are implemented so far"
            )));
        }
        let bit_res = (stems.bits as u32).clamp(16, 24);
        let bit_res = if bit_res == 16 || bit_res == 20 { bit_res } else { 24 };
        // the stereo set rides under the core, which stops at 96 kHz
        let mut downmix = req.bitstream.downmix.clone();
        if rate > 96_000 {
            downmix.embed_stereo = false;
        }
        let map = map_layout_with(req.layout, &downmix);
        let nch = map.order.len();
        if nch > 24 {
            return Err(MaError::Unsupported("more than 24 channels".into()));
        }
        if nch > 8 && nch as u64 * rate as u64 > 12 * 96_000 {
            return Err(MaError::Unsupported(format!(
                "{nch} channels at {rate} Hz do not fit the stream's 64 kB frame size (up to 12 channels at 96 kHz, \
                 24 at 48 kHz)"
            )));
        }
        if rate > 96_000 && nch > 6 {
            return Err(MaError::Unsupported("above 96 kHz: up to 5.1 channels".into()));
        }
        // a core under 176.4 / 192 kHz lossless data sits under band 0 (up to 5.1); wider streams stand alone
        let core_kbps = if nch > 8 || (rate > 96_000 && nch > 6) {
            None
        } else {
            req.core_bitrate_kbps
        };
        // the remapping sets describe a wide 7.1 layout: the switch does nothing for any other
        let wide_remap = req.bitstream.wide_remap && req.layout.channels.contains(&sw_core::Channel::Lw);
        Ok(Plan {
            rate,
            bit_res,
            downmix,
            map,
            core_kbps,
            wide_remap,
        })
    }

    fn channels(&self) -> usize {
        self.map.order.len()
    }

    fn make_encoder(&self, req: &MaRequest) -> Result<Enc, MaError> {
        Ok(match self.core_kbps {
            Some(kbps) => Enc::Core(MaCoreEncoder::new(MaCoreConfig {
                layout: req.layout,
                sample_rate: self.rate,
                bit_res: self.bit_res,
                core_bitrate: kbps * 1000,
                dialnorm: req.dialnorm,
                x96: req.x96,
                wide_remap: self.wide_remap,
                downmix: self.downmix.clone(),
                effort: req.effort,
            })?),
            None => Enc::Lossless(MaEncoder::new(MaConfig {
                sample_rate: self.rate,
                map: self.map.clone(),
                bit_res: self.bit_res,
                dialnorm: (-req.dialnorm) as u32,
                wide_remap: self.wide_remap,
                repr_type: (req.layout.kind == sw_core::layout::LayoutKind::LtRt).then_some(2),
                effort: req.effort,
            })),
        })
    }

    /// Bytes of each frame that belong to the core.
    fn core_payload(&self) -> u32 {
        self.core_kbps.map_or(0, |k| (k * 1000 * 512 / 48_000 / 8) & !3)
    }

    /// The header of the `.dtshd` file.
    fn container_info(&self, req: &MaRequest, frame_samples: usize, want: u64, lead: usize) -> DtsHdInfo {
        let kind = if self.core_kbps.is_some() {
            Kind::CoreAndExtension
        } else {
            Kind::ExtensionOnly
        };
        let mut info = DtsHdInfo::new(self.rate, frame_samples as u16, want, self.map.exss_mask as u16, kind);
        let secs = want as f64 / self.rate as f64;
        crate::container::mixmeta::apply_container_options(&mut info, &req.bitstream, secs);
        info.vbr = true;
        info.core_kbps = self.core_kbps.unwrap_or(0);
        info.core_payload = self.core_payload();
        info.codec_delay = lead as u16;
        if self.core_kbps.is_some() {
            // CORESSMD describes the core: 48 kHz and the speakers of the base channel set
            info.core_rate = Some(if req.x96 && self.rate == 96_000 { 96_000 } else { 48_000 });
            let core_speakers = self.map.order[..self.map.core_channels()]
                .iter()
                .fold(0u32, |m, &c| m | crate::lossless::ma::exss_bit(req.layout.channels[c]));
            info.core_mask = Some(core_speakers as u16);
        }
        info
    }

    /// One silent frame, kept at the end of the file for tools that add silence.
    fn silent_frame(&self, req: &MaRequest, n: usize) -> Result<Option<Vec<u8>>, MaError> {
        let mut enc = self.make_encoder(req)?;
        let zeros = vec![vec![0i32; n * 12]; self.channels()];
        let mut ready = enc.push(&zeros, n * 12);
        ready.extend(enc.finish());
        Ok(ready.get(ready.len() / 2).cloned())
    }
}

/// The output file and a count of what went into it.
struct Writer {
    file: StreamOut,
    core_payload: u32,
    frames: u64,
    bytes: u64,
}

impl Writer {
    fn write(&mut self, frames: Vec<Vec<u8>>) -> std::io::Result<()> {
        for f in frames {
            self.file
                .write_frame(&f, f.len().saturating_sub(self.core_payload as usize))?;
            self.bytes += f.len() as u64;
            self.frames += 1;
        }
        Ok(())
    }
}

pub fn encode_to_file(
    stems: &mut StemSet,
    req: &MaRequest,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Result<MaStats, MaError> {
    let plan = Plan::new(stems, req)?;
    let mut enc = plan.make_encoder(req)?;
    let n = enc.frame_samples();
    let want = stems.select_span(req.start_secs, req.duration_secs)?;
    if want == 0 {
        return Err(MaError::Empty);
    }
    // A stream with a core opens with two frames of silence, as the commercial encoder does: the decoder has no
    // history for the first frame (the lossless stage cannot reproduce it exactly when the channels sit in two
    // sets), and the file's codec delay field tells decoders to drop them, so every source sample is kept.
    let lead = match (req.container, plan.core_kbps.is_some()) {
        (true, true) => 2 * n,
        (true, false) => n,
        (false, _) => 0,
    };
    let info = req.container.then(|| plan.container_info(req, n, want, lead));
    let nch = plan.channels();
    let mut out = Writer {
        file: StreamOut::create(req.output, info)?,
        core_payload: plan.core_payload(),
        frames: 0,
        bytes: 0,
    };
    if lead > 0 {
        out.write(enc.push(&vec![vec![0i32; lead]; nch], lead))?;
    }
    let scale = (1i64 << (plan.bit_res - 1)) as f32;
    let (lo, hi) = (-(1i64 << (plan.bit_res - 1)), (1i64 << (plan.bit_res - 1)) - 1);
    let mut planar: Vec<Vec<f32>> = Vec::new();
    let mut remaining = want;
    // several frames per push so the lossless stage can run in parallel
    let chunk = n * 16;
    let mut pcm: Vec<Vec<i32>> = vec![vec![0; chunk]; nch];
    while remaining > 0 {
        if cancel.load(Ordering::Relaxed) {
            return Err(MaError::Canceled);
        }
        let got = (remaining as usize).min(chunk);
        stems.read_planar(&mut planar, got)?;
        for (dst, &layout_channel) in pcm.iter_mut().zip(&plan.map.order) {
            dst.iter_mut().for_each(|v| *v = 0);
            for i in 0..got {
                dst[i] = ((planar[layout_channel][i] * scale).round() as i64).clamp(lo, hi) as i32;
            }
        }
        remaining -= got as u64;
        out.write(enc.push(&pcm, got))?;
        on_progress(1.0 - remaining as f32 / want as f32);
    }
    out.write(enc.finish())?;
    let blackout = if req.container {
        plan.silent_frame(req, n)?
    } else {
        None
    };
    out.file.finish(blackout.as_deref())?;
    on_progress(1.0);
    Ok(MaStats {
        frames: out.frames,
        bytes: out.bytes,
    })
}
