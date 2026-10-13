//! Planning a frame from its transformed coefficients: channel plans, the coupling channel, exponents and masking,
//! and the search for the highest SNR offset whose mantissas fit in the frame.

use crate::bitalloc::{calc_bap, calc_bap_from, calc_mask, calc_mask_cpl, calc_psd, calc_psd_from, FGAINCOD};
use crate::coupling::{pin_reference, CplInfo};
use crate::exponents::{condition_exponents, exponent_of, Strat};
use crate::mantissas::BAP_BITS;
use crate::mdct::M;
use crate::plan::{ChannelPlan, SetPlan};
use crate::tables::FAST_GAIN;
use crate::{Encoder, BANDS, BLOCKS};

/// Bits at the end of a frame that are never available to mantissas: `auxdatae`, `crcrsv` and `crc2`.
const TRAILER_BITS: usize = 18;

/// Highest SNR offset code (coarse 0 to 63 in the high bits, fine 0 to 15 in the low four).
const MAX_SNR_CODE: u32 = 1023;

impl Encoder {
    /// Encode a frame from already-transformed coefficients (`[channel][block][bin]`).
    pub(crate) fn encode_coefficients(
        &mut self,
        mut coefs: Vec<Vec<[f32; M]>>,
        frmsizecod: u32,
        words: usize,
    ) -> Vec<u8> {
        let mut coupling = self.plan_coupling(&mut coefs);
        self.rematrix(&mut coefs, coupling.as_ref().map_or(M, |c| c.strt));
        // the channels as they were before coupling takes the high bands from them (the coordinates are measured on them)
        let uncoupled: Vec<Vec<[f32; M]>> = coefs.iter().take(self.nfch).cloned().collect();
        let mut chans = self.channel_plans(coefs, coupling.as_ref());
        if let Some(c) = &mut coupling {
            let coupling_channel = self.coupling_channel(c, &mut chans);
            chans.push(coupling_channel);
        }
        self.plan_exponents(&mut chans);
        if let Some(c) = coupling.as_mut() {
            self.fill_coords(c, &chans, &uncoupled);
        }
        self.cur_coupling = coupling;

        // the size of everything but mantissas, then the highest SNR offset whose mantissas fit in the rest
        let side_bits = self.write_frame(&chans, frmsizecod, words, 0, true).0;
        let budget = (words * 16).saturating_sub(side_bits + TRAILER_BITS);
        let snr_code = self.highest_snr_that_fits(&chans, budget);
        self.write_frame(&chans, frmsizecod, words, snr_code, false).1
    }

    /// One plan per channel: its coefficients, how far up it is coded and its exponent strategy.
    fn channel_plans(&self, coefs: Vec<Vec<[f32; M]>>, coupling: Option<&CplInfo>) -> Vec<ChannelPlan> {
        coefs
            .into_iter()
            .enumerate()
            .map(|(ch, coef)| {
                let is_lfe = self.cfg.lfe && ch == self.nfch;
                let end = match coupling {
                    Some(c) if ch < self.nfch => c.strt,
                    _ => self.end[ch],
                };
                ChannelPlan {
                    start: 0,
                    is_cpl: false,
                    end,
                    strat: if is_lfe { Strat::D15 } else { self.strat },
                    is_lfe,
                    coef,
                    sets: Vec::new(),
                }
            })
            .collect()
    }

