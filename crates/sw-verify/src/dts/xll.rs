//! The lossless extension's frame header and channel set headers (clause 8), and the DTS:X marker that may trail
//! the lossless audio.

use crate::bits::Bits;
use crate::dts::consts::{SAMPLE_RATES_EXSS, SYNC_XLL, SYNC_XLL_X, SYNC_XLL_X_IMAX};
use crate::{be32, crc};

#[derive(Debug, Default, Clone)]
pub(crate) struct XllInfo {
    pub(crate) version: u32,
    pub(crate) header_size: usize,
    pub(crate) frame_size: usize,
    pub(crate) nchsets: u32,
    pub(crate) nsegs: u32,
    pub(crate) seg_samples: u32,
    pub(crate) seg_size_nbits: u32,
    pub(crate) band_crc: u32,
    pub(crate) scalable_lsbs: bool,
    pub(crate) ch_mask_nbits: u32,
    pub(crate) fixed_lsb: u32,
    pub(crate) common_crc_ok: bool,
    pub(crate) chset_headers: u32,
    pub(crate) chset_header_bad: u32,
    pub(crate) chset_bytes: Vec<usize>,
    /// First channel set: channels, which of them carry a residual of the core (all ones: plain lossless), bit depths, rate.
    pub(crate) first_set: Option<(u32, u32, u32, u32, u32)>,
    /// Bytes of coded data per frequency band (from the navigation table).
    pub(crate) band_bytes: Vec<usize>,
    pub(crate) bands: Vec<BandHdr>,
}

pub(crate) fn parse_xll(d: &[u8]) -> Result<XllInfo, String> {
    if d.len() < 12 || be32(d) != SYNC_XLL {
        return Err("no XLL sync word".into());
    }
    let mut b = Bits::new(&d[4..]);
    let mut x = XllInfo {
        version: b.get(4) + 1,
        ..Default::default()
    };
    x.header_size = b.get(8) as usize + 1;
    if x.header_size > d.len() {
        return Err("XLL header runs past the data".into());
    }
    x.common_crc_ok = crc::dts(&d[4..x.header_size]) == 0;
    let n = b.get(5) as usize + 1;
    x.frame_size = b.get(n) as usize + 1;
    x.nchsets = b.get(4) + 1;
    let seg_log2 = b.get(4);
    x.nsegs = 1 << seg_log2;
    let ss = b.get(4);
    x.seg_samples = if ss == 0 { 0 } else { 1 << ss };
    x.seg_size_nbits = b.get(5) + 1;
    x.band_crc = b.get(2);
    x.scalable_lsbs = b.flag();
    x.ch_mask_nbits = b.get(5) + 1;
    if x.scalable_lsbs {
        x.fixed_lsb = b.get(4);
    }
    if x.seg_samples == 0 {
        return Err("no samples per XLL segment".into());
    }
    if x.nsegs > 1024 || x.seg_samples > 512 {
        return Err("XLL segment structure out of range".into());
    }
    // channel set headers follow, each ending in its own checksum
    let mut pos = x.header_size;
    for _ in 0..x.nchsets {
        if pos + 3 > d.len() {
            break;
        }
        let size = (((d[pos] as usize) << 2) | (d[pos + 1] as usize >> 6)) + 1;
        if x.first_set.is_none() && pos + 8 <= d.len() {
            let mut cb = Bits::new(&d[pos..]);
            cb.skip(10);
            let nch = cb.get(4) + 1;
            let resid = cb.get(nch as usize);
            let pcm = cb.get(5) + 1;
            let storage = cb.get(5) + 1;
            let freq = SAMPLE_RATES_EXSS[cb.get(4) as usize];
            x.first_set = Some((nch, resid, pcm, storage, freq));
        }
        x.chset_headers += 1;
        if pos + size > d.len() {
            // the rest of the frame (smoothing buffer) is in the next substream: nothing more to check here
            x.chset_bytes.push(size);
            break;
        }
        if crc::dts(&d[pos..pos + size]) != 0 {
            x.chset_header_bad += 1;
        }
        x.chset_bytes.push(size);
        pos += size;
    }
    // navigation table: for each band, segment and channel set, the size of its data
    let nbands = if x.first_set.map_or(0, |f| f.4) > 96_000 { 2 } else { 1 };
    let mut nb = Bits::new(&d[pos.min(d.len())..]);
    x.band_bytes = vec![0; nbands];
    for band in 0..nbands {
        for _ in 0..x.nsegs {
            for _ in 0..x.nchsets {
                x.band_bytes[band] += nb.get(x.seg_size_nbits as usize) as usize + 1;
            }
        }
    }
    if nb.over {
        x.band_bytes.clear();
    }
    let bands = parse_band_headers(d, &x);
    x.bands = bands;
    Ok(x)
}

