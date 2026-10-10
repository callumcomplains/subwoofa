//! DTS-HD Master Audio streams: lossless extension (XLL) in extension substream frames, optionally on
//! top of a core.

use crate::exss::{exss_frame, ExssParams};
use crate::dmix_table::{DMIX, DMIX_INV};
use crate::xll::{DmixHeader, DmixRow, SetCfg, XllConfig, XllEncoder};
use sw_core::{Channel, ChannelLayout};

/// Speaker position bit of the lossless extension's channel mask.
fn xll_bit(c: Channel) -> u32 {
    use Channel::*;
    match c {
        C => 0,
        L | Lt => 1,
        R | Rt => 2,
        Ls => 3,
        Rs => 4,
        Lfe => 5,
        Cs => 6,
        Lsr => 7,
        Rsr => 8,
        Lss => 9,
        Rss => 10,
        Lh => 13,
        Ch => 14,
        Rh => 15,
        Lhs => 20,
        Rhs => 21,
        Lw => 17,
        Rw => 18,
        Oh | Tc => 19,
        Lfe2 => 16,
        Tfl => 13,
        Tfc => 14,
        Tfr => 15,
        Tsl => 20,
        Tsr => 21,
        Tbc => 22,
        Tbl => 23,
        Tbr => 24,
        Bfc => 25,
        Bfl => 26,
        Bfr => 27,
        _ => 31,
    }
}

/// Loudspeaker group bit of the extension substream's activity mask (Table 7-10).
pub fn exss_bit(c: Channel) -> u32 {
    use Channel::*;
    match c {
        C => 0x1,
        L | R | Lt | Rt => 0x2,
        Ls | Rs => 0x4,
        Lfe => 0x8,
        Cs => 0x10,
        Lh | Rh => 0x20,
        Lsr | Rsr => 0x40,
        Ch => 0x80,
        Oh => 0x100,
        Lw | Rw => 0x400,
        Lss | Rss => 0x800,
        Lfe2 => 0x1000,
        Lhs | Rhs => 0x2000,
        Tbc => 0x4000,
        Tbl | Tbr => 0x8000,
        Tfl | Tfr => 0x20,
        Tfc => 0x80,
        Tc => 0x100,
        Tsl | Tsr => 0x2000,
        _ => 0, // the bottom layer has no group in the 16-bit activity mask
    }
}

/// Embedded downmix of a two-set hierarchy: the extra channels (set 1) are folded into the channels of
/// set 0, so a decoder that reads only set 0 (or the core) still gets a complete programme.
#[derive(Debug, Clone)]
pub struct Hier {
    /// Channels in set 0.
    pub base: usize,
    /// Scale applied to each channel of set 0 when the set alone is decoded (15-bit fixed point), and the
    /// 9-bit code that carries it.
    pub scale: Vec<i32>,
    pub scale_code: Vec<u16>,
    /// 16-bit inverse of `scale`.
    pub inv: Vec<i32>,
    /// Effective coefficient of every extra channel in every channel of set 0 (15-bit fixed point).
    pub coeff: Vec<Vec<i32>>,
    pub coeff_codes: Vec<Vec<u16>>,
    /// Extra sets as column ranges of `coeff` (set 1 first). Set 1 carries the row scales and the coefficients
    /// relative to them; later sets use unit scales and carry the effective coefficient directly.
    pub sets: Vec<std::ops::Range<usize>>,
    /// Code of the unit scale.
    pub unity_code: u16,
}

#[inline]
fn mul15(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64 + (1 << 14)) >> 15) as i32
}

impl Hier {
    /// Turn the original channels (set 0 then set 1) into the transmitted ones: set 0 gets the weighted
    /// extra channels added, exactly inverted by the decoder.
    pub fn apply(&self, pcm: &mut [Vec<i32>]) {
        let (head, tail) = pcm.split_at_mut(self.base);
        for (j, row) in head.iter_mut().enumerate() {
            for (k, extra) in tail.iter().enumerate() {
                let c = self.coeff[j][k];
                if c != 0 {
                    for (d, s) in row.iter_mut().zip(extra) {
                        *d += mul15(*s, c);
                    }
                }
            }
        }
    }

    /// Gain the core sees relative to the transmitted samples of channel `j` of set 0.
    pub fn core_gain(&self, j: usize) -> f64 {
        self.scale[j] as f64 / 32768.0
    }
}

#[derive(Debug, Clone)]
pub struct MaMap {
    /// Layout channel index for each coded channel (set 0 first, ascending speaker bit within a set).
    pub order: Vec<usize>,
    /// Channels per set.
    pub set_sizes: Vec<usize>,
    /// Speaker mask of each set and of the whole stream.
    pub set_masks: Vec<u32>,
    pub xll_mask: u32,
    pub exss_mask: u32,
    pub hier: Option<Hier>,
}

