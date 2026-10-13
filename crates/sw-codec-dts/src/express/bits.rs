//! Bit-level output of LBR streams: a least-significant-bit-first writer and the variable length code tables.

use crate::express::tables::{
    HUFF_AVG_G3, HUFF_FST_RSD_AMP, HUFF_GRID_2, HUFF_GRID_3, HUFF_RSD, HUFF_RSD_AMP, HUFF_RSD_APPRX, HUFF_ST_GRID,
    RSD_PACK_3_IN_7, RSD_PACK_5_IN_8,
};

/// Bit writer, least significant bit first (the bit order of LBR streams).
#[derive(Default, Clone)]
pub struct BitsLe {
    pub(crate) bytes: Vec<u8>,
    pub(crate) cur: u64,
    pub(crate) n: u32,
}

impl BitsLe {
    pub fn new() -> BitsLe {
        BitsLe::default()
    }

    pub fn put(&mut self, v: u32, bits: u32) {
        debug_assert!(bits <= 32);
        if bits == 0 {
            return;
        }
        let v = if bits == 32 {
            v as u64
        } else {
            (v as u64) & ((1u64 << bits) - 1)
        };
        self.cur |= v << self.n;
        self.n += bits;
        while self.n >= 8 {
            self.bytes.push(self.cur as u8);
            self.cur >>= 8;
            self.n -= 8;
        }
    }

    pub fn bit_len(&self) -> usize {
        self.bytes.len() * 8 + self.n as usize
    }

    pub fn into_bytes(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.bytes.push(self.cur as u8);
        }
        self.bytes
    }
}

/// Canonical code of a table given as (symbol, length) in code order (the first entry gets the all-zero
/// code; every next code is the previous plus one, shifted to the length of the entry). Returns the
/// (code, length) of every symbol (the shortest one when a symbol appears twice).
pub(crate) fn codes_of(table: &[(u8, u8)], symbols: usize) -> Vec<Option<(u32, u8)>> {
    let mut out: Vec<Option<(u32, u8)>> = vec![None; symbols];
    let mut code = 0u32;
    let mut prev_len = table[0].1;
    for (i, &(sym, len)) in table.iter().enumerate() {
        if i > 0 {
            code += 1;
            if len > prev_len {
                code <<= len - prev_len;
            } else {
                code >>= prev_len - len;
            }
        }
        prev_len = len;
        let slot = &mut out[sym as usize];
        if slot.is_none_or(|(_, l)| len < l) {
            *slot = Some((code, len));
        }
    }
    out
}

/// A variable length code table whose decoded value is `symbol - 1` (symbol 0 is the escape).
pub(crate) struct Vlc {
    pub(crate) codes: Vec<Option<(u32, u8)>>,
}

impl Vlc {
    pub(crate) fn new(table: &[(u8, u8)]) -> Vlc {
        let n = table.iter().map(|t| t.0 as usize).max().unwrap() + 1;
        Vlc {
            codes: codes_of(table, n),
        }
    }

    /// Write the value `v` (what the decoder returns, 0 or more).
    pub(crate) fn put(&self, w: &mut BitsLe, v: u32) {
        let sym = v as usize + 1;
        if let Some(Some((code, len))) = self.codes.get(sym) {
            put_code(w, *code, *len);
            return;
        }
        // escape: symbol 0, then a 3-bit length and the raw value
        let (code, len) = self.codes[0].expect("escape code");
        put_code(w, code, len);
        let nbits = (32 - v.leading_zeros()).max(1);
        w.put(nbits - 1, 3);
        w.put(v, nbits);
    }
}

/// Codes are defined most significant bit first; they are written bit by bit.
pub(crate) fn put_code(w: &mut BitsLe, code: u32, len: u8) {
    for i in (0..len).rev() {
        w.put((code >> i) & 1, 1);
    }
}

pub(crate) struct Tables {
    pub(crate) fst_rsd_amp: Vlc,
    pub(crate) rsd_apprx: Vlc,
    pub(crate) rsd_amp: Vlc,
    pub(crate) avg_g3: Vlc,
    pub(crate) st_grid: Vlc,
    pub(crate) grid_3: Vlc,
    pub(crate) grid_2: Vlc,
    pub(crate) rsd: Vec<Option<(u32, u8)>>,
    pub(crate) pack5: Vec<Option<u8>>,
    pub(crate) pack3: Vec<Option<u8>>,
}

impl Tables {
    pub(crate) fn new() -> Tables {
        // reverse maps of the packed sample codes
        let mut pack5 = vec![None; 243];
        for (code, v) in RSD_PACK_5_IN_8.iter().enumerate() {
            let mut idx = 0usize;
            let mut valid = true;
            for j in 0..5 {
                let f = ((*v >> (2 * j)) & 3) as usize;
                if f > 2 {
                    valid = false;
                    break;
                }
                idx += f * 3usize.pow(j as u32);
            }
            if valid && pack5[idx].is_none() {
                pack5[idx] = Some(code as u8);
            }
        }
        let mut pack3 = vec![None; 125];
        for (code, v) in RSD_PACK_3_IN_7.iter().enumerate() {
            if v.iter().all(|x| *x < 5) {
                let idx = v[0] as usize + 5 * v[1] as usize + 25 * v[2] as usize;
                if pack3[idx].is_none() {
                    pack3[idx] = Some(code as u8);
                }
            }
        }
        Tables {
            fst_rsd_amp: Vlc::new(&HUFF_FST_RSD_AMP),
            rsd_apprx: Vlc::new(&HUFF_RSD_APPRX),
            rsd_amp: Vlc::new(&HUFF_RSD_AMP),
            avg_g3: Vlc::new(&HUFF_AVG_G3),
            st_grid: Vlc::new(&HUFF_ST_GRID),
            grid_3: Vlc::new(&HUFF_GRID_3),
            grid_2: Vlc::new(&HUFF_GRID_2),
            rsd: codes_of(&HUFF_RSD, 8),
            pack5,
            pack3,
        }
    }
}