/// The sync word is there but the common header (its size is in the first bytes) reaches beyond the data given.
pub(crate) fn parse_xll_runs_past(d: &[u8]) -> bool {
    d.len() >= 6 && (((d[4] as usize) & 0x0F) << 4 | (d[5] as usize) >> 4) + 1 > d.len()
}

pub(crate) fn xll_rows(x: &XllInfo) -> Vec<(String, String)> {
    vec![
        ("version".to_string(), x.version.to_string()),
        (
            "common header".to_string(),
            format!(
                "{} bytes, checksum {}",
                x.header_size,
                if x.common_crc_ok { "ok" } else { "BAD" }
            ),
        ),
        ("frame size".to_string(), format!("{} bytes", x.frame_size)),
        ("channel sets".to_string(), x.nchsets.to_string()),
        (
            "first channel set".to_string(),
            match x.first_set {
                Some((n, r, pcm, st, f)) => format!(
                    "{n} channels at {f} Hz, {pcm}-bit in {st}-bit words, {}",
                    if r == (1 << n) - 1 {
                        "full lossless data (not a residual of the core)".to_string()
                    } else {
                        format!("residual of the core on channels 0x{r:X}")
                    }
                ),
                None => "unknown".to_string(),
            },
        ),
        (
            "channel set headers".to_string(),
            format!(
                "{} bytes in all, {} of {} checksums ok",
                x.chset_bytes.iter().sum::<usize>(),
                x.chset_headers - x.chset_header_bad,
                x.chset_headers
            ),
        ),
        (
            "coded data per band".to_string(),
            if x.band_bytes.is_empty() {
                "unknown".to_string()
            } else {
                x.band_bytes
                    .iter()
                    .enumerate()
                    .map(|(i, b)| format!("band {i}: {b} bytes"))
                    .collect::<Vec<_>>()
                    .join(", ")
            },
        ),
        ("segments per frame".to_string(), x.nsegs.to_string()),
        ("samples per segment (per band)".to_string(), x.seg_samples.to_string()),
        (
            "samples per frame (per band)".to_string(),
            (x.nsegs * x.seg_samples).to_string(),
        ),
        ("segment size field".to_string(), format!("{} bits", x.seg_size_nbits)),
        (
            "band checksums (band_crc_present)".to_string(),
            ["none", "MSB part", "MSB and LSB parts", "every band"][x.band_crc as usize].to_string(),
        ),
        (
            "scalable LSBs".to_string(),
            if x.scalable_lsbs {
                format!("yes, fixed LSB width {}", x.fixed_lsb)
            } else {
                "no".to_string()
            },
        ),
        ("channel mask field".to_string(), format!("{} bits", x.ch_mask_nbits)),
    ]
}

#[derive(Debug, Default, Clone)]
pub(crate) struct BandHdr {
    pub(crate) decor: bool,
    pub(crate) pairs_used: usize,
    pub(crate) adapt: Vec<u32>,
    pub(crate) fixed: Vec<u32>,
}

