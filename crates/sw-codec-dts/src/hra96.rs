//! DTS-HD High Resolution Audio for 7.1 / 7.0 at 96 kHz.
//!
//! The layout of the commercial encoder's streams: a plain 48 kHz core (5.1 downmix of the 7.1 programme, the rear
//! pair folded into the sides) in the core substream, and in the extension substream an XXCH extension (the rear pair,
//! coded at 48 kHz) next to an X96 extension with two channel sets: the five core channels and the rear pair. X96
//! carries what the 48 kHz coding cannot: the upper half of the spectrum, and a refinement of the lower half.

use crate::dts96::{decimator_taps, HALF};
use crate::encoder::{Config as CoreConfig, Encoder as CoreEncoder, XxchParams};
use crate::exss::{exss_frame_xxch_x96, ExssParams};
use crate::native::{NativeError, Request, Stats};
use crate::native_xxch::XxchMap;
use crate::qmf64::{Analysis64, BANDS as BANDS64};
use crate::x96::X96Encoder;
use crate::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use sw_pcm::StemSet;

/// Pre-scaling of every core channel (the decoder multiplies by `SCALE_INV` code 55) and the rear coefficient
/// (dmix table entry 217, 23170 / 32768), as in the 48 kHz 7.1 stream.
const SCALE: f64 = 65536.0 / 92682.0;
const REAR_COEFF: f64 = 23170.0 / 32768.0;
/// Share of the extension substream's payload the XXCH extension gets (the commercial encoder's split).
const XXCH_SHARE: f64 = 0.202;

pub struct Hra96Config {
    pub has_lfe: bool,
    pub total_kbps: u32,
    pub dialnorm: i32,
    pub pcm_bits: u32,
    pub exss: ExssParams,
}

pub struct Hra96Encoder {
    core: CoreEncoder,
    ext: CoreEncoder,
    ext_ana: Vec<Analysis64>,
    core_ana: Vec<Analysis64>,
    x96: X96Encoder,
    params: XxchParams,
    exss: ExssParams,
    taps: Vec<f64>,
    /// Source samples (96 kHz) per channel: the core channels (LFE last), then the two rears; not yet consumed.
    buf: Vec<Vec<f64>>,
    dropped: usize,
    emitted: usize,
    ncore: usize,
    nprim: usize,
    n: usize,
    l48: usize,
    phase: usize,
    lookahead: usize,
    ana_delay: usize,
    ana_shift: isize,
    ana_adj: isize,
    bits_num: u64,
    bits_den: u64,
    overhead: usize,
    timing: [std::time::Duration; 5],
}

fn mk(amode: u8, channels: usize, lfe: bool, bitrate: u32, dialnorm: i32, pcm_bits: u32) -> CoreConfig {
    CoreConfig { sample_rate: 48_000, amode, channels, lfe, bitrate, dialnorm, pcm_bits, es: false, xch: false, perfect: false, blocks: 16 }
}

impl Hra96Encoder {
    pub fn new(cfg: Hra96Config) -> Result<Hra96Encoder, Error> {
        let nprim = 5;
        let ncore = nprim + usize::from(cfg.has_lfe);
        let mut core = CoreEncoder::new(mk(9, nprim, cfg.has_lfe, 1_509_000, cfg.dialnorm, cfg.pcm_bits))?;
        core.keep_residual = true;
        let mut ext = CoreEncoder::new(mk(2, 2, false, 1_536_000, cfg.dialnorm, cfg.pcm_bits))?;
        ext.keep_residual = true;
        let n = core.frame_samples() * 2;
        let l48: usize = std::env::var("SW_L48").ok().and_then(|v| v.parse().ok()).unwrap_or(480);
        let phase: f64 = std::env::var("SW_PHASE").ok().and_then(|v| v.parse().ok()).unwrap_or(0.5);
        let frac = phase - phase.floor();
        let phase = phase.floor() as usize;
        let core_ana: Vec<Analysis64> = (0..nprim).map(|_| Analysis64::new()).collect();
        let ext_ana: Vec<Analysis64> = (0..2).map(|_| Analysis64::new()).collect();
        let ana_delay = core_ana[0].delay();
        let ana_shift: isize = std::env::var("SW_X96_SHIFT").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
        let lookahead = (2 * (l48 + 1) + phase + HALF + 2).max(ana_delay + 64 * 16 + 64);
        let core_mask = 0x1 | 0x2 | 0x4 | (if cfg.has_lfe { 0x20 } else { 0 }) | (1 << 9) | (1 << 10);
        let params = XxchParams { core_mask, spk_mask: (1 << 7) | (1 << 8), mask_bits: 11, dmix: Some((55, vec![vec![(9, 119)], vec![(10, 119)]])) };
        let overhead = exss_frame_xxch_x96(&cfg.exss, &[0u8; 4], &[0u8; 4]).len() - 8;
        Ok(Hra96Encoder {
            core,
            ext,
            ext_ana,
            core_ana,
            x96: X96Encoder::new(16),
            params,
            exss: cfg.exss,
            taps: decimator_taps(frac),
            buf: vec![Vec::new(); ncore + 2],
            dropped: 0,
            emitted: 0,
            ncore,
            nprim,
            n,
            l48,
            phase,
            lookahead,
            ana_delay,
            ana_shift,
            ana_adj: std::env::var("SW_X96_ADJ").ok().and_then(|v| v.parse().ok()).unwrap_or(0),
            bits_num: cfg.total_kbps as u64 * 1000 * n as u64,
            bits_den: 96_000,
            overhead,
            timing: [std::time::Duration::ZERO; 5],
        })
    }

