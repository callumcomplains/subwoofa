//! Audio files: WAV (BWF, RF64), AIFF, CAF, FLAC (also in Ogg), Apple Lossless, TTA, WavPack, Monkey's Audio,
//! MPEG-4 ALS, DSD (DSF, DSDIFF) and digital cinema sound (MXF).
//!
//! The stems are read as 32-bit floats (exact for anything up to 24 bits), converted to the chosen sample format and
//! written in blocks. Each container module only knows its own file layout; this file reads the stems, orders the
//! channels and hands the blocks over.

mod aiff;
mod alac;
mod als;
mod bitwriter;
mod caf;
pub mod channels;
mod crc;
mod dcp;
mod dsd;
mod flac;
mod le_bits;
mod monkeys;
mod mp4_audio;
mod ogg;
mod tta;
mod wav;
mod wavpack;

pub use dcp::{DcpOptions, FRAME_RATES as DCP_FRAME_RATES};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use sw_core::ChannelLayout;
use sw_pcm::StemSet;

/// The file formats this crate writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Wav,
    Aiff,
    Caf,
    Flac,
    /// FLAC frames in an Ogg stream (`.oga`).
    OggFlac,
    /// Apple Lossless in an M4A file.
    Alac,
    /// True Audio.
    Tta,
    /// WavPack (`.wv`), lossless.
    WavPack,
    /// Monkey's Audio (`.ape`), lossless.
    MonkeysAudio,
    /// MPEG-4 ALS, lossless (`.als`, or an MP4 file).
    MpegAls,
    /// Digital cinema sound: an AS-DCP MXF track file of 24-bit PCM.
    DcpAudio,
    /// DSD in a `.dsf` file.
    Dsf,
    /// DSD in a DSDIFF `.dff` file.
    Dff,
}

/// How each sample is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    Int16,
    Int24,
    Int32,
    Float32,
    Float64,
}

impl SampleFormat {
    pub fn from_id(id: &str) -> Option<SampleFormat> {
        Some(match id.to_ascii_lowercase().as_str() {
            "s16" | "16" => SampleFormat::Int16,
            "s24" | "24" => SampleFormat::Int24,
            "s32" | "32" => SampleFormat::Int32,
            "f32" => SampleFormat::Float32,
            "f64" => SampleFormat::Float64,
            _ => return None,
        })
    }

    pub const fn id(self) -> &'static str {
        match self {
            SampleFormat::Int16 => "s16",
            SampleFormat::Int24 => "s24",
            SampleFormat::Int32 => "s32",
            SampleFormat::Float32 => "f32",
            SampleFormat::Float64 => "f64",
        }
    }

    pub const fn bits(self) -> u32 {
        match self {
            SampleFormat::Int16 => 16,
            SampleFormat::Int24 => 24,
            SampleFormat::Int32 | SampleFormat::Float32 => 32,
            SampleFormat::Float64 => 64,
        }
    }

    pub const fn bytes(self) -> usize {
        self.bits() as usize / 8
    }

    pub const fn is_float(self) -> bool {
        matches!(self, SampleFormat::Float32 | SampleFormat::Float64)
    }
}

pub struct Request<'a> {
    pub layout: &'static ChannelLayout,
    pub container: Container,
    pub format: SampleFormat,
    pub start_secs: f64,
    pub duration_secs: Option<f64>,
    pub output: &'a Path,
    /// DSD only: the DSD rate as a multiple of 44.1 kHz (64 = DSD64, 128 = DSD128).
    pub dsd_multiplier: u32,
    /// DCP sound only: frame rate and label standard.
    pub dcp: DcpOptions,
    /// How hard the lossless formats search for a smaller file.
    pub effort: sw_core::Effort,
}

#[derive(Debug, thiserror::Error)]
pub enum PcmError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Pcm(#[from] sw_pcm::Error),
    #[error("{0}")]
    Unsupported(String),
    #[error("nothing to encode: the start time is at or past the end of the input")]
    Empty,
    #[error("canceled")]
    Canceled,
}

pub struct Stats {
    pub frames: u64,
    pub bytes: u64,
}

/// What a container writer needs to know before the first sample.
pub(crate) struct Params {
    pub rate: u32,
    pub format: SampleFormat,
    pub total_frames: u64,
    /// Layout channel index for every channel of the file, in file order.
    pub order: Vec<usize>,
    /// WAV-style channel mask (0 = unpositioned).
    pub mask: u32,
    /// CAF channel labels, in file order.
    pub labels: Vec<u32>,
    /// BWF time reference in samples, if the source had one.
    pub time_reference: Option<u64>,
    pub effort: sw_core::Effort,
}

impl Params {
    pub fn channels(&self) -> usize {
        self.order.len()
    }
}

