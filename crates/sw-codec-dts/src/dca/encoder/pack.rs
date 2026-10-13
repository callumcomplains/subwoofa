//! Writing a core frame: header, subframes (bit allocation, scale factors, samples), LFE, auxiliary data.

use crate::bits::BitWriter;
use crate::dca::encoder::search::Plan;
use crate::dca::encoder::tables::{raw_bits, BLOCK_BITS, GROUP_SIZE, SEL_BITS};
use crate::dca::encoder::Encoder;
use crate::dca::qmf::BANDS;
use crate::dca::tables::{BITALLOC_HUFF, BIT_RATES, QUANT_HUFF, QUANT_LEVELS, TMODE_HUFF};

impl Encoder {
    pub(crate) fn pack(&self, plan: &Plan, lfe: Option<&(Vec<i32>, u8)>) -> Vec<u8> {
        let cfg = &self.cfg;
        let mut w = BitWriter::new();
        self.put_frame_header(&mut w);
        self.put_coding_header(&mut w, plan);
        self.put_side_info(&mut w, plan);
        // LFE
        if let Some((q, idx)) = lfe {
            for v in q {
                w.put_signed(*v, 8);
            }
            w.put(*idx as u32, 8);
        }
        self.put_audio(&mut w, plan);
        w.put(0xFFFF, 16);
        if let Some(aux) = &self.aux {
            w.put(aux.size() as u32, 6); // bytes of auxiliary data
        }
        let mut bytes = w.into_bytes();
        if let Some(aux) = &self.aux {
            // a decoder looks for the sync word at the next 32-bit boundary after the size field
            let at = bytes.len().div_ceil(4) * 4;
            assert!(
                at <= self.aux_sync(aux),
                "frame overflow before the auxiliary data: {at} > {}",
                self.aux_sync(aux)
            );
            bytes.resize(at, 0);
            bytes.extend_from_slice(&aux.block());
        }
        if cfg.xch {
            // the extension ends the frame and its size field has 10 bits: after a quiet frame's short core data it
            // starts later than the data ends, with zeros between
            let start = bytes.len().div_ceil(4) * 4;
            let start = start.max((self.frame_bytes - 1020) & !3);
            bytes.resize(start, 0);
            bytes.extend_from_slice(&self.pack_xch(plan, self.frame_bytes - start));
        }
        assert!(
            bytes.len() <= self.frame_bytes,
            "frame overflow: {} > {}",
            bytes.len(),
            self.frame_bytes
        );
        bytes.resize(self.frame_bytes, 0);
        bytes
    }

    /// The frame header: sync word, sizes, rate, extension flags, dialogue level.
    fn put_frame_header(&self, w: &mut BitWriter) {
        let cfg = &self.cfg;
        let blocks = cfg.blocks;
        w.put(0x7FFE_8001, 32);
        w.put(1, 1);
        w.put(31, 5);
        w.put(0, 1);
        w.put(blocks as u32 - 1, 7);
        w.put(self.frame_bytes as u32 - 1, 14);
        w.put(cfg.amode as u32, 6);
        w.put(sfreq_code(cfg.sample_rate).unwrap(), 4);
        // 1234.8 kbit/s is the 14-bit DTS CD stream, labelled with the disc's 1411.2 kbit/s rate code
        let rate_code = if cfg.bitrate == 1_234_800 {
            22
        } else {
            BIT_RATES.iter().take(25).position(|r| *r >= cfg.bitrate).unwrap_or(29) as u32
        };
        w.put(rate_code, 5);
        w.put(0, 1); // fixed bit
        w.put(0, 1); // dynamic range coefficients
        w.put(0, 1); // time stamp
        w.put(u32::from(self.aux.is_some()), 1); // auxiliary data
        w.put(0, 1); // HDCD
        w.put(if self.x96_reserve > 0 { 2 } else { 0 }, 3); // extension audio id: XCH (0) or X96 (2)
        w.put(u32::from(cfg.xch || self.x96_reserve > 0), 1); // extension audio present
        w.put(0, 1); // DSYNC only per subframe
        w.put(if cfg.lfe { 2 } else { 0 }, 2); // LFE interpolation by 64
        w.put(1, 1); // predictor history valid
        w.put(u32::from(cfg.perfect), 1); // perfect (1) or non-perfect (0) reconstruction bank
        let (vernum, dn) = if cfg.dialnorm >= -15 {
            (7, -cfg.dialnorm)
        } else {
            (6, -cfg.dialnorm - 16)
        };
        w.put(vernum, 4);
        w.put(1, 2); // copy history
        let pcmr = match (cfg.pcm_bits, cfg.es) {
            (16, false) => 0b000,
            (16, true) => 0b001,
            (20, false) => 0b010,
            (20, true) => 0b011,
            (24, false) => 0b110,
            _ => 0b101,
        };
        w.put(pcmr, 3);
        w.put(0, 1); // front sum/difference
        w.put(0, 1); // surround sum/difference
        w.put(dn as u32, 4);
    }

