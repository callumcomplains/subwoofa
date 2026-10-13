//! Structure checks for plain audio files: WAV / RF64, AIFF, CAF, FLAC (also in Ogg), ALAC / ALS in MP4, TTA,
//! WavPack, Monkey's Audio, raw ALS, MXF sound and DSD (DSF and DSDIFF). Each check reads the headers the way a player would and compares what they say with each other
//! and with the size of the file. Deep verification then decodes the audio (see `deep.rs`).

use crate::{Collector, Probe, Status, Window};

/// The file kinds this module understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AudioKind {
    Wav,
    Aiff,
    Caf,
    Flac,
    OggFlac,
    M4a,
    Tta,
    WavPack,
    Monkeys,
    Als,
    Mxf,
    Dsf,
    Dff,
}

impl AudioKind {
    pub fn name(self) -> &'static str {
        match self {
            AudioKind::Wav => "WAV / Broadcast Wave",
            AudioKind::Aiff => "AIFF",
            AudioKind::Caf => "CAF (Core Audio Format)",
            AudioKind::Flac => "FLAC",
            AudioKind::OggFlac => "FLAC in Ogg",
            AudioKind::M4a => "M4A (MPEG-4 audio)",
            AudioKind::Tta => "TTA (True Audio)",
            AudioKind::WavPack => "WavPack",
            AudioKind::Monkeys => "Monkey's Audio",
            AudioKind::Als => "MPEG-4 ALS",
            AudioKind::Mxf => "MXF (digital cinema sound)",
            AudioKind::Dsf => "DSD (DSF)",
            AudioKind::Dff => "DSD (DSDIFF)",
        }
    }
}

/// Recognises the file kinds by their first bytes.
pub(crate) fn sniff(h: &[u8]) -> Option<AudioKind> {
    if h.len() >= 12 {
        if (h.starts_with(b"RIFF") || h.starts_with(b"RF64")) && &h[8..12] == b"WAVE" {
            return Some(AudioKind::Wav);
        }
        if h.starts_with(b"FORM") && (&h[8..12] == b"AIFF" || &h[8..12] == b"AIFC") {
            return Some(AudioKind::Aiff);
        }
        if h.starts_with(b"FRM8") && h.len() >= 16 && &h[12..16] == b"DSD " {
            return Some(AudioKind::Dff);
        }
        if &h[4..8] == b"ftyp" {
            return Some(AudioKind::M4a);
        }
    }
    if h.starts_with(b"caff") {
        return Some(AudioKind::Caf);
    }
    if h.starts_with(b"fLaC") {
        return Some(AudioKind::Flac);
    }
    if h.starts_with(b"OggS") && h.len() >= 33 && h[28..33] == [0x7F, b'F', b'L', b'A', b'C'] {
        return Some(AudioKind::OggFlac);
    }
    if h.starts_with(b"TTA1") {
        return Some(AudioKind::Tta);
    }
    if h.starts_with(&[
        0x06, 0x0e, 0x2b, 0x34, 0x02, 0x05, 0x01, 0x01, 0x0d, 0x01, 0x02, 0x01, 0x01, 0x02,
    ]) {
        return Some(AudioKind::Mxf);
    }
    if h.starts_with(b"ALS\0") {
        return Some(AudioKind::Als);
    }
    if h.starts_with(b"MAC ") {
        return Some(AudioKind::Monkeys);
    }
    if h.starts_with(b"wvpk") {
        return Some(AudioKind::WavPack);
    }
    if h.starts_with(b"DSD ") {
        return Some(AudioKind::Dsf);
    }
    None
}

/// What the headers say about the audio.
#[derive(Default, Clone)]
struct Facts {
    rate: u32,
    channels: u32,
    bits: u32,
    /// Samples per channel, when the headers give it.
    frames: Option<u64>,
    lossless: bool,
}

pub(crate) fn verify(w: &mut Window, c: &mut Collector, kind: AudioKind) -> Probe {
    c.check("Recognised file", Status::Pass, kind.name());
    let facts = match kind {
        AudioKind::Wav => wav(w, c),
        AudioKind::Aiff => aiff(w, c),
        AudioKind::Caf => caf(w, c),
        AudioKind::Flac => flac(w, c),
        AudioKind::OggFlac => ogg_flac(w, c),
        AudioKind::M4a => m4a(w, c),
        AudioKind::Tta => tta(w, c),
        AudioKind::WavPack => wavpack(w, c),
        AudioKind::Monkeys => monkeys(w, c),
        AudioKind::Als => als(w, c),
        AudioKind::Mxf => mxf(w, c),
        AudioKind::Dsf => dsf(w, c),
        AudioKind::Dff => dff(w, c),
    };
    let duration = match (facts.frames, facts.rate) {
        (Some(f), r) if r > 0 => f as f64 / r as f64,
        _ => 0.0,
    };
    if facts.rate > 0 {
        c.kv("Sample rate", format!("{} Hz", facts.rate));
    }
    if facts.channels > 0 {
        c.kv("Channels", facts.channels.to_string());
    }
    if facts.bits > 0 {
        c.kv(
            "Sample format",
            if kind == AudioKind::Dsf || kind == AudioKind::Dff {
                "1-bit DSD".to_string()
            } else {
                format!("{}-bit", facts.bits)
            },
        );
    }
    if let Some(f) = facts.frames {
        c.kv("Length", format!("{f} samples per channel ({duration:.3} s)"));
    }
    Probe {
        duration_secs: duration,
        sample_rate: facts.rate,
        channels: facts.channels,
        lossless: facts.lossless,
        ..Default::default()
    }
}

