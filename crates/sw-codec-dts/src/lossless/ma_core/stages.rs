//! Running the core encoder over a batch of frames and handing the result to the lossless stage.

use crate::dca::qmf64::Analysis64;
use crate::dca::qmf64::BANDS as BANDS64;
use crate::lossless::ma_core::setup::xll_stage;
use crate::lossless::ma_core::{Job, MaCoreEncoder};

impl MaCoreEncoder {
    /// Core-encodes the next `count` frames while the lossless stage runs on the batch before them, and returns
    /// the finished frames of that earlier batch (`finish` collects the last one).
    pub(crate) fn next_batch(&mut self, count: usize) -> Vec<Vec<u8>> {
        let (cfg, exss) = (self.xll_cfg.clone(), self.exss.clone());
        let earlier = self.pending.take();
        let (done, jobs) = rayon::join(
            move || earlier.map(|jobs| xll_stage(jobs, cfg, exss)).unwrap_or_default(),
            || self.core_stages(count),
        );
        self.pending = (!jobs.is_empty()).then_some(jobs);
        done
    }

    pub(crate) fn flush_pending(&mut self) -> Vec<Vec<u8>> {
        self.pending
            .take()
            .map(|jobs| xll_stage(jobs, self.xll_cfg.clone(), self.exss.clone()))
            .unwrap_or_default()
    }

    /// Core encoding of the next `count` frames and their residuals. The core encodes the frames together; what
    /// follows for each frame (the X96 block, the residual, dropping old samples) is in order, because it
    /// reads the core's leftovers of that frame.
    pub(crate) fn core_stages(&mut self, count: usize) -> Vec<Job> {
        use rayon::prelude::*;
        if count == 0 {
            return Vec::new();
        }
        let first = self.emitted;
        let inputs: Vec<Vec<Vec<f64>>> = {
            let me: &MaCoreEncoder = &*self;
            (first..first + count)
                .into_par_iter()
                .map(|frame| me.core_inputs(frame))
                .collect()
        };
        let refs: Vec<&[Vec<f64>]> = inputs.iter().map(|f| f.as_slice()).collect();
        let coded = self.core.encode_batch(&refs);
        coded
            .into_iter()
            .map(|(bytes, left)| {
                self.core.adopt(left);
                self.after_core(bytes)
            })
            .collect()
    }

    /// The rest of the next frame after the core has coded it: the X96 block, and the residual the lossless
    /// stage codes (sequential: the decoder state carries over).
    pub(crate) fn after_core(&mut self, mut core_bytes: Vec<u8>) -> Job {
        let n = self.n;
        let start = self.emitted * n - self.dropped;
        let nch = self.buf.len();
        if self.x96.is_some() {
            self.add_x96(&mut core_bytes);
        }
        let round = if self.shift > 0 { 1i32 << (self.shift - 1) } else { 0 };
        if self.ratio == 4 {
            // 176.4 / 192 kHz: the core (48 kHz, synthesised to 96 kHz by the decoder) sits under band 0; the
            // lossless stage splits the plain frame into its two bands and codes band 0 minus the core
            let half = n / 2;
            let core0: Vec<Vec<i32>> = self
                .xll_to_core
                .iter()
                .map(|slot| match *slot {
                    None => vec![0; half],
                    Some(slot) => {
                        let c = &self.core.last_core_pcm[slot];
                        debug_assert_eq!(c.len(), half);
                        (0..half)
                            .map(|i| {
                                (((c[i] as i64 + round as i64) >> self.shift) as i32).clamp(-(1 << 23), (1 << 23) - 1)
                            })
                            .collect::<Vec<i32>>()
                    }
                })
                .collect();
            let frame: Vec<Vec<i32>> = (0..nch).map(|x| self.buf[x][start..start + n].to_vec()).collect();
            core_bytes.resize(core_bytes.len() + self.core_pad, 0);
            self.emitted += 1;
            let keep_from = (self.emitted * n).saturating_sub(self.half + 4);
            if keep_from > self.dropped + (1 << 16) {
                let cut = keep_from - self.dropped;
                for b in self.buf.iter_mut().chain(self.cbuf.iter_mut()) {
                    b.drain(..cut);
                }
                self.dropped = keep_from;
            }
            return (core_bytes, frame, Some(core0));
        }
        let mut resid: Vec<Vec<i32>> = Vec::with_capacity(nch);
        for (x, slot) in self.xll_to_core.iter().enumerate() {
            let Some(slot) = *slot else {
                resid.push(self.buf[x][start..start + n].to_vec());
                continue;
            };
            if !self.resid.contains(&x) {
                // carried by the core but coded on its own (the stereo downmix of an embedded stereo set)
                resid.push(self.buf[x][start..start + n].to_vec());
                continue;
            }
            let c = &self.core.last_core_pcm[slot];
            debug_assert_eq!(c.len(), n);
            match &self.hier {
                // the decoder undoes the downmix scale on the core before adding the residual
                Some(h) => {
                    let inv = h.inv[x] as i64;
                    resid.push(
                        (0..n)
                            .map(|i| {
                                let up = ((c[i] as i64 * inv + (1 << 15)) >> 16) as i32;
                                let core = (((up as i64 + round as i64) >> self.shift) as i32)
                                    .clamp(-(1 << 23), (1 << 23) - 1);
                                self.buf[x][start + i] - core
                            })
                            .collect(),
                    )
                }
                None => resid.push(
                    (0..n)
                        .map(|i| self.buf[x][start + i] - ((c[i] + round) >> self.shift))
                        .collect(),
                ),
            }
        }
        core_bytes.resize(core_bytes.len() + self.core_pad, 0);
        self.emitted += 1;
        // drop samples no later frame needs (keep the decimator's look-back)
        let keep_from = (self.emitted * n).saturating_sub(self.half + 4);
        if keep_from > self.dropped + (1 << 16) {
            let cut = keep_from - self.dropped;
            for b in self.buf.iter_mut().chain(self.cbuf.iter_mut()) {
                b.drain(..cut);
            }
            self.dropped = keep_from;
        }
        (core_bytes, resid, None)
    }

