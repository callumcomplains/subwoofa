//! Writing the chunks of a channel pair (or a lone channel): the order a decoder parses them in.

use crate::express::bits::{put_code, BitsLe};
use crate::express::channel::{ChanCoding, Grids};
use crate::express::encoder::{chunk, LbrEncoder};
use crate::express::filterbank::TS;
use crate::express::levels::{alphabet, levels_from_profile, nearest};

/// A channel pair (or a lone channel) after quantisation: everything its four chunks are written from.
struct PairCoding {
    /// First and second channel of the pair (the same for a lone channel).
    ch1: usize,
    ch2: usize,
    stereo: bool,
    /// Bands that are written, counted in coding order.
    kw: usize,
    /// Band at each coding position.
    ord: Vec<usize>,
    levels: Vec<usize>,
    profile: u32,
    c1: ChanCoding,
    c2: Option<ChanCoding>,
}

impl PairCoding {
    /// Which of the three LPC bands the channel uses.
    fn lpc_used(&self, ch: usize) -> [bool; 3] {
        if ch == self.ch1 {
            self.c1.lpc_used
        } else {
            self.c2.as_ref().map_or([false; 3], |c| c.lpc_used)
        }
    }
}

impl LbrEncoder {
    /// The four chunks of one channel pair (or of a lone channel).
    pub(crate) fn write_pair(&self, pair: usize, ts: &[Vec<Vec<f64>>], profile: u32, k: usize) -> Vec<Vec<u8>> {
        self.write_pair_ex(pair, ts, profile, k, false).0
    }

    /// `muted`: emergency mode, every coded sample becomes the zero symbol (keeps the stream legal when even
    /// the cheapest setting does not fit the frame).
    pub(crate) fn write_pair_ex(
        &self,
        pair: usize,
        ts: &[Vec<Vec<f64>>],
        profile: u32,
        k: usize,
        muted: bool,
    ) -> (Vec<Vec<u8>>, f64, Vec<(usize, [[f64; 8]; 3], [bool; 3])>) {
        let coding = self.code_pair(pair, ts, profile, k, muted);
        let pid = pair as u8;
        let chunks = vec![
            chunk(0x30 + pid, self.grid1_chunk(&coding)),
            chunk(0x40 + pid, self.high_resolution_chunk(&coding)),
            chunk(0x50 + pid, self.samples1_chunk(&coding)),
            chunk(0x60 + pid, self.samples2_chunk(&coding)),
        ];
        let (c1, c2) = (&coding.c1, &coding.c2);
        let err = c1.err + c2.as_ref().map_or(0.0, |c| c.err);
        let tails: Vec<(usize, [[f64; 8]; 3], [bool; 3])> = std::iter::once((coding.ch1, c1.tails, c1.lpc_used))
            .chain(c2.as_ref().map(|c| (coding.ch2, c.tails, c.lpc_used)))
            .collect();
        (chunks, err, tails)
    }