/// The per-band predictor settings of the first channel set (assumes a primary set without embedded downmix, as
/// every stream made by the encoders handled here is); empty when the header is outside that shape.
pub(crate) fn parse_band_headers(d: &[u8], x: &XllInfo) -> Vec<BandHdr> {
    let Some((nch, _, pcm, storage, freq)) = x.first_set else {
        return Vec::new();
    };
    let _ = pcm;
    let pos = x.header_size;
    if pos + 4 > d.len() {
        return Vec::new();
    }
    let mut b = Bits::new(&d[pos..]);
    b.skip(10 + 4);
    b.skip(nch as usize); // residual_encode
    b.skip(5 + 5 + 4 + 2 + 2);
    let primary = b.flag();
    let dmix_present = b.flag();
    if dmix_present {
        return Vec::new();
    }
    let _ = primary;
    b.skip(1); // hier_chset
    if !b.flag() {
        return Vec::new(); // channel mask disabled
    }
    b.skip(x.ch_mask_nbits as usize);
    let nbands = if freq > 96_000 { 2 } else { 1 };
    if freq > 96_000 && b.flag() {
        return Vec::new();
    }
    let _ = storage;
    let linear = |b: &mut Bits, n: usize| -> i32 {
        let v = b.get(n) as i32;
        (v >> 1) ^ -(v & 1)
    };
    let ch_nbits = 32 - (nch.max(2) - 1).leading_zeros() as usize;
    let mut out = Vec::new();
    for band in 0..nbands {
        let mut h = BandHdr::default();
        h.decor = b.flag();
        if h.decor && nch > 1 {
            b.skip(ch_nbits * nch as usize);
            for _ in 0..nch / 2 {
                if b.flag() {
                    let c = linear(&mut b, 7);
                    if c != 0 {
                        h.pairs_used += 1;
                    }
                }
            }
        }
        for _ in 0..nch {
            h.adapt.push(b.get(4));
        }
        for i in 0..nch as usize {
            h.fixed.push(if h.adapt[i] == 0 { b.get(2) } else { 0 });
        }
        for i in 0..nch as usize {
            for _ in 0..h.adapt[i] {
                linear(&mut b, 8);
            }
        }
        // MSB/LSB split (a size field and a width per channel), then the bit width adjustment (a width per channel)
        let split = if band == 0 { x.scalable_lsbs } else { b.flag() };
        if split {
            b.skip(x.seg_size_nbits as usize + 4 * nch as usize);
        }
        let adjust = if band == 0 { x.scalable_lsbs } else { b.flag() };
        if adjust {
            b.skip(4 * nch as usize);
        }
        if b.over {
            return Vec::new();
        }
        out.push(h);
    }
    out
}

/// Which DTS:X marker, if any, sits on a 32-bit boundary of this lossless frame (counted from its first byte). The
/// lossless audio ends first and the marker follows it, so it is looked for from the first quarter of the frame on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum XMark {
    X,
    Imax,
}

pub(crate) fn find_x_marker(frame: &[u8]) -> Option<XMark> {
    let from = (frame.len() / 4) & !3;
    let mut i = from;
    while i + 4 <= frame.len() {
        let w = be32(&frame[i..]);
        if w == SYNC_XLL_X {
            return Some(XMark::X);
        }
        if w >> 1 == SYNC_XLL_X_IMAX >> 1 {
            return Some(XMark::Imax);
        }
        i += 4;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dts::consts::{SYNC_XLL_X, SYNC_XLL_X_IMAX};

    fn frame_with(word: Option<u32>, at: usize, len: usize) -> Vec<u8> {
        let mut f: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
        if let Some(w) = word {
            f[at..at + 4].copy_from_slice(&w.to_be_bytes());
        }
        f
    }

    #[test]
    fn the_dtsx_marker_is_found_on_a_dword_boundary_after_the_audio() {
        assert_eq!(find_x_marker(&frame_with(Some(SYNC_XLL_X), 4000, 4096)), Some(XMark::X));
        assert_eq!(
            find_x_marker(&frame_with(Some(SYNC_XLL_X_IMAX), 3000, 4096)),
            Some(XMark::Imax)
        );
        // the low bit of the IMAX word is free, as in the decoder's own test
        assert_eq!(
            find_x_marker(&frame_with(Some(SYNC_XLL_X_IMAX | 1), 3000, 4096)),
            Some(XMark::Imax)
        );
        // not on a dword boundary, or in the first quarter (the lossless headers): ignored
        assert_eq!(find_x_marker(&frame_with(Some(SYNC_XLL_X), 4001, 4096)), None);
        assert_eq!(find_x_marker(&frame_with(Some(SYNC_XLL_X), 400, 4096)), None);
        assert_eq!(find_x_marker(&frame_with(None, 0, 4096)), None);
    }
}