/// Cuts the whole frames of `span` samples off the per-channel buffers (`frames[frame][channel]`), once at least
/// `min_frames` of them are ready: the frames of a batch can then be encoded on all cores at once.
pub(crate) fn take_frames(pending: &mut [Vec<i32>], span: usize, min_frames: usize) -> Vec<Vec<Vec<i32>>> {
    let ready = pending[0].len() / span;
    if ready < min_frames.max(1) {
        return Vec::new();
    }
    (0..ready)
        .map(|_| pending.iter_mut().map(|p| p.drain(..span).collect()).collect())
        .collect()
}

/// A cheap stand-in for how many bits a channel will take: the size of its second differences. Encoders use it to
/// choose between ways of storing a stereo pair without coding each of them.
pub(crate) fn roughness(x: &[i32]) -> usize {
    x.windows(3)
        .map(|w| (w[2] as i64 - 2 * w[1] as i64 + w[0] as i64).unsigned_abs())
        .sum::<u64>() as usize
}

/// Sample bytes for `frames` frames of the file's channels, interleaved.
pub(crate) fn interleave(
    planar: &[Vec<f32>],
    order: &[usize],
    frames: usize,
    format: SampleFormat,
    big_endian: bool,
    out: &mut Vec<u8>,
) {
    match format {
        SampleFormat::Int16 => interleave_with(planar, order, frames, big_endian, out, |v| {
            (to_int(v, 15) as i16).to_le_bytes()
        }),
        SampleFormat::Int24 => interleave_with(planar, order, frames, big_endian, out, |v| {
            let b = (to_int(v, 23) as i32).to_le_bytes();
            [b[0], b[1], b[2]]
        }),
        SampleFormat::Int32 => interleave_with(planar, order, frames, big_endian, out, |v| {
            (to_int(v, 31) as i32).to_le_bytes()
        }),
        SampleFormat::Float32 => interleave_with(planar, order, frames, big_endian, out, |v| v.to_le_bytes()),
        SampleFormat::Float64 => interleave_with(planar, order, frames, big_endian, out, |v| (v as f64).to_le_bytes()),
    }
}

/// Interleaving for samples of `W` bytes: `convert` gives a sample's little-endian bytes. Stretches of frames are
/// filled on all cores.
fn interleave_with<const W: usize>(
    planar: &[Vec<f32>],
    order: &[usize],
    frames: usize,
    big_endian: bool,
    out: &mut Vec<u8>,
    convert: impl Fn(f32) -> [u8; W] + Sync,
) {
    use rayon::prelude::*;
    const STRETCH: usize = 4096;
    let frame_bytes = order.len() * W;
    out.clear();
    out.resize(frames * frame_bytes, 0);
    out.par_chunks_mut(STRETCH * frame_bytes.max(1))
        .enumerate()
        .for_each(|(stretch, bytes)| {
            for (i, frame) in bytes.chunks_exact_mut(frame_bytes).enumerate() {
                let at = stretch * STRETCH + i;
                for (slot, &ch) in frame.chunks_exact_mut(W).zip(order) {
                    let mut b = convert(planar[ch][at]);
                    if big_endian {
                        b.reverse();
                    }
                    slot.copy_from_slice(&b);
                }
            }
        });
}

/// Rounds a sample in -1.0..1.0 to a signed integer with `magnitude_bits` bits besides the sign, clipping at the ends.
fn to_int(v: f32, magnitude_bits: u32) -> i64 {
    let scale = (1u64 << magnitude_bits) as f64;
    let max = scale - 1.0;
    ((v as f64 * scale).round()).clamp(-scale, max) as i64
}

/// The channels as integers of `bits` bits (FLAC works on integers).
pub(crate) fn to_integers(planar: &[Vec<f32>], order: &[usize], frames: usize, bits: u32) -> Vec<Vec<i32>> {
    use rayon::prelude::*;
    order
        .par_iter()
        .map(|&ch| {
            planar[ch][..frames]
                .iter()
                .map(|&v| to_int(v, bits - 1) as i32)
                .collect()
        })
        .collect()
}

/// The layout channel placed at each channel of a DSD file (the file has Apple-style fixed arrangements), or `None`
/// for layouts DSD has no arrangement for.
pub fn dsd_order(layout: &ChannelLayout) -> Option<Vec<usize>> {
    dsd::dsd_layout(layout).map(|d| d.order)
}

