//! Writing a frame in the bitstream syntax of AC-3 (A/52 clause 5) or E-AC-3 (Annex E).
//!
//! A frame is a header (`syncinfo` and `bsi` in AC-3, the frame header in E-AC-3), six audio blocks and, in AC-3,
//! two CRCs. Both syntaxes carry the same coding decisions; they differ in where the decisions are written: AC-3
//! repeats its bit-allocation and exponent-strategy fields in the blocks, E-AC-3 moves much of it into the frame
//! header. Field names in the comments are the standard's.

use crate::bitalloc::{DBPBCOD, FDCYCOD, FGAINCOD, FLOORCOD, SDCYCOD, SGAINCOD};
use crate::bits::{crc16, crc1_field, BitWriter};
use crate::coupling::write_cpl_exponents;
use crate::exponents::write_exponents;
use crate::mantissas::{quantize, write_mantissas};
use crate::plan::ChannelPlan;
use crate::tables::EAC3_FRM_EXPSTR;
use crate::{Encoder, BLOCKS};

/// Where a block sits in the frame: its index, the set of blocks that share its exponents, and whether it is the
/// first block of that set (the exponents and coupling coordinates are sent there and reused by the others).
#[derive(Clone, Copy)]
struct Block {
    index: usize,
    set: usize,
    new_exps: bool,
}

impl Encoder {
    /// Serialize a frame. Returns (bits used before mantissas and trailer, frame bytes). `snr_code` is the SNR
    /// offset (coarse in the high four bits, fine in the low four). With `measure_only` the mantissas and trailer
    /// are left out: that is how the size of everything else is found.
    pub(crate) fn write_frame(
        &self,
        chans: &[ChannelPlan],
        frmsizecod: u32,
        words: usize,
        snr_code: u32,
        measure_only: bool,
    ) -> (usize, Vec<u8>) {
        let mut w = BitWriter::new();
        if self.cfg.eac3 {
            self.write_eac3_header(&mut w, chans, words, snr_code);
        } else {
            self.write_ac3_header(&mut w, frmsizecod);
        }
        let baps: Vec<Vec<Vec<u8>>> = if measure_only {
            Vec::new()
        } else {
            chans
                .iter()
                .map(|cp| (0..self.cur_sets.len()).map(|s| self.baps(cp, s, snr_code)).collect())
                .collect()
        };
        for index in 0..BLOCKS {
            let set = self
                .cur_sets
                .iter()
                .position(|&(first, count)| index >= first && index < first + count)
                .unwrap();
            let block = Block {
                index,
                set,
                new_exps: self.cur_sets[set].0 == index,
            };
            self.write_block(&mut w, chans, block, snr_code);
            if !measure_only {
                self.write_block_mantissas(&mut w, chans, &baps, block);
            }
        }
        let side_bits = w.bit_len();
        if measure_only {
            return (side_bits, Vec::new());
        }
        (side_bits, self.finish_frame(w, words))
    }

    /// AC-3 `syncinfo` and `bsi`.
    fn write_ac3_header(&self, w: &mut BitWriter, frmsizecod: u32) {
        let cfg = &self.cfg;
        // syncinfo
        w.put(0x0B77, 16);
        w.put(0, 16); // crc1, patched when the frame is finished
        w.put(self.fscod as u32, 2);
        w.put(frmsizecod, 6);
        // bsi
        w.put(8, 5); // bsid
        w.put(0, 3); // bsmod
        w.put(cfg.acmod as u32, 3);
        if cfg.acmod & 1 != 0 && cfg.acmod != 1 {
            w.put(0, 2); // cmixlev -3 dB
        }
        if cfg.acmod & 4 != 0 {
            w.put(0, 2); // surmixlev -3 dB
        }
        if cfg.acmod == 2 {
            w.put(if cfg.dolby_surround { 2 } else { 0 }, 2);
        }
        w.put(u32::from(cfg.lfe), 1);
        w.put(cfg.dialnorm.clamp(1, 31) as u32, 5);
        w.put(0, 1); // compre
        w.put(0, 1); // langcode
        w.put(0, 1); // audprodie
        w.put(0, 1); // copyrightb
        w.put(1, 1); // origbs
        w.put(0, 1); // timecod1e
        w.put(0, 1); // timecod2e
        w.put(0, 1); // addbsie
    }

