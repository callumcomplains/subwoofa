//! Writing a frame of a channel set: the segments of residuals, the set header and the frame around them.

use crate::bits::BitWriter;
use crate::lossless::xll::residual::{
    choose_code_b, isolated_positions, put_rice, put_values, recode, signed_to_raw, stored_param, zigzag, Code,
};
use crate::lossless::xll::{crc16, fs_index, BandPlan, ChannelPlan, SetCfg, SetPlan, XllEncoder, SYNC_XLL};

impl XllEncoder {
    /// The entropy-coded segments of one channel set.
    pub(crate) fn set_segments(&self, set: &SetPlan, band: usize, nseg: usize) -> Vec<Vec<u8>> {
        let plans = &set.bands[band].plans;
        let seg_len = self.band_len() / nseg;
        let nabits = self.nabits();
        let posbits = seg_len.trailing_zeros();
        let mut segs: Vec<Vec<u8>> = Vec::new();
        for seg in 0..nseg {
            let mut w = BitWriter::new();
            // coding choices per channel
            let mut choice_b: Vec<Code> = Vec::new();
            let mut part_a_len: Vec<usize> = Vec::new();
            for p in plans {
                let a_len = if seg == 0 { p.adapt_order } else { 0 };
                let slice = &p.residual[seg * seg_len..(seg + 1) * seg_len];
                let ub: Vec<u32> = slice[a_len..].iter().map(|v| zigzag(*v)).collect();
                choice_b.push(choose_code_b(&ub, posbits, nabits).0);
                part_a_len.push(a_len);
            }
            write_segment_params(&mut w, plans, &choice_b, &part_a_len, seg, nabits);
            write_segment_data(&mut w, plans, &choice_b, &part_a_len, seg, seg_len, posbits);
            if band == 1 && seg == 0 {
                write_decimator_history(&mut w, set);
            }
            w.align(8);
            segs.push(w.into_bytes());
        }
        segs
    }

    pub(crate) fn set_header(&self, index: usize, set_plan: &SetPlan) -> Vec<u8> {
        let cfg = &self.cfg;
        let set = &cfg.sets[index];
        let nch = set.channels;
        let mut h = BitWriter::new();
        h.put(0, 10); // size placeholder, patched below
        h.put(nch as u32 - 1, 4);
        h.put(if set.residual_of_core { 0 } else { (1u32 << nch) - 1 }, nch as u32);
        h.put(cfg.bit_res - 1, 5);
        h.put(cfg.bit_res - 1, 5);
        h.put(fs_index(cfg.sample_rate), 4);
        h.put(0, 2); // no interpolation
        h.put(0, 2); // not a replacement set
        h.put(u32::from(index == 0), 1); // primary channel set
        write_downmix(&mut h, set);
        h.put(1, 1); // channel mask enabled
        h.put(set.ch_mask, self.mask_bits());
        if cfg.sample_rate > 96_000 {
            h.put(0, 1); // no extra frequency bands beyond two
        }
        for (bi, sp) in set_plan.bands.iter().enumerate() {
            write_band_header(&mut h, sp, nch, bi, set.dmix.is_some());
        }
        // reserved: none; byte align; CRC
        h.align(8);
        let mut hb = h.into_bytes();
        let total = hb.len() + 2;
        hb[0] = ((total - 1) >> 2) as u8; // size - 1 in 10 bits: patch top 8 bits
        hb[1] = (hb[1] & 0x3F) | ((((total - 1) & 3) as u8) << 6);
        let crc = crc16(&hb);
        hb.extend_from_slice(&crc.to_be_bytes());
        hb
    }

    pub(crate) fn mask_bits(&self) -> u32 {
        let m = self.cfg.sets.iter().fold(0u32, |m, s| m | s.ch_mask);
        (32 - m.leading_zeros()).max(6)
    }

