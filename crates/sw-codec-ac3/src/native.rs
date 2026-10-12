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
    let mut planar: Vec<Vec<f32>> = Vec::new();
    let mut remaining = want;
    let mut bytes = 0u64;
    let mut buf: Vec<Vec<f32>> = vec![vec![0.0; SAMPLES_PER_FRAME]; nch];
    for f in 0..total_frames {
        if cancel.load(Ordering::Relaxed) {
            return Err(NativeError::Canceled);
        }
        let n = (remaining as usize).min(SAMPLES_PER_FRAME);
        if n > 0 {
            stems.read_planar(&mut planar, n)?;
        }
        for (ac3_ch, &src) in map.source_index.iter().enumerate() {
            let dst = &mut buf[ac3_ch];
            dst.fill(0.0);
            if n > 0 {
                dst[..n].copy_from_slice(&planar[src][..n]);
            }
        }
        remaining -= n as u64;
        let refs: Vec<&[f32]> = buf.iter().map(|v| v.as_slice()).collect();
        let frame = enc.encode_frame(&refs);
        out.write_all(&frame)?;
        bytes += frame.len() as u64;
        if f % 64 == 0 {
            on_progress(f as f32 / total_frames as f32);
        }
    }
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
fn encode_extended(
    stems: &mut StemSet,
    req: &Request,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Result<Stats, NativeError> {
    use sw_core::Channel::*;
    let unsupported = || Error::Unsupported(format!("layout '{}' cannot be carried by E-AC-3", req.layout.id));
    let pos = |chs: &[sw_core::Channel]| req.layout.channels.iter().position(|c| chs.contains(c));
    let (l, r, c) = (
        pos(&[L]).ok_or_else(unsupported)?,
        pos(&[R]).ok_or_else(unsupported)?,
        pos(&[C]).ok_or_else(unsupported)?,
    );
    let (sl, sr) = (
        pos(&[Ls, Lss]).ok_or_else(unsupported)?,
        pos(&[Rs, Rss]).ok_or_else(unsupported)?,
    );
    let (bl, br) = (
        pos(&[Lsr]).ok_or_else(unsupported)?,
        pos(&[Rsr]).ok_or_else(unsupported)?,
    );
    let lfe = pos(&[Lfe]);
    if req.layout.channel_count() != 7 + usize::from(lfe.is_some()) {
        return Err(unsupported().into());
    }
    let rate = stems.sample_rate;
    // bit rate shared by channel count: the dependent substream has four of the 7.1 programme's channels
    let total = req.bitrate_kbps;
    let share = |w: f64| ((total as f64 * w).round() as u32).max(8);
    let indep_kbps = share(if lfe.is_some() { 5.1 / 9.1 } else { 5.0 / 9.0 });
    let dep_kbps = total.saturating_sub(indep_kbps).max(8);
    let mut indep = Encoder::new(Config {
        acmod: 7,
        lfe: lfe.is_some(),
        sample_rate: rate,
        bitrate_kbps: indep_kbps,
        dialnorm: req.dialnorm,
        dolby_surround: false,
        bandwidth_hz: None,
        eac3: true,
    })?;
    let mut dep = Encoder::new(Config {
        acmod: 6,
        lfe: false,
        sample_rate: rate,
        bitrate_kbps: dep_kbps,
        dialnorm: req.dialnorm,
        dolby_surround: false,
        bandwidth_hz: None,
        eac3: true,
    })?;
    // Ls, Rs and the Lrs/Rrs pair: custom channel map bits 3, 4 and 6 (bit 15 is the Left location)
    dep.set_dependent((1 << 12) | (1 << 11) | (1 << 9));
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
    let n = SAMPLES_PER_FRAME;
    let mut planar: Vec<Vec<f32>> = Vec::new();
    let mut remaining = want;
    let mut bytes = 0u64;
    let mut ibuf: Vec<Vec<f32>> = vec![vec![0.0; n]; 5 + usize::from(lfe.is_some())];
    let mut dbuf: Vec<Vec<f32>> = vec![vec![0.0; n]; 4];
    const FOLD: f32 = std::f32::consts::FRAC_1_SQRT_2;
    for f in 0..total_frames {
        if cancel.load(Ordering::Relaxed) {
            return Err(NativeError::Canceled);
        }
        let have = (remaining as usize).min(n);
        if have > 0 {
            stems.read_planar(&mut planar, have)?;
        }
        for b in ibuf.iter_mut().chain(dbuf.iter_mut()) {
            b.fill(0.0);
        }
        for i in 0..have {
            ibuf[0][i] = planar[l][i];
            ibuf[1][i] = planar[c][i];
            ibuf[2][i] = planar[r][i];
            ibuf[3][i] = planar[sl][i] + FOLD * planar[bl][i];
            ibuf[4][i] = planar[sr][i] + FOLD * planar[br][i];
            if let Some(x) = lfe {
                ibuf[5][i] = planar[x][i];
            }
            dbuf[0][i] = planar[sl][i];
            dbuf[1][i] = planar[sr][i];
            dbuf[2][i] = planar[bl][i];
            dbuf[3][i] = planar[br][i];
        }
        remaining -= have as u64;
        let irefs: Vec<&[f32]> = ibuf.iter().map(|v| v.as_slice()).collect();
        let drefs: Vec<&[f32]> = dbuf.iter().map(|v| v.as_slice()).collect();
        let a = indep.encode_frame(&irefs);
        let b = dep.encode_frame(&drefs);
        out.write_all(&a)?;
        out.write_all(&b)?;
        bytes += (a.len() + b.len()) as u64;
        if f % 64 == 0 {
            on_progress(f as f32 / total_frames as f32);
        }
    }
    out.flush()?;
    on_progress(1.0);
    Ok(Stats {
        frames: total_frames,
        input_frames: want,
        bytes,
    })
}
