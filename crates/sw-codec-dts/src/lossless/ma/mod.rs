//! DTS-HD Master Audio streams: lossless extension (XLL) in extension substream frames, optionally on
//! top of a core.

mod fold;
mod map;
mod masks;
mod plan;

pub use fold::{Fold, Hier};
pub use map::{core_aux, map_layout, map_layout_with, MaMap};
pub use masks::exss_bit;

use crate::container::exss::{exss_frame, ExssParams};
use crate::lossless::xll::{XllConfig, XllEncoder};

pub struct MaConfig {
    pub sample_rate: u32,
    pub map: MaMap,
    pub bit_res: u32,
    pub dialnorm: u32,
    /// Add the speaker remapping sets of a wide 7.1 stream (Bitstream tab).
    pub wide_remap: bool,
    /// Representation type for matrix-encoded stereo (2) instead of speakers.
    pub repr_type: Option<u32>,
    pub effort: sw_core::Effort,
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
            effort: cfg.effort,
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
                extras: crate::container::exss::ExssExtras {
                    min_header: 0,
                    emb_5_1: self.cfg.map.hier.as_ref().is_some_and(|h| h.extras),
                    emb_stereo: self.cfg.map.hier.as_ref().is_some_and(|h| h.stereo),
                    wide_remap: self.cfg.wide_remap,
                    repr_type: self.cfg.repr_type,
                    ..Default::default()
                },
            },
            &data,
        )
    }
}