    pub(crate) fn assemble(&self, set_plans: &[SetPlan], set_segs: &[Vec<Vec<Vec<u8>>>], nseg: usize) -> Vec<u8> {
        let cfg = &self.cfg;
        let mut hb = Vec::new();
        for (i, sp) in set_plans.iter().enumerate() {
            hb.extend_from_slice(&self.set_header(i, sp));
        }

        // ---- NAVI: one size per segment and channel set
        let max_seg = set_segs.iter().flatten().flatten().map(|s| s.len()).max().unwrap();
        let nb4ssize = (32 - (max_seg as u32 - 1).leading_zeros()).max(1);
        let mut nv = BitWriter::new();
        for band in 0..self.nbands() {
            for seg in 0..nseg {
                for segs in set_segs {
                    nv.put(segs[band][seg].len() as u32 - 1, nb4ssize);
                }
            }
        }
        nv.align(8);
        let mut nb = nv.into_bytes();
        let crc = crc16(&nb);
        nb.extend_from_slice(&crc.to_be_bytes());

        // ---- common header (fixed layout, 14 bytes with the CRC)
        let body_len: usize = set_segs.iter().flatten().flatten().map(|s| s.len()).sum();
        let fbits = 16u32;
        let header_len = (74 + fbits as usize).div_ceil(8) + 2;
        let mut frame_len = header_len + hb.len() + nb.len() + body_len;
        frame_len += (4 - frame_len % 4) % 4;
        let mask_bits = self.mask_bits();
        let mut tmp = BitWriter::new();
        tmp.put(SYNC_XLL, 32);
        tmp.put(0, 4); // version 1
        tmp.put((header_len - 1) as u32, 8);
        tmp.put(fbits - 1, 5);
        tmp.put((frame_len - 1) as u32, fbits);
        tmp.put(cfg.sets.len() as u32 - 1, 4);
        tmp.put(nseg.trailing_zeros(), 4);
        tmp.put((self.band_len() / nseg).trailing_zeros(), 4);
        tmp.put(nb4ssize - 1, 5);
        tmp.put(0, 2); // no band CRC
                       // The original sets this flag when the stream has downmix information, and then adds fields for the
                       // scalable resolution of every channel to the set headers; decoders do not need either, so it stays off.
        tmp.put(0, 1);
        tmp.put(mask_bits - 1, 5);
        tmp.align(8);
        let mut cb = tmp.into_bytes();
        let crc = crc16(&cb[4..]);
        cb.extend_from_slice(&crc.to_be_bytes());
        assert_eq!(cb.len(), header_len);

        let mut out = Vec::with_capacity(frame_len);
        out.extend_from_slice(&cb);
        out.extend_from_slice(&hb);
        out.extend_from_slice(&nb);
        for band in 0..self.nbands() {
            for seg in 0..nseg {
                for segs in set_segs {
                    out.extend_from_slice(&segs[band][seg]);
                }
            }
        }
        out.resize(frame_len, 0);
        out
    }
}

/// The coding parameters of a segment: whether they repeat the last segment's (never), the code of each channel and
/// the parameter of each part.
fn write_segment_params(
    w: &mut BitWriter,
    plans: &[ChannelPlan],
    choice_b: &[Code],
    part_a_len: &[usize],
    seg: usize,
    nabits: u32,
) {
    let reuse = false; // a reused parameter set would keep segment 0's part A length in some decoders
    if seg > 0 {
        w.put(u32::from(reuse), 1);
    }
    if !reuse {
        w.put(0, 1); // distinct parameters per channel
        for c in choice_b {
            // flags are written from choice B (rice vs linear applies to both parts)
            let rice = matches!(c, Code::Rice(_) | Code::Hybrid { .. });
            w.put(u32::from(rice), 1);
            if rice {
                match c {
                    Code::Hybrid { l, .. } => {
                        w.put(1, 1); // hybrid Rice coding
                        w.put(l - 1, nabits);
                    }
                    _ => w.put(0, 1),
                }
            }
        }
        for (i, _) in plans.iter().enumerate() {
            let rice = matches!(choice_b[i], Code::Rice(_) | Code::Hybrid { .. });
            if seg == 0 {
                // part A shares the Rice/linear flag with part B
                let code = recode(&plans[i].residual[..part_a_len[i]], rice);
                w.put(stored_param(code, nabits, rice), nabits);
            }
            w.put(stored_param(choice_b[i], nabits, rice), nabits);
        }
    }
}