// ------------------------------------------------------------------------------------------------
// byte helpers

fn le16(d: &[u8]) -> u16 {
    u16::from_le_bytes([d[0], d[1]])
}
fn le32(d: &[u8]) -> u32 {
    u32::from_le_bytes([d[0], d[1], d[2], d[3]])
}
fn le64(d: &[u8]) -> u64 {
    u64::from_le_bytes(d[..8].try_into().unwrap())
}
fn be16(d: &[u8]) -> u16 {
    u16::from_be_bytes([d[0], d[1]])
}
fn be32(d: &[u8]) -> u32 {
    u32::from_be_bytes([d[0], d[1], d[2], d[3]])
}
fn be64(d: &[u8]) -> u64 {
    u64::from_be_bytes(d[..8].try_into().unwrap())
}

/// Whether the given size claims fit the file; reports the outcome as one check.
fn size_check(c: &mut Collector, name: &str, claimed: u64, actual: u64) {
    if claimed == actual {
        c.check(name, Status::Pass, format!("{actual} bytes, as the header says"));
    } else if claimed < actual {
        c.check(
            name,
            Status::Warn,
            format!("the header accounts for {claimed} bytes but the file has {actual} (trailing data)"),
        );
    } else {
        c.check(
            name,
            Status::Fail,
            format!("the header says {claimed} bytes but the file has only {actual} (truncated)"),
        );
    }
}

// ------------------------------------------------------------------------------------------------
// WAV

fn wav(w: &mut Window, c: &mut Collector) -> Facts {
    let len = w.len;
    let head = w.get(0, 12).to_vec();
    let rf64 = &head[..4] == b"RF64";
    let riff_size = le32(&head[4..8]);
    if rf64 {
        c.check(
            "RF64 marker",
            if riff_size == u32::MAX {
                Status::Pass
            } else {
                Status::Warn
            },
            "RF64 files keep the real sizes in the ds64 chunk and 0xFFFFFFFF in the RIFF header",
        );
    } else {
        size_check(c, "RIFF size", riff_size as u64 + 8, len);
    }
    let mut facts = Facts {
        lossless: true,
        ..Default::default()
    };
    let mut pos = 12u64;
    let (mut fmt_seen, mut data_seen) = (false, false);
    let mut ds64_data: Option<u64> = None;
    let mut block_align = 0u32;
    while pos + 8 <= len {
        let h = w.get(pos, 8).to_vec();
        if h.len() < 8 {
            break;
        }
        let id = [h[0], h[1], h[2], h[3]];
        let mut size = le32(&h[4..8]) as u64;
        let body = pos + 8;
        match &id {
            b"ds64" => {
                let d = w.get(body, 28).to_vec();
                if d.len() >= 24 {
                    ds64_data = Some(le64(&d[8..16]));
                    facts.frames = Some(le64(&d[16..24]));
                }
            }
            b"fmt " => {
                let d = w.get(body, (size as usize).min(40)).to_vec();
                if d.len() >= 16 {
                    fmt_seen = true;
                    let tag = le16(&d[0..2]);
                    facts.channels = le16(&d[2..4]) as u32;
                    facts.rate = le32(&d[4..8]);
                    let byte_rate = le32(&d[8..12]);
                    block_align = le16(&d[12..14]) as u32;
                    facts.bits = le16(&d[14..16]) as u32;
                    let mut format = match tag {
                        1 => "PCM".to_string(),
                        3 => "IEEE float".to_string(),
                        0xFFFE => "extensible".to_string(),
                        other => format!("format tag {other:#x}"),
                    };
                    if tag == 0xFFFE && d.len() >= 40 {
                        let mask = le32(&d[20..24]);
                        let sub = le16(&d[24..26]);
                        format = format!(
                            "extensible, {}, channel mask {mask:#x}",
                            if sub == 3 {
                                "float"
                            } else if sub == 1 {
                                "PCM"
                            } else {
                                "other"
                            }
                        );
                    }
                    c.kv("Format", format);
                    let consistent =
                        block_align == facts.channels * facts.bits.div_ceil(8) && byte_rate == facts.rate * block_align;
                    c.check(
                        "Format header",
                        if consistent { Status::Pass } else { Status::Fail },
                        if consistent {
                            format!(
                                "{} channels, {} bits, {} Hz; block size and byte rate agree",
                                facts.channels, facts.bits, facts.rate
                            )
                        } else {
                            "block alignment or byte rate disagree with channels, bits and rate".to_string()
                        },
                    );
                }
            }
            b"data" => {
                data_seen = true;
                if size == u32::MAX as u64 {
                    if let Some(real) = ds64_data {
                        size = real;
                    }
                }
                let end = body + size;
                if end > len {
                    c.check(
                        "Audio data size",
                        Status::Fail,
                        format!(
                            "the data chunk says {size} bytes but only {} remain (truncated)",
                            len - body
                        ),
                    );
                } else {
                    c.check("Audio data size", Status::Pass, format!("{size} bytes of audio"));
                }
                if block_align > 0 {
                    facts.frames = Some(size.min(len - body) / block_align as u64);
                    if !size.is_multiple_of(block_align as u64) {
                        c.check(
                            "Whole frames",
                            Status::Warn,
                            "the audio data is not a whole number of frames",
                        );
                    }
                }
            }
            b"bext" => {
                let d = w.get(body, (size as usize).min(612)).to_vec();
                if d.len() >= 346 {
                    let reference = le32(&d[338..342]) as u64 | (le32(&d[342..346]) as u64) << 32;
                    c.kv("Broadcast time reference", format!("{reference} samples"));
                }
            }
            _ => {}
        }
        pos = body + size + size % 2;
    }
    c.check(
        "Required chunks",
        if fmt_seen && data_seen {
            Status::Pass
        } else {
            Status::Fail
        },
        if fmt_seen && data_seen {
            "fmt and data are present".to_string()
        } else {
            "the fmt or data chunk is missing".to_string()
        },
    );
    facts
}

