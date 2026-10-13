//! DTS-HD High Resolution Audio for 7.1 / 7.0: a 5.1 (5.0) core plus an XXCH extension that carries the rear
//! pair, in the extension substream. The core's surround channels hold the sides with the rears folded in
//! (embedded downmix, signalled in the XXCH channel set header), so a decoder without XXCH plays a proper
//! 5.1 mix and an XXCH decoder subtracts the rears again.

use crate::container::exss::{exss_frame_xbr_xxch, exss_frame_xxch, ExssExtras, ExssParams};
use crate::dca::encoder::XxchParams;
use crate::dca::native::{NativeError, Request, Stats};
use crate::hra::xbr::XbrEncoder;
use crate::{Config, Encoder};
use std::sync::atomic::{AtomicBool, Ordering};
use sw_core::{Channel, Downmix};
use sw_pcm::StemSet;

/// Where the channels of a 7.1 / 7.0 layout go.
pub struct XxchMap {
    /// Layout index of C, L, R, Ls (side), Rs (side) in core order.
    pub core: [usize; 5],
    pub lfe: Option<usize>,
    /// Layout index of the extra pair, in XXCH coding order.
    pub rear: [usize; 2],
    /// XXCH channel-mask bits of the extra pair (ascending), of the core's side surrounds, and whether there is an LFE.
    pub rear_bits: [u32; 2],
    pub side_bits: [u32; 2],
    pub has_lfe: bool,
}

/// Gain of a 6-bit level code (the grid of the Downmix tab: half-dB steps to 15 dB, one dB to 30, two dB to 60).
/// The default 3 dB is the exact square root of one half, as the original's table has it.
pub fn code_gain(code: u32) -> f64 {
    if code == 55 {
        return 23170.0 / 32768.0;
    }
    let db = match code {
        31..=61 => (61 - code) as f64 * 0.5,
        16..=30 => (46 - code) as f64,
        _ => (62 - 2 * code.min(31)) as f64,
    };
    10f64.powf(-db / 20.0)
}

/// How the core is made from a 7.x programme: every core channel scaled, and the extra pair added to the core
/// channels named in the Downmix tab (`extra[e][slot]`, slots in core order C L R Ls Rs LFE).
pub struct XxchMix {
    pub scale: f64,
    pub extra: [[f64; 6]; 2],
}

impl XxchMap {
    /// The channel mask bit of each core slot.
    fn slot_bit(&self, slot: usize) -> u32 {
        match slot {
            0 => 0,
            1 => 1,
            2 => 2,
            3 => self.side_bits[0],
            4 => self.side_bits[1],
            _ => 5,
        }
    }

    /// Levels of the downmix as level codes: the scale and, per extra channel, the core slots it joins.
    fn codes(&self, dm: &Downmix) -> (u32, [Vec<(usize, u32)>; 2]) {
        let code = |db: f64| crate::container::mixmeta::level_code(db) as u32;
        let scale = dm.primary[0].map_or(61, code);
        let rows = [&dm.extra_a, &dm.extra_b].map(|table| {
            (0..6)
                .filter(|&s| s < 5 || self.has_lfe)
                .filter_map(|s| table[s].map(|db| (s, code(db))))
                .collect::<Vec<_>>()
        });
        (scale, rows)
    }

    /// The XXCH header: the core's speakers, the speakers coded here, and the embedded downmix (each extra channel
    /// was added to the core channels the Downmix tab names, by default the surround on its side at -3 dB, and the
    /// whole core pre-scaled).
    pub fn params(&self, dm: &Downmix) -> XxchParams {
        let core_mask: u32 = 0x1
            | 0x2
            | 0x4
            | (if self.has_lfe { 0x20 } else { 0 })
            | (1 << self.side_bits[0])
            | (1 << self.side_bits[1]);
        let spk_mask: u32 = (1 << self.rear_bits[0]) | (1 << self.rear_bits[1]);
        let top = 32 - (core_mask | spk_mask).leading_zeros();
        let (scale, rows) = self.codes(dm);
        let rows: Vec<Vec<(u32, u32)>> = rows
            .iter()
            .map(|r| {
                let mut v: Vec<(u32, u32)> = r.iter().map(|&(slot, code)| (self.slot_bit(slot), 64 + code)).collect();
                v.sort();
                v
            })
            .collect();
        XxchParams {
            core_mask,
            spk_mask,
            mask_bits: top.max(9),
            dmix: Some((scale, rows)),
        }
    }