    /// Source samples (96 kHz) per frame.
    pub fn frame_samples(&self) -> usize {
        self.n
    }

    /// Delay in 96 kHz samples between the source and what the decoder outputs: the filter bank's, less the
    /// lookahead the decimated core input is taken ahead by.
    pub fn delay(&self) -> usize {
        (2 * self.core.delay()).saturating_sub(2 * self.l48)
    }

    /// Feed source samples: core channels (C L R Ls Rs, LFE) in the core's pre-scaled mix, then the rear pair.
    pub fn push(&mut self, frame: &[Vec<f64>]) -> Vec<Vec<u8>> {
        for (b, f) in self.buf.iter_mut().zip(frame) {
            b.extend_from_slice(f);
        }
        let mut out = Vec::new();
        while self.buf[0].len() + self.dropped >= (self.emitted + 1) * self.n + self.lookahead {
            out.push(self.next_frame());
        }
        out
    }

    pub fn finish(&mut self) -> Vec<Vec<u8>> {
        let pushed = self.buf[0].len() + self.dropped;
        let frames = pushed.div_ceil(self.n) + 1;
        let want = frames * self.n + self.lookahead;
        for b in self.buf.iter_mut() {
            b.resize((want - self.dropped).max(b.len()), 0.0);
        }
        let mut out = Vec::new();
        while self.emitted < frames {
            out.push(self.next_frame());
        }
        if std::env::var_os("SW_TIMING").is_some() {
            eprintln!("hra96 timing: decimate {:?}, core+xxch {:?}, 64-band analysis {:?}, x96 coding {:?}", self.timing[0], self.timing[1], self.timing[2], self.timing[3]);
        }
        out
    }

    fn decimated(&self, ch: usize, frame: usize) -> Vec<f64> {
        let n48 = self.n / 2;
        let src = &self.buf[ch];
        let mut out = Vec::with_capacity(n48);
        for i in 0..n48 {
            let m = frame * n48 + i + self.l48;
            let centre = (2 * m + self.phase) as isize - self.dropped as isize;
            let mut acc = 0.0;
            for (j, h) in self.taps.iter().enumerate() {
                let idx = centre + j as isize - HALF as isize;
                if idx >= 0 && (idx as usize) < src.len() {
                    acc += h * src[idx as usize];
                }
            }
            out.push(acc);
        }
        out
    }

    /// 64-band analysis of one frame of a channel, aligned with what the decoder's synthesis outputs, minus the
    /// 48 kHz coding's reconstruction in the lower 32 bands.
    fn analyse(ana: &mut Analysis64, src: &[f64], f_off: usize, dropped: usize, start_adj: isize, delay: usize, deq: &[Vec<i32>]) -> Vec<Vec<f64>> {
        let blocks = 16;
        let start = f_off as isize + delay as isize + start_adj - dropped as isize;
        let input: Vec<f64> = (0..blocks * 64)
            .map(|i| {
                let idx = start + i as isize;
                if idx >= 0 && (idx as usize) < src.len() { src[idx as usize] } else { 0.0 }
            })
            .collect();
        let mut bands = ana.analyse_blocks(&input);
        debug_assert_eq!(bands.len(), BANDS64);
        for band in 0..32 {
            for b in 0..blocks {
                bands[band][b] -= deq[band][b] as f64;
            }
        }
        bands
    }