/// Writes the stems as the requested file.
pub fn encode_to_file(
    stems: &mut StemSet,
    req: &Request,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Result<Stats, PcmError> {
    let rate = stems.sample_rate;
    let first = ((req.start_secs * rate as f64).round() as u64).min(stems.frames);
    stems.seek_frame(first)?;
    let available = stems.frames - first;
    let wanted = req
        .duration_secs
        .map(|d| (d * rate as f64).round() as u64)
        .unwrap_or(available)
        .min(available);
    if wanted == 0 {
        return Err(PcmError::Empty);
    }
    let mask_order = channels::wav_order(req.layout);
    let wav_style = matches!(
        req.container,
        Container::Wav
            | Container::Flac
            | Container::OggFlac
            | Container::Tta
            | Container::WavPack
            | Container::MonkeysAudio
            | Container::MpegAls
    );
    // WAV and FLAC reorder the channels by speaker position; AIFF and CAF keep the layout's order
    let mut layout_tag = 0;
    let dsd_layout = if matches!(req.container, Container::Dsf | Container::Dff) {
        Some(dsd::dsd_layout(req.layout).ok_or_else(|| {
            PcmError::Unsupported(format!(
                "DSD files have channel arrangements for mono, stereo, 3.0, 3.1, quad, 5.0 and 5.1, not {}",
                req.layout.id
            ))
        })?)
    } else {
        None
    };
    let order: Vec<usize> = if let Some(d) = &dsd_layout {
        d.order.clone()
    } else if req.container == Container::Alac {
        let (order, tag) = alac::alac_order(req.layout).ok_or_else(|| PcmError::Unsupported(format!("ALAC has no channel arrangement for {} (it takes mono, stereo, 3.0, 4.0 LCRS, 5.0, 5.1, 6.1 and 7.1 with wides)", req.layout.id)))?;
        layout_tag = tag;
        order
    } else if wav_style {
        mask_order.order.clone()
    } else {
        (0..req.layout.channels.len()).collect()
    };
    let labels = order
        .iter()
        .map(|&i| channels::caf_label(req.layout.channels[i], i))
        .collect();
    let params = Params {
        rate,
        format: req.format,
        total_frames: wanted,
        order,
        mask: mask_order.mask,
        labels,
        time_reference: stems.time_reference.map(|t| t + first),
        effort: req.effort,
    };

    let mut writer: Box<dyn BlockWriter> = match req.container {
        Container::Wav => Box::new(wav::WavWriter::create(req.output, params)?),
        Container::Aiff => Box::new(aiff::AiffWriter::create(req.output, params)?),
        Container::Caf => Box::new(caf::CafWriter::create(req.output, params)?),
        Container::Flac => Box::new(flac::FlacWriter::create(req.output, params, false)?),
        Container::OggFlac => Box::new(flac::FlacWriter::create(req.output, params, true)?),
        Container::Alac => Box::new(alac::AlacWriter::create(req.output, params, layout_tag)?),
        Container::Tta => Box::new(tta::TtaWriter::create(req.output, params)?),
        Container::WavPack => Box::new(wavpack::WavPackWriter::create(req.output, params)?),
        Container::MonkeysAudio => Box::new(monkeys::MonkeysWriter::create(req.output, params)?),
        Container::MpegAls => Box::new(als::AlsWriter::create(req.output, params)?),
        Container::DcpAudio => Box::new(dcp::DcpWriter::create(req.output, params, req.dcp)?),
        Container::Dsf | Container::Dff => {
            let file = if req.container == Container::Dsf {
                dsd::DsdFile::Dsf
            } else {
                dsd::DsdFile::Dff
            };
            Box::new(dsd::DsdWriter::create(
                req.output,
                params,
                file,
                dsd_layout.as_ref().unwrap(),
                req.dsd_multiplier,
            )?)
        }
    };
    // big reads, so that the writers get batches of frames they can encode in parallel; the next block is read while
    // the writer is working on this one
    let block = 1usize << 18;
    let mut written = 0u64;
    stems.read_ahead(block, wanted, |planar, got| {
        if cancel.load(Ordering::Relaxed) {
            return Err(PcmError::Canceled);
        }
        writer.write_block(planar, got)?;
        written += got as u64;
        on_progress(written as f32 / wanted as f32);
        Ok(())
    })?;
    let bytes = writer.finish()?;
    on_progress(1.0);
    Ok(Stats { frames: written, bytes })
}

/// A container being written, block by block.
pub(crate) trait BlockWriter {
    /// Appends `frames` frames; `planar[channel][frame]` holds layout channel `channel`.
    fn write_block(&mut self, planar: &[Vec<f32>], frames: usize) -> std::io::Result<()>;
    /// Completes the file and returns its size in bytes.
    fn finish(self: Box<Self>) -> std::io::Result<u64>;
}

/// Pads a chunk to an even length, as RIFF, AIFF and the like require.
pub(crate) fn pad_even<W: Write>(w: &mut W, len: u64) -> std::io::Result<()> {
    if len % 2 == 1 {
        w.write_all(&[0])?;
    }
    Ok(())
}