    pub fn mix(&self, dm: &Downmix) -> XxchMix {
        let (scale, rows) = self.codes(dm);
        let mut extra = [[0.0; 6]; 2];
        for (e, row) in rows.iter().enumerate() {
            for &(slot, code) in row {
                extra[e][slot] = code_gain(code);
            }
        }
        // the decoder multiplies by the table's inverse of the scale, which for 3 dB is 92682 / 65536
        XxchMix {
            scale: if scale == 55 {
                65536.0 / 92682.0
            } else {
                code_gain(scale)
            },
            extra,
        }
    }
}

/// `Some` for the 7.1 / 7.0 layouts: a 5.x base and a pair of extra channels (rears, wides, heights, height sides,
/// or the centre surround with a centre height or overhead).
pub fn xxch_map(layout: &sw_core::ChannelLayout) -> Option<XxchMap> {
    use Channel::*;
    let pos = |list: &[Channel]| layout.channels.iter().position(|c| list.contains(c));
    // (channels, their bits in the XXCH channel mask) in the order the original encoder codes them
    let pairs: [([Channel; 2], [u32; 2]); 6] = [
        ([Lsr, Rsr], [7, 8]),
        ([Lw, Rw], [17, 18]),
        ([Lh, Rh], [13, 15]),
        ([Lhs, Rhs], [20, 21]),
        ([Cs, Ch], [6, 14]),
        ([Cs, Oh], [6, 19]),
    ];
    let (pair, rear_bits) = pairs
        .iter()
        .find(|(p, _)| p.iter().all(|c| layout.channels.contains(c)))?;
    let map = XxchMap {
        core: [pos(&[C])?, pos(&[L])?, pos(&[R])?, pos(&[Ls, Lss])?, pos(&[Rs, Rss])?],
        lfe: pos(&[Lfe]),
        rear: [pos(&[pair[0]])?, pos(&[pair[1]])?],
        rear_bits: *rear_bits,
        side_bits: if layout.channels.contains(&Lss) {
            [9, 10]
        } else {
            [3, 4]
        },
        has_lfe: layout.channels.contains(&Lfe),
    };
    (layout.channel_count() == 7 + usize::from(map.lfe.is_some())).then_some(map)
}

/// The three encoders and the tables that make one 7.1 stream: the 5.1 core, the extra pair coded as XXCH, and the XBR
/// refinement of the core channels.
struct Xxch {
    core: Encoder,
    ext: Encoder,
    xbr: XbrEncoder,
    exss: ExssParams,
    params: XxchParams,
    /// Bytes of the extension substream's headers with the XXCH set alone, and with XBR as well.
    overhead: usize,
    overhead_both: usize,
    total_kbps: u32,
    /// Bytes of the whole stream up to frame `k` is `k * bits_num / (bits_den * 8)`.
    bits_num: u64,
    bits_den: u64,
}

impl Xxch {
    fn new(req: &Request, map: &XxchMap, rate: u32) -> Result<Xxch, NativeError> {
        let total_kbps = req.hra_kbps.unwrap_or(2046);
        let has_lfe = map.lfe.is_some();
        // the core keeps at most 1509 kbps; the XXCH extension gets the rest
        let core_kbps = (total_kbps * 3 / 4).min(1509);
        let blocks = 16;
        let config = |amode: u8, channels: usize, lfe: bool, bitrate: u32| Config {
            sample_rate: rate,
            amode,
            channels,
            lfe,
            bitrate,
            dialnorm: req.dialnorm,
            pcm_bits: req.pcm_bits,
            es: req.layout.id == "7.1-ls-rs-lsr-rsr",
            xch: false,
            perfect: false,
            blocks,
        };
        let mut core = Encoder::new(config(9, 5, has_lfe, core_kbps * 1000))?;
        core.set_aux_downmix(crate::lossless::ma::core_aux(
            &req.bitstream.downmix,
            req.layout,
            9,
            has_lfe,
            false,
        ));
        core.keep_residual = true;
        let ext = Encoder::new(config(2, 2, false, 1_536_000))?;
        let n = core.frame_samples();
        let exss = ExssParams {
            sample_rate: rate,
            channels: 7 + usize::from(has_lfe),
            bit_res: req.pcm_bits,
            spk_mask: crate::lossless::ma::map_layout(req.layout).exss_mask,
            with_core: true,
            dialnorm: (-req.dialnorm) as u32,
            frame_duration_code: 0,
            x96_in_core: false,
            xch_in_core: false,
            secondary: false,
            extras: ExssExtras {
                min_header: 28,
                ..Default::default()
            },
        };
        Ok(Xxch {
            overhead: exss_frame_xxch(&exss, &[0u8; 4]).len() - 4,
            overhead_both: exss_frame_xbr_xxch(&exss, &[0u8; 4], &[0u8; 4]).len() - 8,
            params: map.params(&req.bitstream.downmix),
            xbr: XbrEncoder::new(blocks),
            core,
            ext,
            exss,
            total_kbps,
            bits_num: total_kbps as u64 * 1000 * n as u64,
            bits_den: rate as u64,
        })
    }

