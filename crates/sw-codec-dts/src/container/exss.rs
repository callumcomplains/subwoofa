//! DTS extension substream (EXSS) framing and the `.dtshd` file container.

use crate::bits::BitWriter;
use crate::lossless::xll::{crc16, fs_index};

pub const SYNC_EXSS: u32 = 0x6458_2025;

#[derive(Debug, Clone)]
pub struct ExssParams {
    pub sample_rate: u32,
    pub channels: usize,
    pub bit_res: u32,
    /// Loudspeaker activity mask of the asset (Table 7-10).
    pub spk_mask: u32,
    /// The XLL data is a residual of a core carried in the core substream.
    pub with_core: bool,
    /// Dialog normalisation attenuation in dB (0 = none), 0..=31.
    pub dialnorm: u32,
    /// Frame duration code (duration = 512 * (code + 1) reference clock ticks).
    pub frame_duration_code: u32,
    /// The core frames carry an X96 extension (a 96/24 core).
    pub x96_in_core: bool,
    /// The core frames carry an XCH extension (the sixth channel of DTS-ES discrete).
    pub xch_in_core: bool,
    /// DTS Express as Blu-ray secondary audio: extension substream index 2, mixed with the primary audio.
    pub secondary: bool,
    /// Details that only some streams need.
    pub extras: ExssExtras,
}

/// Optional parts of an extension substream header, measured on the original encoder's output.
#[derive(Debug, Clone, Default)]
pub struct ExssExtras {
    /// Mixing metadata of this frame (secondary audio): the attenuation of the primary audio, and for a mono
    /// secondary where it goes in the output speakers.
    pub mix: Option<crate::container::mixmeta::FrameMix>,
    /// Smallest header size in bytes. The original encoder pads its headers with zero bytes to a fixed size
    /// per stream type (32 for lossless, 28 otherwise) and, beyond that, to a multiple of four. 0 writes the
    /// header as small as it can be.
    pub min_header: usize,
    /// The asset says that it embeds a stereo downmix.
    pub emb_stereo: bool,
    /// The asset says that it embeds a 5.1 downmix (XXCH and XCH streams always do).
    pub emb_5_1: bool,
    /// Add the six speaker remapping sets of a wide 7.1 stream.
    pub wide_remap: bool,
    /// Describe the channels by a representation type instead of speakers (2 for Lt/Rt matrix-encoded stereo).
    pub repr_type: Option<u32>,
}

/// Speaker remapping sets of a wide 7.1 stream (L R C LFE Ls Rs Lw Rw), as the original encoder writes them: for
/// each target layout its speaker mask, the number of decoded channels (8), and per target speaker which decoded
/// channels feed it with which 5-bit remapping code (30 is 0 dB, 12 is -9 dB, ...).
pub fn wide_remap_sets() -> (Vec<u32>, Vec<Vec<(u32, Vec<u32>)>>) {
    let masks = vec![2123, 8207, 47, 79, 159, 287];
    let c = |m: u32, g: u32| (m, vec![g]);
    let rows: Vec<Vec<(u32, Vec<u32>)>> = vec![
        vec![
            c(1, 30),
            c(2, 30),
            c(4, 30),
            c(32, 30),
            c(8, 30),
            c(16, 30),
            c(64, 30),
            c(128, 30),
        ],
        vec![
            c(1, 30),
            c(2, 30),
            c(4, 30),
            c(8, 30),
            c(16, 30),
            c(32, 30),
            c(64, 12),
            c(128, 12),
        ],
        vec![
            c(1, 30),
            c(2, 30),
            c(4, 30),
            c(8, 30),
            c(16, 30),
            c(32, 30),
            (0, vec![]),
            (0, vec![]),
        ],
        vec![
            c(1, 30),
            (66, vec![30, 12]),
            (132, vec![30, 12]),
            (72, vec![24, 18]),
            (144, vec![24, 18]),
            c(32, 30),
            c(8, 24),
            c(16, 24),
        ],
        vec![
            c(1, 30),
            (66, vec![30, 12]),
            (132, vec![30, 12]),
            (72, vec![30, 18]),
            (144, vec![30, 18]),
            c(32, 30),
            (24, vec![6, 6]),
            (0, vec![]),
        ],
        vec![
            c(1, 30),
            (66, vec![30, 12]),
            (132, vec![30, 12]),
            (72, vec![30, 18]),
            (144, vec![30, 18]),
            c(32, 30),
            (24, vec![6, 6]),
            (0, vec![]),
        ],
    ];
    (masks, rows)
}

fn ref_clock_code(rate: u32) -> u32 {
    match rate {
        32000 | 64000 | 128000 => 0,
        44100 | 88200 | 176400 => 1,
        _ => 2,
    }
}