    /// The coupling channel: the mean of the coupled channels above the coupling start. In 2/0 mode, bands where the
    /// right channel mostly opposes the left (which would cancel in the mean) are flagged, and the difference is
    /// coupled there instead. The coupled channels lose everything above the start.
    fn coupling_channel(&self, c: &mut CplInfo, chans: &mut [ChannelPlan]) -> ChannelPlan {
        let mut coef = vec![[0f32; M]; BLOCKS];
        for blk in 0..BLOCKS {
            for bin in c.strt..c.end {
                coef[blk][bin] = (0..self.nfch).map(|ch| chans[ch].coef[blk][bin]).sum::<f32>() / self.nfch as f32;
            }
        }
        if self.cfg.acmod == 2 {
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
                    *flag = dot < 0.0;
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
        for chan in chans.iter_mut().take(self.nfch) {
            for blk in 0..BLOCKS {
                for bin in c.strt..M {
                    chan.coef[blk][bin] = 0.0;
                }
            }
        }
        ChannelPlan {
            start: c.strt,
            is_cpl: true,
            end: c.end,
            strat: self.strat,
            is_lfe: false,
            coef,
            sets: Vec::new(),
        }
    }

    /// Exponents, power spectral density and masking curve for every channel and set of blocks.
    fn plan_exponents(&self, chans: &mut [ChannelPlan]) {
        for (ch, cp) in chans.iter_mut().enumerate() {
            for &(first, count) in &self.cur_sets {
                // a bin's exponent is the smallest (the loudest coefficient) over the blocks that share it
                let mut smallest = vec![24i32; cp.end];
                for blk in first..first + count {
                    for bin in cp.start..cp.end {
                        smallest[bin] = smallest[bin].min(exponent_of(cp.coef[blk][bin]));
                    }
                }
                let set = if cp.is_cpl {
                    self.coupling_set_plan(cp, &smallest)
                } else {
                    // a short block's exponents are coded coarsely: they change every block
                    let strat = if ch < self.nfch && self.cur_blksw[ch][first] {
                        Strat::D45
                    } else {
                        cp.strat
                    };
                    self.plain_set_plan(cp, &smallest, strat)
                };
                cp.sets.push(set);
            }
        }
    }

    /// The exponents of a channel from its smallest-exponent profile, with their power spectral density and masking.
    fn plain_set_plan(&self, cp: &ChannelPlan, smallest: &[i32], strat: Strat) -> SetPlan {
        let exp = condition_exponents(smallest, strat);
        let mut psd = vec![0i32; cp.end];
        let mut band_psd = [0i32; BANDS];
        calc_psd(&exp, cp.end, &self.band_of_bin, &mut psd, &mut band_psd);
        let mut mask = [0i32; BANDS];
        calc_mask(
            &band_psd,
            cp.end,
            FAST_GAIN[FGAINCOD],
            cp.is_lfe,
            self.fscod,
            &self.band_of_bin,
            &mut mask,
        );
        SetPlan {
            strat,
            virt: Vec::new(),
            exp,
            psd,
            mask,
        }
    }

    /// The coupling channel's exponents: a reference at or below the first group's (even, because it is sent
    /// halved), then the groups pinned inside the +-2 step limit, spread over their bins again.
    fn coupling_set_plan(&self, cp: &ChannelPlan, smallest: &[i32]) -> SetPlan {
        let (strat, lo) = (cp.strat, cp.start);
        let group = strat.gsize();
        let nbins = cp.end - lo;
        let first_group = smallest[lo..lo + group].iter().copied().min().unwrap();
        let mut raw = vec![first_group & !1];
        raw.extend_from_slice(&smallest[lo..cp.end]);
        let mut virt = condition_exponents(&raw, strat);
        let mut groups: Vec<i32> = std::iter::once(virt[0])
            .chain((0..nbins / group).map(|i| virt[1 + i * group]))
            .collect();
        pin_reference(&mut groups);
        virt[0] = groups[0];
        for bin in 0..nbins {
            virt[1 + bin] = groups[1 + bin / group];
        }
        let mut exp = vec![0i32; cp.end];
        for bin in lo..cp.end {
            exp[bin] = virt[1 + bin - lo];
        }
        let mut psd = vec![0i32; cp.end];
        let mut band_psd = [0i32; BANDS];
        calc_psd_from(&exp, lo, cp.end, &self.band_of_bin, &mut psd, &mut band_psd);
        let mut mask = [0i32; BANDS];
        calc_mask_cpl(
            &band_psd,
            lo,
            cp.end,
            FAST_GAIN[FGAINCOD],
            self.fscod,
            &self.band_of_bin,
            &mut mask,
        );
        debug_assert_eq!(virt.len(), nbins + 1);
        SetPlan {
            strat,
            virt,
            exp,
            psd,
            mask,
        }
    }

    /// The highest SNR offset code whose mantissas take at most `budget` bits, by bisection (more SNR never takes
    /// fewer bits).
    fn highest_snr_that_fits(&self, chans: &[ChannelPlan], budget: usize) -> u32 {
        let (mut lo, mut hi) = (0u32, MAX_SNR_CODE);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if self.mantissa_bits(chans, mid) <= budget {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        lo
    }

    /// The SNR offset in the units of the bit allocation: the code's coarse part in steps of 64, its fine part in
    /// steps of 4 (A/52 7.2.2.7).
    pub(crate) fn snr_offset(snr_code: u32) -> i32 {
        let (coarse, fine) = ((snr_code >> 4) as i32, (snr_code & 15) as i32);
        (((coarse - 15) << 4) + fine) << 2
    }

    /// The number of mantissa bits (`bap`) of every bin of a channel for one set of blocks.
    pub(crate) fn baps(&self, cp: &ChannelPlan, set: usize, snr_code: u32) -> Vec<u8> {
        let mut bap = vec![0u8; cp.end];
        let snr = Self::snr_offset(snr_code);
        let (mask, psd) = (&cp.sets[set].mask, &cp.sets[set].psd);
        if cp.is_cpl {
            calc_bap_from(mask, psd, cp.start, cp.end, snr, &self.band_of_bin, &mut bap);
        } else {
            calc_bap(mask, psd, cp.end, snr, &self.band_of_bin, &mut bap);
        }
        bap
    }

    /// Bits the mantissas of the whole frame take at an SNR offset code.
    pub(crate) fn mantissa_bits(&self, chans: &[ChannelPlan], snr_code: u32) -> usize {
        let baps: Vec<Vec<Vec<u8>>> = chans
            .iter()
            .map(|cp| (0..self.cur_sets.len()).map(|s| self.baps(cp, s, snr_code)).collect())
            .collect();
        let mut total = 0usize;
        for (set, &(_, blocks)) in self.cur_sets.iter().enumerate() {
            let mut count = [0usize; 16];
            let mut direct = 0usize;
            for channel in &baps {
                for &bap in &channel[set] {
                    count[bap as usize] += 1;
                    direct += BAP_BITS[bap as usize] as usize;
                }
            }
            // baps 1, 2 and 4 are grouped: three values in 5 bits, three in 7, two in 7
            let grouped = count[1].div_ceil(3) * 5 + count[2].div_ceil(3) * 7 + count[4].div_ceil(2) * 7;
            total += (direct + grouped) * blocks;
        }
        total
    }
}