    /// Bytes of the stream up to frame `k`.
    fn bytes_upto(&self, k: u64) -> u64 {
        (k * self.bits_num) / (self.bits_den * 8)
    }

    /// Frame `f`: the core, then XBR for the core channels and XXCH for the extra pair in the extension substream.
    fn frame(&mut self, core_pcm: &[Vec<f64>], ext_pcm: &[Vec<f64>], f: u64) -> Vec<u8> {
        let mut data = self.core.encode_frame(core_pcm);
        let pad = (4 - data.len() % 4) % 4;
        data.resize(data.len() + pad, 0);
        let total = (self.bytes_upto(f + 1) - self.bytes_upto(f)) as usize;
        // the rate beyond what the extra pair needs at the core's per-channel rate goes to XBR for the core
        // channels and XXCH in the ratio of their channel counts (a commercial 5376 kbps stream splits it that way)
        let rest = total.saturating_sub(data.len());
        // the original encoder's split, measured at every legal rate with real rear content: the pair's mean size
        // grows linearly with the stream's rate (to the multiple of four); below 2304 kbps nothing is left for XBR
        // and the pair takes it all
        let want_xxch = ((0.2702 * self.total_kbps as f64 + 58.5) / 4.0).round() as usize * 4;
        let (xbr_bytes, xxch_bytes) = if rest > self.overhead_both + want_xxch + 64 {
            (rest - self.overhead_both - want_xxch, want_xxch)
        } else {
            (0, rest.saturating_sub(self.overhead).clamp(64, 16_000))
        };
        let x = self.ext.encode_xxch(ext_pcm, xxch_bytes.min(16_000), &self.params);
        if xbr_bytes > 0 {
            let mut b = self.xbr.encode(&self.core.last_residual, xbr_bytes);
            b.resize(xbr_bytes, 0);
            data.extend_from_slice(&exss_frame_xbr_xxch(&self.exss, &b, &x));
        } else {
            data.extend_from_slice(&exss_frame_xxch(&self.exss, &x));
        }
        data
    }

    /// The silent frame StreamTools' Add silence inserts: fresh encoders fed zeros until the filter bank has settled.
    fn silent_frame(req: &Request, map: &XxchMap, rate: u32) -> Result<Vec<u8>, NativeError> {
        let mut fresh = Xxch::new(req, map, rate)?;
        let n = fresh.core.frame_samples();
        let core_zeros = vec![vec![0.0; n]; 5 + usize::from(map.lfe.is_some())];
        let ext_zeros = vec![vec![0.0; n]; 2];
        let mut last = Vec::new();
        for f in 0..4 {
            last = fresh.frame(&core_zeros, &ext_zeros, f);
        }
        Ok(last)
    }
}

