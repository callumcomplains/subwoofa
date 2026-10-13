//! File-level driver: stems in, `.ac3` out.

use crate::{map_layout, Config, Encoder, Error, SAMPLES_PER_FRAME};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use sw_core::ChannelLayout;
use sw_pcm::StemSet;

#[derive(Debug, thiserror::Error)]
pub enum NativeError {
    #[error(transparent)]
    Encoder(#[from] Error),
    #[error(transparent)]
    Pcm(#[from] sw_pcm::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("canceled")]
    Canceled,
}

pub struct Request<'a> {
    pub layout: &'static ChannelLayout,
    pub bitrate_kbps: u32,
    pub dialnorm: u8,
    pub start_secs: f64,
    pub duration_secs: Option<f64>,
    pub output: &'a Path,
    /// Write E-AC-3 instead of AC-3.
    pub eac3: bool,
}

pub struct Stats {
    pub frames: u64,
    pub input_frames: u64,
    pub bytes: u64,
}

/// Frames encoded together; they are spread over the cores.
const BATCH: usize = 128;

/// The samples of frame `k` of a block holding `got` samples: frames past the end of the input are empty.
fn frame_span(k: usize, got: usize) -> (usize, usize) {
    ((k * SAMPLES_PER_FRAME).min(got), ((k + 1) * SAMPLES_PER_FRAME).min(got))
}

/// Reads the input in batches of frames, passes each to `code(planar, samples, frame_count)` (which returns the
/// bytes it wrote), then passes empty frames until `total_frames` have been coded: the tail that flushes the
/// transform delay. Returns the bytes written.
fn drive(
    stems: &mut StemSet,
    want: u64,
    total_frames: u64,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(f32),
    mut code: impl FnMut(&[Vec<f32>], usize, usize) -> Result<u64, NativeError>,
) -> Result<u64, NativeError> {
    let (mut done, mut bytes) = (0u64, 0u64);
    stems.read_ahead(BATCH * SAMPLES_PER_FRAME, want, |planar, got| {
        if cancel.load(Ordering::Relaxed) {
            return Err(NativeError::Canceled);
        }
        let count = got.div_ceil(SAMPLES_PER_FRAME);
        bytes += code(planar, got, count)?;
        done += count as u64;
        on_progress(done as f32 / total_frames as f32);
        Ok(())
    })?;
    while done < total_frames {
        let count = BATCH.min((total_frames - done) as usize);
        bytes += code(&[], 0, count)?;
        done += count as u64;
    }
    Ok(bytes)
}

pub fn encode_to_file(
    stems: &mut StemSet,
    req: &Request,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Result<Stats, NativeError> {
    if req.eac3
        && req
            .layout
            .channels
            .iter()
            .any(|c| matches!(c, sw_core::Channel::Lsr | sw_core::Channel::Rsr))
    {
        return encode_extended(stems, req, on_progress, cancel);
    }
    let map = map_layout(req.layout)?;
    let rate = stems.sample_rate;
    let mut enc = Encoder::new(Config {
        acmod: map.acmod,
        lfe: map.lfe,
        sample_rate: rate,
        bitrate_kbps: req.bitrate_kbps,
        dialnorm: req.dialnorm,
        dolby_surround: map.dolby_surround,
        bandwidth_hz: None,
        eac3: req.eac3,
    })?;
    let first = ((req.start_secs * rate as f64) as u64).min(stems.frames);
    stems.seek_frame(first)?;
    let avail = stems.frames - first;
    let want = req
        .duration_secs
        .map(|d| (d * rate as f64) as u64)
        .unwrap_or(avail)
        .min(avail);
    // one extra block of zeros flushes the transform delay
    let total_frames = (want + 256).div_ceil(SAMPLES_PER_FRAME as u64);
    let mut out = std::io::BufWriter::new(std::fs::File::create(req.output)?);

    let nch = enc.channels();
    let bytes = drive(stems, want, total_frames, cancel, on_progress, |planar, got, count| {
        let batch: Vec<Vec<Vec<f32>>> = (0..count)
            .map(|k| {
                let (from, to) = frame_span(k, got);
                let mut frame = vec![vec![0.0f32; SAMPLES_PER_FRAME]; nch];
                if to > from {
                    for (ac3_ch, &src) in map.source_index.iter().enumerate() {
                        frame[ac3_ch][..to - from].copy_from_slice(&planar[src][from..to]);
                    }
                }
                frame
            })
            .collect();
        let mut bytes = 0u64;
        for frame in enc.encode_batch(&batch) {
            out.write_all(&frame)?;
            bytes += frame.len() as u64;
        }
        Ok(bytes)
    })?;
    out.flush()?;
    on_progress(1.0);
    Ok(Stats {
        frames: total_frames,
        input_frames: want,
        bytes,
    })
}

/// 7.1 / 7.0 as E-AC-3: the independent substream carries a 5.1 (5.0) downmix in which the rear channels are
/// folded into the surrounds; a dependent substream carries the true side surrounds and the rear pair, with a
/// custom channel map that makes a 7.1 decoder replace the downmixed surrounds and add the rears.
/// Where the channels of a 7.1 (or 7.0) layout are: the independent substream carries L C R and the surrounds with
/// the rears folded in, the dependent one the surrounds and rears themselves.
struct Map71 {
    l: usize,
    r: usize,
    c: usize,
    sl: usize,
    sr: usize,
    bl: usize,
    br: usize,
    lfe: Option<usize>,
}

fn map_71(layout: &sw_core::ChannelLayout) -> Result<Map71, Error> {
    use sw_core::Channel::*;
    let unsupported = || Error::Unsupported(format!("layout '{}' cannot be carried by E-AC-3", layout.id));
    let pos = |chs: &[sw_core::Channel]| layout.channels.iter().position(|c| chs.contains(c));
    let map = Map71 {
        l: pos(&[L]).ok_or_else(unsupported)?,
        r: pos(&[R]).ok_or_else(unsupported)?,
        c: pos(&[C]).ok_or_else(unsupported)?,
        sl: pos(&[Ls, Lss]).ok_or_else(unsupported)?,
        sr: pos(&[Rs, Rss]).ok_or_else(unsupported)?,
        bl: pos(&[Lsr]).ok_or_else(unsupported)?,
        br: pos(&[Rsr]).ok_or_else(unsupported)?,
        lfe: pos(&[Lfe]),
    };
    if layout.channel_count() != 7 + usize::from(map.lfe.is_some()) {
        return Err(unsupported());
    }
    Ok(map)
}

/// The two encoders of a 7.1 stream, sharing the bit rate by channel count (the dependent substream has four of the
/// programme's channels).
fn encoders_71(req: &Request, rate: u32, has_lfe: bool) -> Result<(Encoder, Encoder), Error> {
    let total = req.bitrate_kbps;
    let share = |w: f64| ((total as f64 * w).round() as u32).max(8);
    let indep_kbps = share(if has_lfe { 5.1 / 9.1 } else { 5.0 / 9.0 });
    let dep_kbps = total.saturating_sub(indep_kbps).max(8);
    let config = |acmod, lfe, bitrate_kbps| Config {
        acmod,
        lfe,
        sample_rate: rate,
        bitrate_kbps,
        dialnorm: req.dialnorm,
        dolby_surround: false,
        bandwidth_hz: None,
        eac3: true,
    };
    let indep = Encoder::new(config(7, has_lfe, indep_kbps))?;
    let mut dep = Encoder::new(config(6, false, dep_kbps))?;
    // Ls, Rs and the Lrs/Rrs pair: custom channel map bits 3, 4 and 6 (bit 15 is the Left location)
    dep.set_dependent((1 << 12) | (1 << 11) | (1 << 9));
    Ok((indep, dep))
}

/// The input of one frame for both substreams: the surrounds carry the rears folded in at -3 dB, the dependent
/// substream the four surround channels themselves.
fn frame_71(m: &Map71, planar: &[Vec<f32>], from: usize, to: usize) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
    const FOLD: f32 = std::f32::consts::FRAC_1_SQRT_2;
    let n = SAMPLES_PER_FRAME;
    let mut ibuf: Vec<Vec<f32>> = vec![vec![0.0; n]; 5 + usize::from(m.lfe.is_some())];
    let mut dbuf: Vec<Vec<f32>> = vec![vec![0.0; n]; 4];
    for i in 0..to - from {
        let at = from + i;
        ibuf[0][i] = planar[m.l][at];
        ibuf[1][i] = planar[m.c][at];
        ibuf[2][i] = planar[m.r][at];
        ibuf[3][i] = planar[m.sl][at] + FOLD * planar[m.bl][at];
        ibuf[4][i] = planar[m.sr][at] + FOLD * planar[m.br][at];
        if let Some(x) = m.lfe {
            ibuf[5][i] = planar[x][at];
        }
        dbuf[0][i] = planar[m.sl][at];
        dbuf[1][i] = planar[m.sr][at];
        dbuf[2][i] = planar[m.bl][at];
        dbuf[3][i] = planar[m.br][at];
    }
    (ibuf, dbuf)
}

fn encode_extended(
    stems: &mut StemSet,
    req: &Request,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Result<Stats, NativeError> {
    let map = map_71(req.layout)?;
    let rate = stems.sample_rate;
    let (mut indep, mut dep) = encoders_71(req, rate, map.lfe.is_some())?;
    let first = ((req.start_secs * rate as f64) as u64).min(stems.frames);
    stems.seek_frame(first)?;
    let avail = stems.frames - first;
    let want = req
        .duration_secs
        .map(|d| (d * rate as f64) as u64)
        .unwrap_or(avail)
        .min(avail);
    let total_frames = (want + 256).div_ceil(SAMPLES_PER_FRAME as u64);
    let mut out = std::io::BufWriter::new(std::fs::File::create(req.output)?);
    let bytes = drive(stems, want, total_frames, cancel, on_progress, |planar, got, count| {
        let (ibatch, dbatch): (Vec<_>, Vec<_>) = (0..count)
            .map(|k| {
                let (from, to) = frame_span(k, got);
                frame_71(&map, planar, from, to)
            })
            .unzip();
        let (a, b) = rayon::join(|| indep.encode_batch(&ibatch), || dep.encode_batch(&dbatch));
        let mut bytes = 0u64;
        for (a, b) in a.iter().zip(&b) {
            out.write_all(a)?;
            out.write_all(b)?;
            bytes += (a.len() + b.len()) as u64;
        }
        Ok(bytes)
    })?;
    out.flush()?;
    on_progress(1.0);
    Ok(Stats {
        frames: total_frames,
        input_frames: want,
        bytes,
    })
}