/// Build one extension substream frame around an XLL frame.
pub fn exss_frame(p: &ExssParams, xll: &[u8]) -> Vec<u8> {
    exss_frame_ext(p, None, Some(xll))
}

/// Build one extension substream frame carrying an XBR extension and/or an XLL frame (the core, if any,
/// lives in the core substream).
pub fn exss_frame_ext(p: &ExssParams, xbr: Option<&[u8]>, xll: Option<&[u8]>) -> Vec<u8> {
    exss_build(p, xbr, xll, None, None)
}

/// Build one extension substream frame carrying an XXCH extension (the core is in the core substream).
pub fn exss_frame_xxch(p: &ExssParams, xxch: &[u8]) -> Vec<u8> {
    exss_build(p, None, None, None, Some(xxch))
}

/// Extension substream frame with XBR (refining the core channels) and XXCH (the extra channels).
pub fn exss_frame_xbr_xxch(p: &ExssParams, xbr: &[u8], xxch: &[u8]) -> Vec<u8> {
    exss_build(p, Some(xbr), None, None, Some(xxch))
}

/// Build one extension substream frame around an LBR (DTS Express) payload.
pub fn exss_frame_lbr(p: &ExssParams, lbr: &[u8]) -> Vec<u8> {
    exss_build(p, None, None, Some(lbr), None)
}

/// Extension substream frame with XXCH (the extra channels) and X96 (the 96 kHz band of every channel); the
/// core stays in the core substream.
/// Extension substream frame with X96 alone (the core, with or without XCH, is in the core substream).
pub fn exss_frame_x96(p: &ExssParams, x96: &[u8]) -> Vec<u8> {
    exss_build_x(p, None, None, None, None, Some(x96))
}

pub fn exss_frame_xxch_x96(p: &ExssParams, xxch: &[u8], x96: &[u8]) -> Vec<u8> {
    exss_build_x(p, None, None, None, Some(xxch), Some(x96))
}

fn exss_build(
    p: &ExssParams,
    xbr: Option<&[u8]>,
    xll: Option<&[u8]>,
    lbr: Option<&[u8]>,
    xxch: Option<&[u8]>,
) -> Vec<u8> {
    exss_build_x(p, xbr, xll, lbr, xxch, None)
}