/// Fills the encoders' input for one frame: the five (six with LFE) core channels, each scaled with the extra pair
/// added where the Downmix tab says, and the extra pair itself.
fn shape_frame(
    map: &XxchMap,
    mix: &XxchMix,
    planar: &[Vec<f32>],
    got: usize,
    zeros: usize,
    core_pcm: &mut [Vec<f64>],
    ext_pcm: &mut [Vec<f64>],
) {
    let to24 = |v: f32| v as f64 * 8_388_608.0;
    for v in core_pcm.iter_mut().chain(ext_pcm.iter_mut()) {
        v.iter_mut().for_each(|x| *x = 0.0);
    }
    for i in 0..got {
        let fold = |slot: usize, src: usize| {
            let mut v = to24(planar[src][i]) * mix.scale;
            for e in 0..2 {
                if mix.extra[e][slot] != 0.0 {
                    v += to24(planar[map.rear[e]][i]) * mix.extra[e][slot];
                }
            }
            v.clamp(-8_388_608.0, 8_388_607.0)
        };
        for (slot, &src) in map.core.iter().enumerate() {
            core_pcm[slot][zeros + i] = fold(slot, src);
        }
        if let Some(l) = map.lfe {
            core_pcm[5][zeros + i] = fold(5, l);
        }
        for (slot, &src) in map.rear.iter().enumerate() {
            ext_pcm[slot][zeros + i] = to24(planar[src][i]).clamp(-8_388_608.0, 8_388_607.0);
        }
    }
}

pub fn encode_to_file(
    stems: &mut StemSet,
    req: &Request,
    map: XxchMap,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Result<Stats, NativeError> {
    let rate = stems.sample_rate;
    if rate != 48_000 {
        return Err(crate::Error::Unsupported("HRA 7.1 is implemented at 48 kHz".into()).into());
    }
    let mut xxch = Xxch::new(req, &map, rate)?;
    let want = stems.select_span(req.start_secs, req.duration_secs)?;
    if want == 0 {
        return Err(NativeError::Empty);
    }
    let n = xxch.core.frame_samples();
    let mix = map.mix(&req.bitstream.downmix);
    let container = req.output.extension().is_some_and(|e| e.eq_ignore_ascii_case("dtshd"));
    let core_payload = (1509u32 * 1000 * 512 / 48_000 / 8) & !3;
    let info = container.then(|| {
        let mut i = crate::container::dtshd::DtsHdInfo::new(
            rate,
            n as u16,
            want,
            xxch.exss.spk_mask as u16,
            crate::container::dtshd::Kind::CoreAndExtension,
        );
        crate::container::mixmeta::apply_container_options(&mut i, &req.bitstream, want as f64 / rate as f64);
        i.core_kbps = 1509;
        i.core_payload = core_payload;
        i.codec_delay = 1024;
        i
    });
    let mut out = crate::container::dtshd::StreamOut::create(req.output, info)?;

    // filter bank delay: in a .dtshd the stream starts with silence so the delay is 1024 samples, which the
    // file states; a raw stream starts that many samples into the source instead (see `native.rs`)
    let delay = xxch.core.delay();
    let (skip, lead) = if container {
        (0usize, 1024usize.saturating_sub(delay))
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
    let mut lead_left = lead;
    let (mut frames, mut bytes) = (0u64, 0u64);
    let total_frames = (remaining + lead as u64).div_ceil(n as u64) + 1;
    let mut core_pcm: Vec<Vec<f64>> = vec![vec![0.0; n]; 5 + usize::from(map.lfe.is_some())];
    let mut ext_pcm: Vec<Vec<f64>> = vec![vec![0.0; n]; 2];
    for f in 0..total_frames {
        if cancel.load(Ordering::Relaxed) {
            return Err(NativeError::Canceled);
        }
        let zeros = lead_left.min(n);
        lead_left -= zeros;
        let got = (remaining as usize).min(n - zeros);
        if got > 0 {
            stems.read_planar(&mut planar, got)?;
        }
        shape_frame(&map, &mix, &planar, got, zeros, &mut core_pcm, &mut ext_pcm);
        remaining -= got as u64;
        let data = xxch.frame(&core_pcm, &ext_pcm, f);
        out.write_frame(&data, data.len().saturating_sub(core_payload as usize))?;
        frames += 1;
        bytes += data.len() as u64;
        on_progress(1.0 - remaining as f32 / want as f32);
    }
    let blackout = if container {
        Some(Xxch::silent_frame(req, &map, rate)?)
    } else {
        None
    };
    out.finish(blackout.as_deref())?;
    on_progress(1.0);
    Ok(Stats { frames, bytes })
}
