//! The bit allocation of A/52 clause 7.2.2: from exponents to power spectral density, masking curve and the
//! number of mantissa bits (`bap`) each bin gets, for a given SNR offset. The encoder uses the standard's fixed
//! parameter codes below (the usual defaults) rather than sending its own.

use crate::tables::{
    BAND_START, BAP_FROM_ADDRESS, DB_PER_BIT, FAST_DECAY, FLOOR, HEARING_THRESHOLD, LOG_ADD, SLOW_DECAY, SLOW_GAIN,
};
use crate::BANDS;

// Fixed bit-allocation parameter codes (the usual encoder defaults).
pub(crate) const SDCYCOD: usize = 2;

pub(crate) const FDCYCOD: usize = 1;

pub(crate) const SGAINCOD: usize = 1;

pub(crate) const DBPBCOD: usize = 2;

pub(crate) const FLOORCOD: usize = 7;

pub(crate) const FGAINCOD: usize = 4;

/// Bin -> critical band map derived from the band start table.
pub(crate) fn bin_to_band() -> [u8; 256] {
    let mut m = [0u8; 256];
    for b in 0..BANDS {
        for bin in BAND_START[b] as usize..BAND_START[b + 1] as usize {
            m[bin] = b as u8;
        }
    }
    m
}

pub(crate) fn calc_psd(exp: &[i32], end: usize, b2b: &[u8; 256], psd: &mut [i32], band_psd: &mut [i32]) {
    for bin in 0..end {
        psd[bin] = 3072 - (exp[bin] << 7);
    }
    let mut bin = 0;
    while bin < end.min(28) {
        band_psd[bin] = psd[bin];
        bin += 1;
    }
    if bin >= end {
        return;
    }
    let mut band = b2b[bin] as usize;
    loop {
        let mut v = psd[bin];
        bin += 1;
        let band_end = (BAND_START[band + 1] as usize).min(end);
        while bin < band_end {
            let max = v.max(psd[bin]);
            let adr = (max - ((v + psd[bin] + 1) >> 1)).min(255) as usize;
            v = max + LOG_ADD[adr] as i32;
            bin += 1;
        }
        band_psd[band] = v;
        band += 1;
        if end <= BAND_START[band] as usize {
            break;
        }
    }
}

/// Power spectral density and banded PSD from `start` (the coupling channel starts above bin 0).
pub(crate) fn calc_psd_from(
    exp: &[i32],
    start: usize,
    end: usize,
    b2b: &[u8; 256],
    psd: &mut [i32],
    band_psd: &mut [i32],
) {
    for bin in start..end {
        psd[bin] = 3072 - (exp[bin] << 7);
    }
    let mut bin = start;
    let mut band = b2b[bin] as usize;
    loop {
        let mut v = psd[bin];
        bin += 1;
        let band_end = (BAND_START[band + 1] as usize).min(end);
        while bin < band_end {
            let max = v.max(psd[bin]);
            let adr = (max - ((v + psd[bin] + 1) >> 1)).min(255) as usize;
            v = max + LOG_ADD[adr] as i32;
            bin += 1;
        }
        band_psd[band] = v;
        band += 1;
        if end <= BAND_START[band] as usize {
            break;
        }
    }
}

/// Masking curve of the coupling channel (A/52 7.2.2.4 with the leak start values of cplfleak = cplsleak = 0).
pub(crate) fn calc_mask_cpl(
    band_psd: &[i32],
    start: usize,
    end: usize,
    fast_gain: i32,
    fscod: usize,
    b2b: &[u8; 256],
    mask: &mut [i32],
) {
    let (slow_decay, fast_decay) = (SLOW_DECAY[SDCYCOD], FAST_DECAY[FDCYCOD]);
    let (slow_gain, db_per_bit) = (SLOW_GAIN[SGAINCOD], DB_PER_BIT[DBPBCOD]);
    let (band_start, band_end) = (b2b[start] as usize, b2b[end - 1] as usize + 1);
    let (mut fastleak, mut slowleak) = (768, 768);
    for band in band_start..band_end {
        fastleak = (fastleak - fast_decay).max(band_psd[band] - fast_gain);
        slowleak = (slowleak - slow_decay).max(band_psd[band] - slow_gain);
        let mut excite = fastleak.max(slowleak);
        let tmp = db_per_bit - band_psd[band];
        if tmp > 0 {
            excite += tmp >> 2;
        }
        mask[band] = (HEARING_THRESHOLD[band * 3 + fscod] as i32).max(excite);
    }
}