// ------------------------------------------------------------------------------------------------
// AIFF

fn aiff(w: &mut Window, c: &mut Collector) -> Facts {
    let len = w.len;
    let head = w.get(0, 12).to_vec();
    size_check(c, "FORM size", be32(&head[4..8]) as u64 + 8, len);
    let mut facts = Facts {
        lossless: true,
        ..Default::default()
    };
    let (mut comm, mut ssnd) = (false, false);
    let mut pos = 12u64;
    while pos + 8 <= len {
        let h = w.get(pos, 8).to_vec();
        if h.len() < 8 {
            break;
        }
        let size = be32(&h[4..8]) as u64;
        let body = pos + 8;
        match &h[..4] {
            b"COMM" => {
                let d = w.get(body, size.min(30) as usize).to_vec();
                if d.len() >= 18 {
                    comm = true;
                    facts.channels = be16(&d[0..2]) as u32;
                    facts.frames = Some(be32(&d[2..6]) as u64);
                    facts.bits = be16(&d[6..8]) as u32;
                    let exponent = (be16(&d[8..10]) & 0x7FFF) as i32 - 16383;
                    let mantissa = be64(&d[10..18]);
                    facts.rate = if (0..=63).contains(&exponent) {
                        (mantissa >> (63 - exponent)) as u32
                    } else {
                        0
                    };
                    if d.len() >= 22 {
                        c.kv("Compression", String::from_utf8_lossy(&d[18..22]).to_string());
                    }
                }
            }
            b"SSND" => {
                ssnd = true;
                if let (Some(frames), true) = (facts.frames, facts.channels > 0) {
                    let want = frames * facts.channels as u64 * facts.bits.div_ceil(8) as u64 + 8;
                    c.check(
                        "Audio data size",
                        if size >= want { Status::Pass } else { Status::Fail },
                        format!(
                            "the SSND chunk holds {} bytes of audio, the header's frame count needs {}",
                            size.saturating_sub(8),
                            want - 8
                        ),
                    );
                }
            }
            _ => {}
        }
        pos = body + size + size % 2;
    }
    c.check(
        "Required chunks",
        if comm && ssnd { Status::Pass } else { Status::Fail },
        if comm && ssnd {
            "COMM and SSND are present".to_string()
        } else {
            "the COMM or SSND chunk is missing".to_string()
        },
    );
    facts
}

// ------------------------------------------------------------------------------------------------
// CAF

fn caf(w: &mut Window, c: &mut Collector) -> Facts {
    let len = w.len;
    let mut facts = Facts {
        lossless: true,
        ..Default::default()
    };
    let (mut desc, mut data) = (false, false);
    let mut pos = 8u64;
    let mut bytes_per_frame = 0u32;
    while pos + 12 <= len {
        let h = w.get(pos, 12).to_vec();
        if h.len() < 12 {
            break;
        }
        let size = be64(&h[4..12]) as i64;
        let body = pos + 12;
        match &h[..4] {
            b"desc" => {
                let d = w.get(body, 32).to_vec();
                if d.len() >= 32 {
                    desc = true;
                    facts.rate = f64::from_bits(be64(&d[0..8])) as u32;
                    // after the sample rate, format id and flags: bytes per packet, frames per packet, channels, bits
                    let frames_per_packet = be32(&d[20..24]).max(1);
                    bytes_per_frame = be32(&d[16..20]) / frames_per_packet;
                    facts.channels = be32(&d[24..28]);
                    facts.bits = be32(&d[28..32]);
                    c.kv("Format", format!("{}", String::from_utf8_lossy(&d[8..12])));
                }
            }
            b"data" => {
                data = true;
                let audio = if size < 0 { len - body } else { size as u64 };
                let audio = audio.saturating_sub(4); // edit count
                if size >= 0 && body + size as u64 > len {
                    c.check(
                        "Audio data size",
                        Status::Fail,
                        "the data chunk runs past the end of the file (truncated)",
                    );
                } else {
                    c.check("Audio data size", Status::Pass, format!("{audio} bytes of audio"));
                }
                if bytes_per_frame > 0 {
                    facts.frames = Some(audio / bytes_per_frame as u64);
                }
            }
            _ => {}
        }
        if size < 0 {
            break;
        }
        pos = body + size as u64;
    }
    c.check(
        "Required chunks",
        if desc && data { Status::Pass } else { Status::Fail },
        if desc && data {
            "desc and data are present".to_string()
        } else {
            "the desc or data chunk is missing".to_string()
        },
    );
    facts
}

