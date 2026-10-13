//! Splitting a layout into channel sets: a 5.x base, the channels folded into it and an optional stereo set.

use crate::dca::dmix_table::{DMIX, DMIX_INV};
use crate::lossless::ma::fold::{Fold, Hier};
use crate::lossless::ma::masks::xll_bit;
use crate::lossless::xll::DmixRow;
use sw_core::{Channel, ChannelLayout, Downmix};

pub(crate) fn dmix_index(v: f64) -> usize {
    let target = v * 32768.0;
    (0..DMIX.len())
        .min_by(|&a, &b| {
            (DMIX[a] as f64 - target)
                .abs()
                .partial_cmp(&(DMIX[b] as f64 - target).abs())
                .unwrap()
        })
        .unwrap()
}

/// Index of a level in dB (`None` is off) in the downmix table.
pub(crate) fn level_index(level: sw_core::bitstream::Level) -> usize {
    level.map_or(0, |db| dmix_index(10f64.powf(-db / 20.0)))
}

/// Table index of a row scale: a channel cannot be switched off, so "off" means no scaling.
pub(crate) fn scale_index(level: sw_core::bitstream::Level) -> usize {
    level.map_or(UNITY, |db| level_index(Some(db)).max(41))
}

/// 9-bit code of a downmix coefficient: bit 8 marks a positive value, the low bits index the table.
pub(crate) fn level_code(level: sw_core::bitstream::Level) -> u16 {
    0x100 | level_index(level) as u16
}