    /// The E-AC-3 frame header: `syncinfo`, `bsi` and the audio frame header, with the frame-level exponent
    /// strategies and the single SNR offset.
    fn write_eac3_header(&self, w: &mut BitWriter, chans: &[ChannelPlan], words: usize, snr_code: u32) {
        let cfg = &self.cfg;
        // The channels in the order the exponent strategies are listed: the coupling channel first.
        let exp_chans: Vec<&ChannelPlan> = chans
            .iter()
            .filter(|c| c.is_cpl)
            .chain(chans.iter().filter(|c| !c.is_cpl).take(self.nfch))
            .collect();
        let frame_expstr = self.frame_exponent_strategies(&exp_chans);
        let cpl_off = usize::from(self.cur_coupling.is_some());
        let (csnr, fsnr) = (snr_code >> 4, snr_code & 15);

        w.put(0x0B77, 16);
        w.put(u32::from(self.dependent.is_some()), 2); // strmtyp: independent (0) or dependent (1)
        w.put(0, 3); // substreamid
        w.put(words as u32 - 1, 11);
        w.put(self.fscod as u32, 2);
        w.put(3, 2); // numblkscod: six blocks
        w.put(cfg.acmod as u32, 3);
        w.put(u32::from(cfg.lfe), 1);
        w.put(16, 5); // bsid
        w.put(cfg.dialnorm.clamp(1, 31) as u32, 5);
        w.put(0, 1); // compre
        if let Some(map) = self.dependent {
            w.put(1, 1); // chanmape
            w.put(map as u32, 16);
        }
        w.put(0, 1); // mixmdate
        w.put(0, 1); // infomdate
        w.put(0, 1); // addbsie
                     // audio frame header
        w.put(u32::from(frame_expstr.is_none()), 1); // expstre: per-block strategies
        w.put(0, 1); // ahte
        w.put(0, 2); // snroffststr: one offset for the frame
        w.put(0, 1); // transproce
        w.put(0, 1); // blkswe
        w.put(1, 1); // dithflage: per-block dither flags follow (all zero)
        w.put(0, 1); // bamode
        w.put(0, 1); // frmfgaincode
        w.put(0, 1); // dbaflde
        w.put(0, 1); // skipflde
        w.put(0, 1); // spxattene
        if cfg.acmod > 1 {
            w.put(u32::from(self.cur_coupling.is_some()), 1); // cplinu, block 0
            for _ in 1..BLOCKS {
                w.put(0, 1); // cplstre: later blocks keep the first block's choice
            }
        }
        match &frame_expstr {
            Some(indexes) => {
                for i in indexes {
                    w.put(*i, 5); // frmcplexpstr, frmchexpstr
                }
            }
            None => {
                for blk in 0..BLOCKS {
                    let new_exps = self.cur_sets.iter().any(|&(f, _)| f == blk);
                    for cp in &exp_chans {
                        w.put(if new_exps { cp.strat.code() } else { 0 }, 2); // cplexpstr, chexpstr
                    }
                }
            }
        }
        if cfg.lfe {
            for blk in 0..BLOCKS {
                w.put(u32::from(self.cur_sets.iter().any(|&(f, _)| f == blk)), 1);
                // lfeexpstr
            }
        }
        if self.dependent.is_none() {
            for ch in 0..self.nfch {
                // convexpstr (independent substreams only)
                w.put(frame_expstr.as_ref().map_or(0, |i| i[ch + cpl_off]), 5);
            }
        }
        w.put(csnr, 6);
        w.put(fsnr, 4);
        w.put(0, 1); // blkstrtinfoe
    }

