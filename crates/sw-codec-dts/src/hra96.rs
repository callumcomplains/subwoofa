//! DTS-HD High Resolution Audio at 96 kHz, in the layout of the commercial encoder's streams.
//!
//! The core substream holds a plain 48 kHz core (1509 kbps, 5.1 or 5.0). The extension substream holds an X96
//! extension carrying what the 48 kHz coding cannot: the upper half of the spectrum, and a refinement of the lower
//! half. What else goes where depends on the layout:
//!
//! | layout            | core                                  | extension substream               |
//! |-------------------|---------------------------------------|-----------------------------------|
//! | 5.1 / 5.0         | the channels                          | X96 (one channel set)             |
//! | 6.1 matrix        | Cs mixed into the surrounds (ES flag) | X96 (one channel set)             |
//! | 6.1 discrete      | Cs mixed in, and sent again in XCH    | X96 (the core channels, then Cs)  |
//! | 7.1 / 7.0         | rears folded into the sides           | XXCH (the rear pair) and X96      |

use crate::dts96::{decimator_taps, HALF};
use crate::encoder::{Config as CoreConfig, Encoder as CoreEncoder, XxchParams};
use crate::exss::{exss_frame_x96, exss_frame_xxch_x96, ExssParams};
use crate::native::{NativeError, Request, Stats};
use crate::qmf64::{Analysis64, BANDS as BANDS64};
use crate::x96::X96Encoder;
use crate::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use sw_core::{Channel, ChannelLayout};
use sw_pcm::StemSet;

/// Pre-scaling of every core channel in the 7.1 stream (the decoder multiplies by `SCALE_INV` code 55) and the rear
/// coefficient (dmix table entry 217, 23170 / 32768).
const SCALE: f64 = 65536.0 / 92682.0;
const REAR_COEFF: f64 = 23170.0 / 32768.0;
/// Bytes of the XXCH extension in a 96 kHz 7.x stream until X96 is full (the original encoder's split, measured at every legal rate).
const XXCH_FIXED_BYTES: usize = 512;
/// The largest X96 component an extension substream header can describe (a 12-bit size, rounded down to words).
const MAX_X96_BYTES: usize = 4092;
/// Gain of the centre surround where it is mixed into the surrounds.
const CS_MIX: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// What the stream carries besides the 5.x core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hra96Kind {
    /// 5.1 or 5.0.
    Plain,
    /// 6.x with the centre surround matrixed into the surrounds (the core carries the ES flag).
    Matrix,
    /// 6.x with the centre surround also sent as the XCH extension of the core.
    Discrete,
    /// 7.x: the rear pair in an XXCH extension.
    Rear,
}

impl Hra96Kind {
    /// Channels coded beside the core's five (and LFE): Cs for discrete, the rears for 7.x.
    fn extra_channels(self) -> usize {
        match self {
            Hra96Kind::Plain | Hra96Kind::Matrix => 0,
            Hra96Kind::Discrete => 1,
            Hra96Kind::Rear => 2,
        }
    }
}

pub struct Hra96Config {
    /// Core channel layout: DTS channel arrangement code and the number of full-range channels (5 for 5.x and above).
    pub amode: u8,
    pub nprim: usize,
    pub has_lfe: bool,
    pub kind: Hra96Kind,
    pub total_kbps: u32,
    pub dialnorm: i32,
    pub pcm_bits: u32,
    pub exss: ExssParams,
    /// Flag the core as ES also for a layout that is not a 6.x one (side-rear pair carried by the surrounds).
    pub core_es: bool,
}

pub struct Hra96Encoder {
    kind: Hra96Kind,
    nprim: usize,
    core: CoreEncoder,
    /// The encoder of the rear pair (7.x only).
    rear: Option<CoreEncoder>,
    /// 64-band analysis of the five core channels, then of the extra channels that get an X96 set.
    core_ana: Vec<Analysis64>,
    extra_ana: Vec<Analysis64>,
    x96: X96Encoder,
    params: XxchParams,
    exss: ExssParams,
    taps: Vec<f64>,
    /// Source samples (96 kHz) per channel: the five core channels, the LFE, then the extra channels; not yet consumed.
    buf: Vec<Vec<f64>>,
    /// Which buffers feed the core encoder, in the order it takes them (the five, the XCH channel, the LFE).
    core_inputs: Vec<usize>,
    /// First buffer of the extra channels.
    extra_start: usize,
    dropped: usize,
    emitted: usize,
    n: usize,
    l48: usize,
    phase: usize,
    lookahead: usize,
    ana_delay: usize,
    bits_num: u64,
    bits_den: u64,
    overhead: usize,
    timing: [std::time::Duration; 5],
}