pub(crate) fn calc_bap_from(
    mask: &[i32],
    psd: &[i32],
    start: usize,
    end: usize,
    snr_offset: i32,
    b2b: &[u8; 256],
    bap: &mut [u8],
) {
    let floor = FLOOR[FLOORCOD];
    if snr_offset == -960 {
        bap[..end].fill(0);
        return;
    }
    let addr = |p: i32, m: i32| (((p - m) >> 5).clamp(0, 63)) as usize;
    let mut bin = start;
    let mut band = b2b[bin] as usize;
    loop {
        let m = ((mask[band] - snr_offset - floor).max(0) & 0x1FE0) + floor;
        band += 1;
        let band_end = (BAND_START[band] as usize).min(end);
        while bin < band_end {
            bap[bin] = BAP_FROM_ADDRESS[addr(psd[bin], m)];
            bin += 1;
        }
        if end <= band_end {
            break;
        }
    }
}

pub(crate) fn lowcomp1(a: i32, b0: i32, b1: i32, c: i32) -> i32 {
    if b0 + 256 == b1 {
        c
    } else if b0 > b1 {
        (a - 64).max(0)
    } else {
        a
    }
}

pub(crate) fn lowcomp(a: i32, b0: i32, b1: i32, bin: usize) -> i32 {
    if bin < 7 {
        lowcomp1(a, b0, b1, 384)
    } else if bin < 20 {
        lowcomp1(a, b0, b1, 320)
    } else {
        (a - 128).max(0)
    }
}

/// Masking curve per band (A/52 7.2.2.4 - 7.2.2.6), start bin 0, no coupling, no delta allocation.
pub(crate) fn calc_mask(
    band_psd: &[i32],
    end: usize,
    fast_gain: i32,
    is_lfe: bool,
    fscod: usize,
    b2b: &[u8; 256],
    mask: &mut [i32],
) {
    let (slow_decay, fast_decay) = (SLOW_DECAY[SDCYCOD], FAST_DECAY[FDCYCOD]);
    let (slow_gain, db_per_bit) = (SLOW_GAIN[SGAINCOD], DB_PER_BIT[DBPBCOD]);
    let mut excite = [0i32; BANDS];
    let band_end = b2b[end - 1] as usize + 1;
    let mut lc = 0;
    lc = lowcomp1(lc, band_psd[0], band_psd[1], 384);
    excite[0] = band_psd[0] - fast_gain - lc;
    lc = lowcomp1(lc, band_psd[1], band_psd[2], 384);
    excite[1] = band_psd[1] - fast_gain - lc;
    let mut begin = 7;
    let (mut fastleak, mut slowleak) = (0, 0);
    for band in 2..7 {
        if !(is_lfe && band == 6) {
            lc = lowcomp1(lc, band_psd[band], band_psd[band + 1], 384);
        }
        fastleak = band_psd[band] - fast_gain;
        slowleak = band_psd[band] - slow_gain;
        excite[band] = fastleak - lc;
        if !(is_lfe && band == 6) && band_psd[band] <= band_psd[band + 1] {
            begin = band + 1;
            break;
        }
    }
    let end1 = band_end.min(22);
    for band in begin..end1 {
        if !(is_lfe && band == 6) {
            lc = lowcomp(lc, band_psd[band], band_psd[band + 1], band);
        }
        fastleak = (fastleak - fast_decay).max(band_psd[band] - fast_gain);
        slowleak = (slowleak - slow_decay).max(band_psd[band] - slow_gain);
        excite[band] = (fastleak - lc).max(slowleak);
    }
    for band in 22.max(begin.min(22))..band_end {
        fastleak = (fastleak - fast_decay).max(band_psd[band] - fast_gain);
        slowleak = (slowleak - slow_decay).max(band_psd[band] - slow_gain);
        excite[band] = fastleak.max(slowleak);
    }
    for band in 0..band_end {
        let tmp = db_per_bit - band_psd[band];
        if tmp > 0 {
            excite[band] += tmp >> 2;
        }
        mask[band] = (HEARING_THRESHOLD[band * 3 + fscod] as i32).max(excite[band]);
    }
}

pub(crate) fn calc_bap(mask: &[i32], psd: &[i32], end: usize, snr_offset: i32, b2b: &[u8; 256], bap: &mut [u8]) {
    let floor = FLOOR[FLOORCOD];
    if snr_offset == -960 {
        bap[..end].fill(0);
        return;
    }
    let addr = |p: i32, m: i32| (((p - m) >> 5).clamp(0, 63)) as usize;
    let mut bin = 0;
    while bin < end.min(28) {
        let m = ((mask[bin] - snr_offset - floor).max(0) & 0x1FE0) + floor;
        bap[bin] = BAP_FROM_ADDRESS[addr(psd[bin], m)];
        bin += 1;
    }
    if bin >= end {
        return;
    }
    let mut band = b2b[bin] as usize;
    loop {
        let m = ((mask[band] - snr_offset - floor).max(0) & 0x1FE0) + floor;
        band += 1;
        let band_end = (BAND_START[band] as usize).min(end);
        while bin < band_end {
            bap[bin] = BAP_FROM_ADDRESS[addr(psd[bin], m)];
            bin += 1;
        }
        if end <= band_end {
            break;
        }
    }
}
