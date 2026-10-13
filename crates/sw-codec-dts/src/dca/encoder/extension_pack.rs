//! The extension channels of a core frame: XCH (a sixth full-range channel) and XXCH (extended channel sets).

use crate::bits::BitWriter;
use crate::dca::encoder::search::Plan;
use crate::dca::encoder::tables::{GROUP_SIZE, SEL_BITS};
use crate::dca::encoder::types::XxchParams;
use crate::dca::encoder::Encoder;
use crate::dca::qmf::BANDS;
use crate::dca::tables::{BITALLOC_HUFF, TMODE_HUFF};

impl Encoder {
    /// XXCH extension frame for the channels of this encoder (which must be set up like a core encoder for
    /// those channels): analysis, rate search to `total_bytes`, packing. `pcm` holds one slice of
    /// `frame_samples()` per channel.
    pub fn encode_xxch(&mut self, pcm: &[Vec<f64>], total_bytes: usize, p: &XxchParams) -> Vec<u8> {
        let nch = self.cfg.channels;
        assert_eq!(pcm.len(), nch);
        let sub = self.analyse(&[pcm]).pop().unwrap();
        self.frame_bytes = total_bytes;
        self.x96_reserve = 0;
        // fixed parts: frame header, channel set header without the per-channel selector tables, ADPCM flags
        let hdr = Self::xxch_set_header_fixed_bits(p, nch).div_ceil(8) * 8 + 16;
        self.header_bits_override = Some(88 + hdr + BANDS * nch + 8);
        let plan = self.search_plan(&sub);
        if self.keep_residual {
            self.keep_planes(&plan, &sub);
        }
        self.pack_xxch(&plan, p, total_bytes)
    }

    pub(crate) fn xxch_set_header_fixed_bits(p: &XxchParams, nch: usize) -> usize {
        let nb = p.mask_bits as usize;
        let mut bits = 7 + 3 + (nb - 6) + 1;
        if let Some((_, maps)) = &p.dmix {
            bits += 1 + 6 + nb * nch + maps.iter().map(|m| 7 * m.len()).sum::<usize>();
        }
        bits + nch * (5 + 5 + 3 + 2 + 3 + 3)
    }

    /// The XXCH frame: frame header, one channel set (header and subframe), padded to `total` bytes.
    pub(crate) fn pack_xxch(&self, plan: &Plan, p: &XxchParams, total: usize) -> Vec<u8> {
        let head = self.xxch_set_header(plan, p);
        let data = self.xxch_subframe(plan);
        let mut out = xxch_frame_header(p, head.len() + data.len());
        out.extend_from_slice(&head);
        out.extend_from_slice(&data);
        assert!(out.len() <= total, "XXCH overflow: {} > {}", out.len(), total);
        out.resize(total, 0);
        out
    }

    /// The channel set header: size, channels, speakers, embedded downmix and the per-channel tables, with a CRC.
    fn xxch_set_header(&self, plan: &Plan, p: &XxchParams) -> Vec<u8> {
        let nch = self.cfg.channels;
        let nb = p.mask_bits;
        let mut w = BitWriter::new();
        w.put(0, 7); // size, patched below
        w.put(nch as u32 - 1, 3);
        w.put(p.spk_mask >> 6, nb - 6);
        match &p.dmix {
            Some((scale, maps)) => {
                w.put(1, 1);
                w.put(1, 1); // the downmix is already in the core
                w.put(*scale, 6);
                for m in maps {
                    w.put(m.iter().fold(0, |a, (spk, _)| a | (1 << spk)), nb);
                }
                for m in maps {
                    let mut sorted = m.clone();
                    sorted.sort_by_key(|(spk, _)| *spk);
                    for (_, code) in sorted {
                        w.put(code, 7);
                    }
                }
            }
            None => w.put(0, 1),
        }
        self.put_channel_tables(&mut w, plan);
        w.align(8);
        let size = w.bit_len() / 8 + 2;
        let mut head = w.into_bytes();
        head[0] = (head[0] & 0x01) | (((size - 1) as u8) << 1);
        let crc = crate::lossless::xll::crc16(&head);
        head.extend_from_slice(&crc.to_be_bytes());
        head
    }

    /// The subframe: side information then samples.
    fn xxch_subframe(&self, plan: &Plan) -> Vec<u8> {
        let mut w = BitWriter::new();
        self.put_band_info(&mut w, plan);
        self.put_audio(&mut w, plan);
        w.put(0xFFFF, 16);
        w.into_bytes()
    }

    /// The XCH extension: the sixth channel coded like a core channel, ending at the end of the frame.
    pub(crate) fn pack_xch(&self, plan: &Plan, dist: usize) -> Vec<u8> {
        let ch = self.cfg.channels;
        let nssc = self.cfg.blocks / 8;
        let mut w = BitWriter::new();
        w.put(0x5A5A_5A5A, 32);
        w.put(dist as u32 - 1, 10);
        w.put(1, 4); // one extension channel
        w.put(0, 3);
        w.put(30, 5); // 32 active subbands
        w.put(31, 5); // no high frequency VQ
        w.put(0, 3); // no joint intensity
        w.put(0, 2); // transient codebook A4
        w.put(6, 3); // 7-bit linear scale factors
        w.put(plan.bhuff[ch] as u32, 3);
        for (i, nb) in SEL_BITS.iter().enumerate() {
            w.put(plan.sel[ch][i] as u32, *nb);
        }
        for (i, g) in GROUP_SIZE.iter().enumerate() {
            if (plan.sel[ch][i] as usize) < *g {
                w.put(0, 2);
            }
        }
        for _ in 0..BANDS {
            w.put(0, 1); // no ADPCM
        }
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
        if nssc > 1 {
            for b in 0..BANDS {
                if plan.abits[ch][b] > 0 {
                    let (c, l) = TMODE_HUFF[0][0];
                    w.put(c as u32, l as u32);
                }
            }
        }
        for b in 0..BANDS {
            if plan.abits[ch][b] > 0 {
                w.put(plan.scale_idx[ch][b] as u32, 7);
            }
        }
        for ss in 0..nssc {
            for b in 0..BANDS {
                let k = plan.abits[ch][b] as usize;
                if k == 0 {
                    continue;
                }
                let vals = &plan.q[ch][b][ss * 8..ss * 8 + 8];
                self.put_samples(&mut w, k, plan.sel[ch][k.min(10) - 1], vals);
            }
        }
        w.put(0xFFFF, 16);
        let mut bytes = w.into_bytes();
        assert!(bytes.len() <= dist, "XCH overflow: {} > {}", bytes.len(), dist);
        bytes.resize(dist, 0);
        bytes
    }
}

/// The XXCH frame header, which announces one channel set of `set_bytes` bytes.
fn xxch_frame_header(p: &XxchParams, set_bytes: usize) -> Vec<u8> {
    let nb = p.mask_bits;
    let mut f = BitWriter::new();
    f.put(0x4700_4A03, 32);
    let hs = (32 + 6 + 1 + 5 + 2 + 14 + nb as usize).div_ceil(8) + 2;
    f.put(hs as u32 - 1, 6);
    f.put(1, 1); // channel set headers carry a CRC
    f.put(nb - 1, 5);
    f.put(0, 2); // one channel set
    f.put(set_bytes as u32 - 1, 14);
    f.put(p.core_mask, nb);
    f.align(8);
    let mut out = f.into_bytes();
    let crc = crate::lossless::xll::crc16(&out[4..]);
    out.extend_from_slice(&crc.to_be_bytes());
    debug_assert_eq!(out.len(), hs);
    out
}
