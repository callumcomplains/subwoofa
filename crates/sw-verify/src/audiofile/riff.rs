//! WAV / RF64, AIFF and CAF: the chunk formats.

use crate::audiofile::bytes::{be16, be32, be64, le16, le32, le64, size_check};
use crate::audiofile::Facts;
use crate::{Collector, Status, Window};

pub(crate) fn wav(w: &mut Window, c: &mut Collector) -> Facts {
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

pub(crate) fn aiff(w: &mut Window, c: &mut Collector) -> Facts {
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

pub(crate) fn caf(w: &mut Window, c: &mut Collector) -> Facts {
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
