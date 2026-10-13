//! Running the decoder's integer arithmetic over the frames the encoder writes, so the lossless extension can code
//! exactly what a decoder will reconstruct from the core.

use crate::core_exact::{dequantize, dequantize_lfe, FixedLfe, FixedSynth};
use crate::encoder::search::Plan;
use crate::encoder::Encoder;
use crate::qmf::BANDS;
use crate::tables::{SCALE_7, STEP_LOSSY};

impl Encoder {
    /// Track the decoder's exact output (`last_core_pcm`) for every frame encoded from now on.
    /// With `x96` the emulated decoder synthesises at twice the core rate (for 96 kHz lossless streams).
    pub fn enable_exact_decode(&mut self, x96: bool) {
        let n = self.cfg.channels;
        self.exact = Some(ExactState {
            synth: (0..n).map(|_| FixedSynth::new(false, x96)).collect(),
            lfe: FixedLfe::new(),
            x96,
        });
    }

    /// Run the decoder's integer arithmetic over this frame's coded data.
    pub(crate) fn emulate_decoder(&mut self, plan: &Plan, lfe: Option<&(Vec<i32>, u8)>) -> Vec<Vec<i32>> {
        let nch = self.cfg.channels;
        let blocks = self.cfg.blocks;
        let st = self.exact.as_mut().unwrap();
        let mut deq = vec![vec![vec![0i32; blocks]; BANDS]; nch];
        for ch in 0..nch {
            for b in 0..BANDS {
                let k = plan.abits[ch][b] as usize;
                if k == 0 {
                    continue;
                }
                let scale = SCALE_7[plan.scale_idx[ch][b] as usize] as i32;
                dequantize(&plan.q[ch][b], STEP_LOSSY[k] as i32, scale, &mut deq[ch][b]);
            }
        }
        let mut out: Vec<Vec<i32>> = Vec::new();
        for ch in 0..nch {
            let mult = if st.x96 { 2 } else { 1 };
            let mut pcm = vec![0i32; blocks * 32 * mult];
            for blk in 0..blocks {
                let mut input = [0i32; 32];
                for b in 0..BANDS {
                    input[b] = deq[ch][b][blk];
                }
                st.synth[ch].step(&input, &mut pcm[blk * 32 * mult..(blk + 1) * 32 * mult]);
            }
            out.push(pcm);
        }
        if let Some((q, idx)) = lfe {
            let samples = dequantize_lfe(q, *idx as usize);
            out.push(st.lfe.interpolate(&samples, st.x96));
        }
        out
    }
}

pub(crate) struct ExactState {
    pub(crate) synth: Vec<FixedSynth>,
    pub(crate) lfe: FixedLfe,
    pub(crate) x96: bool,
}