fn exss_build_x(
    p: &ExssParams,
    xbr: Option<&[u8]>,
    xll: Option<&[u8]>,
    lbr: Option<&[u8]>,
    xxch: Option<&[u8]>,
    x96: Option<&[u8]>,
) -> Vec<u8> {
    let nb4f = 16u32;
    let payload = xbr.map_or(0, |x| x.len())
        + xll.map_or(0, |x| x.len())
        + lbr.map_or(0, |x| x.len())
        + xxch.map_or(0, |x| x.len())
        + x96.map_or(0, |x| x.len());
    // ---- asset descriptor (its own byte size)
    let mut a = BitWriter::new();
    a.put(0, 9); // size placeholder
    a.put(0, 3); // asset index
    a.put(0, 1); // no asset type
    a.put(0, 1); // no language
    a.put(0, 1); // no info text
    a.put(p.bit_res - 1, 5);
    a.put(fs_index(p.sample_rate), 4);
    a.put(p.channels as u32 - 1, 8);
    // a mono secondary stream is not tied to a speaker: its mixing matrix says where it goes
    let mono_secondary = lbr.is_some() && p.secondary && p.channels == 1 && p.extras.mix.is_some();
    if mono_secondary || p.extras.repr_type.is_some() {
        a.put(0, 1); // representation type instead of a speaker mapping
        a.put(p.extras.repr_type.unwrap_or(0), 3);
    } else {
        a.put(1, 1); // channels are speaker feeds
        if p.channels > 2 {
            a.put(u32::from(p.extras.emb_stereo), 1); // embedded stereo downmix
        }
        if p.channels > 6 {
            a.put(u32::from(xxch.is_some() || p.xch_in_core || p.extras.emb_5_1), 1);
            // embedded 5.1 downmix (in the XXCH extension, in the 5.1 core of an XCH stream, or in the XLL hierarchy)
        }
        let mask_bits: u32 = if p.extras.wide_remap {
            16
        } else if p.spk_mask <= 0xF {
            4
        } else if p.spk_mask <= 0xFF {
            8
        } else if p.spk_mask <= 0xFFF {
            12
        } else {
            16
        };
        a.put(1, 1); // speaker mask enabled
        a.put(mask_bits / 4 - 1, 2);
        a.put(p.spk_mask, mask_bits);
        if p.extras.wide_remap {
            let (masks, rows) = wide_remap_sets();
            a.put(masks.len() as u32, 3);
            for m in &masks {
                a.put(*m, mask_bits);
            }
            for set in &rows {
                a.put(p.channels as u32 - 1, 5); // decoded channels the sets refer to
                for (mask, codes) in set {
                    a.put(*mask, p.channels as u32);
                    for c in codes {
                        a.put(*c, 5);
                    }
                }
            }
        } else {
            a.put(0, 3); // no remapping sets
        }
    }
    a.put(0, 1); // no dynamic range coefficient
    if p.dialnorm > 0 {
        a.put(1, 1);
        a.put(p.dialnorm, 5);
    } else {
        a.put(0, 1);
    }
    let secondary = lbr.is_some() && p.secondary;
    if secondary {
        match (&p.extras.mix, mono_secondary) {
            (Some(m), true) => {
                a.put(1, 1); // mixing metadata present
                a.put(1, 1); // mixed externally
                a.put(30, 6); // gain of the secondary audio itself
                a.put(2, 2); // DRC prior to mixing: limited
                a.put(7, 3);
                a.put(0, 1); // one scaling code for all outputs
                a.put(m.scale as u32, 6); // attenuation of the primary audio
                a.put(m.speakers as u32, 6); // speakers fed by the mono signal
                for g in &m.gains {
                    a.put(*g as u32, 6);
                }
            }
            _ => a.put(0, 1), // no mixing matrix: the channels go to their own speakers
        }
    }
    let write_xll = |a: &mut BitWriter, x: &[u8]| {
        a.put(x.len() as u32 - 1, nb4f);
        a.put(1, 1); // XLL sync word present
        a.put(0, 4); // peak bit rate smoothing buffer
        a.put(0, 5); // 1-bit initial decoding delay field
        a.put(0, 1);
        a.put(0, nb4f); // sync word offset
    };
    if let Some(l) = lbr {
        a.put(2, 2); // coding mode: low bit rate
        a.put(l.len() as u32 - 1, 14);
        a.put(1, 1); // LBR sync word present in every frame
        a.put(0, 2); // sync distance 0 frames
    } else if p.with_core || xbr.is_some() || xxch.is_some() || x96.is_some() {
        a.put(0, 2); // coding mode: core plus extensions
        let mask = (if p.with_core { 0x001 } else { 0 })
            | (if p.x96_in_core { 0x004 } else { 0 })
            | (if p.xch_in_core { 0x008 } else { 0 })
            | (if xbr.is_some() { 0x020 } else { 0 })
            | (if xxch.is_some() { 0x040 } else { 0 })
            | (if x96.is_some() { 0x080 } else { 0 })
            | (if xll.is_some() { 0x200 } else { 0 });
        a.put(mask, 12); // core in the core substream, XBR, XLL
        if let Some(b) = xbr {
            a.put(b.len() as u32 - 1, 14);
        }
        if let Some(x) = xxch {
            a.put(x.len() as u32 - 1, 14);
        }
        if let Some(x) = x96 {
            a.put(x.len() as u32 - 1, 12);
        }
        if let Some(x) = xll {
            write_xll(&mut a, x);
        }
    } else {
        a.put(1, 2); // lossless only
        write_xll(&mut a, xll.expect("an XLL frame"));
    }
    if xll.is_some() {
        a.put(0, 3); // stream id
    }
    if secondary {
        if !mono_secondary {
            // channels mapped one to one to speakers carry the attenuation of the primary audio here
            a.put(1, 1); // one-to-one mixing
            a.put(0, 1); // one scaling code for all channels
            a.put(p.extras.mix.as_ref().map_or(49, |m| m.scale as u32), 6);
        }
        a.put(1, 1); // decoded by the secondary decoder
        a.put(0, 1); // no revision 2 DRC
    } else {
        a.put(0, 1); // decode in primary decoder
        if xll.is_some() {
            a.put(0, 1); // no revision 2 DRC
        }
    }
    a.align(8);
    let mut ab = a.into_bytes();
    let alen = ab.len();
    assert!(alen <= 512);
    // patch the 9-bit size field (value alen - 1)
    let v = (alen - 1) as u32;
    ab[0] = (v >> 1) as u8;
    ab[1] = (ab[1] & 0x7F) | (((v & 1) as u8) << 7);

    // ---- header
    // secondary audio: presentation mask of 3 bits plus two asset masks, then the mixing configuration (10 bits)
    let sel_bits = if secondary { 3 + 16 + 1 + 10 } else { 1 + 8 + 1 };
    let bits_before_desc = 32 + 8 + 2 + 1 + 8 + nb4f as usize + 1 + 2 + 3 + 1 + 3 + 3 + sel_bits + nb4f as usize;
    let bits_total = bits_before_desc + alen * 8 + if p.with_core { 6 } else { 1 };
    let natural = bits_total.div_ceil(8) + 2;
    // the original pads its headers with zero bytes to a multiple of four
    let hsize = natural.div_ceil(4) * 4;
    // whole 32-bit words: DTS-HD tools (StreamTools) refuse streams whose frames are not padded to 4 bytes
    let pad = (4 - (hsize + payload) % 4) % 4;
    let frame_size = hsize + payload + pad;
    let mut h = BitWriter::new();
    h.put(SYNC_EXSS, 32);
    h.put(0, 8); // user defined bits
    h.put(if secondary { 2 } else { 0 }, 2); // extension substream index (secondary audio lives in 2)
    h.put(0, 1); // short header sizes
    h.put(hsize as u32 - 1, 8);
    h.put(frame_size as u32 - 1, nb4f);
    h.put(1, 1); // static fields present
    h.put(ref_clock_code(p.sample_rate), 2);
    h.put(p.frame_duration_code, 3);
    h.put(0, 1); // no time stamp
    h.put(0, 3); // one audio presentation
    h.put(0, 3); // one asset
    if secondary {
        // the presentation mixes the primary audio (substream 0) with this secondary stream (substream 2)
        h.put(0b101, 3);
        h.put(1, 8);
        h.put(1, 8);
        h.put(1, 1); // mixing metadata present
        h.put(0, 2); // adjustment level
        h.put(0, 2); // 4 bits per output mask
        h.put(0, 2); // one mix output configuration
        h.put(0xF, 4);
    } else {
        h.put(1, 1); // active extension substream mask
        h.put(1, 8); // active asset mask
        h.put(0, 1); // no mixing metadata
    }
    h.put(payload as u32 - 1, nb4f); // asset size
    for b in &ab {
        h.put(*b as u32, 8);
    }
    if p.with_core {
        // the backward compatible core sits in the core substream (substream 0, asset 0), as in the commercial streams
        h.put(1, 1);
        h.put(0, 2);
        h.put(0, 3);
    } else {
        h.put(0, 1); // no backward compatible core
    }
    h.align(8);
    let mut hb = h.into_bytes();
    assert_eq!(hb.len() + 2, natural);
    hb.resize(hsize - 2, 0); // the original pads its headers up to a fixed size
    let crc = crc16(&hb[5..]);
    hb.extend_from_slice(&crc.to_be_bytes());
    if let Some(x) = xbr {
        hb.extend_from_slice(x);
    }
    if let Some(x) = xxch {
        hb.extend_from_slice(x);
    }
    if let Some(x) = x96 {
        hb.extend_from_slice(x);
    }
    if let Some(x) = xll {
        hb.extend_from_slice(x);
    }
    if let Some(x) = lbr {
        hb.extend_from_slice(x);
    }
    hb.resize(hb.len() + pad, 0);
    hb
}

