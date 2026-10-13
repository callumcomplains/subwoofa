//! Deciding on channel coupling for a frame and working out its coordinates.

use crate::coupling::{quantize_coords, CplInfo};
use crate::mdct::M;
use crate::plan::ChannelPlan;
use crate::Encoder;

impl Encoder {
    /// Decide on coupling for this frame (not with block switching in it) and describe it.
    pub(crate) fn plan_coupling(&self, coefs: &mut [Vec<[f32; M]>]) -> Option<CplInfo> {
        let _ = coefs;
        if !self.coupling || self.dependent.is_some() || self.cur_blksw.iter().any(|c| c.iter().any(|b| *b)) {
            return None;
        }
        let per_ch = self.cfg.bitrate_kbps as f64 / (self.nfch as f64 + 0.1);
        let begf = if self.cfg.acmod == 2 {
            if per_ch < 56.0 {
                4
            } else {
                6
            }
        } else if per_ch < 40.0 {
            2
        } else if per_ch < 56.0 {
            4
        } else if per_ch < 72.0 {
            6
        } else {
            8
        };
        let end_bin = self.end[0];
        let endf = (((end_bin.saturating_sub(37)) / 12).saturating_sub(3)).min(15);
        if std::env::var_os("SW_AC3_DEBUG").is_some() {
            eprintln!("coupling: end_bin {end_bin} begf {begf} endf {endf}");
        }
        // low bandwidth: start coupling as low as the format allows (stereo needs four rematrix bands, begf > 2)
        let begf = if self.cfg.acmod == 2 {
            begf.min(endf.saturating_sub(2)).max(3)
        } else {
            begf.min(endf.saturating_sub(1))
        };
        if endf < begf + 1 {
            return None;
        }
        Some(CplInfo {
            begf,
            endf,
            strt: 37 + 12 * begf,
            end: 37 + 12 * (endf + 3),
            coords: Vec::new(),
            phase: Vec::new(),
        })
    }

    /// Coupling coordinates per set of blocks, from the energy of each channel against the coupling channel.
    pub(crate) fn fill_coords(&self, c: &mut CplInfo, chans: &[ChannelPlan], orig: &[Vec<[f32; M]>]) {
        let nbands = 3 + c.endf - c.begf;
        let cplch = chans.last().unwrap();
        for &(first, count) in &self.cur_sets {
            let mut per_ch = Vec::new();
            for ch in 0..self.nfch {
                let mut gains = Vec::with_capacity(nbands);
                for band in 0..nbands {
                    let (lo, hi) = (c.strt + 12 * band, c.strt + 12 * band + 12);
                    let (mut ec, mut ep) = (0f64, 0f64);
                    for blk in first..first + count {
                        for bin in lo..hi {
                            ec += (orig[ch][blk][bin] as f64).powi(2);
                            ep += (cplch.coef[blk][bin] as f64).powi(2);
                        }
                    }
                    gains.push(if ep > 1e-18 { (ec / ep).sqrt().min(7.9) } else { 0.0 });
                }
                per_ch.push(quantize_coords(&gains));
            }
            c.coords.push(per_ch);
        }
    }
}