    /// Quantises the channels of a pair: picks the bands' quantizer levels and codes each channel.
    fn code_pair(&self, pair: usize, ts: &[Vec<Vec<f64>>], profile: u32, k: usize, muted: bool) -> PairCoding {
        let ch1 = pair * 2;
        let ch2 = (ch1 + 1).min(self.cfg.channels - 1);
        let stereo = ch1 != ch2;
        let geo = self.geo;
        let nsb = geo.nsb;
        let k = k.min(nsb);
        // the strongest of the first eight bands gets the finest quantizer (the low bits of the profile name it)
        let energy = |b: usize| -> f64 { (ch1..=ch2).map(|c| ts[c][b].iter().map(|v| v * v).sum::<f64>()).sum() };
        let loudest = (0..8.min(nsb))
            .max_by(|&a, &b| energy(a).partial_cmp(&energy(b)).unwrap())
            .unwrap_or(0);
        let profile = (profile & !7) | loudest as u32;
        let levels = levels_from_profile(profile, nsb, geo.limited_rate);
        let kw = if stereo { k.max(geo.max_mono) } else { k };
        // a lone channel codes its bands in order of energy (the quantizer levels fall with the position, so the
        // strongest bands get the finest ones); a pair keeps them in order
        let ord = if stereo {
            (0..nsb).collect::<Vec<usize>>()
        } else {
            self.band_order(&ts[ch1])
        };
        let mut pos = vec![0usize; nsb];
        for (p, &b) in ord.iter().enumerate() {
            pos[b] = p;
        }
        // the second grid stays flat: its attenuation steps did not pay for their bits
        let g2_for = |_ch: usize| -> Vec<[u8; 64]> { vec![[0u8; 64]; geo.ng2] };
        let mut c1 = self.code_channel(ch1, &ts[ch1], &levels, &pos, k, kw, g2_for(ch1));
        let mut c2 = if stereo {
            Some(self.code_channel(ch2, &ts[ch2], &levels, &pos, k, kw, g2_for(ch2)))
        } else {
            None
        };
        if muted {
            for c in std::iter::once(&mut c1).chain(c2.as_mut()) {
                for sb in 0..nsb {
                    let zero = nearest(&alphabet(levels[pos[sb]]), 0.0) as i32;
                    for slot in c.q[sb].iter_mut() {
                        for v in slot.iter_mut() {
                            *v = zero;
                        }
                    }
                }
            }
        }
        PairCoding {
            ch1,
            ch2,
            stereo,
            kw,
            ord,
            levels,
            profile,
            c1,
            c2,
        }
    }

    /// Chunk 0x30: first-grid scale factors and the third grid's averages of the first channel.
    fn grid1_chunk(&self, pc: &PairCoding) -> BitsLe {
        let (geo, nsb) = (self.geo, self.geo.nsb);
        let mut w = BitsLe::new();
        for sb in 2..geo.ng1 {
            self.put_scale_factors(&mut w, &pc.c1.grids.g1[sb]);
        }
        for sb in 0..nsb - 4 {
            self.t.avg_g3.put(&mut w, (pc.c1.grids.avg[sb] + 16) as u32);
        }
        if pc.stereo {
            w.put(0, 4); // min_v of both channels
            w.put(0, 4);
            for _ in 0..(nsb - geo.min_mono).div_ceil(4) * 2 * 4 {
                self.t.st_grid.put(&mut w, 0); // neutral stereo image
            }
        }
        w.put(0, 24);
        w
    }

    /// Chunk 0x40: the profile, the LPC codes and the first two (highest resolution) bands.
    fn high_resolution_chunk(&self, pc: &PairCoding) -> BitsLe {
        let mut w = BitsLe::new();
        w.put(pc.profile, 8);
        {
            let plan = self.lpc_plan.read().unwrap();
            for sb in 0..2 {
                for ch in pc.ch1..=pc.ch2 {
                    for set in 0..2 {
                        for code in if pc.lpc_used(ch)[sb] {
                            plan[ch].codes[sb][set]
                        } else {
                            [8u8; 8]
                        } {
                            w.put(code as u32, 4);
                        }
                    }
                }
            }
        }
        for sb in 0..2 {
            if pc.stereo {
                w.put(0, 8); // secondary channel flags: no mid/side
            }
            self.put_ts(&mut w, &pc.c1.q[sb], pc.levels[sb]);
            if let Some(c2) = &pc.c2 {
                self.put_ts(&mut w, &c2.q[sb], pc.levels[sb]);
            }
        }
        for sb in 0..2 {
            self.put_scale_factors(&mut w, &pc.c1.grids.g1[sb]);
            if let Some(c2) = &pc.c2 {
                self.put_scale_factors(&mut w, &c2.grids.g1[sb]);
            }
        }
        w.put(0, 24);
        w
    }