    /// The primary audio coding header: channels, subbands, scale-factor tables, bit-allocation selectors.
    fn put_coding_header(&self, w: &mut BitWriter, plan: &Plan) {
        let nch = self.cfg.channels;
        w.put(0, 4); // one subframe
        w.put(nch as u32 - 1, 3);
        for _ in 0..nch {
            w.put(30, 5); // 32 active subbands
        }
        for _ in 0..nch {
            w.put(31, 5); // no high frequency VQ
        }
        for _ in 0..nch {
            w.put(0, 3); // no joint intensity
        }
        for _ in 0..nch {
            w.put(0, 2); // transient codebook A4
        }
        for _ in 0..nch {
            w.put(6, 3); // 7-bit linear scale factors
        }
        for ch in 0..nch {
            w.put(plan.bhuff[ch] as u32, 3);
        }
        for (i, nb) in SEL_BITS.iter().enumerate() {
            for ch in 0..nch {
                w.put(plan.sel[ch][i] as u32, *nb);
            }
        }
        for (i, g) in GROUP_SIZE.iter().enumerate() {
            for ch in 0..nch {
                if (plan.sel[ch][i] as usize) < *g {
                    w.put(0, 2); // adjustment 1.0
                }
            }
        }
    }

    /// The subframe side information: ADPCM flags, bit allocation, transient modes and scale factors.
    fn put_side_info(&self, w: &mut BitWriter, plan: &Plan) {
        let nch = self.cfg.channels;
        let nssc = self.cfg.blocks / 8;
        w.put(nssc as u32 - 1, 2);
        w.put(0, 3);
        for _ in 0..nch {
            for _ in 0..BANDS {
                w.put(0, 1); // no ADPCM
            }
        }
        for ch in 0..nch {
            for b in 0..BANDS {
                let a = plan.abits[ch][b] as u32;
                match plan.bhuff[ch] {
                    6 => w.put(a, 5),
                    5 => w.put(a, 4),
                    j => {
                        let (c, l) = BITALLOC_HUFF[j as usize][a as usize - 1];
                        w.put(c as u32, l as u32);
                    }
                }
            }
        }
        if nssc > 1 {
            for ch in 0..nch {
                for b in 0..BANDS {
                    if plan.abits[ch][b] > 0 {
                        let (c, l) = TMODE_HUFF[0][0];
                        w.put(c as u32, l as u32);
                    }
                }
            }
        }
        for ch in 0..nch {
            for b in 0..BANDS {
                if plan.abits[ch][b] > 0 {
                    w.put(plan.scale_idx[ch][b] as u32, 7);
                }
            }
        }
    }

    /// The samples of every subband, a sub-subframe at a time.
    fn put_audio(&self, w: &mut BitWriter, plan: &Plan) {
        let nch = self.cfg.channels;
        let nssc = self.cfg.blocks / 8;
        for ss in 0..nssc {
            for ch in 0..nch {
                for b in 0..BANDS {
                    let k = plan.abits[ch][b] as usize;
                    if k == 0 {
                        continue;
                    }
                    let vals = &plan.q[ch][b][ss * 8..ss * 8 + 8];
                    self.put_samples(w, k, plan.sel[ch][k.min(10) - 1], vals);
                }
            }
        }
    }

    pub(crate) fn put_samples(&self, w: &mut BitWriter, k: usize, sel: u8, vals: &[i32]) {
        if k > 10 {
            for v in vals {
                w.put_signed(*v, raw_bits(k));
            }
            return;
        }
        if (sel as usize) < GROUP_SIZE[k - 1] {
            let t = QUANT_HUFF[k - 1][sel as usize];
            let off = (t.len() as i32 - 1) / 2;
            for v in vals {
                let (c, l) = t[(*v + off) as usize];
                w.put(c as u32, l as u32);
            }
        } else if k <= 7 {
            let levels = QUANT_LEVELS[k] as i32;
            let off = (levels - 1) / 2;
            for blk in vals.chunks(4) {
                let mut code = 0i32;
                for v in blk.iter().rev() {
                    code = code * levels + (*v + off);
                }
                w.put(code as u32, BLOCK_BITS[k - 1]);
            }
        } else {
            for v in vals {
                w.put_signed(*v, raw_bits(k));
            }
        }
    }
}

/// The sampling-frequency code of the core frame header (table 5-5 of the standard).
pub(crate) fn sfreq_code(rate: u32) -> Option<u32> {
    Some(match rate {
        8000 => 1,
        16000 => 2,
        32000 => 3,
        11025 => 6,
        22050 => 7,
        44100 => 8,
        12000 => 11,
        24000 => 12,
        48000 => 13,
        _ => return None,
    })
}