impl MaMap {
    pub fn xll_sets(&self, residual_of_core: bool) -> Vec<SetCfg> {
        (0..self.set_sizes.len())
            .map(|i| SetCfg {
                channels: self.set_sizes[i],
                ch_mask: self.set_masks[i],
                residual_of_core: residual_of_core && i == 0,
                dmix: if i == 0 {
                    None
                } else {
                    let h = self.hier.as_ref().expect("hierarchy");
                    let cols = h.sets[i - 1].clone();
                    // one row for every channel of the sets below: the base channels carry the downmix, the
                    // channels of earlier extra sets are not folded into
                    let lower: usize = h.base + h.sets[..i - 1].iter().map(|r| r.len()).sum::<usize>();
                    Some(DmixHeader {
                        rows: (0..lower)
                            .map(|j| {
                                if j < h.base {
                                    DmixRow { scale_code: if i == 1 { h.scale_code[j] } else { h.unity_code }, coeff_codes: h.coeff_codes[j][cols.clone()].to_vec() }
                                } else {
                                    DmixRow { scale_code: h.unity_code, coeff_codes: vec![0x100; cols.len()] }
                                }
                            })
                            .collect(),
                    })
                },
            })
            .collect()
    }
}

fn dmix_index(v: f64) -> usize {
    let target = v * 32768.0;
    (0..DMIX.len()).min_by(|&a, &b| (DMIX[a] as f64 - target).abs().partial_cmp(&(DMIX[b] as f64 - target).abs()).unwrap()).unwrap()
}

/// How a channel outside the 5.1 base folds into it: `(base channel, row scale, coefficient)` per target,
/// scales and coefficients as fractions (the row scale gives headroom for what is added to the row).
fn fold_targets(c: Channel, side: (Channel, Channel), rear: Option<(Channel, Channel)>) -> Option<Vec<(Channel, f64, f64)>> {
    use Channel::*;
    let m3 = std::f64::consts::FRAC_1_SQRT_2;
    Some(match c {
        Lsr | Lss => vec![(side.0, m3, m3)],
        Rsr | Rss => vec![(side.1, m3, m3)],
        Lh | Lhs | Lw => vec![(L, m3, m3)],
        Rh | Rhs | Rw => vec![(R, m3, m3)],
        // the centre surround joins both surrounds at -3 dB, as in DTS-ES
        Cs => vec![(rear.unwrap_or(side).0, 1.0, m3), (rear.unwrap_or(side).1, 1.0, m3)],
        Ch | Oh => vec![(C, 1.0, m3)],
        // NHK 22.2 top and bottom layers
        Lfe2 => vec![(Lfe, 1.0, 1.0)],
        Tfc | Tc | Bfc => vec![(C, 1.0, m3)],
        Tfl | Bfl => vec![(L, m3, m3)],
        Tfr | Bfr => vec![(R, m3, m3)],
        Tsl => vec![(side.0, m3, m3)],
        Tsr => vec![(side.1, m3, m3)],
        Tbl => vec![(rear.unwrap_or(side).0, m3, m3)],
        Tbr => vec![(rear.unwrap_or(side).1, m3, m3)],
        Tbc => vec![(rear.unwrap_or(side).0, 1.0, m3), (rear.unwrap_or(side).1, 1.0, m3)],
        _ => return None,
    })
}

/// Split a 6.0 to 7.1 layout into a 5.x base (with the surround pair) and the channels folded into it.
fn hierarchy(layout: &ChannelLayout) -> Option<(Vec<usize>, Vec<usize>, Hier)> {
    use Channel::*;
    let n = layout.channels.len();
    if n < 6 || n > 24 {
        return None;
    }
    let pos = |c: Channel| layout.channels.iter().position(|x| *x == c);
    let side = if pos(Ls).is_some() && pos(Rs).is_some() {
        (Ls, Rs)
    } else if pos(Lss).is_some() && pos(Rss).is_some() {
        (Lss, Rss)
    } else {
        return None;
    };
    let mut base_ch = vec![C, L, R, side.0, side.1];
    if pos(Lfe).is_some() {
        base_ch.push(Lfe);
    }
    // More than eight channels (immersive layouts, NHK 22.2): the base set is a full 7.1 (the decoder this was
    // checked with reads three sets of at most eight channels each)
    let rear = if n > 8 && pos(Lsr).is_some() && pos(Rsr).is_some() { Some((Lsr, Rsr)) } else { None };
    if let Some((a, b)) = rear {
        base_ch.push(a);
        base_ch.push(b);
    }
    let mut base: Vec<usize> = base_ch.iter().map(|c| pos(*c)).collect::<Option<Vec<_>>>()?;
    base.sort_by_key(|&i| xll_bit(layout.channels[i]));
    let mut extra: Vec<usize> = (0..n).filter(|i| !base.contains(i)).collect();
    if extra.is_empty() {
        return None;
    }
    extra.sort_by_key(|&i| xll_bit(layout.channels[i]));
    let rows = base.len();
    let unity = DMIX.len() - 1;
    let mut scale_val = vec![1.0f64; rows];
    let mut coeff_val = vec![vec![0.0f64; extra.len()]; rows];
    for (k, &e) in extra.iter().enumerate() {
        for (target, scale, coef) in fold_targets(layout.channels[e], side, rear)? {
            let row = base.iter().position(|&b| layout.channels[b] == target)?;
            scale_val[row] = scale_val[row].min(scale);
            coeff_val[row][k] = coef;
        }
    }
    let scale_idx: Vec<usize> = scale_val.iter().map(|&v| if v >= 1.0 { unity } else { dmix_index(v) }).collect();
    // columns of the first extra set (up to 8 channels) carry the scaled coefficients; later sets state the
    // effective coefficient (coefficient over row scale) with unit scales
    let first_cols = extra.len().min(8);
    let coeff_idx: Vec<Vec<usize>> = (0..rows)
        .map(|j| {
            (0..extra.len())
                .map(|k| {
                    let v = coeff_val[j][k];
                    if v == 0.0 {
                        0
                    } else if k < first_cols {
                        dmix_index(v)
                    } else {
                        dmix_index(v / scale_val[j])
                    }
                })
                .collect()
        })
        .collect();
    let scale: Vec<i32> = scale_idx.iter().map(|&i| DMIX[i] as i32).collect();
    let inv: Vec<i32> = scale_idx.iter().map(|&i| DMIX_INV[i - 41] as i32).collect();
    let coeff: Vec<Vec<i32>> = (0..rows)
        .map(|j| {
            (0..extra.len())
                .map(|k| if k < first_cols { ((inv[j] as i64 * DMIX[coeff_idx[j][k]] as i64 + (1 << 15)) >> 16) as i32 } else { DMIX[coeff_idx[j][k]] as i32 })
                .collect()
        })
        .collect();
    let sets: Vec<std::ops::Range<usize>> = (0..extra.len()).step_by(8).map(|a| a..(a + 8).min(extra.len())).collect();
    let hier = Hier {
        base: rows,
        scale,
        scale_code: scale_idx.iter().map(|&i| 0x100 | i as u16).collect(),
        inv,
        coeff,
        coeff_codes: coeff_idx.iter().map(|r| r.iter().map(|&i| 0x100 | i as u16).collect()).collect(),
        sets,
        unity_code: 0x100 | unity as u16,
    };
    Some((base, extra, hier))
}

