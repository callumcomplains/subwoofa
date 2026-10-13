//! The bit allocation of A/52 clause 7.2.2: from exponents to power spectral density, masking curve and the
//! number of mantissa bits (`bap`) each bin gets, for a given SNR offset.
//!
//! The encoder uses the standard's fixed parameter codes below (the usual defaults) rather than sending its own,
//! so the decoder runs this same computation from nothing but the exponents and the SNR offsets in the frame.
//! Bins are grouped into bands (`BAND_START`): below bin 28 every bin is its own band; above, bands widen.

use crate::tables::{
    BAND_START, BAP_FROM_ADDRESS, DB_PER_BIT, FAST_DECAY, FLOOR, HEARING_THRESHOLD, LOG_ADD, SLOW_DECAY, SLOW_GAIN,
};
use crate::BANDS;

/// Slow decay code (`sdcycod`).
pub(crate) const SDCYCOD: usize = 2;
/// Fast decay code (`fdcycod`).
pub(crate) const FDCYCOD: usize = 1;
/// Slow gain code (`sgaincod`).
pub(crate) const SGAINCOD: usize = 1;
/// DB-per-bit code (`dbpbcod`).
pub(crate) const DBPBCOD: usize = 2;
/// Masking floor code (`floorcod`).
pub(crate) const FLOORCOD: usize = 7;
/// Fast gain code (`fgaincod`), the same for every channel.
pub(crate) const FGAINCOD: usize = 4;

/// The first bin from which bands are wider than one bin.
const FIRST_WIDE_BIN: usize = 28;

/// The SNR offset at which every bin gets no mantissa bits at all (the lowest coarse offset).
const SILENT_SNR_OFFSET: i32 = -960;

/// Bin -> band map derived from the band start table.
pub(crate) fn bin_to_band() -> [u8; 256] {
    let mut m = [0u8; 256];
    for b in 0..BANDS {
        for bin in BAND_START[b] as usize..BAND_START[b + 1] as usize {
            m[bin] = b as u8;
        }
    }
    m
}

/// Power spectral density of the bins up to `end`, and the banded PSD (A/52 7.2.2.2 and 7.2.2.3).
pub(crate) fn calc_psd(exp: &[i32], end: usize, band_of_bin: &[u8; 256], psd: &mut [i32], band_psd: &mut [i32]) {
    let single = end.min(FIRST_WIDE_BIN);
    for bin in 0..single {
        psd[bin] = 3072 - (exp[bin] << 7);
        band_psd[bin] = psd[bin];
    }
    if end > single {
        calc_psd_from(exp, single, end, band_of_bin, psd, band_psd);
    }
}