    fn next_frame(&mut self) -> Vec<u8> {
        let f = self.emitted;
        let (ncore, nprim) = (self.ncore, self.nprim);
        let t0 = std::time::Instant::now();
        let dec: Vec<Vec<f64>> = {
            let me: &Hra96Encoder = &*self;
            use rayon::prelude::*;
            (0..ncore + 2).into_par_iter().map(|c| me.decimated(c, f)).collect()
        };
        self.timing[0] += t0.elapsed();
        let t0 = std::time::Instant::now();
        let mut data = self.core.encode_frame(&dec[..ncore]);
        data.resize(data.len() + (4 - data.len() % 4) % 4, 0);
        // sizes: the rest of the frame after the core and the extension substream header, split between XXCH and X96
        let upto = |k: u64| (k * self.bits_num) / (self.bits_den * 8);
        let total = (upto(f as u64 + 1) - upto(f as u64)) as usize;
        let rest = total.saturating_sub(data.len() + self.overhead);
        let x96_bytes = ((rest as f64 * (1.0 - XXCH_SHARE)) as usize & !3).clamp(64, 4092);
        let xxch_bytes = (rest.saturating_sub(x96_bytes) & !3).clamp(64, 16_000);
        let xxch = self.ext.encode_xxch(&dec[ncore..], xxch_bytes, &self.params);
        self.timing[1] += t0.elapsed();
        let t0 = std::time::Instant::now();
        // X96: the five core channels, then the rears
        let (f_off, dropped, delay, adj) = (f * self.n, self.dropped, self.ana_delay, 64 * self.ana_shift + self.ana_adj);
        let bufs = &self.buf;
        let core_deq = &self.core.last_deq;
        let ext_deq = &self.ext.last_deq;
        let (r_core, r_ext): (Vec<Vec<Vec<f64>>>, Vec<Vec<Vec<f64>>>) = {
            use rayon::prelude::*;
            let a: Vec<_> = self.core_ana.par_iter_mut().enumerate().map(|(ch, ana)| Self::analyse(ana, &bufs[ch], f_off, dropped, adj, delay, &core_deq[ch])).collect();
            let b: Vec<_> = self.ext_ana.par_iter_mut().enumerate().map(|(ch, ana)| Self::analyse(ana, &bufs[ncore + ch], f_off, dropped, adj, delay, &ext_deq[ch])).collect();
            (a, b)
        };
        let _ = nprim;
        self.timing[2] += t0.elapsed();
        let t0 = std::time::Instant::now();
        let x96 = self.x96.encode_exss(&[&r_core, &r_ext], x96_bytes);
        self.timing[3] += t0.elapsed();
        data.extend_from_slice(&exss_frame_xxch_x96(&self.exss, &xxch, &x96));
        self.emitted += 1;
        // drop samples no later frame needs
        let keep_from = (self.emitted * self.n).saturating_sub(HALF + 4 + 64);
        if keep_from > self.dropped + (1 << 16) {
            let cut = keep_from - self.dropped;
            for b in self.buf.iter_mut() {
                b.drain(..cut);
            }
            self.dropped = keep_from;
        }
        data
    }
}

