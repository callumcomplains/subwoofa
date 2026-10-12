//! DTS-HD Master Audio streams: lossless extension (XLL) in extension substream frames, optionally on
//! top of a core.

use crate::dmix_table::{DMIX, DMIX_INV};
use crate::exss::{exss_frame, ExssParams};
use crate::xll::{DmixHeader, DmixRow, SetCfg, XllConfig, XllEncoder};
use sw_core::{Channel, ChannelLayout, Downmix};

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

/// One term of an embedded downmix: `row += column x coef` (15-bit fixed point), the column taken from the
/// original channel.
#[derive(Debug, Clone)]
pub struct Fold {
    pub row: usize,
    pub col: usize,
    pub coef: i32,
    /// The term belongs to the downmix to 5.1 (the core carries it too), not the one to stereo.
    pub to_5_1: bool,
}

/// Embedded downmixes of a stream with several channel sets. The channels of a set are folded into the channels
/// of the sets below it, so a decoder that reads only the lower sets (or the core) still gets a complete
/// programme: 7.x folds into 5.1, and 5.1 can fold into a stereo set of its own.
#[derive(Debug, Clone)]
pub struct Hier {
    /// Coded channels of every set, set 0 first.
    pub sets: Vec<std::ops::Range<usize>>,
    /// Header rows of every set from set 1 up: one per channel of the sets below.
    pub rows: Vec<Vec<DmixRow>>,
    pub folds: Vec<Fold>,
    /// Gain the core carries relative to the transmitted samples of each coded channel (15-bit fixed point) and
    /// its 16-bit inverse; unity where no set scales the channel.
    pub scale: Vec<i32>,
    pub inv: Vec<i32>,
    /// Set 0 is a stereo downmix.
    pub stereo: bool,
    /// Channels beyond 5.x are folded into the 5.x ones.
    pub extras: bool,
}

#[inline]
fn mul15(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64 + (1 << 14)) >> 15) as i32
}

impl Hier {
    /// Turn the original channels (in coded order) into the transmitted ones: every row gets the weighted columns
    /// added, exactly inverted by the decoder. With `core_only` just the downmix to 5.1 is applied (what the
    /// core carries).
    pub fn apply(&self, pcm: &mut [Vec<i32>], core_only: bool) {
        // the decoder undoes the sets from the bottom up, so the stereo set is built from the 5.1 channels as
        // they are transmitted (with the extra channels already folded in), and those from the originals
        for to_5_1 in [true, false] {
            if !to_5_1 && core_only {
                break;
            }
            let mut snap: Vec<Option<Vec<i32>>> = vec![None; pcm.len()];
            for f in self.folds.iter().filter(|f| f.to_5_1 == to_5_1) {
                if snap[f.col].is_none() {
                    snap[f.col] = Some(pcm[f.col].clone());
                }
            }
            for f in self.folds.iter().filter(|f| f.to_5_1 == to_5_1) {
                let src = snap[f.col].as_ref().expect("snapshot");
                for (d, s) in pcm[f.row].iter_mut().zip(src) {
                    *d += mul15(*s, f.coef);
                }
            }
        }
    }

    /// Gain the core sees relative to the transmitted samples of coded channel `x`.
    pub fn core_gain(&self, x: usize) -> f64 {
        self.scale[x] as f64 / 32768.0
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
    /// Leading sets that the core covers, and the one of them coded as the residual of the core.
    pub core_sets: usize,
    pub resid_set: usize,
    /// Stereo downmix coefficients described in the primary set (type, then one code per channel for each output).
    pub matrix: Option<(u8, Vec<Vec<u16>>)>,
}

impl MaMap {
    pub fn xll_sets(&self, residual_of_core: bool) -> Vec<SetCfg> {
        (0..self.set_sizes.len())
            .map(|i| SetCfg {
                channels: self.set_sizes[i],
                ch_mask: self.set_masks[i],
                residual_of_core: residual_of_core && i == self.resid_set,
                dmix: if i == 0 {
                    None
                } else {
                    let h = self.hier.as_ref().expect("hierarchy");
                    Some(DmixHeader {
                        rows: h.rows[i - 1].clone(),
                    })
                },
                matrix: if i == 0 { self.matrix.clone() } else { None },
            })
            .collect()
    }