/// Container for `.dtshd` files: header chunks, then the raw stream.
pub struct DtsHdHeader {
    pub sample_rate: u32,
    /// Number of access units (frames) and samples per frame.
    pub frames: u32,
    pub samples_per_frame: u16,
    /// Total samples per channel of the original.
    pub total_samples: u64,
    pub ch_mask: u16,
    pub initial_padding: u16,
}

pub fn dtshd_header(h: &DtsHdHeader, data_len: u64) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chunk = |name: &[u8; 8], payload: &[u8]| {
        out.extend_from_slice(name);
        out.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        out.extend_from_slice(payload);
    };
    chunk(b"DTSHDHDR", &[0, 0, 0, 0, 0x80, 0, 0, 0, 0, 2, 0, 0x1d, 1, 0, 0, 0]);
    let mut aupr = Vec::new();
    aupr.extend_from_slice(&[0, 0, 5]);
    aupr.extend_from_slice(&h.sample_rate.to_be_bytes()[1..]);
    aupr.extend_from_slice(&h.frames.to_be_bytes());
    aupr.extend_from_slice(&h.samples_per_frame.to_be_bytes());
    aupr.extend_from_slice(&((h.total_samples >> 8) as u32).to_be_bytes());
    aupr.push(h.total_samples as u8);
    aupr.extend_from_slice(&h.ch_mask.to_be_bytes());
    aupr.extend_from_slice(&h.initial_padding.to_be_bytes());
    aupr.extend_from_slice(&[0, 0, 0]);
    chunk(b"AUPR-HDR", &aupr);
    out.extend_from_slice(b"STRMDATA");
    out.extend_from_slice(&data_len.to_be_bytes());
    out
}

/// Container header for a DTS Express (LBR) stream: constant bit rate, extension substream only (no core).
pub fn dtshd_header_lbr(h: &DtsHdHeader, data_len: u64) -> Vec<u8> {
    let mut out = dtshd_header(h, data_len);
    out[16 + 11] = 0x10; // only the extension substream mask
    out[48 + 2] = 0x08; // LBR encoding, no backward compatible core, constant rate
    out
}