    /// Chunk 0x50: bands 2 to 5 of the first channel.
    fn samples1_chunk(&self, pc: &PairCoding) -> BitsLe {
        let geo = self.geo;
        let mut w = BitsLe::new();
        {
            let plan = self.lpc_plan.read().unwrap();
            for ch in pc.ch1..=pc.ch2 {
                for code in if pc.lpc_used(ch)[2] {
                    plan[ch].codes[2][0]
                } else {
                    [8u8; 8]
                } {
                    w.put(code as u32, 4);
                }
            }
        }
        for sb in 2..4 {
            if pc.stereo {
                w.put(0, 8);
            }
            self.put_ts(&mut w, &pc.c1.q[sb], pc.levels[sb]);
        }
        self.put_grid2(&mut w, &pc.c1.g2, 0, 1.min(geo.ng2));
        for sb in 4..6 {
            self.put_g3(&mut w, &pc.c1.grids, sb);
            if pc.stereo {
                w.put(0, 8);
            }
            self.put_ts(&mut w, &pc.c1.q[sb], pc.levels[sb]);
        }
        w.put(0, 24);
        w
    }

    /// Chunk 0x60: the remaining bands, and for a pair the second channel's grids and samples.
    fn samples2_chunk(&self, pc: &PairCoding) -> BitsLe {
        let (geo, nsb, kw) = (self.geo, self.geo.nsb, pc.kw);
        let (c1, levels) = (&pc.c1, &pc.levels);
        let mut w = BitsLe::new();
        self.put_grid2(&mut w, &c1.g2, 1, 3.min(geo.ng2));
        // third grid scale factors are sent the first time a band shows up (bands 4 and 5 came with the chunk before),
        // and all that are still missing at position 12
        let mut pres = vec![false; nsb];
        for b in 4..6.min(nsb) {
            pres[b] = true;
        }
        for p in 6..geo.max_mono.min(kw) {
            let sb = pc.ord[p];
            w.put(sb as u32, geo.idx_bits);
            if p == 12 {
                for sg in 0..self.g3_avg_only_start().saturating_sub(4) {
                    if !pres[sg + 4] {
                        self.put_g3(&mut w, &c1.grids, sg + 4);
                        pres[sg + 4] = true;
                    }
                }
            } else if p < 12 && sb >= 4 && !pres[sb] {
                self.put_g3(&mut w, &c1.grids, sb);
                pres[sb] = true;
            }
            if pc.stereo {
                w.put(0, 8);
            }
            self.put_ts(&mut w, &c1.q[sb], levels[p]);
        }
        if let Some(c2) = &pc.c2 {
            self.put_second_channel(&mut w, pc, c2);
        } else {
            for p in geo.max_mono..kw {
                let sb = pc.ord[p];
                w.put(sb as u32, geo.idx_bits);
                self.put_ts(&mut w, &c1.q[sb], levels[p]);
            }
        }
        w.put(0, 24);
        w
    }

    /// The secondary channel of a pair: first grid scale factors and third grid averages, then a second pass over
    /// the bands (the secondary channel of the mono region, both channels above it).
    fn put_second_channel(&self, w: &mut BitsLe, pc: &PairCoding, c2: &ChanCoding) {
        let (geo, nsb, kw) = (self.geo, self.geo.nsb, pc.kw);
        let (c1, levels) = (&pc.c1, &pc.levels);
        for sb in 2..geo.ng1 {
            self.put_scale_factors(w, &c2.grids.g1[sb]);
        }
        for sb in 0..nsb - 4 {
            self.t.avg_g3.put(w, (c2.grids.avg[sb] + 16) as u32);
        }
        self.put_grid2(w, &c2.g2, 0, 3.min(geo.ng2));
        for sb in geo.min_mono..kw {
            let in_mono = sb < geo.max_mono;
            if sb >= 6 && !in_mono {
                w.put(sb as u32, geo.idx_bits);
            }
            if sb == 12 {
                for sg in 0..self.g3_avg_only_start() - 4 {
                    if sg + 4 > 11 {
                        self.put_g3(w, &c2.grids, sg + 4);
                    }
                }
            } else if (4..12).contains(&sb) {
                self.put_g3(w, &c2.grids, sb);
            }
            if !in_mono {
                w.put(0, 8); // sbms
            }
            w.put(0, 8); // lrms
            if in_mono {
                self.put_ts(w, &c2.q[sb], levels[sb]);
            } else {
                self.put_ts(w, &c1.q[sb], levels[sb]);
                self.put_ts(w, &c2.q[sb], levels[sb]);
            }
        }
    }