    /// Frame-level exponent strategy (E-AC-3 only): usable when every channel's six-block pattern appears in the
    /// standard's table of 32 combinations. Gives the table index of each channel, or `None` when one is missing.
    fn frame_exponent_strategies(&self, exp_chans: &[&ChannelPlan]) -> Option<Vec<u32>> {
        let pattern = |cp: &ChannelPlan| -> [u8; 6] {
            let mut p = [0u8; 6];
            for &(first, _) in &self.cur_sets {
                p[first] = cp.strat.code() as u8;
            }
            p
        };
        exp_chans
            .iter()
            .map(|cp| EAC3_FRM_EXPSTR.iter().position(|r| *r == pattern(cp)).map(|i| i as u32))
            .collect()
    }

    /// Everything of an audio block but its mantissas.
    fn write_block(&self, w: &mut BitWriter, chans: &[ChannelPlan], block: Block, snr_code: u32) {
        let eac3 = self.cfg.eac3;
        if !eac3 {
            for ch in 0..self.nfch {
                w.put(u32::from(self.cur_blksw[ch][block.index]), 1); // blksw
            }
        }
        // dither off: zero-bit bins decode as silence rather than noise (explicit in both
        // syntaxes; E-AC-3 enables the per-block flags through dithflage in the frame header)
        for _ in 0..self.nfch {
            w.put(0, 1); // dithflag
        }
        w.put(0, 1); // dynrnge
        self.write_coupling_strategy(w, block);
        self.write_coupling_coordinates(w, block);
        self.write_rematrix_flags(w, block);
        if !eac3 {
            self.write_exponent_strategies(w, chans, block);
        }
        if block.new_exps {
            self.write_exponents_of_block(w, chans, block);
        }
        if eac3 {
            self.write_eac3_leak_info(w, block);
        } else {
            self.write_ac3_bit_allocation(w, chans, block, snr_code);
        }
    }

    /// Whether and how channels are coupled: `cplstre`, `cplinu` and the coupling band structure.
    fn write_coupling_strategy(&self, w: &mut BitWriter, block: Block) {
        let acmod = self.cfg.acmod;
        let nfch = self.nfch;
        if self.cfg.eac3 {
            w.put(0, 1); // spxinu
            if let (0, Some(c)) = (block.index, &self.cur_coupling) {
                w.put(0, 1); // ecplinu: plain coupling
                if acmod > 2 {
                    for _ in 0..nfch {
                        w.put(1, 1); // chincpl (2/0 couples both channels without saying so)
                    }
                }
                if acmod == 2 {
                    w.put(1, 1); // phsflginu
                }
                w.put(c.begf as u32, 4);
                w.put(c.endf as u32, 4);
                w.put(1, 1); // cplbndstrce: send the band structure rather than take the default
                for _ in 1..3 + c.endf - c.begf {
                    w.put(0, 1); // every 12-bin sub-band is a band of its own
                }
            }
        } else if block.index == 0 {
            w.put(1, 1); // cplstre
            match &self.cur_coupling {
                Some(c) => {
                    w.put(1, 1); // cplinu
                    for _ in 0..nfch {
                        w.put(1, 1); // chincpl
                    }
                    if acmod == 2 {
                        w.put(1, 1); // phsflginu
                    }
                    w.put(c.begf as u32, 4);
                    w.put(c.endf as u32, 4);
                    for _ in 1..3 + c.endf - c.begf {
                        w.put(0, 1); // every 12-bin sub-band is a band of its own
                    }
                }
                None => w.put(0, 1), // cplinu
            }
        } else {
            w.put(0, 1); // cplstre
        }
    }