/// How a channel outside the 5.1 base folds into it: `(base channel, row scale, coefficient)` per target,
/// scales and coefficients as fractions (the row scale gives headroom for what is added to the row).
pub(crate) fn fold_targets(
    c: Channel,
    side: (Channel, Channel),
    rear: Option<(Channel, Channel)>,
) -> Option<Vec<(Channel, f64, f64)>> {
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

/// Everything `hierarchy` works out.
pub(crate) struct Plan {
    /// Layout channel index of every coded channel, and the channels per set.
    pub(crate) order: Vec<usize>,
    pub(crate) set_sizes: Vec<usize>,
    pub(crate) hier: Hier,
    pub(crate) core_sets: usize,
    pub(crate) resid_set: usize,
}

/// Unity scale: index of 0 dB in the downmix table.
pub(crate) const UNITY: usize = 241;

/// Row scale of a table index: the scale and its inverse (the inverse is defined from index 41, about -40 dB).
pub(crate) fn scale_of(idx: usize) -> (i32, i32) {
    let idx = idx.max(41);
    (DMIX[idx] as i32, DMIX_INV[idx - 41] as i32)
}

/// Effective coefficient of a column for a row with the given scale inverse.
pub(crate) fn effective(inv: i32, code_index: usize) -> i32 {
    if code_index == 0 {
        0
    } else {
        ((inv as i64 * DMIX[code_index] as i64 + (1 << 15)) >> 16) as i32
    }
}

/// How a layout divides into a 5.x base and the channels folded into it.
struct Split {
    /// The surround pair of the base, and the rear pair of a wider base (immersive layouts).
    side: (Channel, Channel),
    rear: Option<(Channel, Channel)>,
    /// Layout indices of the base channels and of the rest, each in speaker-bit order.
    base: Vec<usize>,
    extra: Vec<usize>,
    /// 6.x (the ES matrix) and immersive layouts fold with fixed levels; 7.x pairs follow the Downmix tab.
    fixed: bool,
    /// A stereo set goes below the base.
    stereo: bool,
}

fn split_layout(layout: &ChannelLayout, dm: &Downmix) -> Option<Split> {
    use Channel::*;
    let n = layout.channels.len();
    if !(5..=24).contains(&n) {
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
    let rear = if n > 8 && pos(Lsr).is_some() && pos(Rsr).is_some() {
        Some((Lsr, Rsr))
    } else {
        None
    };
    if let Some((a, b)) = rear {
        base_ch.push(a);
        base_ch.push(b);
    }
    let mut base: Vec<usize> = base_ch.iter().map(|c| pos(*c)).collect::<Option<Vec<_>>>()?;
    base.sort_by_key(|&i| xll_bit(layout.channels[i]));
    let mut extra: Vec<usize> = (0..n).filter(|i| !base.contains(i)).collect();
    extra.sort_by_key(|&i| xll_bit(layout.channels[i]));
    let fixed = n > 8 || layout.id.starts_with("6.");
    let stereo = dm.embed_stereo && n <= 8 && pos(L).is_some() && pos(R).is_some();
    if extra.is_empty() && !stereo {
        return None;
    }
    Some(Split {
        side,
        rear,
        base,
        extra,
        fixed,
        stereo,
    })
}

/// The channel sets in coded order.
struct Sets {
    /// Layout index of every coded channel.
    order: Vec<usize>,
    sizes: Vec<usize>,
    /// Coded channels of each set.
    ranges: Vec<std::ops::Range<usize>>,
    /// Index of the first set made of folded-in channels.
    first_extra: usize,
}

fn make_sets(layout: &ChannelLayout, split: &Split) -> Sets {
    use Channel::*;
    let ch = |i: usize| layout.channels[i];
    let (set0, set1): (Vec<usize>, Vec<usize>) = if split.stereo {
        split.base.iter().partition(|&&i| matches!(ch(i), L | R))
    } else {
        (split.base.clone(), Vec::new())
    };
    let mut sets: Vec<Vec<usize>> = vec![set0];
    if split.stereo {
        sets.push(set1);
    }
    let first_extra = sets.len();
    for chunk in split.extra.chunks(8) {
        sets.push(chunk.to_vec());
    }
    let order: Vec<usize> = sets.iter().flatten().copied().collect();
    let sizes: Vec<usize> = sets.iter().map(Vec::len).collect();
    let mut ranges = Vec::new();
    let mut first = 0;
    for &len in &sizes {
        ranges.push(first..first + len);
        first += len;
    }
    Sets {
        order,
        sizes,
        ranges,
        first_extra,
    }
}

/// Works out the downmix rows, folds and scales of a layout's sets.
struct Builder<'a> {
    layout: &'a ChannelLayout,
    dm: &'a Downmix,
    split: &'a Split,
    sets: &'a Sets,
    folds: Vec<Fold>,
    /// Scale (table index) of each coded channel as seen by the set above the 5.x base.
    base_scale: Vec<Option<usize>>,
    /// Per extra column: the (coded row, coefficient index) terms it adds to.
    extra_terms: Vec<Vec<(usize, usize)>>,
}

impl Builder<'_> {
    fn ch(&self, layout_idx: usize) -> Channel {
        self.layout.channels[layout_idx]
    }

    fn pos(&self, c: Channel) -> Option<usize> {
        self.layout.channels.iter().position(|x| *x == c)
    }

    fn coded(&self, layout_idx: usize) -> usize {
        self.sets
            .order
            .iter()
            .position(|&o| o == layout_idx)
            .expect("channel in a set")
    }

    /// Position in the Downmix tab's tables: C, L, R, side L, side R, LFE.
    fn ident(&self, layout_idx: usize) -> Option<usize> {
        let c = self.ch(layout_idx);
        let side = self.split.side;
        if c == Channel::C {
            Some(0)
        } else if c == Channel::L {
            Some(1)
        } else if c == Channel::R {
            Some(2)
        } else if c == side.0 {
            Some(3)
        } else if c == side.1 {
            Some(4)
        } else if c == Channel::Lfe {
            Some(5)
        } else {
            None
        }
    }

    /// Fixed levels (6.x and immersive layouts): every extra channel is added to its place in the base.
    fn fold_extras_fixed(&mut self) -> Option<()> {
        let split = self.split;
        let rows = split.base.len();
        let mut scale_val = vec![1.0f64; rows];
        let mut coeff_val = vec![vec![0.0f64; split.extra.len()]; rows];
        for (k, &e) in split.extra.iter().enumerate() {
            for (target, scale, coef) in fold_targets(self.ch(e), split.side, split.rear)? {
                let row = split.base.iter().position(|&b| self.ch(b) == target)?;
                scale_val[row] = scale_val[row].min(scale);
                coeff_val[row][k] = coef;
            }
        }
        let first_cols = split.extra.len().min(8);
        for (r, &b) in split.base.iter().enumerate() {
            let idx = if scale_val[r] >= 1.0 {
                UNITY
            } else {
                dmix_index(scale_val[r]).max(41)
            };
            let row = self.coded(b);
            self.base_scale[row] = Some(idx);
            for k in 0..split.extra.len() {
                let v = coeff_val[r][k];
                if v == 0.0 {
                    continue;
                }
                // columns of the first extra set (up to 8 channels) carry the scaled coefficients; later sets state
                // the effective coefficient (coefficient over row scale) with unit scales
                let ci = if k < first_cols {
                    dmix_index(v)
                } else {
                    dmix_index(v / scale_val[r])
                };
                self.extra_terms[k].push((row, ci));
            }
        }
        Some(())
    }

    /// Levels from the Downmix tab (7.x): the first two extra channels join the base as the tab says.
    fn fold_extras_by_tab(&mut self) -> Option<()> {
        let dm = self.dm;
        let split = self.split;
        for &b in &split.base {
            let id = self.ident(b)?;
            let row = self.coded(b);
            self.base_scale[row] = Some(scale_index(dm.primary[id]));
        }
        for k in 0..split.extra.len().min(2) {
            let table = if k == 0 { &dm.extra_a } else { &dm.extra_b };
            for &b in &split.base {
                let id = self.ident(b)?;
                if table[id].is_some() {
                    let row = self.coded(b);
                    self.extra_terms[k].push((row, level_index(table[id])));
                }
            }
        }
        Some(())
    }

    /// The stereo set below the base: two rows (L and R), each built from the 5.1 channels as the 5.1 level sees them.
    fn stereo_rows(&mut self) -> Option<Vec<DmixRow>> {
        let dm = self.dm;
        let cols = self.sets.ranges[1].clone();
        let mut rows = Vec::new();
        let l_idx = self.coded(self.pos(Channel::L)?);
        let r_idx = self.coded(self.pos(Channel::R)?);
        let row_scale = [
            (l_idx, scale_index(dm.stereo_a[1])),
            (r_idx, scale_index(dm.stereo_b[2])),
        ];
        for &(row, si) in &row_scale {
            let (_, inv) = scale_of(si);
            let mut coeff_codes = Vec::new();
            for x in cols.clone() {
                let id = self.ident(self.sets.order[x])?;
                let lvl = if row == l_idx { dm.stereo_a[id] } else { dm.stereo_b[id] };
                let ci = level_index(lvl);
                coeff_codes.push(0x100 | ci as u16);
                if ci != 0 {
                    // The stereo mix is made from the 5.1 channels as the 5.1 level sees them (scaled by the extra
                    // set's row scales): the coefficient carries the row's inverse scale at both levels and the
                    // column's scale.
                    let inv_row = self.base_scale[row].map_or(65536, |i| scale_of(i).1);
                    let s_col = self.base_scale[x].map_or(32768, |i| scale_of(i).0);
                    let at_stereo = effective(inv, ci) as i64;
                    let coef =
                        ((((at_stereo * inv_row as i64 + (1 << 15)) >> 16) * s_col as i64 + (1 << 14)) >> 15) as i32;
                    self.folds.push(Fold {
                        row,
                        col: x,
                        coef,
                        to_5_1: false,
                    });
                }
            }
            rows.push(DmixRow {
                scale_code: 0x100 | si as u16,
                coeff_codes,
            });
        }
        Some(rows)
    }

    /// One set of folded-in channels: a row for every channel of the sets below it.
    fn extra_set_rows(&mut self, si: usize) -> Vec<DmixRow> {
        let range = self.sets.ranges[si].clone();
        let first_extra = self.sets.first_extra;
        let mut rows = Vec::new();
        for j in 0..range.start {
            let scale_idx = if si == first_extra {
                self.base_scale[j].unwrap_or(UNITY)
            } else {
                UNITY
            };
            let (_, inv) = scale_of(scale_idx);
            let mut coeff_codes = Vec::new();
            for x in range.clone() {
                let k = x - self.sets.ranges[first_extra].start;
                let hit = self
                    .extra_terms
                    .get(k)
                    .and_then(|t| t.iter().find(|(row, _)| *row == j))
                    .map(|(_, ci)| *ci);
                match hit {
                    // the first extra set's coefficients are scaled by the row; later ones state the effective value
                    Some(ci) => {
                        coeff_codes.push(0x100 | ci as u16);
                        let coef = if si == first_extra {
                            effective(inv, ci)
                        } else {
                            DMIX[ci] as i32
                        };
                        self.folds.push(Fold {
                            row: j,
                            col: x,
                            coef,
                            to_5_1: true,
                        });
                    }
                    None => coeff_codes.push(0x100),
                }
            }
            rows.push(DmixRow {
                scale_code: 0x100 | scale_idx as u16,
                coeff_codes,
            });
        }
        rows
    }

    /// What the core sees: the 5.x channels, scaled by the extra set's row scales; and the inverse scales.
    fn core_view(&self, total: usize) -> (Vec<i32>, Vec<i32>) {
        let mut scale = vec![32768i32; total];
        let mut inv = vec![65536i32; total];
        if !self.split.extra.is_empty() {
            for x in 0..total {
                if let Some(si) = self.base_scale[x] {
                    let (s, i) = scale_of(si);
                    scale[x] = s;
                    inv[x] = i;
                }
            }
        }
        (scale, inv)
    }
}