pub fn map_layout(layout: &ChannelLayout) -> MaMap {
    let mask_of = |ids: &[usize]| ids.iter().fold(0u32, |m, &i| m | (1 << xll_bit(layout.channels[i])));
    let exss_mask = layout.channels.iter().fold(0u32, |m, c| m | exss_bit(*c));
    if let Some((base, extra, hier)) = hierarchy(layout) {
        let order: Vec<usize> = base.iter().chain(&extra).copied().collect();
        let mut set_masks = vec![mask_of(&base)];
        let mut set_sizes = vec![base.len()];
        for r in &hier.sets {
            set_masks.push(mask_of(&extra[r.clone()]));
            set_sizes.push(r.len());
        }
        let xll_mask = set_masks.iter().fold(0, |m, x| m | x);
        return MaMap { order, set_sizes, set_masks, xll_mask, exss_mask, hier: Some(hier) };
    }
    let mut idx: Vec<usize> = (0..layout.channels.len()).collect();
    idx.sort_by_key(|&i| xll_bit(layout.channels[i]));
    let xll_mask = mask_of(&idx);
    MaMap { set_sizes: vec![idx.len()], order: idx, set_masks: vec![xll_mask], xll_mask, exss_mask, hier: None }
}

pub struct MaConfig {
    pub sample_rate: u32,
    pub map: MaMap,
    pub bit_res: u32,
    pub dialnorm: u32,
}

pub struct MaEncoder {
    cfg: MaConfig,
    xll: XllEncoder,
    frame_samples: usize,
}

impl MaEncoder {
    pub fn new(cfg: MaConfig) -> MaEncoder {
        let frame_samples = match cfg.sample_rate {
            44_100 | 48_000 => 512,
            88_200 | 96_000 => 1024,
            176_400 | 192_000 => 2048,
            _ => 512,
        };
        let xll = XllEncoder::new(XllConfig {
            sample_rate: cfg.sample_rate,
            bit_res: cfg.bit_res,
            frame_samples,
            sets: cfg.map.xll_sets(false),
        });
        MaEncoder { cfg, xll, frame_samples }
    }

    pub fn frame_samples(&self) -> usize {
        self.frame_samples
    }

    /// `pcm[i]` is coded channel `i` (see `MaMap::order`), `frame_samples()` signed samples of `bit_res` bits.
    pub fn encode_frame(&mut self, pcm: &[Vec<i32>]) -> Vec<u8> {
        let data = match &self.cfg.map.hier {
            Some(h) => {
                let mut sent = pcm.to_vec();
                h.apply(&mut sent);
                self.xll.encode_frame(&sent)
            }
            None => self.xll.encode_frame(pcm),
        };
        exss_frame(
            &ExssParams {
                sample_rate: self.cfg.sample_rate,
                channels: self.cfg.map.order.len(),
                bit_res: self.cfg.bit_res,
                spk_mask: self.cfg.map.exss_mask,
                with_core: false,
                dialnorm: self.cfg.dialnorm,
                frame_duration_code: 0,
                x96_in_core: false, secondary: false,
            },
            &data,
        )
    }
}
