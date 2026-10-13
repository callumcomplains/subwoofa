//! Frames in and bytes out: the filter banks run in order, the plan search and packing of the frames of a batch run
//! on all cores, and the decoder emulation follows in order.

use crate::core_exact::dequantize;
use crate::encoder::search::Plan;
use crate::encoder::tables::SUBBAND_GAIN;
use crate::encoder::Encoder;
use crate::qmf::BANDS;
use crate::tables::{SCALE_7, STEP_LOSSY};

impl Encoder {
    pub(crate) fn run_batch(&mut self, frames: &[&[Vec<f64>]]) -> Vec<(Vec<u8>, Leftovers)> {
        use rayon::prelude::*;
        let nch = self.nplan();
        let subbands = self.analyse(frames);
        let lfe: Vec<Option<(Vec<i32>, u8)>> = frames
            .iter()
            .map(|pcm| self.cfg.lfe.then(|| self.lfe_samples(&pcm[nch])))
            .collect();
        // a lone frame searches with all cores; many frames keep a core each
        let spread = frames.len() < 4;
        let hint = self.last_offset;
        let me = &*self;
        let coded: Vec<Coded> = subbands
            .par_iter()
            .zip(lfe.par_iter())
            .map(|(sub, lfe)| me.plan_and_pack(sub, lfe.as_ref(), hint, spread))
            .collect();
        coded
            .into_iter()
            .zip(&lfe)
            .map(|(c, lfe)| self.settle(c, lfe.as_ref()))
            .collect()
    }

    /// The subband samples `[frame][channel][band][block]` of frames in order. Each channel's filter bank runs on
    /// its own thread; it only depends on its own earlier input.
    pub(crate) fn analyse(&mut self, frames: &[&[Vec<f64>]]) -> Vec<Vec<Vec<Vec<f64>>>> {
        use rayon::prelude::*;
        let (nch, blocks) = (self.nplan(), self.cfg.blocks);
        let per_channel: Vec<Vec<Vec<Vec<f64>>>> = self.analysis[..nch]
            .par_iter_mut()
            .enumerate()
            .map(|(ch, an)| {
                frames
                    .iter()
                    .map(|pcm| {
                        let mut out = vec![vec![0.0; blocks]; BANDS];
                        for b in 0..blocks {
                            let mut o = [0f64; BANDS];
                            an.step(&pcm[ch][b * 32..b * 32 + 32], &mut o);
                            for (band, v) in o.iter().enumerate() {
                                out[band][b] = v * SUBBAND_GAIN;
                            }
                        }
                        out
                    })
                    .collect()
            })
            .collect();
        let mut columns: Vec<_> = per_channel.into_iter().map(|c| c.into_iter()).collect();
        (0..frames.len())
            .map(|_| columns.iter_mut().map(|c| c.next().unwrap()).collect())
            .collect()
    }

    /// The plan and bytes of a frame. This needs nothing from the frames around it except the starting point
    /// `hint` of the search, so frames can be done in any order or together.
    pub(crate) fn plan_and_pack(
        &self,
        sub: &[Vec<Vec<f64>>],
        lfe: Option<&(Vec<i32>, u8)>,
        hint: f64,
        spread: bool,
    ) -> Coded {
        let (offset, plan) = self.plan_for(sub, hint, spread);
        let bytes = self.pack(&plan, lfe);
        let planes = self.keep_residual.then(|| self.planes_of(&plan, sub));
        Coded {
            bytes,
            plan,
            offset,
            planes,
        }
    }

    /// The part of a frame that follows the frames before it: the search's starting point and the decoder
    /// emulation.
    pub(crate) fn settle(&mut self, coded: Coded, lfe: Option<&(Vec<i32>, u8)>) -> (Vec<u8>, Leftovers) {
        self.last_offset = coded.offset;
        let core_pcm = if self.exact.is_some() {
            self.emulate_decoder(&coded.plan, lfe)
        } else {
            Vec::new()
        };
        let (residual, deq) = coded.planes.unwrap_or_default();
        (
            coded.bytes,
            Leftovers {
                core_pcm,
                residual,
                deq,
            },
        )
    }

    /// The core's reconstruction of a frame (`deq`) and what it left over (`residual`), `[channel][band][block]`.
    pub(crate) fn planes_of(&self, best: &Plan, sub: &[Vec<Vec<f64>>]) -> (Vec<Vec<Vec<f64>>>, Vec<Vec<Vec<i32>>>) {
        let blocks = self.cfg.blocks;
        let planes = self.nplan(); // the primaries, and the XCH channel when there is one
        let mut res = Vec::with_capacity(planes);
        let mut all_deq = Vec::with_capacity(planes);
        for ch in 0..planes {
            let mut bands = Vec::with_capacity(BANDS);
            let mut deqs = Vec::with_capacity(BANDS);
            for b in 0..BANDS {
                let k = best.abits[ch][b] as usize;
                let mut deq = vec![0i32; blocks];
                if k > 0 {
                    let scale = SCALE_7[best.scale_idx[ch][b] as usize] as i32;
                    dequantize(&best.q[ch][b], STEP_LOSSY[k] as i32, scale, &mut deq);
                }
                bands.push(sub[ch][b].iter().zip(&deq).map(|(v, d)| v - *d as f64).collect());
                deqs.push(deq);
            }
            res.push(bands);
            all_deq.push(deqs);
        }
        (res, all_deq)
    }

    /// Keeps the core's reconstruction of this frame (`last_deq`) and what it left over (`last_residual`).
    pub(crate) fn keep_planes(&mut self, best: &Plan, sub: &[Vec<Vec<f64>>]) {
        (self.last_residual, self.last_deq) = self.planes_of(best, sub);
    }

    /// Highest quality plan that fits the frame, searched from where the last frame ended.
    pub(crate) fn search_plan(&mut self, sub: &[Vec<Vec<f64>>]) -> Plan {
        let (offset, plan) = self.plan_for(sub, self.last_offset, true);
        self.last_offset = offset;
        plan
    }

    /// Encode one frame from ready-made subband samples `sub[ch][band][block]` (bypasses the analysis bank).
    pub fn encode_subbands(&mut self, sub: &[Vec<Vec<f64>>], lfe: Option<(Vec<i32>, u8)>) -> Vec<u8> {
        let coded = self.plan_and_pack(sub, lfe.as_ref(), self.last_offset, true);
        let (bytes, left) = self.settle(coded, lfe.as_ref());
        self.adopt(left);
        bytes
    }
}

/// What a frame leaves behind besides its bytes, for the extensions that are built on the core.
#[derive(Default)]
pub struct Leftovers {
    /// The PCM the decoder produces for the frame (only with `enable_exact_decode`).
    pub core_pcm: Vec<Vec<i32>>,
    /// Input subband samples minus the core's reconstruction (only with `keep_residual`).
    pub residual: Vec<Vec<Vec<f64>>>,
    /// The core's reconstructed subband samples (only with `keep_residual`).
    pub deq: Vec<Vec<Vec<i32>>>,
}

/// A frame planned and packed, waiting for the part that has to follow the frames before it.
pub(crate) struct Coded {
    pub(crate) bytes: Vec<u8>,
    pub(crate) plan: Plan,
    pub(crate) offset: f64,
    pub(crate) planes: Option<(Vec<Vec<Vec<f64>>>, Vec<Vec<Vec<i32>>>)>,
}