/// Power spectral density and banded PSD from bin `start` (a band boundary above bin 28): the coupling channel
/// starts there, and the other channels continue there. Each band's PSD is the log-sum of its bins.
pub(crate) fn calc_psd_from(
    exp: &[i32],
    start: usize,
    end: usize,
    band_of_bin: &[u8; 256],
    psd: &mut [i32],
    band_psd: &mut [i32],
) {
    for bin in start..end {
        psd[bin] = 3072 - (exp[bin] << 7);
    }
    let mut bin = start;
    let mut band = band_of_bin[bin] as usize;
    loop {
        let mut sum = psd[bin];
        bin += 1;
        let band_end = (BAND_START[band + 1] as usize).min(end);
        while bin < band_end {
            let max = sum.max(psd[bin]);
            let address = (max - ((sum + psd[bin] + 1) >> 1)).min(255) as usize;
            sum = max + LOG_ADD[address] as i32;
            bin += 1;
        }
        band_psd[band] = sum;
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
    band_of_bin: &[u8; 256],
    mask: &mut [i32],
) {
    let (slow_decay, fast_decay) = (SLOW_DECAY[SDCYCOD], FAST_DECAY[FDCYCOD]);
    let (slow_gain, db_per_bit) = (SLOW_GAIN[SGAINCOD], DB_PER_BIT[DBPBCOD]);
    let (first_band, band_end) = (band_of_bin[start] as usize, band_of_bin[end - 1] as usize + 1);
    let (mut fastleak, mut slowleak) = (768, 768);
    for band in first_band..band_end {
        fastleak = (fastleak - fast_decay).max(band_psd[band] - fast_gain);
        slowleak = (slowleak - slow_decay).max(band_psd[band] - slow_gain);
        let mut excite = fastleak.max(slowleak);
        let below_threshold = db_per_bit - band_psd[band];
        if below_threshold > 0 {
            excite += below_threshold >> 2;
        }
        mask[band] = (HEARING_THRESHOLD[band * 3 + fscod] as i32).max(excite);
    }
}

/// The low-frequency compensation step of the excitation (A/52 7.2.2.4): `comp` is the running value, `psd0` and
/// `psd1` the PSD of this band and the next, and `start` the value it jumps to when the next band is a long way up.
fn low_comp_step(comp: i32, psd0: i32, psd1: i32, start: i32) -> i32 {
    if psd0 + 256 == psd1 {
        start
    } else if psd0 > psd1 {
        (comp - 64).max(0)
    } else {
        comp
    }
}

/// `low_comp_step` for a band in the range where its `start` value depends on the band (7.2.2.4).
fn low_comp(comp: i32, psd0: i32, psd1: i32, band: usize) -> i32 {
    if band < 7 {
        low_comp_step(comp, psd0, psd1, 384)
    } else if band < 20 {
        low_comp_step(comp, psd0, psd1, 320)
    } else {
        (comp - 128).max(0)
    }
}

/// Masking curve per band (A/52 7.2.2.4 - 7.2.2.6), start bin 0, no coupling, no delta allocation.
pub(crate) fn calc_mask(
    band_psd: &[i32],
    end: usize,
    fast_gain: i32,
    is_lfe: bool,
    fscod: usize,
    band_of_bin: &[u8; 256],
    mask: &mut [i32],
) {
    let (slow_decay, fast_decay) = (SLOW_DECAY[SDCYCOD], FAST_DECAY[FDCYCOD]);
    let (slow_gain, db_per_bit) = (SLOW_GAIN[SGAINCOD], DB_PER_BIT[DBPBCOD]);
    let mut excite = [0i32; BANDS];
    let band_end = band_of_bin[end - 1] as usize + 1;
    // the LFE channel skips the compensation for its band 6
    let compensated = |band: usize| !(is_lfe && band == 6);
    let mut comp = low_comp_step(0, band_psd[0], band_psd[1], 384);
    excite[0] = band_psd[0] - fast_gain - comp;
    comp = low_comp_step(comp, band_psd[1], band_psd[2], 384);
    excite[1] = band_psd[1] - fast_gain - comp;
    // bands 2 to 6 follow the PSD directly until it first stops falling
    let mut resume = 7;
    let (mut fastleak, mut slowleak) = (0, 0);
    for band in 2..7 {
        if compensated(band) {
            comp = low_comp_step(comp, band_psd[band], band_psd[band + 1], 384);
        }
        fastleak = band_psd[band] - fast_gain;
        slowleak = band_psd[band] - slow_gain;
        excite[band] = fastleak - comp;
        if compensated(band) && band_psd[band] <= band_psd[band + 1] {
            resume = band + 1;
            break;
        }
    }
    // up to band 22 the compensation still applies; from there the excitation is just the leaks
    for band in resume..band_end.min(22) {
        if compensated(band) {
            comp = low_comp(comp, band_psd[band], band_psd[band + 1], band);
        }
        fastleak = (fastleak - fast_decay).max(band_psd[band] - fast_gain);
        slowleak = (slowleak - slow_decay).max(band_psd[band] - slow_gain);
        excite[band] = (fastleak - comp).max(slowleak);
    }
    for band in 22..band_end {
        fastleak = (fastleak - fast_decay).max(band_psd[band] - fast_gain);
        slowleak = (slowleak - slow_decay).max(band_psd[band] - slow_gain);
        excite[band] = fastleak.max(slowleak);
    }
    for band in 0..band_end {
        let below_threshold = db_per_bit - band_psd[band];
        if below_threshold > 0 {
            excite[band] += below_threshold >> 2;
        }
        mask[band] = (HEARING_THRESHOLD[band * 3 + fscod] as i32).max(excite[band]);
    }
}

/// The mantissa bits (`bap`, A/52 7.2.2.7) of every bin from 0 to `end`, given the masking curve and the SNR offset.
pub(crate) fn calc_bap(
    mask: &[i32],
    psd: &[i32],
    end: usize,
    snr_offset: i32,
    band_of_bin: &[u8; 256],
    bap: &mut [u8],
) {
    if snr_offset == SILENT_SNR_OFFSET {
        bap[..end].fill(0);
        return;
    }
    let single = end.min(FIRST_WIDE_BIN);
    for bin in 0..single {
        bap[bin] = bap_of(psd[bin], mask[bin], snr_offset);
    }
    if end > single {
        bap_in_bands(mask, psd, single, end, snr_offset, band_of_bin, bap);
    }
}

/// `calc_bap` from bin `start` (a band boundary above bin 28), for the coupling channel.
pub(crate) fn calc_bap_from(
    mask: &[i32],
    psd: &[i32],
    start: usize,
    end: usize,
    snr_offset: i32,
    band_of_bin: &[u8; 256],
    bap: &mut [u8],
) {
    if snr_offset == SILENT_SNR_OFFSET {
        bap[..end].fill(0);
        return;
    }
    bap_in_bands(mask, psd, start, end, snr_offset, band_of_bin, bap);
}

/// The `bap` of one bin: how far its PSD is above the mask, lowered by the SNR offset and quantized to steps of 32.
fn bap_of(psd: i32, mask: i32, snr_offset: i32) -> u8 {
    let floor = FLOOR[FLOORCOD];
    let masked = ((mask - snr_offset - floor).max(0) & 0x1FE0) + floor;
    BAP_FROM_ADDRESS[((psd - masked) >> 5).clamp(0, 63) as usize]
}

/// `bap` for the bins from `start` to `end`, where each band shares one mask value.
fn bap_in_bands(
    mask: &[i32],
    psd: &[i32],
    start: usize,
    end: usize,
    snr_offset: i32,
    band_of_bin: &[u8; 256],
    bap: &mut [u8],
) {
    let mut bin = start;
    let mut band = band_of_bin[bin] as usize;
    loop {
        band += 1;
        let band_end = (BAND_START[band] as usize).min(end);
        while bin < band_end {
            bap[bin] = bap_of(psd[bin], mask[band - 1], snr_offset);
            bin += 1;
        }
        if end <= band_end {
            break;
        }
    }
}
