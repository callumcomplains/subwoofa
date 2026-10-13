//! The extension substream header (clause 7): its assets, the components of each, and the checks on them.

use crate::bits::Bits;
use crate::crc;
use crate::dts::consts::{SAMPLE_RATES_EXSS, SPEAKERS};

#[derive(Debug, Clone, Default)]
pub(crate) struct Exss {
    pub(crate) index: u32,
    pub(crate) wide: bool,
    pub(crate) header_size: usize,
    pub(crate) size: usize,
    pub(crate) hdr_crc_ok: bool,
    pub(crate) static_fields: bool,
    pub(crate) ref_clock: u32,
    pub(crate) frame_dur: u32,
    pub(crate) npresent: u32,
    pub(crate) nassets: u32,
    pub(crate) mix_metadata: bool,
    pub(crate) asset_size: usize,
    pub(crate) pcm_bits: u32,
    pub(crate) max_rate: u32,
    pub(crate) nchannels: u32,
    pub(crate) spk_mask: u32,
    pub(crate) one_to_one: bool,
    pub(crate) embedded_stereo: bool,
    pub(crate) embedded_6ch: bool,
    pub(crate) coding_mode: u32,
    pub(crate) ext_mask: u32,
    pub(crate) core_size: usize,
    pub(crate) xbr_size: usize,
    pub(crate) xxch_size: usize,
    pub(crate) x96_size: usize,
    pub(crate) lbr_size: usize,
    pub(crate) xll_size: usize,
    pub(crate) xll_sync_present: bool,
    pub(crate) xll_delay: u32,
    pub(crate) xll_sync_offset: usize,
    pub(crate) xll_pbr: u32,
    pub(crate) hd_stream_id: u32,
    pub(crate) dialnorm: Option<u32>,
    pub(crate) drc: Option<u32>,
    pub(crate) core_offset: usize,
    pub(crate) xbr_offset: usize,
    pub(crate) xxch_offset: usize,
    pub(crate) x96_offset: usize,
    pub(crate) lbr_offset: usize,
    pub(crate) xll_offset: usize,
}

pub(crate) fn mask_count(mut m: u32) -> u32 {
    let mut n = 0;
    while m != 0 {
        n += m & 1;
        m >>= 1;
    }
    n
}

pub(crate) fn parse_exss(d: &[u8]) -> Result<Exss, String> {
    if d.len() < 12 {
        return Err("header cut short".into());
    }
    let mut b = Bits::new(d);
    b.skip(32 + 8);
    let mut e = Exss {
        index: b.get(2),
        wide: b.flag(),
        ..Default::default()
    };
    let wide = e.wide as usize;
    e.header_size = b.get(8 + 4 * wide) as usize + 1;
    if e.header_size > d.len() {
        return Err("header larger than the data given".into());
    }
    e.hdr_crc_ok = crc::dts(&d[5..e.header_size]) == 0;
    let nbits = 16 + 4 * wide;
    e.size = b.get(nbits) as usize + 1;
    e.static_fields = b.flag();
    let mix_out = parse_static_fields(&mut b, &mut e);
    if e.nassets > 1 {
        return Err(format!("{} audio assets (only one is understood)", e.nassets));
    }
    e.asset_size = b.get(nbits) as usize + 1;
    // asset descriptor
    let descr_pos = b.pos;
    let descr_size = b.get(9) as usize + 1;
    b.get(3);
    parse_asset_format(&mut b, &mut e)?;
    let drc = b.flag();
    if drc {
        e.drc = Some(b.get(8));
    }
    if b.flag() {
        e.dialnorm = Some(b.get(5));
    }
    if drc && e.embedded_stereo {
        b.skip(8);
    }
    parse_asset_mixing(&mut b, &mut e, &mix_out)?;
    parse_coding_mode(&mut b, nbits, &mut e);
    if b.over {
        return Err("the header runs past the data given".into());
    }
    if descr_pos + descr_size * 8 > e.header_size * 8 {
        return Err("asset descriptor runs past the header".into());
    }
    locate_components(&mut e)?;
    Ok(e)
}

/// The static fields: reference clock, frame duration, presentations and assets, mixing outputs. Returns the number of
/// channels of each mixing output.
fn parse_static_fields(b: &mut Bits, e: &mut Exss) -> Vec<u32> {
    let mut mix_out = Vec::new();
    if e.static_fields {
        e.ref_clock = b.get(2);
        e.frame_dur = b.get(3);
        if b.flag() {
            b.skip(36);
        }
        e.npresent = b.get(3) + 1;
        e.nassets = b.get(3) + 1;
        let mut active = Vec::new();
        for _ in 0..e.npresent {
            active.push(b.get(e.index as usize + 1));
        }
        for m in &active {
            b.skip(mask_count(*m) as usize * 8);
        }
        e.mix_metadata = b.flag();
        if e.mix_metadata {
            b.skip(2);
            let nb = (b.get(2) as usize + 1) << 2;
            let n = b.get(2) + 1;
            for _ in 0..n {
                mix_out.push(mask_count(b.get(nb)));
            }
        }
    } else {
        e.npresent = 1;
        e.nassets = 1;
    }
    mix_out
}

