//! DTS extension substream (EXSS) framing and the `.dtshd` file container.

use crate::bits::BitWriter;
use crate::xll::{crc16, fs_index};

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
    let nb4f = 16u32;
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
    a.put(1, 1); // channels are speaker feeds
    if p.channels > 2 {
        a.put(0, 1); // no embedded stereo downmix
    }
    if p.channels > 6 {
        a.put(0, 1); // no embedded 5.1 downmix
    }
    let mask_bits: u32 = if p.spk_mask <= 0xF { 4 } else if p.spk_mask <= 0xFF { 8 } else if p.spk_mask <= 0xFFF { 12 } else { 16 };
    a.put(1, 1); // speaker mask enabled
    a.put(mask_bits / 4 - 1, 2);
    a.put(p.spk_mask, mask_bits);
    a.put(0, 3); // no remapping sets
    a.put(0, 1); // no dynamic range coefficient
    if p.dialnorm > 0 {
        a.put(1, 1);
        a.put(p.dialnorm, 5);
    } else {
        a.put(0, 1);
    }
    if p.with_core {
        a.put(0, 2); // coding mode: core plus extensions
        a.put(0x201, 12); // core in the core substream + XLL
        a.put(xll.len() as u32 - 1, nb4f);
    } else {
        a.put(1, 2); // lossless only
        a.put(xll.len() as u32 - 1, nb4f);
    }
    a.put(1, 1); // XLL sync word present
    a.put(0, 4); // peak bit rate smoothing buffer
    a.put(0, 5); // 1-bit initial decoding delay field
    a.put(0, 1);
    a.put(0, nb4f); // sync word offset
    a.put(0, 3); // stream id
    a.put(0, 1); // decode in primary decoder
    a.put(0, 1); // no revision 2 DRC
    a.align(8);
    let mut ab = a.into_bytes();
    let alen = ab.len();
    assert!(alen <= 512);
    // patch the 9-bit size field (value alen - 1)
    let v = (alen - 1) as u32;
    ab[0] = (v >> 1) as u8;
    ab[1] = (ab[1] & 0x7F) | (((v & 1) as u8) << 7);

    // ---- header
    let bits_before_desc = 32 + 8 + 2 + 1 + 8 + nb4f as usize + 1 + 2 + 3 + 1 + 3 + 3 + 1 + 8 + 1 + nb4f as usize;
    let bits_total = bits_before_desc + alen * 8 + 1;
    let hsize = bits_total.div_ceil(8) + 2;
    let frame_size = hsize + xll.len();
    let mut h = BitWriter::new();
    h.put(SYNC_EXSS, 32);
    h.put(0, 8); // user defined bits
    h.put(0, 2); // extension substream index
    h.put(0, 1); // short header sizes
    h.put(hsize as u32 - 1, 8);
    h.put(frame_size as u32 - 1, nb4f);
    h.put(1, 1); // static fields present
    h.put(ref_clock_code(p.sample_rate), 2);
    h.put(p.frame_duration_code, 3);
    h.put(0, 1); // no time stamp
    h.put(0, 3); // one audio presentation
    h.put(0, 3); // one asset
    h.put(1, 1); // active extension substream mask
    h.put(1, 8); // active asset mask
    h.put(0, 1); // no mixing metadata
    h.put(xll.len() as u32 - 1, nb4f); // asset size
    for b in &ab {
        h.put(*b as u32, 8);
    }
    h.put(0, 1); // no backward compatible core in an extension
    h.align(8);
    let mut hb = h.into_bytes();
    assert_eq!(hb.len() + 2, hsize);
    let crc = crc16(&hb[5..]);
    hb.extend_from_slice(&crc.to_be_bytes());
    hb.extend_from_slice(xll);
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
    aupr.extend_from_slice(&[0, 0, 0, 0]);
    chunk(b"AUPR-HDR", &aupr);
    out.extend_from_slice(b"STRMDATA");
    out.extend_from_slice(&data_len.to_be_bytes());
    out
}