    /// The coupling coordinates (`cplcoe`, `mstrcplco`, `cplcoexp`, `cplcomant`) and phase flags.
    fn write_coupling_coordinates(&self, w: &mut BitWriter, block: Block) {
        let Some(c) = &self.cur_coupling else { return };
        for ch in 0..self.nfch {
            // E-AC-3 takes the first coordinates of a frame as sent without a flag
            if !(self.cfg.eac3 && block.index == 0) {
                w.put(u32::from(block.new_exps), 1); // cplcoe
            }
            if block.new_exps {
                let (master, bands) = &c.coords[block.set][ch];
                w.put(*master, 2);
                for (exp, mantissa) in bands {
                    w.put(*exp, 4);
                    w.put(*mantissa, 4);
                }
            }
        }
        if self.cfg.acmod == 2 && block.new_exps {
            for &flag in &c.phase[block.set] {
                w.put(u32::from(flag), 1); // phsflg
            }
        }
    }

    /// Stereo rematrixing flags (2/0 mode only): `rematstr` and `rematflg`.
    fn write_rematrix_flags(&self, w: &mut BitWriter, block: Block) {
        if self.cfg.acmod != 2 {
            return;
        }
        let blk = block.index;
        if self.cfg.eac3 {
            if blk == 0 {
                w.put(0, 4); // no band rematrixed
            } else {
                w.put(0, 1);
            }
        } else if blk == 0 || self.cur_remat[blk] != self.cur_remat[blk - 1] {
            w.put(1, 1); // rematstr
            for band in 0..4 {
                w.put(u32::from(self.cur_remat[blk][band]), 1);
            }
        } else {
            w.put(0, 1);
        }
    }

    /// AC-3 exponent strategies of a block: `cplexpstr`, `chexpstr` and `lfeexpstr` (zero reuses the previous
    /// block's exponents).
    fn write_exponent_strategies(&self, w: &mut BitWriter, chans: &[ChannelPlan], block: Block) {
        let strat = |cp: &ChannelPlan| {
            if block.new_exps {
                cp.sets[block.set].strat.code()
            } else {
                0
            }
        };
        if self.cur_coupling.is_some() {
            w.put(strat(chans.last().unwrap()), 2); // cplexpstr
        }
        for cp in chans.iter().take(self.nfch) {
            w.put(strat(cp), 2); // chexpstr
        }
        if self.cfg.lfe {
            w.put(u32::from(block.new_exps), 1); // lfeexpstr
        }
    }

    /// The exponents of a block that starts a set: bandwidth codes (or the coupling channel's exponents), then every
    /// channel's exponents and gain range.
    fn write_exponents_of_block(&self, w: &mut BitWriter, chans: &[ChannelPlan], block: Block) {
        let set = block.set;
        if self.cur_coupling.is_none() {
            for cp in chans.iter().take(self.nfch) {
                w.put(((cp.end - 73) / 3) as u32, 6); // chbwcod (coupled channels have none)
            }
        } else {
            let coupling = chans.last().unwrap();
            write_cpl_exponents(w, &coupling.sets[set].virt, coupling.sets[set].strat);
        }
        for cp in chans.iter().filter(|c| !c.is_cpl) {
            write_exponents(w, &cp.sets[set].exp, cp.sets[set].strat, cp.end);
            if !cp.is_lfe {
                w.put(0, 2); // gainrng
            }
        }
    }

    /// E-AC-3 bit-allocation fields of a block: nothing is sent except the coupling leak initialisation.
    fn write_eac3_leak_info(&self, w: &mut BitWriter, block: Block) {
        if self.dependent.is_none() {
            w.put(0, 1); // convsnroffste (independent substreams only)
        }
        if self.cur_coupling.is_some() {
            if block.index == 0 {
                w.put(0, 3); // cplfleak (cplleake is implied in a frame's first coupling block)
                w.put(0, 3); // cplsleak
            } else {
                w.put(0, 1); // cplleake
            }
        }
    }