/// The asset descriptor's format fields: bit depth, rate, channel count and speaker layout.
fn parse_asset_format(b: &mut Bits, e: &mut Exss) -> Result<(), String> {
    if e.static_fields {
        if b.flag() {
            b.skip(4);
        }
        if b.flag() {
            b.skip(24);
        }
        if b.flag() {
            let n = b.get(10) as usize + 1;
            b.skip(n * 8);
        }
        e.pcm_bits = b.get(5) + 1;
        e.max_rate = SAMPLE_RATES_EXSS[b.get(4) as usize];
        e.nchannels = b.get(8) + 1;
        e.one_to_one = b.flag();
        if e.one_to_one {
            e.embedded_stereo = e.nchannels > 2 && b.flag();
            e.embedded_6ch = e.nchannels > 6 && b.flag();
            let mut nbits_mask = 0usize;
            if b.flag() {
                nbits_mask = (b.get(2) as usize + 1) << 2;
                e.spk_mask = b.get(nbits_mask);
            }
            let nsets = b.get(3) as usize;
            if nsets > 0 && nbits_mask == 0 {
                return Err("speaker mask disabled yet there are remapping sets".into());
            }
            let mut counts = Vec::new();
            for _ in 0..nsets {
                counts.push(mask_count(b.get(nbits_mask)) as usize);
            }
            for n in counts {
                let nch = b.get(5) as usize + 1;
                for _ in 0..n {
                    let m = b.get(nch);
                    b.skip(mask_count(m) as usize * 5);
                }
            }
        } else {
            b.get(3);
        }
    }
    Ok(())
}

/// Mixing metadata of the asset (skipped over: only its size matters here).
fn parse_asset_mixing(b: &mut Bits, e: &mut Exss, mix_out: &[u32]) -> Result<(), String> {
    if e.mix_metadata && b.flag() {
        b.skip(1 + 6);
        if b.get(2) == 3 {
            b.skip(8);
        } else {
            b.skip(3);
        }
        if b.flag() {
            for n in mix_out {
                b.skip(6 * *n as usize);
            }
        } else {
            b.skip(6 * mix_out.len());
        }
        let mut nd = e.nchannels as usize;
        if e.embedded_6ch {
            nd += 6;
        }
        if e.embedded_stereo {
            nd += 2;
        }
        for n in mix_out {
            if *n == 0 {
                return Err("invalid speaker layout mask for a mixing configuration".into());
            }
            for _ in 0..nd {
                let m = b.get(*n as usize);
                b.skip(mask_count(m) as usize * 6);
            }
        }
    }
    Ok(())
}

/// How the asset is coded and the sizes of its parts.
fn parse_coding_mode(b: &mut Bits, nbits: usize, e: &mut Exss) {
    e.coding_mode = b.get(2);
    match e.coding_mode {
        0 => {
            e.ext_mask = b.get(12);
            if e.ext_mask & 0x10 != 0 {
                e.core_size = b.get(14) as usize + 1;
                if b.flag() {
                    b.skip(2);
                }
            }
            if e.ext_mask & 0x20 != 0 {
                e.xbr_size = b.get(14) as usize + 1;
            }
            if e.ext_mask & 0x40 != 0 {
                e.xxch_size = b.get(14) as usize + 1;
            }
            if e.ext_mask & 0x80 != 0 {
                e.x96_size = b.get(12) as usize + 1;
            }
            if e.ext_mask & 0x100 != 0 {
                e.lbr_size = b.get(14) as usize + 1;
                if b.flag() {
                    b.skip(2);
                }
            }
            if e.ext_mask & 0x200 != 0 {
                parse_xll_params(b, nbits, e);
            }
            if e.ext_mask & 0x400 != 0 {
                b.skip(16);
            }
            if e.ext_mask & 0x800 != 0 {
                b.skip(16);
            }
        }
        1 => {
            e.ext_mask = 0x200;
            parse_xll_params(b, nbits, e);
        }
        2 => {
            e.ext_mask = 0x100;
            e.lbr_size = b.get(14) as usize + 1;
            if b.flag() {
                b.skip(2);
            }
        }
        _ => {
            b.skip(14 + 8);
            if b.flag() {
                b.skip(3);
            }
        }
    }
    if e.ext_mask & 0x200 != 0 {
        e.hd_stream_id = b.get(3);
    }
}

/// Where each component of the asset starts, checking that they fit.
fn locate_components(e: &mut Exss) -> Result<(), String> {
    // component offsets inside the asset
    let mut off = e.header_size;
    let mut left = e.asset_size;
    let mut place = |present: bool, size: usize, offset: &mut usize| -> Result<(), String> {
        if present {
            *offset = off;
            if size > left {
                return Err(format!("a component ({size} bytes) is larger than the {left} bytes left in the asset (asset {} bytes, ext mask 0x{:X}, header {} bytes)", e.asset_size, e.ext_mask, e.header_size));
            }
            off += size;
            left -= size;
        }
        Ok(())
    };
    place(e.ext_mask & 0x10 != 0, e.core_size, &mut e.core_offset)?;
    place(e.ext_mask & 0x20 != 0, e.xbr_size, &mut e.xbr_offset)?;
    place(e.ext_mask & 0x40 != 0, e.xxch_size, &mut e.xxch_offset)?;
    place(e.ext_mask & 0x80 != 0, e.x96_size, &mut e.x96_offset)?;
    place(e.ext_mask & 0x100 != 0, e.lbr_size, &mut e.lbr_offset)?;
    place(e.ext_mask & 0x200 != 0, e.xll_size, &mut e.xll_offset)?;
    Ok(())
}

