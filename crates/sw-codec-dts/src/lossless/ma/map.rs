//! A layout as the lossless extension codes it: channel order, sets and masks, and the stereo downmix data.

use crate::lossless::ma::fold::Hier;
use crate::lossless::ma::masks::{exss_bit, xll_bit};
use crate::lossless::ma::plan::{hierarchy, level_code};
use crate::lossless::xll::{DmixHeader, SetCfg};
use sw_core::{Channel, ChannelLayout, Downmix};

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

/// Stereo downmix coefficients of the primary set, for layouts whose first set is a plain 5.x group.
pub(crate) fn stereo_matrix(layout: &ChannelLayout, set0: &[usize], dm: &Downmix) -> Option<(u8, Vec<Vec<u16>>)> {
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
) -> Option<crate::dca::encoder::AuxDownmix> {
    if !dm.stereo_matrix || amode != 9 || xch || es_matrix(layout) {
        return None;
    }
    let n = 5 + usize::from(lfe);
    let row = |t: &[sw_core::bitstream::Level; 6]| (0..n).map(|i| level_code(t[i])).collect::<Vec<u16>>();
    Some(crate::dca::encoder::AuxDownmix {
        kind: if dm.lt_rt { 2 } else { 1 },
        rows: vec![row(&dm.stereo_a), row(&dm.stereo_b)],
    })
}

/// 6.x layouts with a centre surround (the ES matrix and discrete forms), whose first set is not a plain 5.x group.
pub(crate) fn es_matrix(layout: &ChannelLayout) -> bool {
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
