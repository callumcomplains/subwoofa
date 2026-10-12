//! DTS-HD High Resolution Audio: the core plus an XBR extension in an extension substream, at a constant
//! total bit rate.

use crate::exss::{exss_frame_ext, ExssParams};
use crate::xbr::XbrEncoder;

pub struct HraStage {
    xbr: XbrEncoder,
    exss: ExssParams,
    /// Bits per frame as a fraction: `bits_num / bits_den` (so odd rates stay exact on average).
    bits_num: u64,
    bits_den: u64,
    frames: u64,
    overhead: usize,
    /// How the channels divide into XBR channel sets (empty: one set of all).
    sets: Vec<usize>,
}

impl HraStage {
    /// `total_kbps` is core plus extension; `exss` describes the asset.
    pub fn new(total_kbps: u32, sample_rate: u32, frame_samples: usize, blocks: usize, exss: ExssParams) -> HraStage {
        let overhead = exss_frame_ext(&exss, Some(&[0u8; 4]), None).len() - 4;
        HraStage {
            xbr: XbrEncoder::new(blocks),
            exss,
            bits_num: total_kbps as u64 * 1000 * frame_samples as u64,
            bits_den: sample_rate as u64,
            frames: 0,
            overhead,
            sets: Vec::new(),
        }
    }

    /// Split the channels of the residual into XBR channel sets of these sizes.
    pub fn set_split(&mut self, sets: Vec<usize>) {
        self.sets = sets;
    }

    /// Wrap one core frame: returns core substream frame + extension substream frame, `total` bytes on
    /// average per `frame_samples` samples.
    pub fn wrap(&mut self, mut core: Vec<u8>, residual: &[Vec<Vec<f64>>]) -> Vec<u8> {
        let pad = (4 - core.len() % 4) % 4;
        core.resize(core.len() + pad, 0);
        let upto = |k: u64| (k * self.bits_num) / (self.bits_den * 8);
        let total = (upto(self.frames + 1) - upto(self.frames)) as usize;
        self.frames += 1;
        let budget = total.saturating_sub(core.len() + self.overhead).clamp(1, 16384);
        let mut xbr = if self.sets.is_empty() {
            self.xbr.encode(residual, budget)
        } else {
            self.xbr.encode_sets(residual, &self.sets, budget)
        };
        if xbr.len() < budget {
            xbr.resize(budget, 0);
        }
        let mut out = core;
        out.extend_from_slice(&exss_frame_ext(&self.exss, Some(&xbr), None));
        out
    }
}