    /// AC-3 bit-allocation fields of a block: the fixed parameters and the SNR offsets in block 0, the coupling
    /// leak values, and no delta allocation or skip field.
    fn write_ac3_bit_allocation(&self, w: &mut BitWriter, chans: &[ChannelPlan], block: Block, snr_code: u32) {
        let (csnr, fsnr) = (snr_code >> 4, snr_code & 15);
        if block.index == 0 {
            w.put(1, 1); // baie
            w.put(SDCYCOD as u32, 2);
            w.put(FDCYCOD as u32, 2);
            w.put(SGAINCOD as u32, 2);
            w.put(DBPBCOD as u32, 2);
            w.put(FLOORCOD as u32, 3);
            w.put(1, 1); // snroffste
            w.put(csnr, 6);
            if self.cur_coupling.is_some() {
                w.put(fsnr, 4); // cplfsnroffst
                w.put(FGAINCOD as u32, 3); // cplfgaincod
            }
            for _ in 0..chans.iter().filter(|c| !c.is_cpl).count() {
                w.put(fsnr, 4);
                w.put(FGAINCOD as u32, 3);
            }
        } else {
            w.put(0, 1); // baie
            w.put(0, 1); // snroffste
        }
        if self.cur_coupling.is_some() {
            if block.index == 0 {
                w.put(1, 1); // cplleake
                w.put(0, 3); // cplfleak
                w.put(0, 3); // cplsleak
            } else {
                w.put(0, 1); // cplleake
            }
        }
        w.put(0, 1); // deltbaie
        w.put(0, 1); // skiple
    }

    /// A block's mantissas: collect the quantized values, then emit them with the grouped quantizers (bap 1, 2, 4)
    /// placed where the decoder expects them.
    fn write_block_mantissas(&self, w: &mut BitWriter, chans: &[ChannelPlan], baps: &[Vec<Vec<u8>>], block: Block) {
        let (blk, set) = (block.index, block.set);
        let mut items: Vec<(usize, i32)> = Vec::new();
        let collect = |items: &mut Vec<(usize, i32)>, ch: usize, cp: &ChannelPlan| {
            let bap = &baps[ch][set];
            let exp = &cp.sets[set].exp;
            for bin in cp.start..cp.end {
                let b = bap[bin] as usize;
                if b == 0 {
                    continue;
                }
                let m =
                    (cp.coef[blk][bin] * f32::from_bits(((127 + exp[bin]) as u32) << 23)).clamp(-0.999_99, 0.999_99);
                items.push((b, quantize(b, m)));
            }
        };
        let cpl_index = chans.len() - 1;
        for (ch, cp) in chans.iter().enumerate() {
            if cp.is_cpl {
                continue;
            }
            collect(&mut items, ch, cp);
            // the coupling channel's mantissas follow the first coupled channel's
            if ch == 0 && self.cur_coupling.is_some() {
                collect(&mut items, cpl_index, &chans[cpl_index]);
            }
        }
        write_mantissas(w, &items);
    }

    /// Pads the frame to its size and adds the CRCs (AC-3: `crc1` over the first five eighths, `crc2` over all).
    fn finish_frame(&self, w: BitWriter, words: usize) -> Vec<u8> {
        let total = words * 16;
        let mut bytes = w.finish_padded(total - 16);
        bytes.extend_from_slice(&[0, 0]);
        let frame_len = total / 8;
        if !self.cfg.eac3 {
            // crc1 covers the first 5/8 of the frame and must be in place before crc2 is computed,
            // because crc2 covers everything after the sync word, crc1 included.
            let five_eighths = ((frame_len >> 2) + (frame_len >> 4)) << 1;
            let after = crc16(&bytes[4..five_eighths]);
            let crc1 = crc1_field(after, five_eighths - 2);
            bytes[2..4].copy_from_slice(&crc1.to_be_bytes());
        }
        let mut crc2 = crc16(&bytes[2..frame_len - 2]);
        if crc2 == 0x0B77 {
            // never let the CRC imitate a sync word: flip the reserved bit (x^16+x^15+x^2+1 xor
            // 0x8005 leaves the residual unchanged)
            bytes[frame_len - 3] ^= 1;
            crc2 ^= 0x8005;
        }
        bytes[frame_len - 2..].copy_from_slice(&crc2.to_be_bytes());
        bytes
    }
}