    pub(crate) fn put_scale_factors(&self, w: &mut BitsLe, vals: &[i32; 8]) {
        // an explicit value for every slot: distance 1 steps
        self.t.fst_rsd_amp.put(w, vals[0].clamp(0, 55) as u32);
        let mut prev = vals[0].clamp(0, 55);
        for sf in 0..7 {
            self.t.rsd_apprx.put(w, 0); // distance 1
            let next = vals[sf + 1].clamp(0, 55);
            let d = next - prev;
            let code = if d > 0 { 2 * d - 1 } else { -2 * d };
            self.t.rsd_amp.put(w, code as u32);
            prev = next;
        }
    }

    /// Second grid, groups `from..to`: a flag for each run of eight steps, then the steps themselves.
    pub(crate) fn put_grid2(&self, w: &mut BitsLe, g2: &[[u8; 64]], from: usize, to: usize) {
        for g in from..to.min(g2.len()) {
            for run in g2[g].chunks(8) {
                if run.iter().any(|s| *s != 0) {
                    w.put(1, 1);
                    for s in run {
                        self.t.grid_2.put(w, *s as u32);
                    }
                } else {
                    w.put(0, 1); // whole group zero
                }
            }
        }
    }

    pub(crate) fn put_g3(&self, w: &mut BitsLe, g: &Grids, sb: usize) {
        for i in 0..8 {
            self.t.grid_3.put(w, (g.g3[sb][i] + 16) as u32);
        }
    }

    /// Time samples of one band: coding method bit, then the samples in the level's packing.
    pub(crate) fn put_ts(&self, w: &mut BitsLe, slots: &[Vec<i32>], level: usize) {
        let samples: Vec<i32> = slots.iter().flatten().copied().collect();
        debug_assert_eq!(samples.len(), TS);
        // three-level bands that are mostly zero are cheaper as a flag per sample (and a sign for the others):
        // 128 + nonzero bits against 205 bits for five samples in eight
        if level == 2 {
            let nonzero = samples.iter().filter(|s| **s != 1).count();
            if 128 + nonzero < 205 {
                w.put(1, 1);
                for s in &samples {
                    if *s == 1 {
                        w.put(0, 1);
                    } else {
                        w.put(1, 1);
                        w.put(u32::from(*s == 2), 1); // the level-three alphabet is -, 0, +
                    }
                }
                return;
            }
        }
        w.put(0, 1); // coding method
        match level {
            1 => {
                for blk in samples.chunks(8) {
                    let mut code = 0u32;
                    for (j, s) in blk.iter().enumerate() {
                        code |= (*s as u32 & 1) << j;
                    }
                    w.put(code, 8);
                }
            }
            2 => {
                for blk in samples.chunks(5) {
                    let mut idx = 0usize;
                    for (j, s) in blk.iter().enumerate() {
                        idx += *s as usize * 3usize.pow(j as u32);
                    }
                    for j in blk.len()..5 {
                        idx += 3usize.pow(j as u32); // pad with the zero level
                    }
                    w.put(self.t.pack5[idx].expect("5-in-8 code") as u32, 8);
                }
            }
            3 => {
                for blk in samples.chunks(3) {
                    let mut v = [2usize; 3];
                    for (j, s) in blk.iter().enumerate() {
                        v[j] = *s as usize;
                    }
                    w.put(
                        self.t.pack3[v[0] + 5 * v[1] + 25 * v[2]].expect("3-in-7 code") as u32,
                        7,
                    );
                }
            }
            4 => {
                for s in &samples {
                    let (code, len) = self.t.rsd[*s as usize].expect("rsd code");
                    put_code(w, code, len);
                }
            }
            _ => {
                for s in &samples {
                    w.put(*s as u32, 4);
                }
            }
        }
    }
}
