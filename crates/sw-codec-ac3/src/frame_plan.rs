//! Planning a frame from its transformed coefficients: channel plans, the coupling channel, exponents and masking,
//! and the search for the highest SNR offset whose mantissas fit in the frame.

use crate::bitalloc::{calc_bap, calc_bap_from, calc_mask, calc_mask_cpl, calc_psd, calc_psd_from, FGAINCOD};
use crate::coupling::pin_reference;
use crate::exponents::{condition_exponents, exponent_of, Strat};
use crate::mantissas::BAP_BITS;
use crate::mdct::M;
use crate::plan::{ChannelPlan, SetPlan};
use crate::tables::FAST_GAIN;
use crate::{Encoder, BANDS, BLOCKS};

impl Encoder {
    /// Encode a frame from already-transformed coefficients (`[channel][block][bin]`).
    pub(crate) fn encode_coefficients(
        &mut self,
        mut coefs: Vec<Vec<[f32; M]>>,
        frmsizecod: u32,
        words: usize,
    ) -> Vec<u8> {
        let mut cpl = self.plan_coupling(&mut coefs);
        self.rematrix(&mut coefs, cpl.as_ref().map_or(M, |c| c.strt));
        let frame_bits = words * 16;
        let mut chans: Vec<ChannelPlan> = Vec::with_capacity(self.nch + 1);
        let orig: Vec<Vec<[f32; M]>> = coefs.iter().take(self.nfch).cloned().collect();
        for (ch, coef) in coefs.into_iter().enumerate() {
            let is_lfe = self.cfg.lfe && ch == self.nfch;
            let end = match &cpl {
                Some(c) if ch < self.nfch => c.strt,
                _ => self.end[ch],
            };
            chans.push(ChannelPlan {
                start: 0,
                is_cpl: false,
                end,
                strat: if is_lfe { Strat::D15 } else { self.strat },
                is_lfe,
                coef,
                sets: Vec::new(),
            });
        }
        if let Some(c) = &mut cpl {
            // the coupling channel: the mean of the coupled channels above the coupling start
            let mut coef = vec![[0f32; M]; BLOCKS];
            for blk in 0..BLOCKS {
                for bin in c.strt..c.end {
                    coef[blk][bin] = (0..self.nfch).map(|ch| chans[ch].coef[blk][bin]).sum::<f32>() / self.nfch as f32;
                }
            }
            if self.cfg.acmod == 2 {
                // bands where the right channel mostly opposes the left would cancel in the mean: flag
                // them (the decoder flips the right channel's coordinate) and couple the difference
                let nbands = 3 + c.endf - c.begf;
                for &(first, count) in &self.cur_sets {
                    let mut flags = vec![false; nbands];
                    for (band, flag) in flags.iter_mut().enumerate() {
                        let (lo, hi) = (c.strt + 12 * band, c.strt + 12 * band + 12);
                        let mut dot = 0f64;
                        for blk in first..first + count {
                            for bin in lo..hi {
                                dot += chans[0].coef[blk][bin] as f64 * chans[1].coef[blk][bin] as f64;
                            }
                        }
                        *flag = dot < 0.0 && std::env::var_os("SW_AC3_NOPHS").is_none();
                        if *flag {
                            for blk in first..first + count {
                                for bin in lo..hi {
                                    coef[blk][bin] = 0.5 * (chans[0].coef[blk][bin] - chans[1].coef[blk][bin]);
                                }
                            }
                        }
                    }
                    c.phase.push(flags);
                }
            }
            // what the channels carry above the start is now the coupling channel's business
            for ch in 0..self.nfch {
                for blk in 0..BLOCKS {
                    for bin in c.strt..M {
                        chans[ch].coef[blk][bin] = 0.0;
                    }
                }
            }
            chans.push(ChannelPlan {
                start: c.strt,
                is_cpl: true,
                end: c.end,
                strat: self.strat,
                is_lfe: false,
                coef,
                sets: Vec::new(),
            });
        }

        // 2. exponents, psd and masking per (channel, set of blocks)
        for (ch, cp) in chans.iter_mut().enumerate() {
            for &(first, count) in &self.cur_sets {
                let lo = cp.start;
                let mut min_e = vec![24i32; cp.end];
                for blk in first..first + count {
                    for bin in lo..cp.end {
                        min_e[bin] = min_e[bin].min(exponent_of(cp.coef[blk][bin]));
                    }
                }
                if cp.is_cpl {
                    let strat = cp.strat;
                    let g = strat.gsize();
                    let nbins = cp.end - lo;
                    // reference: an even exponent at or below the first group's
                    let first_group = min_e[lo..lo + g].iter().copied().min().unwrap();
                    let mut raw = vec![first_group & !1];
                    raw.extend_from_slice(&min_e[lo..cp.end]);
                    let mut virt = condition_exponents(&raw, strat);
                    {
                        // pin the reference on the group sequence, then spread the groups over their bins again
                        let mut seq: Vec<i32> = std::iter::once(virt[0])
                            .chain((0..nbins / g).map(|i| virt[1 + i * g]))
                            .collect();
                        pin_reference(&mut seq);
                        virt[0] = seq[0];
                        for bin in 0..nbins {
                            virt[1 + bin] = seq[1 + bin / g];
                        }
                    }
                    let mut exp = vec![0i32; cp.end];
                    for bin in lo..cp.end {
                        exp[bin] = virt[1 + bin - lo];
                    }
                    let mut psd = vec![0i32; cp.end];
                    let mut band_psd = [0i32; BANDS];
                    calc_psd_from(&exp, lo, cp.end, &self.b2b, &mut psd, &mut band_psd);
                    let mut mask = [0i32; BANDS];
                    calc_mask_cpl(
                        &band_psd,
                        lo,
                        cp.end,
                        FAST_GAIN[FGAINCOD],
                        self.fscod,
                        &self.b2b,
                        &mut mask,
                    );
                    debug_assert_eq!(virt.len(), nbins + 1);
                    cp.sets.push(SetPlan {
                        strat,
                        virt,
                        exp,
                        psd,
                        mask,
                    });
                    continue;
                }
                let strat = if ch < self.nfch && self.cur_blksw[ch][first] {
                    Strat::D45
                } else {
                    cp.strat
                };
                let exp = condition_exponents(&min_e, strat);
                let mut psd = vec![0i32; cp.end];
                let mut band_psd = [0i32; BANDS];
                calc_psd(&exp, cp.end, &self.b2b, &mut psd, &mut band_psd);
                let mut mask = [0i32; BANDS];
                calc_mask(
                    &band_psd,
                    cp.end,
                    FAST_GAIN[FGAINCOD],
                    cp.is_lfe,
                    self.fscod,
                    &self.b2b,
                    &mut mask,
                );
                cp.sets.push(SetPlan {
                    strat,
                    virt: Vec::new(),
                    exp,
                    psd,
                    mask,
                });
            }
        }
        if let Some(c) = cpl.as_mut() {
            self.fill_coords(c, &chans, &orig);
        }
        self.cur_cpl = cpl;

        // 3. side-information size (everything but mantissas), then find the highest SNR offset that fits
        let side = self.write_frame(&chans, frmsizecod, words, 0, true).0;
        // 18 bits at the end are reserved for auxdatae, crcrsv and crc2
        let budget = frame_bits.saturating_sub(side + 18);
        let (mut lo, mut hi) = (0u32, 1023u32);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if self.mantissa_bits(&chans, mid) <= budget {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        self.frame_no += 1;
        self.write_frame(&chans, frmsizecod, words, lo, false).1
    }

    /// Test hook: encode a frame whose coefficients are given directly, bypassing the transform.
    #[doc(hidden)]
    pub fn encode_frame_from_coefficients(&mut self, coefs: Vec<Vec<[f32; M]>>) -> Vec<u8> {
        let (words, frmsizecod) = self.frame_words();
        self.cur_sets = self.sets.clone();
        self.cur_blksw = vec![[false; BLOCKS]; self.nfch];
        self.encode_coefficients(coefs, frmsizecod, words)
    }

    pub(crate) fn snr_offset(v: u32) -> i32 {
        let (csnr, fsnr) = ((v >> 4) as i32, (v & 15) as i32);
        (((csnr - 15) << 4) + fsnr) << 2
    }

    pub(crate) fn baps(&self, cp: &ChannelPlan, set: usize, v: u32) -> Vec<u8> {
        let mut bap = vec![0u8; cp.end];
        if cp.is_cpl {
            calc_bap_from(
                &cp.sets[set].mask,
                &cp.sets[set].psd,
                cp.start,
                cp.end,
                Self::snr_offset(v),
                &self.b2b,
                &mut bap,
            );
            return bap;
        }
        calc_bap(
            &cp.sets[set].mask,
            &cp.sets[set].psd,
            cp.end,
            Self::snr_offset(v),
            &self.b2b,
            &mut bap,
        );
        bap
    }

    pub(crate) fn mantissa_bits(&self, chans: &[ChannelPlan], v: u32) -> usize {
        let per: Vec<Vec<Vec<u8>>> = chans
            .iter()
            .map(|cp| (0..self.cur_sets.len()).map(|s| self.baps(cp, s, v)).collect())
            .collect();
        let mut total = 0usize;
        for (set, &(_, count)) in self.cur_sets.iter().enumerate() {
            let mut cnt = [0usize; 16];
            let mut direct = 0usize;
            for p in &per {
                for &b in &p[set] {
                    cnt[b as usize] += 1;
                    direct += BAP_BITS[b as usize] as usize;
                }
            }
            let grouped = cnt[1].div_ceil(3) * 5 + cnt[2].div_ceil(3) * 7 + cnt[4].div_ceil(2) * 7;
            total += (direct + grouped) * count;
        }
        total
    }
}
