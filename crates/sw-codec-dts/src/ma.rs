//! DTS-HD Master Audio streams: lossless extension (XLL) in extension substream frames, optionally on
//! top of a core.

use crate::exss::{exss_frame, ExssParams};
use crate::xll::{XllConfig, XllEncoder};
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
        Lh | Lhs => 13,
        Ch => 14,
        Rh | Rhs => 15,
        Lw => 17,
        Rw => 18,
        Oh => 19,
    }
}

/// Loudspeaker group bit of the extension substream's activity mask (Table 7-10).
fn exss_bit(c: Channel) -> u32 {
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
        Lhs | Rhs => 0x2000,
    }
}

#[derive(Debug, Clone)]
pub struct MaMap {
    /// Layout channel index for each coded channel (ascending speaker bit).
    pub order: Vec<usize>,
    pub xll_mask: u32,
    pub exss_mask: u32,
}

pub fn map_layout(layout: &ChannelLayout) -> MaMap {
    let mut idx: Vec<usize> = (0..layout.channels.len()).collect();
    idx.sort_by_key(|&i| xll_bit(layout.channels[i]));
    let xll_mask = layout.channels.iter().fold(0u32, |m, c| m | (1 << xll_bit(*c)));
    let exss_mask = layout.channels.iter().fold(0u32, |m, c| m | exss_bit(*c));
    MaMap { order: idx, xll_mask, exss_mask }
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
            _ => 512,
        };
        let xll = XllEncoder::new(XllConfig {
            sample_rate: cfg.sample_rate,
            channels: cfg.map.order.len(),
            ch_mask: cfg.map.xll_mask,
            bit_res: cfg.bit_res,
            residual_of_core: false,
            frame_samples,
        });
        MaEncoder { cfg, xll, frame_samples }
    }

    pub fn frame_samples(&self) -> usize {
        self.frame_samples
    }

    /// `pcm[i]` is coded channel `i` (see `MaMap::order`), `frame_samples()` signed samples of `bit_res` bits.
    pub fn encode_frame(&mut self, pcm: &[Vec<i32>]) -> Vec<u8> {
        let data = self.xll.encode_frame(pcm);
        exss_frame(
            &ExssParams {
                sample_rate: self.cfg.sample_rate,
                channels: self.cfg.map.order.len(),
                bit_res: self.cfg.bit_res,
                spk_mask: self.cfg.map.exss_mask,
                with_core: false,
                dialnorm: self.cfg.dialnorm,
                frame_duration_code: 0,
            },
            &data,
        )
    }
}