pub(crate) fn parse_xll_params(b: &mut Bits, nbits: usize, e: &mut Exss) {
    e.xll_size = b.get(nbits) as usize + 1;
    e.xll_sync_present = b.flag();
    if e.xll_sync_present {
        e.xll_pbr = b.get(4);
        let n = b.get(5) as usize + 1;
        e.xll_delay = b.get(n);
        e.xll_sync_offset = b.get(nbits) as usize;
    }
}

pub(crate) fn exss_rows(e: &Exss) -> Vec<(String, String)> {
    let mut v = vec![
        ("substream index".to_string(), e.index.to_string()),
        (
            "header".to_string(),
            format!(
                "{} bytes ({}), checksum {}",
                e.header_size,
                if e.wide { "wide" } else { "short" },
                if e.hdr_crc_ok { "ok" } else { "BAD" }
            ),
        ),
        ("substream size".to_string(), format!("{} bytes", e.size)),
    ];
    if e.static_fields {
        v.push((
            "reference clock".to_string(),
            ["32 kHz", "44.1 kHz", "48 kHz", "reserved"][e.ref_clock as usize].to_string(),
        ));
        v.push((
            "frame duration".to_string(),
            format!(
                "code {} ({} samples at the clock)",
                e.frame_dur,
                (e.frame_dur + 1) * 512
            ),
        ));
        v.push((
            "presentations / assets".to_string(),
            format!("{} / {}", e.npresent, e.nassets),
        ));
        v.push(("mixing metadata".to_string(), e.mix_metadata.to_string()));
        v.push(("PCM resolution".to_string(), format!("{} bits", e.pcm_bits)));
        v.push(("maximum sample rate".to_string(), format!("{} Hz", e.max_rate)));
        v.push(("total channels".to_string(), e.nchannels.to_string()));
        if e.one_to_one {
            v.push((
                "embedded downmixes".to_string(),
                format!("stereo {}, 5.1 {}", e.embedded_stereo, e.embedded_6ch),
            ));
            v.push((
                "speaker activity mask".to_string(),
                format!("0x{:X} = {}", e.spk_mask, speakers(e.spk_mask)),
            ));
        }
    }
    v.push(("asset size".to_string(), format!("{} bytes", e.asset_size)));
    v.push((
        "coding mode".to_string(),
        ["several components", "lossless only", "low bit rate only", "auxiliary"][e.coding_mode as usize].to_string(),
    ));
    let mut comps = Vec::new();
    for (bit, name, size) in [
        (0x10, "core", e.core_size),
        (0x20, "XBR", e.xbr_size),
        (0x40, "XXCH", e.xxch_size),
        (0x80, "X96", e.x96_size),
        (0x100, "LBR", e.lbr_size),
        (0x200, "XLL", e.xll_size),
    ] {
        if e.ext_mask & bit != 0 {
            comps.push(format!("{name} {size} B"));
        }
    }
    v.push((
        "components in this substream".to_string(),
        if comps.is_empty() {
            "none".to_string()
        } else {
            comps.join(", ")
        },
    ));
    if e.ext_mask & 0xF != 0 {
        let low: Vec<&str> = [(1, "core"), (4, "X96")]
            .iter()
            .filter(|(b, _)| e.ext_mask & b != 0)
            .map(|(_, n)| *n)
            .collect();
        v.push((
            "components kept in the core substream".to_string(),
            format!(
                "mask 0x{:X}{}",
                e.ext_mask & 0xF,
                if low.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", low.join(", "))
                }
            ),
        ));
    }
    if e.ext_mask & 0x200 != 0 {
        v.push((
            "XLL sync".to_string(),
            if e.xll_sync_present {
                format!(
                    "at +{}, decoding delay {} frames, peak bit rate buffer code {}",
                    e.xll_sync_offset, e.xll_delay, e.xll_pbr
                )
            } else {
                "not signalled".to_string()
            },
        ));
        v.push(("hd stream id".to_string(), e.hd_stream_id.to_string()));
    }
    if let Some(d) = e.dialnorm {
        v.push(("dialogue normalization".to_string(), format!("-{d} dBFS")));
    }
    if let Some(d) = e.drc {
        v.push(("dynamic range code".to_string(), d.to_string()));
    }
    v
}

pub(crate) fn speakers(mask: u32) -> String {
    let names: Vec<&str> = (0..16).filter(|b| mask >> b & 1 == 1).map(|b| SPEAKERS[b].0).collect();
    if names.is_empty() {
        "none".to_string()
    } else {
        names.join(" + ")
    }
}