/// File-level driver: 96 kHz 7.1 / 7.0 stems in, `.dtshd` (or raw) out.
pub fn encode_to_file(stems: &mut StemSet, req: &Request, map: XxchMap, on_progress: &mut dyn FnMut(f32), cancel: &AtomicBool) -> Result<Stats, NativeError> {
    let rate = stems.sample_rate;
    if rate != 96_000 {
        return Err(Error::Unsupported("HRA 7.1 above 48 kHz is implemented at 96 kHz".into()).into());
    }
    let total_kbps = req.hra_kbps.unwrap_or(5376);
    let has_lfe = map.lfe.is_some();
    let nch_total = 7 + usize::from(has_lfe);
    let spk = crate::ma::map_layout(req.layout).exss_mask;
    let exss = ExssParams { sample_rate: rate, channels: nch_total, bit_res: req.pcm_bits, spk_mask: spk, with_core: true, dialnorm: 0, frame_duration_code: 0, x96_in_core: false, secondary: false };
    let mut enc = Hra96Encoder::new(Hra96Config { has_lfe, total_kbps, dialnorm: req.dialnorm, pcm_bits: req.pcm_bits, exss })?;
    let first = ((req.start_secs * rate as f64).round() as u64).min(stems.frames);
    stems.seek_frame(first)?;
    let avail = stems.frames - first;
    let want = req.duration_secs.map(|d| (d * rate as f64).round() as u64).unwrap_or(avail).min(avail);
    if want == 0 {
        return Err(NativeError::Empty);
    }
    let n = enc.frame_samples();
    let container = req.output.extension().map_or(false, |e| e.eq_ignore_ascii_case("dtshd"));
    let core_payload = (1509u32 * 1000 * 512 / 48_000 / 8) & !3;
    let codec_delay = 2048usize;
    let core_mask: u32 = map.core.iter().map(|&i| crate::ma::exss_bit(req.layout.channels[i])).fold(0, |m, b| m | b) | if has_lfe { 0x8 } else { 0 };
    let info = container.then(|| {
        let mut i = crate::dtshd::DtsHdInfo::new(rate, n as u16, want, spk as u16, crate::dtshd::Kind::CoreAndExtension);
        i.core_kbps = 1509;
        i.core_payload = core_payload;
        i.codec_delay = codec_delay as u16;
        // the core is the 48 kHz 5.1 downmix
        i.core_rate = Some(48_000);
        i.core_mask = Some(core_mask as u16);
        i
    });
    let mut out = crate::dtshd::StreamOut::create(req.output, info)?;
    // the decoder lags the source by `delay`: a container states `codec_delay` and the stream starts with silence
    // to make up the difference; a raw stream starts that many samples into the source instead
    let delay = enc.delay();
    let (skip, lead) = if container { (0usize, codec_delay.saturating_sub(delay)) } else { (delay.min(want as usize), 0) };
    let mut planar: Vec<Vec<f32>> = Vec::new();
    let mut left = skip as u64;
    while left > 0 {
        let k = left.min(n as u64) as usize;
        stems.read_planar(&mut planar, k)?;
        left -= k as u64;
    }
    let mut remaining = want - skip as u64;
    let to24 = |v: f32| v as f64 * 8_388_608.0;
    let clamp = |v: f64| v.clamp(-8_388_608.0, 8_388_607.0);
    let (mut frames, mut bytes) = (0u64, 0u64);
    let mut write = |out: &mut crate::dtshd::StreamOut, fr: Vec<Vec<u8>>, frames: &mut u64, bytes: &mut u64| -> Result<(), NativeError> {
        for f in fr {
            out.write_frame(&f, f.len().saturating_sub(core_payload as usize))?;
            *frames += 1;
            *bytes += f.len() as u64;
        }
        Ok(())
    };
    if lead > 0 {
        let silence = vec![vec![0.0f64; lead]; nch_total];
        let fr = enc.push(&silence);
        write(&mut out, fr, &mut frames, &mut bytes)?;
    }
    let chunk = n * 4;
    while remaining > 0 {
        if cancel.load(Ordering::Relaxed) {
            return Err(NativeError::Canceled);
        }
        let got = (remaining as usize).min(chunk);
        stems.read_planar(&mut planar, got)?;
        let mut block: Vec<Vec<f64>> = vec![vec![0.0; got]; nch_total];
        for i in 0..got {
            for (slot, &src) in map.core.iter().enumerate() {
                let mut v = to24(planar[src][i]) * SCALE;
                if slot >= 3 {
                    // the sides carry the rear on the same side
                    v += to24(planar[map.rear[slot - 3]][i]) * REAR_COEFF;
                }
                block[slot][i] = clamp(v);
            }
            if let Some(l) = map.lfe {
                block[5][i] = clamp(to24(planar[l][i]) * SCALE);
            }
            for (slot, &src) in map.rear.iter().enumerate() {
                block[nch_total - 2 + slot][i] = clamp(to24(planar[src][i]));
            }
        }
        remaining -= got as u64;
        let fr = enc.push(&block);
        write(&mut out, fr, &mut frames, &mut bytes)?;
        on_progress(1.0 - remaining as f32 / want as f32);
    }
    let fr = enc.finish();
    write(&mut out, fr, &mut frames, &mut bytes)?;
    // the silent frame StreamTools' Add silence inserts: a fresh encoder fed zeros until its filters have settled
    let blackout = if container {
        let exss = ExssParams { sample_rate: rate, channels: nch_total, bit_res: req.pcm_bits, spk_mask: spk, with_core: true, dialnorm: 0, frame_duration_code: 0, x96_in_core: false, secondary: false };
        Hra96Encoder::new(Hra96Config { has_lfe, total_kbps, dialnorm: req.dialnorm, pcm_bits: req.pcm_bits, exss }).ok().and_then(|mut e| {
            let zeros = vec![vec![0.0f64; 6 * n]; nch_total];
            let mut fr = e.push(&zeros);
            fr.pop()
        })
    } else {
        None
    };
    out.finish(blackout.as_deref())?;
    on_progress(1.0);
    Ok(Stats { frames, bytes })
}