    /// Analyse the (core-domain) programme with the 64-band bank, subtract what the core coded in the lower
    /// half and code the rest into the X96 block at the end of the core frame.
    pub(crate) fn add_x96(&mut self, frame: &mut [u8]) {
        use rayon::prelude::*;
        let blocks = 16usize;
        let (n, f_off, dropped, nprim) = (self.n, self.emitted * self.n, self.dropped, self.nprim);
        let gain0 = (1u64 << self.shift) as f64;
        let hier = &self.hier;
        let bufs = if self.cbuf.is_empty() { &self.buf } else { &self.cbuf };
        let deq = &self.core.last_deq;
        let work: Vec<(usize, usize)> = self
            .xll_to_core
            .iter()
            .enumerate()
            .filter_map(|(x, slot)| slot.filter(|s| *s < nprim).map(|s| (x, s)))
            .collect();
        let x96 = self.x96.as_mut().unwrap();
        let delay = x96.delay;
        let _ = n;
        let mut resid: Vec<Vec<Vec<f64>>> = vec![Vec::new(); nprim];
        let jobs: Vec<(usize, usize, &mut Analysis64, &mut Vec<Vec<f64>>)> = {
            let mut anas: Vec<Option<&mut Analysis64>> = x96.ext.iter_mut().map(Some).collect();
            let mut outs: Vec<Option<&mut Vec<Vec<f64>>>> = resid.iter_mut().map(Some).collect();
            work.iter()
                .map(|&(x, slot)| (x, slot, anas[slot].take().unwrap(), outs[slot].take().unwrap()))
                .collect()
        };
        jobs.into_par_iter().for_each(|(x, slot, ana, out)| {
            let g = gain0 * hier.as_ref().map_or(1.0, |h| h.core_gain(x));
            let src = &bufs[x];
            let start = f_off as isize + delay as isize - dropped as isize;
            let input: Vec<f64> = (0..blocks * 64)
                .map(|i| {
                    let idx = start + i as isize;
                    if idx >= 0 && (idx as usize) < src.len() {
                        src[idx as usize] as f64 * g
                    } else {
                        0.0
                    }
                })
                .collect();
            let mut bands = ana.analyse_blocks(&input);
            debug_assert_eq!(bands.len(), BANDS64);
            for band in 0..32 {
                for b in 0..blocks {
                    bands[band][b] -= deq[slot][band][b] as f64;
                }
            }
            *out = bands;
        });
        let bytes = x96.bytes;
        let block = x96.enc.encode(&resid, bytes);
        let start = frame.len() - bytes;
        frame[start..].copy_from_slice(&block);
    }
}