// ------------------------------------------------------------------------------------------------
// FLAC

/// STREAMINFO fields, from its 34 bytes.
fn streaminfo(c: &mut Collector, d: &[u8]) -> Facts {
    let packed = be64(&d[10..18]);
    let rate = (packed >> 44) as u32;
    let channels = ((packed >> 41) & 7) as u32 + 1;
    let bits = ((packed >> 36) & 31) as u32 + 1;
    let total = packed & 0xF_FFFF_FFFF;
    let md5: String = d[18..34].iter().map(|b| format!("{b:02x}")).collect();
    c.kv(
        "Stored audio MD5",
        if md5.chars().all(|ch| ch == '0') {
            "not set".to_string()
        } else {
            md5
        },
    );
    Facts {
        rate,
        channels,
        bits,
        frames: if total > 0 { Some(total) } else { None },
        lossless: true,
    }
}

fn flac(w: &mut Window, c: &mut Collector) -> Facts {
    let len = w.len;
    let mut pos = 4u64;
    let mut facts = Facts {
        lossless: true,
        ..Default::default()
    };
    let mut have_info = false;
    loop {
        let h = w.get(pos, 4).to_vec();
        if h.len() < 4 {
            c.check("Metadata blocks", Status::Fail, "the file ends inside the metadata");
            return facts;
        }
        let last = h[0] & 0x80 != 0;
        let kind = h[0] & 0x7F;
        let size = u32::from_be_bytes([0, h[1], h[2], h[3]]) as u64;
        if kind == 0 && size == 34 {
            let d = w.get(pos + 4, 34).to_vec();
            if d.len() == 34 {
                facts = streaminfo(c, &d);
                have_info = true;
            }
        }
        if kind == 4 {
            let d = w.get(pos + 4, size.min(4096) as usize).to_vec();
            let text = String::from_utf8_lossy(&d).to_string();
            if let Some(i) = text.find("WAVEFORMATEXTENSIBLE_CHANNEL_MASK=") {
                c.kv(
                    "Channel mask",
                    text[i + 34..]
                        .chars()
                        .take_while(|ch| ch.is_ascii_alphanumeric())
                        .collect::<String>(),
                );
            }
        }
        pos += 4 + size;
        if last {
            break;
        }
    }
    c.check(
        "Metadata blocks",
        if have_info { Status::Pass } else { Status::Fail },
        if have_info {
            "STREAMINFO found".to_string()
        } else {
            "there is no STREAMINFO block".to_string()
        },
    );
    let first = w.get(pos, 2).to_vec();
    c.check(
        "First audio frame",
        if first.len() == 2 && first[0] == 0xFF && first[1] & 0xFE == 0xF8 {
            Status::Pass
        } else {
            Status::Fail
        },
        if first.len() == 2 && first[0] == 0xFF && first[1] & 0xFE == 0xF8 {
            format!("frame sync found at byte {pos}")
        } else {
            format!("no frame sync at byte {pos}, where the metadata ends")
        },
    );
    let _ = len;
    facts
}