/// Split a layout into the sets of the lossless extension: a 5.x base (with the surround pair) and the channels
/// folded into it, plus (when asked) a stereo set below the base.
pub(crate) fn hierarchy(layout: &ChannelLayout, dm: &Downmix) -> Option<Plan> {
    let split = split_layout(layout, dm)?;
    let sets = make_sets(layout, &split);
    let total = sets.order.len();
    let mut b = Builder {
        layout,
        dm,
        split: &split,
        sets: &sets,
        folds: Vec::new(),
        base_scale: vec![None; total],
        extra_terms: vec![Vec::new(); split.extra.len()],
    };
    if !split.extra.is_empty() {
        if split.fixed {
            b.fold_extras_fixed()?;
        } else {
            b.fold_extras_by_tab()?;
        }
    }
    let mut set_rows: Vec<Vec<DmixRow>> = Vec::new();
    if split.stereo {
        set_rows.push(b.stereo_rows()?);
    }
    for si in sets.first_extra..sets.ranges.len() {
        set_rows.push(b.extra_set_rows(si));
    }
    let (scale, inv) = b.core_view(total);
    let hier = Hier {
        sets: sets.ranges.clone(),
        rows: set_rows,
        folds: b.folds,
        scale,
        inv,
        stereo: split.stereo,
        extras: !split.extra.is_empty(),
    };
    Some(Plan {
        order: sets.order,
        set_sizes: sets.sizes,
        hier,
        core_sets: if split.stereo { 2 } else { 1 },
        resid_set: if split.stereo { 1 } else { 0 },
    })
}
