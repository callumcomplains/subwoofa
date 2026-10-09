//! File-level driver for DTS-HD Master Audio: stems in, `.dtshd` (or raw) stream out.

use crate::exss::{dtshd_header, DtsHdHeader};
use crate::ma::{map_layout, MaConfig, MaEncoder};
use crate::ma_core::{MaCoreConfig, MaCoreEncoder};
use std::io::{Seek, SeekFrom, Write};
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
}

pub struct MaStats {
    pub frames: u64,
    pub bytes: u64,
}

fn lossless_frames(e: &mut MaEncoder, pcm: &[Vec<i32>], got: usize, n: usize) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < got {
        let part: Vec<Vec<i32>> = pcm.iter().map(|c| { let mut v = c[pos..(pos + n).min(c.len())].to_vec(); v.resize(n, 0); v }).collect();
        out.push(e.encode_frame(&part));
        pos += n;
    }
    out
}

enum Enc {
    Core(MaCoreEncoder),
    Lossless(MaEncoder),
}

pub fn encode_to_file(stems: &mut StemSet, req: &MaRequest, on_progress: &mut dyn FnMut(f32), cancel: &AtomicBool) -> Result<MaStats, MaError> {
    let rate = stems.sample_rate;
    if !matches!(rate, 44_100 | 48_000 | 88_200 | 96_000 | 176_400 | 192_000) {
        return Err(MaError::Unsupported(format!("{rate} Hz: only 44.1 to 192 kHz are implemented so far")));
    }
    let bit_res = (stems.bits as u32).clamp(16, 24);
    let bit_res = if bit_res == 16 || bit_res == 20 { bit_res } else { 24 };
    let map = map_layout(req.layout);
    let nch = map.order.len();
    if nch > 8 {
        return Err(MaError::Unsupported("more than 8 channels".into()));
    }
    if rate > 96_000 && nch > 6 {
        return Err(MaError::Unsupported("above 96 kHz: up to 5.1 channels".into()));
    }
    // a core at 176.4/192 kHz is not implemented (the lossless stream stands alone, as in a core-less MA stream)
    let core_bitrate_kbps = if rate > 96_000 { None } else { req.core_bitrate_kbps };
    let mut enc = match core_bitrate_kbps {
        Some(kbps) => Enc::Core(MaCoreEncoder::new(MaCoreConfig { layout: req.layout, sample_rate: rate, bit_res, core_bitrate: kbps * 1000, dialnorm: req.dialnorm, x96: req.x96 })?),
        None => Enc::Lossless(MaEncoder::new(MaConfig { sample_rate: rate, map: map.clone(), bit_res, dialnorm: 0 })),
    };
    let n = match &enc {
        Enc::Core(e) => e.frame_samples(),
        Enc::Lossless(e) => e.frame_samples(),
    };
    let first = ((req.start_secs * rate as f64).round() as u64).min(stems.frames);
    stems.seek_frame(first)?;
    let avail = stems.frames - first;
    let want = req.duration_secs.map(|d| (d * rate as f64).round() as u64).unwrap_or(avail).min(avail);
    if want == 0 {
        return Err(MaError::Empty);
    }
    let frames_total = want.div_ceil(n as u64);

    let mut file = std::io::BufWriter::new(std::fs::File::create(req.output)?);
    let header_len = if req.container {
        let h = dtshd_header(&DtsHdHeader { sample_rate: rate, frames: frames_total as u32, samples_per_frame: n as u16, total_samples: want, ch_mask: map.exss_mask as u16, initial_padding: 0 }, 0);
        file.write_all(&h)?;
        h.len() as u64
    } else {
        0
    };

    let scale = (1i64 << (bit_res - 1)) as f32;
    let (lo, hi) = (-(1i64 << (bit_res - 1)), (1i64 << (bit_res - 1)) - 1);
    let mut planar: Vec<Vec<f32>> = Vec::new();
    let mut remaining = want;
    let mut tm = [0f64; 3];
    let mut bytes = 0u64;
    let mut frames = 0u64;
    // several frames per push so the lossless stage can run in parallel
    let chunk = n * 16;
    let mut pcm: Vec<Vec<i32>> = vec![vec![0; chunk]; nch];
    while remaining > 0 {
        if cancel.load(Ordering::Relaxed) {
            return Err(MaError::Canceled);
        }
        let got = (remaining as usize).min(chunk);
        let t_r = std::time::Instant::now();
        stems.read_planar(&mut planar, got)?;
        tm[0] += t_r.elapsed().as_secs_f64();
        let t_c = std::time::Instant::now();
        for (j, &lc) in map.order.iter().enumerate() {
            let dst = &mut pcm[j];
            dst.iter_mut().for_each(|v| *v = 0);
            for i in 0..got {
                dst[i] = ((planar[lc][i] * scale).round() as i64).clamp(lo, hi) as i32;
            }
        }
        remaining -= got as u64;
        tm[1] += t_c.elapsed().as_secs_f64();
        let t_p = std::time::Instant::now();
        let ready: Vec<Vec<u8>> = match &mut enc {
            Enc::Core(e) => {
                let part: Vec<Vec<i32>> = pcm.iter().map(|c| c[..got].to_vec()).collect();
                e.push(&part)
            }
            Enc::Lossless(e) => pcm.iter().map(|c| c.len()).max().map_or(vec![], |_| lossless_frames(e, &pcm, got, n)),
        };
        tm[2] += t_p.elapsed().as_secs_f64();
        for f in ready {
            file.write_all(&f)?;
            bytes += f.len() as u64;
            frames += 1;
        }
        on_progress(1.0 - remaining as f32 / want as f32);
    }
    if std::env::var_os("SW_TIMING").is_some() {
        eprintln!("ma driver: read {:.2}s convert {:.2}s encode {:.2}s", tm[0], tm[1], tm[2]);
    }
    if let Enc::Core(e) = &mut enc {
        for f in e.finish() {
            file.write_all(&f)?;
            bytes += f.len() as u64;
            frames += 1;
        }
    }
    if req.container {
        let h = dtshd_header(&DtsHdHeader { sample_rate: rate, frames: frames as u32, samples_per_frame: n as u16, total_samples: want, ch_mask: map.exss_mask as u16, initial_padding: 0 }, bytes);
        debug_assert_eq!(h.len() as u64, header_len);
        file.flush()?;
        let mut inner = file.into_inner().map_err(|e| e.into_error())?;
        inner.seek(SeekFrom::Start(0))?;
        inner.write_all(&h)?;
        inner.flush()?;
    } else {
        file.flush()?;
    }
    on_progress(1.0);
    Ok(MaStats { frames, bytes })
}