/// CRC-32 of an Ogg page (polynomial 0x04C11DB7, most significant bit first).
fn ogg_crc(data: &[u8]) -> u32 {
    let mut crc = 0u32;
    for &b in data {
        crc ^= (b as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn ogg_flac(w: &mut Window, c: &mut Collector) -> Facts {
    let len = w.len;
    let mut facts = Facts {
        lossless: true,
        ..Default::default()
    };
    let (mut pos, mut pages, mut bad_crc, mut bad_order, mut sequence) = (0u64, 0u64, 0u64, 0u64, 0u32);
    let mut last_flag = false;
    let mut last_granule = 0u64;
    while pos + 27 <= len {
        let h = w.get(pos, 27).to_vec();
        if h.len() < 27 || &h[..4] != b"OggS" {
            c.check("Ogg pages", Status::Fail, format!("no page starts at byte {pos}"));
            return facts;
        }
        let segments = h[26] as usize;
        let table = w.get(pos + 27, segments).to_vec();
        let body: u64 = table.iter().map(|&s| s as u64).sum();
        let total = 27 + segments as u64 + body;
        if pos + total > len {
            c.check("Ogg pages", Status::Fail, "the last page is cut short");
            return facts;
        }
        let page = w.get(pos, total as usize).to_vec();
        let mut zeroed = page.clone();
        zeroed[22..26].fill(0);
        if ogg_crc(&zeroed) != le32(&page[22..26]) {
            bad_crc += 1;
        }
        let seq = le32(&page[18..22]);
        if pages > 0 && seq != sequence + 1 || (pages == 0 && seq != 0) {
            bad_order += 1;
        }
        sequence = seq;
        if pages == 0 {
            // identification packet: 0x7F FLAC, version, header count, fLaC, block header, STREAMINFO
            let d = &page[27 + segments..];
            if d.len() >= 13 + 4 + 34 && &d[9..13] == b"fLaC" {
                facts = streaminfo(c, &d[13 + 4..13 + 4 + 34]);
            }
        }
        let granule = le64(&page[6..14]);
        if granule != u64::MAX {
            last_granule = granule;
        }
        last_flag = page[5] & 4 != 0;
        pages += 1;
        pos += total;
    }
    c.check(
        "Ogg page checksums",
        if bad_crc == 0 { Status::Pass } else { Status::Fail },
        if bad_crc == 0 {
            format!("{pages} pages, every CRC is right")
        } else {
            format!("{bad_crc} of {pages} pages have a wrong CRC")
        },
    );
    c.check(
        "Ogg page order",
        if bad_order == 0 { Status::Pass } else { Status::Fail },
        if bad_order == 0 {
            "page numbers are consecutive".to_string()
        } else {
            format!("{bad_order} pages are out of sequence")
        },
    );
    c.check(
        "End of stream",
        if last_flag { Status::Pass } else { Status::Warn },
        if last_flag {
            "the last page carries the end flag".to_string()
        } else {
            "the last page is not marked as the end".to_string()
        },
    );
    if let Some(total) = facts.frames {
        c.check(
            "Granule position",
            if last_granule == total {
                Status::Pass
            } else {
                Status::Warn
            },
            format!("last page says {last_granule} samples, STREAMINFO says {total}"),
        );
    }
    facts
}

// ------------------------------------------------------------------------------------------------
// M4A

/// The boxes of a container, as (name, body start, body size).
fn boxes(w: &mut Window, start: u64, end: u64) -> Vec<([u8; 4], u64, u64)> {
    let mut out = Vec::new();
    let mut pos = start;
    while pos + 8 <= end {
        let h = w.get(pos, 16).to_vec();
        if h.len() < 8 {
            break;
        }
        let mut size = be32(&h[0..4]) as u64;
        let mut header = 8u64;
        if size == 1 && h.len() >= 16 {
            size = be64(&h[8..16]);
            header = 16;
        } else if size == 0 {
            size = end - pos;
        }
        if size < header || pos + size > end {
            break;
        }
        out.push(([h[4], h[5], h[6], h[7]], pos + header, size - header));
        pos += size;
    }
    out
}

fn find(list: &[([u8; 4], u64, u64)], name: &[u8; 4]) -> Option<(u64, u64)> {
    list.iter().find(|b| &b.0 == name).map(|b| (b.1, b.2))
}

fn m4a(w: &mut Window, c: &mut Collector) -> Facts {
    let len = w.len;
    let top = boxes(w, 0, len);
    let mut facts = Facts {
        lossless: true,
        ..Default::default()
    };
    let names: Vec<String> = top.iter().map(|b| String::from_utf8_lossy(&b.0).to_string()).collect();
    c.check("Top-level boxes", Status::Info, names.join(", "));
    let reached: u64 = top.iter().map(|b| b.1 + b.2).max().unwrap_or(0);
    size_check(c, "Boxes cover the file", reached, len);
    let Some((moov, moov_size)) = find(&top, b"moov") else {
        c.check("Movie box", Status::Fail, "there is no moov box");
        return facts;
    };
    let mdat = find(&top, b"mdat");
    let trak = boxes(w, moov, moov + moov_size);
    let Some((trak_start, trak_size)) = find(&trak, b"trak") else {
        c.check("Track", Status::Fail, "there is no track in the moov box");
        return facts;
    };
    let in_trak = boxes(w, trak_start, trak_start + trak_size);
    let Some((mdia, mdia_size)) = find(&in_trak, b"mdia") else {
        return facts;
    };
    let in_mdia = boxes(w, mdia, mdia + mdia_size);
    if let Some((mdhd, _)) = find(&in_mdia, b"mdhd") {
        let d = w.get(mdhd, 36).to_vec();
        if d.len() >= 32 && d[0] == 1 {
            facts.rate = be32(&d[20..24]);
            facts.frames = Some(be64(&d[24..32]));
        } else if d.len() >= 20 {
            facts.rate = be32(&d[12..16]);
            facts.frames = Some(be32(&d[16..20]) as u64);
        }
    }
    let Some((minf, minf_size)) = find(&in_mdia, b"minf") else {
        return facts;
    };
    let in_minf = boxes(w, minf, minf + minf_size);
    let Some((stbl, stbl_size)) = find(&in_minf, b"stbl") else {
        return facts;
    };
    let table = boxes(w, stbl, stbl + stbl_size);
    if let Some((stsd, size)) = find(&table, b"stsd") {
        let entry = boxes(w, stsd + 8, stsd + size);
        if let Some(([a, b, c4, d4], e, e_size)) = entry.first().cloned() {
            let name = String::from_utf8_lossy(&[a, b, c4, d4]).to_string();
            c.kv("Codec", name.clone());
            let kids = boxes(w, e + 28, e + e_size);
            if let Some((cookie, size)) = find(&kids, b"alac") {
                let d = w.get(cookie, size as usize).to_vec();
                if d.len() >= 28 {
                    facts.bits = d[9] as u32;
                    facts.channels = d[13] as u32;
                    c.kv("Frame length", format!("{} samples", be32(&d[4..8])));
                    c.check(
                        "ALAC description",
                        Status::Pass,
                        format!(
                            "{} bits, {} channels, rice parameters {}/{}/{}",
                            d[9], d[13], d[10], d[11], d[12]
                        ),
                    );
                    if facts.rate == 0 {
                        facts.rate = be32(&d[24..28]);
                    }
                }
            } else if name == "alac" {
                c.check("ALAC description", Status::Fail, "the sample entry has no alac box");
            }
        }
    }
    if let (Some((stsz, _)), Some((_, mdat_size))) = (find(&table, b"stsz"), mdat) {
        let d = w.get(stsz, 12).to_vec();
        if d.len() >= 12 {
            let count = be32(&d[8..12]) as usize;
            let sizes = w.get(stsz + 12, count * 4).to_vec();
            let sum: u64 = sizes.chunks_exact(4).map(|s| be32(s) as u64).sum();
            c.check(
                "Sample table",
                if sum <= mdat_size { Status::Pass } else { Status::Fail },
                format!("{count} frames totalling {sum} bytes; the mdat box holds {mdat_size}"),
            );
        }
    }
    facts
}

// ------------------------------------------------------------------------------------------------
// TTA

fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn tta(w: &mut Window, c: &mut Collector) -> Facts {
    let len = w.len;
    let h = w.get(0, 22).to_vec();
    if h.len() < 22 {
        c.check("TTA header", Status::Fail, "the file is shorter than the header");
        return Facts::default();
    }
    let channels = le16(&h[6..8]) as u32;
    let bits = le16(&h[8..10]) as u32;
    let rate = le32(&h[10..14]);
    let samples = le32(&h[14..18]) as u64;
    c.check(
        "Header checksum",
        if crc32_ieee(&h[..18]) == le32(&h[18..22]) {
            Status::Pass
        } else {
            Status::Fail
        },
        "CRC-32 of the first 18 bytes",
    );
    let frame_samples = 256 * rate as u64 / 245;
    let frames = if frame_samples > 0 {
        samples.div_ceil(frame_samples)
    } else {
        0
    };
    let table = w.get(22, (frames * 4 + 4) as usize).to_vec();
    if table.len() as u64 == frames * 4 + 4 {
        let (sizes, crc) = table.split_at(frames as usize * 4);
        c.check(
            "Seek table checksum",
            if crc32_ieee(sizes) == le32(crc) {
                Status::Pass
            } else {
                Status::Fail
            },
            format!("{frames} frames"),
        );
        let audio: u64 = sizes.chunks_exact(4).map(|s| le32(s) as u64).sum();
        size_check(c, "Frame sizes add up", 22 + frames * 4 + 4 + audio, len);
    } else {
        c.check("Seek table", Status::Fail, "the file ends inside the seek table");
    }
    Facts {
        rate,
        channels,
        bits,
        frames: Some(samples),
        lossless: true,
    }
}

// ------------------------------------------------------------------------------------------------
// WavPack

/// Walks the blocks: each starts with `wvpk`, names its own size, and the sample counts of the blocks that open a
/// stretch of time must add up to the total in the first header.
fn wavpack(w: &mut Window, c: &mut Collector) -> Facts {
    const HEADER: usize = 32;
    const RATES: [u32; 15] = [
        6000, 8000, 9600, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000, 64000, 88200, 96000, 192000,
    ];
    let len = w.len;
    let mut at = 0u64;
    let (mut blocks, mut frames, mut total, mut channels, mut counting) = (0u64, 0u64, 0u64, 0u32, true);
    let (mut rate, mut bits) = (0u32, 0u32);
    while at + HEADER as u64 <= len {
        let h = w.get(at, HEADER).to_vec();
        if h.len() < HEADER || &h[..4] != b"wvpk" {
            c.check("Block header", Status::Fail, format!("no \"wvpk\" at byte {at}"));
            return Facts::default();
        }
        let flags = le32(&h[24..28]);
        if blocks == 0 {
            total = le32(&h[12..16]) as u64 | (h[11] as u64) << 32;
            bits = ((flags & 3) + 1) * 8;
            rate = RATES.get(((flags >> 23) & 0xF) as usize).copied().unwrap_or(0);
        }
        if flags & 0x800 != 0 {
            frames += le32(&h[20..24]) as u64;
            if blocks > 0 && channels > 0 {
                counting = false;
            }
        }
        if counting {
            channels += if flags & 0x4 != 0 { 1 } else { 2 };
        }
        blocks += 1;
        at += 8 + le32(&h[4..8]) as u64;
    }
    c.check(
        "Blocks fill the file",
        if at == len { Status::Pass } else { Status::Fail },
        format!("{blocks} blocks"),
    );
    c.check(
        "Sample count",
        if total == 0 || total == frames {
            Status::Pass
        } else {
            Status::Fail
        },
        format!("the header says {total}, the blocks hold {frames}"),
    );
    Facts {
        rate,
        channels,
        bits,
        frames: Some(frames),
        lossless: true,
    }
}

// ------------------------------------------------------------------------------------------------
// MXF sound (digital cinema)

/// Key prefix of a partition pack (the byte after it says header, body or footer) and of a generic-container
/// essence element.
const PARTITION_PREFIX: [u8; 13] = [
    0x06, 0x0e, 0x2b, 0x34, 0x02, 0x05, 0x01, 0x01, 0x0d, 0x01, 0x02, 0x01, 0x01,
];
const ESSENCE_PREFIX: [u8; 12] = [0x06, 0x0e, 0x2b, 0x34, 0x01, 0x02, 0x01, 0x01, 0x0d, 0x01, 0x03, 0x01];
const DESCRIPTOR_KEY_TAIL: [u8; 4] = [0x01, 0x01, 0x48, 0x00];

/// Walks the KLV triplets: partition packs must sit where they say they do, every essence frame must be the same
/// size, and the descriptor's duration must match the number of frames.
fn mxf(w: &mut Window, c: &mut Collector) -> Facts {
    let len = w.len;
    let (mut at, mut frames, mut sizes) = (0u64, 0u64, std::collections::BTreeSet::new());
    let (mut partitions, mut misplaced, mut footer_at, mut broken) = (0, 0, None, false);
    let mut descriptor: Vec<u8> = Vec::new();
    while at + 17 <= len {
        let head = w.get(at, 24).to_vec();
        let (value_len, header) = match head.get(16) {
            Some(&b) if b < 0x80 => (b as u64, 17u64),
            Some(&b) if (0x81..=0x88).contains(&b) && head.len() >= 17 + (b & 0x7F) as usize => {
                let n = (b & 0x7F) as usize;
                (
                    head[17..17 + n].iter().fold(0u64, |acc, &x| acc << 8 | x as u64),
                    17 + n as u64,
                )
            }
            _ => {
                broken = true;
                break;
            }
        };
        if head.starts_with(&PARTITION_PREFIX) && matches!(head[13], 2..=4) {
            partitions += 1;
            let pack = w.get(at + header, 40).to_vec();
            if pack.len() >= 24 && u64::from_be_bytes(pack[8..16].try_into().unwrap()) != at {
                misplaced += 1;
            }
            if head[13] == 0x04 {
                footer_at = Some(at);
            }
        } else if head.starts_with(&ESSENCE_PREFIX) {
            frames += 1;
            sizes.insert(value_len);
        } else if head[8..12] == [0x0d, 0x01, 0x01, 0x01] && head[12..16] == DESCRIPTOR_KEY_TAIL {
            descriptor = w.get(at + header, value_len as usize).to_vec();
        }
        at += header + value_len;
    }
    c.check(
        "KLV structure",
        if broken {
            Status::Fail
        } else if at == len {
            Status::Pass
        } else {
            Status::Warn
        },
        format!("{partitions} partitions, {frames} essence frames"),
    );
    c.check(
        "Partition positions",
        if misplaced == 0 && partitions >= 2 {
            Status::Pass
        } else {
            Status::Fail
        },
        "each partition pack says where it starts",
    );
    c.check(
        "Frame sizes",
        if sizes.len() == 1 { Status::Pass } else { Status::Fail },
        format!("{} different sizes (a sound track file has one)", sizes.len()),
    );
    c.check(
        "Footer",
        if footer_at.is_some() {
            Status::Pass
        } else {
            Status::Fail
        },
        "a footer partition closes the file",
    );

    // properties of the wave audio descriptor
    let mut tags = std::collections::HashMap::new();
    let mut o = 0;
    while o + 4 <= descriptor.len() {
        let (tag, l) = (be16(&descriptor[o..]), be16(&descriptor[o + 2..]) as usize);
        if o + 4 + l > descriptor.len() {
            break;
        }
        tags.insert(tag, descriptor[o + 4..o + 4 + l].to_vec());
        o += 4 + l;
    }
    let number = |tag: u16| {
        tags.get(&tag)
            .map(|v| v.iter().fold(0u64, |acc, &x| acc << 8 | x as u64))
            .unwrap_or(0)
    };
    let rate = tags.get(&0x3d03).map(|v| be32(v)).unwrap_or(0);
    let duration = number(0x3002);
    c.check(
        "Duration",
        if duration == frames { Status::Pass } else { Status::Fail },
        format!("the descriptor says {duration} frames, the file holds {frames}"),
    );
    let edit = tags.get(&0x3001).map(|v| (be32(v), be32(&v[4..]))).unwrap_or((0, 1));
    let seconds = if edit.0 > 0 {
        frames as f64 * edit.1 as f64 / edit.0 as f64
    } else {
        0.0
    };
    Facts {
        rate,
        channels: number(0x3d07) as u32,
        bits: number(0x3d01) as u32,
        frames: Some((seconds * rate as f64).round() as u64),
        lossless: true,
    }
}

// ------------------------------------------------------------------------------------------------
// MPEG-4 ALS

/// A raw `.als` file starts with its configuration record. The frames have no index, so only the record is read.
fn als(w: &mut Window, c: &mut Collector) -> Facts {
    let h = w.get(0, 22).to_vec();
    if h.len() < 22 {
        c.check(
            "Configuration",
            Status::Fail,
            "the file is shorter than the ALS configuration record",
        );
        return Facts::default();
    }
    let (rate, samples, channels) = (be32(&h[4..8]), be32(&h[8..12]) as u64, be16(&h[12..14]) as u32 + 1);
    let bits = (((h[14] >> 2) & 7) as u32 + 1) * 8;
    let frame_length = be16(&h[15..17]) as u32 + 1;
    c.kv("Frame length", format!("{frame_length} samples"));
    c.check(
        "Configuration",
        if rate > 0 && samples > 0 {
            Status::Pass
        } else {
            Status::Fail
        },
        "the record names a rate and a length",
    );
    c.check(
        "Frames",
        Status::Info,
        "frame sizes are not indexed in a raw file; open it as MP4 for a full decode",
    );
    Facts {
        rate,
        channels,
        bits,
        frames: Some(samples),
        lossless: true,
    }
}

// ------------------------------------------------------------------------------------------------
// Monkey's Audio

/// The descriptor, header and seek table must agree with each other and with the size of the file.
fn monkeys(w: &mut Window, c: &mut Collector) -> Facts {
    let len = w.len;
    let h = w.get(0, 76).to_vec();
    if h.len() < 76 {
        c.check(
            "Descriptor",
            Status::Fail,
            "the file is shorter than the descriptor and header",
        );
        return Facts::default();
    }
    let version = le16(&h[4..6]);
    c.kv("Version", format!("{}.{:02}", version / 1000, version % 1000 / 10));
    if version < 3980 {
        c.check(
            "Version",
            Status::Warn,
            "older than 3.98: only the header fields are read",
        );
        return Facts::default();
    }
    let (descriptor, header, seek) = (le32(&h[8..12]) as u64, le32(&h[12..16]) as u64, le32(&h[16..20]) as u64);
    let (wav_head, audio, wav_tail) = (
        le32(&h[20..24]) as u64,
        le32(&h[24..28]) as u64 | (le32(&h[28..32]) as u64) << 32,
        le32(&h[32..36]) as u64,
    );
    let (level, blocks_per_frame, final_blocks, frames) = (
        le16(&h[52..54]),
        le32(&h[56..60]) as u64,
        le32(&h[60..64]) as u64,
        le32(&h[64..68]) as u64,
    );
    let (bits, channels, rate) = (le16(&h[68..70]) as u32, le16(&h[70..72]) as u32, le32(&h[72..76]));
    c.kv("Compression level", level.to_string());
    c.check(
        "Seek table size",
        if seek >= frames * 4 { Status::Pass } else { Status::Fail },
        format!("{frames} frames"),
    );
    size_check(
        c,
        "Audio data size",
        descriptor + header + seek + wav_head + audio + wav_tail,
        len,
    );
    let samples = if frames > 0 {
        blocks_per_frame * (frames - 1) + final_blocks
    } else {
        0
    };
    Facts {
        rate,
        channels,
        bits,
        frames: Some(samples),
        lossless: true,
    }
}

// ------------------------------------------------------------------------------------------------
// DSD

fn dsf(w: &mut Window, c: &mut Collector) -> Facts {
    let len = w.len;
    let h = w.get(0, 92).to_vec();
    if h.len() < 92 || &h[28..32] != b"fmt " || &h[80..84] != b"data" {
        c.check(
            "DSF chunks",
            Status::Fail,
            "the DSD, fmt and data chunks are not where a DSF file has them",
        );
        return Facts::default();
    }
    size_check(c, "File size", le64(&h[12..20]), len);
    let channels = le32(&h[52..56]);
    let rate = le32(&h[56..60]);
    let bit_order = le32(&h[60..64]);
    let samples = le64(&h[64..72]);
    let block = le32(&h[72..76]) as u64;
    c.kv(
        "Bit order",
        if bit_order == 1 {
            "least significant bit first"
        } else {
            "most significant bit first"
        },
    );
    c.kv("DSD rate", format!("DSD{} ({} Hz)", rate / 44_100, rate));
    c.kv("Channel type", le32(&h[48..52]).to_string());
    let data = le64(&h[84..92]).saturating_sub(12);
    let want = (samples.div_ceil(8 * block.max(1)) * block.max(1)) * channels as u64;
    c.check(
        "Audio data size",
        if data == want { Status::Pass } else { Status::Fail },
        format!("{data} bytes of audio, {want} expected for {samples} samples in {channels} channels"),
    );
    c.check(
        "Block size",
        if block == 4096 { Status::Pass } else { Status::Warn },
        format!("{block} bytes per channel in each block"),
    );
    Facts {
        rate,
        channels,
        bits: 1,
        frames: Some(samples),
        lossless: false,
    }
}

fn dff(w: &mut Window, c: &mut Collector) -> Facts {
    let len = w.len;
    let h = w.get(0, 16).to_vec();
    size_check(c, "FRM8 size", be64(&h[4..12]) + 12, len);
    let mut facts = Facts::default();
    let mut pos = 16u64;
    let mut data_bytes = 0u64;
    while pos + 12 <= len {
        let ch = w.get(pos, 12).to_vec();
        if ch.len() < 12 {
            break;
        }
        let size = be64(&ch[4..12]);
        let body = pos + 12;
        match &ch[..4] {
            b"PROP" => {
                let end = body + size;
                let mut p = body + 4;
                while p + 12 <= end {
                    let sub = w.get(p, 12).to_vec();
                    let sub_size = be64(&sub[4..12]);
                    match &sub[..4] {
                        b"FS  " => facts.rate = be32(w.get(p + 12, 4)),
                        b"CHNL" => facts.channels = be16(w.get(p + 12, 2)) as u32,
                        _ => {}
                    }
                    p += 12 + sub_size + sub_size % 2;
                }
            }
            b"DSD " => data_bytes = size,
            _ => {}
        }
        pos = body + size + size % 2;
    }
    if facts.rate > 0 && facts.channels > 0 {
        facts.frames = Some(data_bytes / facts.channels as u64 * 8);
        c.kv("DSD rate", format!("DSD{} ({} Hz)", facts.rate / 44_100, facts.rate));
    }
    c.check(
        "Audio data",
        if data_bytes > 0 && facts.channels > 0 && data_bytes.is_multiple_of(facts.channels as u64) {
            Status::Pass
        } else {
            Status::Fail
        },
        format!("{data_bytes} bytes for {} channels", facts.channels),
    );
    facts.bits = 1;
    facts
}
