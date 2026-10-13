//! The part of a frame that depends on the frames before it: the transform with its overlap, transient
//! detection and block switching, and the sets of blocks that share exponents. Also stereo rematrixing.

use crate::mdct::M;
use crate::plan::Analysed;
use crate::{Encoder, BLOCKS, SAMPLES_PER_FRAME};

impl Encoder {
    /// The part of a frame that depends on the frames before it: size, transform and block switching.
    pub(crate) fn analyse(&mut self, input: &[&[f32]]) -> Analysed {
        assert_eq!(input.len(), self.nch);
        let frame: Vec<Vec<f32>> = input.iter().map(|c| c.to_vec()).collect();
        self.analyse_batch(&[frame]).pop().unwrap()
    }

    /// Frames in order, `frames[n][channel]`. Each channel is transformed on its own thread: it carries only its
    /// own overlap and detector state from frame to frame.
    pub(crate) fn analyse_batch(&mut self, frames: &[Vec<Vec<f32>>]) -> Vec<Analysed> {
        use rayon::prelude::*;
        let sizes: Vec<(usize, u32)> = frames.iter().map(|_| self.frame_words()).collect();
        let (nfch, switching) = (self.nfch, self.block_switching);
        let (mdct, switched) = (&self.mdct, &self.switched);
        let mut dets = self.dets.iter_mut();
        let work: Vec<_> = self
            .hist
            .iter_mut()
            .enumerate()
            .map(|(ch, hist)| (ch, hist, if ch < nfch { dets.next() } else { None }))
            .collect();
        // per channel: for every frame, its coefficients and block-switch flags
        let per_channel: Vec<Vec<(Vec<[f32; M]>, [bool; BLOCKS])>> = work
            .into_par_iter()
            .map(|(ch, hist, mut det)| {
                frames
                    .iter()
                    .map(|frame| {
                        assert_eq!(frame[ch].len(), SAMPLES_PER_FRAME);
                        let mut coef = vec![[0f32; M]; BLOCKS];
                        let mut flags = [false; BLOCKS];
                        let mut buf = [0f32; 2 * M];
                        for blk in 0..BLOCKS {
                            let new = &frame[ch][blk * M..(blk + 1) * M];
                            let short = switching && det.as_mut().is_some_and(|d| d.block(new));
                            flags[blk] = short;
                            buf[..M].copy_from_slice(hist);
                            buf[M..].copy_from_slice(new);
                            if short {
                                switched.forward(&buf, true, &mut coef[blk]);
                            } else {
                                mdct.forward(&buf, &mut coef[blk]);
                            }
                            hist.copy_from_slice(&buf[M..]);
                        }
                        (coef, flags)
                    })
                    .collect()
            })
            .collect();
        let mut columns: Vec<_> = per_channel.into_iter().map(|c| c.into_iter()).collect();
        sizes
            .into_iter()
            .map(|(words, frmsizecod)| {
                let mut coefs = Vec::with_capacity(self.nch);
                let mut blksw = Vec::with_capacity(nfch);
                for (ch, column) in columns.iter_mut().enumerate() {
                    let (coef, flags) = column.next().unwrap();
                    coefs.push(coef);
                    if ch < nfch {
                        blksw.push(flags);
                    }
                }
                Analysed {
                    coefs,
                    blksw,
                    words,
                    frmsizecod,
                }
            })
            .collect()
    }

    /// Exponent sets for a frame: the configured grouping, with every block that is switched in any channel
    /// as a set of its own (the exponents of a short block do not carry over to its neighbours).
    pub(crate) fn frame_sets(&self, blksw: &[[bool; BLOCKS]]) -> Vec<(usize, usize)> {
        let any: Vec<bool> = (0..BLOCKS).map(|b| blksw.iter().any(|c| c[b])).collect();
        let mut out = Vec::new();
        for &(first, count) in &self.sets {
            let mut start = first;
            for blk in first..first + count {
                if any[blk] {
                    if start < blk {
                        out.push((start, blk - start));
                    }
                    out.push((blk, 1));
                    start = blk + 1;
                }
            }
            if start < first + count {
                out.push((start, first + count - start));
            }
        }
        out
    }

    /// 2/0 mode: per block and band, send sum and difference instead of left and right where that leaves
    /// less energy to code (clause 7.5.3).
    pub(crate) fn rematrix(&mut self, coefs: &mut [Vec<[f32; M]>], below: usize) {
        self.cur_remat = vec![[false; 4]; BLOCKS];
        if self.cfg.acmod != 2 || self.cfg.eac3 || !self.rematrixing {
            return;
        }
        const EDGES: [(usize, usize); 4] = [(13, 25), (25, 37), (37, 61), (61, 253)];
        let end = self.end[0].min(self.end[1]).min(below);
        let (a, b) = coefs.split_at_mut(1);
        let (left, right) = (&mut a[0], &mut b[0]);
        for blk in 0..BLOCKS {
            if self.cur_blksw[0][blk] != self.cur_blksw[1][blk] {
                continue;
            }
            for (band, &(lo, hi)) in EDGES.iter().enumerate() {
                let hi = hi.min(end);
                if lo >= hi {
                    continue;
                }
                let (mut sl, mut sr, mut sp, mut sm) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
                for k in lo..hi {
                    let (l, r) = (left[blk][k] as f64, right[blk][k] as f64);
                    sl += l * l;
                    sr += r * r;
                    sp += (l + r) * (l + r);
                    sm += (l - r) * (l - r);
                }
                if sp.min(sm) < sl.min(sr) {
                    self.cur_remat[blk][band] = true;
                    for k in lo..hi {
                        let (l, r) = (left[blk][k], right[blk][k]);
                        left[blk][k] = 0.5 * (l + r);
                        right[blk][k] = 0.5 * (l - r);
                    }
                }
            }
        }
    }
}