fn core_config(amode: u8, nprim: usize, es: bool, xch: bool, has_lfe: bool, dialnorm: i32, pcm_bits: u32) -> CoreConfig {
    CoreConfig { sample_rate: 48_000, amode, channels: nprim, lfe: has_lfe, bitrate: 1_509_000, dialnorm, pcm_bits, es, xch, perfect: false, blocks: 16 }
}

impl Hra96Encoder {
    pub fn new(cfg: Hra96Config) -> Result<Hra96Encoder, Error> {
        let has_lfe = cfg.has_lfe;
        let es = matches!(cfg.kind, Hra96Kind::Matrix | Hra96Kind::Discrete) || cfg.core_es;
        let mut core = CoreEncoder::new(core_config(cfg.amode, cfg.nprim, es, cfg.kind == Hra96Kind::Discrete, has_lfe, cfg.dialnorm, cfg.pcm_bits))?;
        core.keep_residual = true;
        let rear = if cfg.kind == Hra96Kind::Rear {
            let mut r = CoreEncoder::new(CoreConfig { amode: 2, channels: 2, lfe: false, bitrate: 1_536_000, es: false, xch: false, ..core_config(2, 2, false, false, false, cfg.dialnorm, cfg.pcm_bits) })?;
            r.keep_residual = true;
            Some(r)
        } else {
            None
        };
        let n = core.frame_samples() * 2;
        let l48: usize = std::env::var("SW_L48").ok().and_then(|v| v.parse().ok()).unwrap_or(480);
        let phase: f64 = std::env::var("SW_PHASE").ok().and_then(|v| v.parse().ok()).unwrap_or(0.5);
        let frac = phase - phase.floor();
        let phase = phase.floor() as usize;
        let nprim = cfg.nprim;
        let core_ana: Vec<Analysis64> = (0..nprim).map(|_| Analysis64::new()).collect();
        // the rears and the XCH channel each get an X96 channel; the LFE does not
        let extra_ana: Vec<Analysis64> = (0..cfg.kind.extra_channels()).map(|_| Analysis64::new()).collect();
        let ana_delay = core_ana[0].delay();
        let lookahead = (2 * (l48 + 1) + phase + HALF + 2).max(ana_delay + 64 * 16 + 64);
        let core_mask = 0x1 | 0x2 | 0x4 | (if has_lfe { 0x20 } else { 0 }) | (1 << 9) | (1 << 10);
        let params = XxchParams { core_mask, spk_mask: (1 << 7) | (1 << 8), mask_bits: 11, dmix: Some((55, vec![vec![(9, 119)], vec![(10, 119)]])) };
        // size of the extension substream header (what the components do not account for)
        let overhead = match cfg.kind {
            Hra96Kind::Rear => exss_frame_xxch_x96(&cfg.exss, &[0u8; 4], &[0u8; 4]).len() - 8,
            _ => exss_frame_x96(&cfg.exss, &[0u8; 4]).len() - 4,
        };
        let extra_start = nprim + usize::from(has_lfe);
        // the core takes its five channels, then the XCH channel (if any), then the LFE
        let mut core_inputs: Vec<usize> = (0..nprim).collect();
        if cfg.kind == Hra96Kind::Discrete {
            core_inputs.push(extra_start);
        }
        if has_lfe {
            core_inputs.push(nprim);
        }
        Ok(Hra96Encoder {
            kind: cfg.kind,
            nprim,
            core,
            rear,
            core_ana,
            extra_ana,
            x96: X96Encoder::new(16),
            params,
            exss: cfg.exss,
            taps: decimator_taps(frac),
            buf: vec![Vec::new(); extra_start + cfg.kind.extra_channels()],
            core_inputs,
            extra_start,
            dropped: 0,
            emitted: 0,
            n,
            l48,
            phase,
            lookahead,
            ana_delay,
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

    /// Feed source samples, one vector per buffer: the five core channels (C L R Ls Rs, already mixed as the core
    /// carries them), the LFE if there is one, then the extra channels.
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
    fn analyse(ana: &mut Analysis64, src: &[f64], f_off: usize, dropped: usize, delay: usize, deq: &[Vec<i32>]) -> Vec<Vec<f64>> {
        let blocks = 16;
        let start = f_off as isize + delay as isize - dropped as isize;
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
        let t0 = std::time::Instant::now();
        let dec: Vec<Vec<f64>> = {
            let me: &Hra96Encoder = &*self;
            use rayon::prelude::*;
            (0..me.buf.len()).into_par_iter().map(|c| me.decimated(c, f)).collect()
        };
        self.timing[0] += t0.elapsed();
        let t0 = std::time::Instant::now();
        let core_in: Vec<Vec<f64>> = self.core_inputs.iter().map(|&i| dec[i].clone()).collect();
        let mut data = self.core.encode_frame(&core_in);
        data.resize(data.len() + (4 - data.len() % 4) % 4, 0);
        // sizes: the rest of the frame after the core and the extension substream header
        let upto = |k: u64| (k * self.bits_num) / (self.bits_den * 8);
        let total = (upto(f as u64 + 1) - upto(f as u64)) as usize;
        let rest = total.saturating_sub(data.len() + self.overhead);
        let (xxch_bytes, x96_bytes) = if self.kind == Hra96Kind::Rear {
            // the rear pair gets a fixed 512 bytes; X96 takes the rest up to its limit and XXCH what is beyond it
            let x96 = ((rest.saturating_sub(XXCH_FIXED_BYTES)) & !3).clamp(64, MAX_X96_BYTES);
            ((rest.saturating_sub(x96) & !3).clamp(64, 16_000), x96)
        } else {
            (0, (rest & !3).clamp(64, MAX_X96_BYTES))
        };
        let xxch = match self.rear.as_mut() {
            Some(r) => r.encode_xxch(&dec[self.extra_start..], xxch_bytes, &self.params),
            None => Vec::new(),
        };
        self.timing[1] += t0.elapsed();
        let t0 = std::time::Instant::now();
        // X96: the five core channels, then the extra channels that have a set of their own
        let (f_off, dropped, delay) = (f * self.n, self.dropped, self.ana_delay);
        let bufs = &self.buf;
        let core_deq = &self.core.last_deq;
        let extra_start = self.extra_start;
        let rear_deq = self.rear.as_ref().map(|r| &r.last_deq);
        let kind = self.kind;
        let (r_core, r_extra): (Vec<Vec<Vec<f64>>>, Vec<Vec<Vec<f64>>>) = {
            use rayon::prelude::*;
            let a: Vec<_> = self.core_ana.par_iter_mut().enumerate().map(|(ch, ana)| Self::analyse(ana, &bufs[ch], f_off, dropped, delay, &core_deq[ch])).collect();
            let b: Vec<_> = self
                .extra_ana
                .par_iter_mut()
                .enumerate()
                .map(|(j, ana)| {
                    // the XCH channel's reconstruction is the core encoder's sixth plan; the rears' is the rear encoder's
                    let deq = if kind == Hra96Kind::Discrete { &core_deq[5] } else { &rear_deq.unwrap()[j] };
                    Self::analyse(ana, &bufs[extra_start + j], f_off, dropped, delay, deq)
                })
                .collect();
            (a, b)
        };
        self.timing[2] += t0.elapsed();
        let t0 = std::time::Instant::now();
        let x96 = if r_extra.is_empty() { self.x96.encode_exss(&[&r_core], x96_bytes) } else { self.x96.encode_exss(&[&r_core, &r_extra], x96_bytes) };
        self.timing[3] += t0.elapsed();
        if self.kind == Hra96Kind::Rear {
            data.extend_from_slice(&exss_frame_xxch_x96(&self.exss, &xxch, &x96));
        } else {
            data.extend_from_slice(&exss_frame_x96(&self.exss, &x96));
        }
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

/// How a layout's channels feed the stream.
pub struct Hra96Map {
    pub kind: Hra96Kind,
    /// Layout index of C, L, R, Ls, Rs.
    pub core: Vec<usize>,
    /// DTS channel arrangement of the core.
    pub amode: u8,
    pub lfe: Option<usize>,
    /// Layout index of the centre surround (6.x) or of the rear pair (7.x).
    pub extra: Vec<usize>,
}

/// `Some` for the layouts this stream type carries: 5.x, 6.x (matrix or discrete) and 7.x with a rear pair.
pub fn hra96_map(layout: &ChannelLayout) -> Option<Hra96Map> {
    use sw_core::layout::LayoutKind;
    if let Some(m) = crate::native_xxch::xxch_map(layout) {
        return Some(Hra96Map { kind: Hra96Kind::Rear, core: m.core.to_vec(), amode: 9, lfe: m.lfe, extra: m.rear.to_vec() });
    }
    let pos = |list: &[Channel]| layout.channels.iter().position(|c| list.contains(c));
    let lfe = pos(&[Channel::Lfe]);
    let cs = pos(&[Channel::Cs]);
    let five = (|| Some([pos(&[Channel::C])?, pos(&[Channel::L])?, pos(&[Channel::R])?, pos(&[Channel::Ls, Channel::Lss])?, pos(&[Channel::Rs, Channel::Rss])?]))();
    if let Some(core) = five {
        if layout.channels.len() != 5 + usize::from(lfe.is_some()) + usize::from(cs.is_some()) {
            return None;
        }
        let kind = match cs {
            None => Hra96Kind::Plain,
            Some(_) if layout.kind == LayoutKind::Discrete => Hra96Kind::Discrete,
            Some(_) => Hra96Kind::Matrix,
        };
        return Some(Hra96Map { kind, core: core.to_vec(), amode: 9, lfe, extra: cs.into_iter().collect() });
    }
    // fewer channels: the core carries them as they are
    let lm = crate::native::map_layout(layout).ok()?;
    Some(Hra96Map { kind: Hra96Kind::Plain, core: lm.order, amode: lm.amode, lfe: lm.lfe, extra: Vec::new() })
}

/// File-level driver: 96 kHz stems in, `.dtshd` (or raw) out.
pub fn encode_to_file(stems: &mut StemSet, req: &Request, map: Hra96Map, on_progress: &mut dyn FnMut(f32), cancel: &AtomicBool) -> Result<Stats, NativeError> {
    let rate = stems.sample_rate;
    if rate != 96_000 {
        return Err(Error::Unsupported("HRA above 5.1 channels, and HRA 6.x, are implemented at 96 kHz (and 7.x at 48 kHz)".into()).into());
    }
    if req.es.map_or(false, |e| e.phase_shift || e.rear_minus_3db) {
        return Err(Error::Unsupported("the ES phase shift and the rear -3 dB trim are not available for DTS-HD High Resolution at 96 kHz".into()).into());
    }
    let total_kbps = req.hra_kbps.unwrap_or(if map.kind == Hra96Kind::Rear { 5376 } else { 4224 });
    let has_lfe = map.lfe.is_some();
    let kind = map.kind;
    // the channels the extension header describes: the core channels, and what is coded beside them
    let coded_extra = if kind == Hra96Kind::Matrix { 0 } else { map.extra.len() };
    let nprim = map.core.len();
    let nch_total = nprim + usize::from(has_lfe) + coded_extra;
    let exss_mask: u32 = req.layout.channels.iter().enumerate().filter(|(i, _)| !(kind == Hra96Kind::Matrix && map.extra.contains(i))).map(|(_, c)| crate::ma::exss_bit(*c)).fold(0, |m, b| m | b);
    let make_exss = || ExssParams { sample_rate: rate, channels: nch_total, bit_res: req.pcm_bits, spk_mask: exss_mask, with_core: true, dialnorm: 0, frame_duration_code: 0, x96_in_core: false, xch_in_core: kind == Hra96Kind::Discrete, secondary: false, extras: crate::exss::ExssExtras { min_header: 28, repr_type: (req.layout.kind == sw_core::layout::LayoutKind::LtRt).then_some(2), ..Default::default() } };
    let make_encoder = || Hra96Encoder::new(Hra96Config { amode: map.amode, nprim, has_lfe, kind, total_kbps, dialnorm: req.dialnorm, pcm_bits: req.pcm_bits, exss: make_exss(), core_es: req.layout.id == "7.1-ls-rs-lsr-rsr" });
    let mut enc = make_encoder()?;
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
    // what the core carries: the 5.1 downmix of a 7.1 programme, or the core channels alone
    let core_mask: u32 = if kind == Hra96Kind::Rear { map.core.iter().map(|&i| crate::ma::exss_bit(req.layout.channels[i])).fold(0, |m, b| m | b) | if has_lfe { 0x8 } else { 0 } } else { exss_mask };
    let info = container.then(|| {
        let mut i = crate::dtshd::DtsHdInfo::new(rate, n as u16, want, exss_mask as u16, crate::dtshd::Kind::CoreAndExtension);
        crate::mixmeta::apply_container_options(&mut i, &req.bitstream, want as f64 / rate as f64);
        i.core_kbps = 1509;
        i.core_payload = core_payload;
        i.codec_delay = codec_delay as u16;
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
    let write = |out: &mut crate::dtshd::StreamOut, fr: Vec<Vec<u8>>, frames: &mut u64, bytes: &mut u64| -> Result<(), NativeError> {
        for f in fr {
            out.write_frame(&f, f.len().saturating_sub(core_payload as usize))?;
            *frames += 1;
            *bytes += f.len() as u64;
        }
        Ok(())
    };
    let extra_channels = kind.extra_channels();
    let buffers = nprim + usize::from(has_lfe) + extra_channels;
    if lead > 0 {
        let silence = vec![vec![0.0f64; lead]; buffers];
        let fr = enc.push(&silence);
        write(&mut out, fr, &mut frames, &mut bytes)?;
    }
    let lfe_slot = nprim;
    let extra_start = nprim + usize::from(has_lfe);
    let chunk = n * 4;
    while remaining > 0 {
        if cancel.load(Ordering::Relaxed) {
            return Err(NativeError::Canceled);
        }
        let got = (remaining as usize).min(chunk);
        stems.read_planar(&mut planar, got)?;
        let mut block: Vec<Vec<f64>> = vec![vec![0.0; got]; buffers];
        for i in 0..got {
            for (slot, &src) in map.core.iter().enumerate() {
                let value = match kind {
                    // 7.x: scaled, with the rear on the same side added to the sides
                    Hra96Kind::Rear => {
                        let mut v = to24(planar[src][i]) * SCALE;
                        if slot >= 3 {
                            v += to24(planar[map.extra[slot - 3]][i]) * REAR_COEFF;
                        }
                        v
                    }
                    // 6.x: the centre surround at -3 dB on both surrounds
                    Hra96Kind::Matrix | Hra96Kind::Discrete if slot >= 3 => to24(planar[src][i]) + to24(planar[map.extra[0]][i]) * CS_MIX,
                    _ => to24(planar[src][i]),
                };
                block[slot][i] = clamp(value);
            }
            if let Some(l) = map.lfe {
                let v = to24(planar[l][i]);
                block[lfe_slot][i] = clamp(if kind == Hra96Kind::Rear { v * SCALE } else { v });
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
        remaining -= got as u64;
        let fr = enc.push(&block);
        write(&mut out, fr, &mut frames, &mut bytes)?;
        on_progress(1.0 - remaining as f32 / want as f32);
    }
    let fr = enc.finish();
    write(&mut out, fr, &mut frames, &mut bytes)?;
    // the silent frame StreamTools' Add silence inserts: a fresh encoder fed zeros until its filters have settled
    let blackout = if container {
        make_encoder().ok().and_then(|mut e| {
            let zeros = vec![vec![0.0f64; 6 * n]; buffers];
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