    /// Coded channels carried by the core.
    pub fn core_channels(&self) -> usize {
        self.set_sizes[..self.core_sets].iter().sum()
    }
}

fn dmix_index(v: f64) -> usize {
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
fn level_index(level: sw_core::bitstream::Level) -> usize {
    level.map_or(0, |db| dmix_index(10f64.powf(-db / 20.0)))
}

/// Table index of a row scale: a channel cannot be switched off, so "off" means no scaling.
fn scale_index(level: sw_core::bitstream::Level) -> usize {
    level.map_or(UNITY, |db| level_index(Some(db)).max(41))
}

/// 9-bit code of a downmix coefficient: bit 8 marks a positive value, the low bits index the table.
fn level_code(level: sw_core::bitstream::Level) -> u16 {
    0x100 | level_index(level) as u16
}

/// How a channel outside the 5.1 base folds into it: `(base channel, row scale, coefficient)` per target,
/// scales and coefficients as fractions (the row scale gives headroom for what is added to the row).
fn fold_targets(
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
struct Plan {
    /// Layout channel index of every coded channel, and the channels per set.
    order: Vec<usize>,
    set_sizes: Vec<usize>,
    hier: Hier,
    core_sets: usize,
    resid_set: usize,
}

/// Unity scale: index of 0 dB in the downmix table.
const UNITY: usize = 241;

/// Row scale of a table index: the scale and its inverse (the inverse is defined from index 41, about -40 dB).
fn scale_of(idx: usize) -> (i32, i32) {
    let idx = idx.max(41);
    (DMIX[idx] as i32, DMIX_INV[idx - 41] as i32)
}

/// Effective coefficient of a column for a row with the given scale inverse.
fn effective(inv: i32, code_index: usize) -> i32 {
    if code_index == 0 {
        0
    } else {
        ((inv as i64 * DMIX[code_index] as i64 + (1 << 15)) >> 16) as i32
    }
}

/// Split a layout into the sets of the lossless extension: a 5.x base (with the surround pair) and the channels
/// folded into it, plus (when asked) a stereo set below the base.
fn hierarchy(layout: &ChannelLayout, dm: &Downmix) -> Option<Plan> {
    use Channel::*;
    let n = layout.channels.len();
    if n < 5 || n > 24 {
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
    // 6.x (the ES matrix) and immersive layouts fold with fixed levels; 7.x pairs follow the Downmix tab
    let fixed = n > 8 || layout.id.starts_with("6.");
    let stereo = dm.embed_stereo && n <= 8 && pos(L).is_some() && pos(R).is_some();
    if extra.is_empty() && !stereo {
        return None;
    }

    // ---- channel sets
    let ch = |i: usize| layout.channels[i];
    let (set0, set1): (Vec<usize>, Vec<usize>) = if stereo {
        let (lr, rest): (Vec<usize>, Vec<usize>) = base.iter().partition(|&&i| matches!(ch(i), L | R));
        (lr, rest)
    } else {
        (base.clone(), Vec::new())
    };
    let mut sets: Vec<Vec<usize>> = vec![set0];
    if stereo {
        sets.push(set1);
    }
    let first_extra_set = sets.len();
    for chunk in extra.chunks(8) {
        sets.push(chunk.to_vec());
    }
    let order: Vec<usize> = sets.iter().flatten().copied().collect();
    let set_sizes: Vec<usize> = sets.iter().map(Vec::len).collect();
    let mut ranges = Vec::new();
    let mut first = 0;
    for &len in &set_sizes {
        ranges.push(first..first + len);
        first += len;
    }
    let coded = |layout_idx: usize| order.iter().position(|&o| o == layout_idx).expect("channel in a set");
    // identity in the Downmix tab's tables: C, L, R, side L, side R, LFE
    let ident = |layout_idx: usize| -> Option<usize> {
        let c = ch(layout_idx);
        if c == C {
            Some(0)
        } else if c == L {
            Some(1)
        } else if c == R {
            Some(2)
        } else if c == side.0 {
            Some(3)
        } else if c == side.1 {
            Some(4)
        } else if c == Lfe {
            Some(5)
        } else {
            None
        }
    };

    let total = order.len();
    let mut folds = Vec::new();
    // scale (table index) of each coded channel as seen by the set above the 5.x base
    let mut base_scale: Vec<Option<usize>> = vec![None; total];

    // ---- downmix of the extra channels into the 5.x base
    // per extra column: (row coded index, level code, coefficient index) terms
    let mut extra_terms: Vec<Vec<(usize, usize)>> = vec![Vec::new(); extra.len()];
    if !extra.is_empty() {
        if fixed {
            let rows = base.len();
            let mut scale_val = vec![1.0f64; rows];
            let mut coeff_val = vec![vec![0.0f64; extra.len()]; rows];
            for (k, &e) in extra.iter().enumerate() {
                for (target, scale, coef) in fold_targets(ch(e), side, rear)? {
                    let row = base.iter().position(|&b| ch(b) == target)?;
                    scale_val[row] = scale_val[row].min(scale);
                    coeff_val[row][k] = coef;
                }
            }
            let first_cols = extra.len().min(8);
            for (r, &b) in base.iter().enumerate() {
                let idx = if scale_val[r] >= 1.0 {
                    UNITY
                } else {
                    dmix_index(scale_val[r]).max(41)
                };
                base_scale[coded(b)] = Some(idx);
                for k in 0..extra.len() {
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
                    extra_terms[k].push((coded(b), ci));
                }
            }
        } else {
            for &b in &base {
                let id = ident(b)?;
                base_scale[coded(b)] = Some(scale_index(dm.primary[id]));
            }
            for (k, &e) in extra.iter().take(2).enumerate() {
                let _ = e;
                let table = if k == 0 { &dm.extra_a } else { &dm.extra_b };
                for &b in &base {
                    let id = ident(b)?;
                    if table[id].is_some() {
                        extra_terms[k].push((coded(b), level_index(table[id])));
                    }
                }
            }
        }
    }
    let mut set_rows: Vec<Vec<DmixRow>> = Vec::new();
    // stereo set below the base
    if stereo {
        let cols = ranges[1].clone();
        let mut rows = Vec::new();
        let l_idx = coded(pos(L)?);
        let r_idx = coded(pos(R)?);
        let row_scale = [
            (l_idx, scale_index(dm.stereo_a[1])),
            (r_idx, scale_index(dm.stereo_b[2])),
        ];
        for &(row, si) in &row_scale {
            let (_, inv) = scale_of(si);
            let mut coeff_codes = Vec::new();
            for x in cols.clone() {
                let id = ident(order[x])?;
                let lvl = if row == l_idx { dm.stereo_a[id] } else { dm.stereo_b[id] };
                let ci = level_index(lvl);
                coeff_codes.push(0x100 | ci as u16);
                if ci != 0 {
                    // The stereo mix is made from the 5.1 channels as the 5.1 level sees them (scaled by the extra set's
                    // row scales): the coefficient carries the row's inverse scale at both levels and the column's scale.
                    let inv_row = base_scale[row].map_or(65536, |i| scale_of(i).1);
                    let s_col = base_scale[x].map_or(32768, |i| scale_of(i).0);
                    let at_stereo = effective(inv, ci) as i64;
                    let c =
                        ((((at_stereo * inv_row as i64 + (1 << 15)) >> 16) * s_col as i64 + (1 << 14)) >> 15) as i32;
                    folds.push(Fold {
                        row,
                        col: x,
                        coef: c,
                        to_5_1: false,
                    });
                }
            }
            rows.push(DmixRow {
                scale_code: 0x100 | si as u16,
                coeff_codes,
            });
        }
        set_rows.push(rows);
    }
    // extra sets: one row for every channel of the sets below
    for (si, range) in ranges.iter().enumerate().skip(first_extra_set) {
        let lower = range.start;
        let cols = range.clone();
        let mut rows = Vec::new();
        for j in 0..lower {
            let scale_idx = if si == first_extra_set {
                base_scale[j].unwrap_or(UNITY)
            } else {
                UNITY
            };
            let (_, inv) = scale_of(scale_idx);
            let mut coeff_codes = Vec::new();
            for x in cols.clone() {
                let k = x - ranges[first_extra_set].start;
                let hit = extra_terms
                    .get(k)
                    .and_then(|t| t.iter().find(|(row, _)| *row == j))
                    .map(|(_, ci)| *ci);
                match hit {
                    // the first extra set's coefficients are scaled by the row; later ones state the effective value
                    Some(ci) => {
                        coeff_codes.push(0x100 | ci as u16);
                        let coef = if si == first_extra_set {
                            effective(inv, ci)
                        } else {
                            DMIX[ci] as i32
                        };
                        folds.push(Fold {
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
        set_rows.push(rows);
    }

    // ---- what the core sees: the 5.x channels, scaled by the extra set's row scales
    let mut scale = vec![32768i32; total];
    let mut inv = vec![65536i32; total];
    if !extra.is_empty() {
        for x in 0..total {
            if let Some(si) = base_scale[x] {
                let (s, i) = scale_of(si);
                scale[x] = s;
                inv[x] = i;
            }
        }
    }
    let hier = Hier {
        sets: ranges,
        rows: set_rows,
        folds,
        scale,
        inv,
        stereo,
        extras: !extra.is_empty(),
    };
    Some(Plan {
        order,
        set_sizes,
        hier,
        core_sets: if stereo { 2 } else { 1 },
        resid_set: if stereo { 1 } else { 0 },
    })
}

/// Stereo downmix coefficients of the primary set, for layouts whose first set is a plain 5.x group.
fn stereo_matrix(layout: &ChannelLayout, set0: &[usize], dm: &Downmix) -> Option<(u8, Vec<Vec<u16>>)> {
    use Channel::*;
    let side = if layout.channels.contains(&Ls) && layout.channels.contains(&Rs) {
        (Ls, Rs)
    } else {
        (Lss, Rss)
    };
    let has = |c: Channel| layout.channels.contains(&c);
    if !(has(C) && has(L) && has(R) && has(side.0) && has(side.1)) {
        return None;
    }
    let ids: Vec<usize> = set0
        .iter()
        .map(|&i| {
            let c = layout.channels[i];
            if c == C {
                Some(0)
            } else if c == L {
                Some(1)
            } else if c == R {
                Some(2)
            } else if c == side.0 {
                Some(3)
            } else if c == side.1 {
                Some(4)
            } else if c == Lfe {
                Some(5)
            } else {
                None
            }
        })
        .collect::<Option<Vec<_>>>()?;
    Some((
        if dm.lt_rt { 2 } else { 1 },
        vec![
            ids.iter().map(|&i| level_code(dm.stereo_a[i])).collect(),
            ids.iter().map(|&i| level_code(dm.stereo_b[i])).collect(),
        ],
    ))
}

/// The stereo downmix levels as the auxiliary data of a 5.x core: the original writes them into every core frame
/// when the Downmix tab's stereo levels are included (not for the other core arrangements or for 6.x).
pub fn core_aux(
    dm: &Downmix,
    layout: &ChannelLayout,
    amode: u8,
    lfe: bool,
    xch: bool,
) -> Option<crate::encoder::AuxDownmix> {
    if !dm.stereo_matrix || amode != 9 || xch || es_matrix(layout) {
        return None;
    }
    let n = 5 + usize::from(lfe);
    let row = |t: &[sw_core::bitstream::Level; 6]| (0..n).map(|i| level_code(t[i])).collect::<Vec<u16>>();
    Some(crate::encoder::AuxDownmix {
        kind: if dm.lt_rt { 2 } else { 1 },
        rows: vec![row(&dm.stereo_a), row(&dm.stereo_b)],
    })
}

/// 6.x layouts with a centre surround (the ES matrix and discrete forms), whose first set is not a plain 5.x group.
fn es_matrix(layout: &ChannelLayout) -> bool {
    layout.id.starts_with("6.") && !layout.id.ends_with("premixed")
}

pub fn map_layout(layout: &ChannelLayout) -> MaMap {
    map_layout_with(layout, &Downmix::default())
}

pub fn map_layout_with(layout: &ChannelLayout, dm: &Downmix) -> MaMap {
    let mask_of = |ids: &[usize]| ids.iter().fold(0u32, |m, &i| m | (1 << xll_bit(layout.channels[i])));
    let exss_mask = layout.channels.iter().fold(0u32, |m, c| m | exss_bit(*c));
    if let Some(plan) = hierarchy(layout, dm) {
        let mut set_masks = Vec::new();
        let mut first = 0;
        for &len in &plan.set_sizes {
            set_masks.push(mask_of(&plan.order[first..first + len]));
            first += len;
        }
        let xll_mask = set_masks.iter().fold(0, |m, x| m | x);
        let matrix = (dm.stereo_matrix && !plan.hier.stereo && !es_matrix(layout) && layout.channels.len() <= 8)
            .then(|| stereo_matrix(layout, &plan.order[..plan.set_sizes[0]], dm))
            .flatten();
        return MaMap {
            order: plan.order,
            set_sizes: plan.set_sizes,
            set_masks,
            xll_mask,
            exss_mask,
            hier: Some(plan.hier),
            core_sets: plan.core_sets,
            resid_set: plan.resid_set,
            matrix,
        };
    }
    let mut idx: Vec<usize> = (0..layout.channels.len()).collect();
    idx.sort_by_key(|&i| xll_bit(layout.channels[i]));
    let xll_mask = mask_of(&idx);
    let matrix =
        (dm.stereo_matrix && !es_matrix(layout) && layout.channels.len() <= 8 && layout.channels.contains(&Channel::C))
            .then(|| stereo_matrix(layout, &idx, dm))
            .flatten();
    MaMap {
        set_sizes: vec![idx.len()],
        order: idx,
        set_masks: vec![xll_mask],
        xll_mask,
        exss_mask,
        hier: None,
        core_sets: 1,
        resid_set: 0,
        matrix,
    }
}

pub struct MaConfig {
    pub sample_rate: u32,
    pub map: MaMap,
    pub bit_res: u32,
    pub dialnorm: u32,
    /// Add the speaker remapping sets of a wide 7.1 stream (Bitstream tab).
    pub wide_remap: bool,
    /// Representation type for matrix-encoded stereo (2) instead of speakers.
    pub repr_type: Option<u32>,
}

pub struct MaEncoder {
    cfg: MaConfig,
    xll: XllEncoder,
    frame_samples: usize,
    /// Frame length in units of 512 samples at the 48 kHz clock.
    units: usize,
}

impl MaEncoder {
    pub fn new(cfg: MaConfig) -> MaEncoder {
        // A stream without a core has longer frames, as the original writes it: two and a half times as many samples
        // for stereo (2048 at the 48 kHz clock), 1024 up to 7.1, and the usual 512 beyond that.
        let units = match cfg.map.order.len() {
            0..=2 => 4,
            3..=8 => 2,
            _ => 1,
        };
        let frame_samples = units
            * match cfg.sample_rate {
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
        MaEncoder {
            cfg,
            xll,
            frame_samples,
            units,
        }
    }

    pub fn frame_samples(&self) -> usize {
        self.frame_samples
    }

    /// `pcm[i]` is coded channel `i` (see `MaMap::order`), `frame_samples()` signed samples of `bit_res` bits.
    pub fn encode_frame(&mut self, pcm: &[Vec<i32>]) -> Vec<u8> {
        let data = match &self.cfg.map.hier {
            Some(h) => {
                let mut sent = pcm.to_vec();
                h.apply(&mut sent, false);
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
                frame_duration_code: self.units as u32 - 1,
                x96_in_core: false,
                xch_in_core: false,
                secondary: false,
                extras: crate::exss::ExssExtras {
                    min_header: 0,
                    emb_5_1: self.cfg.map.hier.as_ref().map_or(false, |h| h.extras),
                    emb_stereo: self.cfg.map.hier.as_ref().map_or(false, |h| h.stereo),
                    wide_remap: self.cfg.wide_remap,
                    repr_type: self.cfg.repr_type,
                    ..Default::default()
                },
            },
            &data,
        )
    }
}