/// The residuals of a segment, channel by channel: part A (the samples before the adaptive predictor has history) in
/// segment 0, then part B in the chosen code.
fn write_segment_data(
    w: &mut BitWriter,
    plans: &[ChannelPlan],
    choice_b: &[Code],
    part_a_len: &[usize],
    seg: usize,
    seg_len: usize,
    posbits: u32,
) {
    for (i, p) in plans.iter().enumerate() {
        let rice = matches!(choice_b[i], Code::Rice(_) | Code::Hybrid { .. });
        let slice = &p.residual[seg * seg_len..(seg + 1) * seg_len];
        let a_len = part_a_len[i];
        let code_a = if seg == 0 {
            recode(&slice[..a_len], rice)
        } else {
            Code::Rice(0)
        };
        if seg == 0 {
            put_values(w, &slice[..a_len], code_a);
        }
        if let Code::Hybrid { k, l, c } = choice_b[i] {
            // isolated samples: how many, where, then every sample in order (isolated ones as l-bit values)
            let b: Vec<u32> = slice[a_len..].iter().map(|v| zigzag(*v)).collect();
            let iso = isolated_positions(&b, c as usize);
            w.put(iso.len() as u32, posbits);
            for &pos in &iso {
                w.put(pos as u32, posbits);
            }
            let mut next = 0usize;
            for (j, v) in b.iter().enumerate() {
                if next < iso.len() && iso[next] == j {
                    w.put(*v, l);
                    next += 1;
                } else {
                    put_rice(w, *v, k);
                }
            }
        } else {
            put_values(w, &slice[a_len..], choice_b[i]);
        }
    }
}

/// The decimator history of the extension band: the band-0 samples in front of this frame, as signed values of a
/// common width.
fn write_decimator_history(w: &mut BitWriter, set: &SetPlan) {
    // decimator history: the band-0 samples in front of this frame, as signed values of a common width
    let peak = set
        .history
        .iter()
        .flatten()
        .map(|v| v.unsigned_abs())
        .max()
        .unwrap_or(0);
    let nbits = (33 - peak.leading_zeros()).max(1);
    w.put(nbits - 1, 5);
    for h in &set.history {
        for v in h {
            w.put(
                (*v as u32) & if nbits >= 32 { u32::MAX } else { (1u32 << nbits) - 1 },
                nbits,
            );
        }
    }
}

/// The downmix coefficients of a channel set: embedded in the header, applied by the encoder, or none.
fn write_downmix(h: &mut BitWriter, set: &SetCfg) {
    match (&set.dmix, &set.matrix) {
        (None, Some((kind, rows))) => {
            h.put(1, 1); // downmix coefficients present
            h.put(1, 1); // embedded in the channel set header
            h.put(u32::from(*kind), 3);
            h.put(1, 1); // part of a hierarchy
            for row in rows {
                for c in row {
                    h.put(*c as u32, 9);
                }
            }
        }
        (Some(d), _) => {
            h.put(1, 1); // downmix coefficients present
            h.put(1, 1); // and already applied by the encoder
            h.put(1, 1); // part of a hierarchy
            for row in &d.rows {
                h.put(row.scale_code as u32, 9);
                for c in &row.coeff_codes {
                    h.put(*c as u32, 9);
                }
            }
        }
        (None, None) => {
            h.put(0, 1); // no downmix coefficients
            h.put(1, 1); // part of a hierarchy
        }
    }
}

/// One frequency band of a channel set header: decorrelation, predictor orders and reflection coefficients.
fn write_band_header(h: &mut BitWriter, sp: &BandPlan, nch: usize, bi: usize, has_dmix: bool) {
    h.put(u32::from(sp.decor), 1);
    let order_bits = if nch <= 1 {
        0
    } else {
        32 - (nch as u32 - 1).leading_zeros()
    };
    if sp.decor {
        for o in &sp.orig_order {
            h.put(*o as u32, order_bits);
        }
        for c in &sp.coeffs {
            if *c != 0 {
                h.put(1, 1);
                h.put(signed_to_raw(*c, 7), 7);
            } else {
                h.put(0, 1);
            }
        }
    }
    for p in &sp.plans {
        h.put(p.adapt_order as u32, 4);
    }
    for p in &sp.plans {
        if p.adapt_order == 0 {
            h.put(p.fixed_order as u32, 2);
        }
    }
    for p in &sp.plans {
        for i in &p.rc_idx {
            h.put(signed_to_raw(*i, 8), 8);
        }
    }
    if bi > 0 {
        if has_dmix {
            h.put(0, 1); // downmix not applied in the extension band
        }
        h.put(0, 1); // no MSB/LSB split in the extension band
        h.put(0, 1); // no scalable resolution in the extension band
    }
}
