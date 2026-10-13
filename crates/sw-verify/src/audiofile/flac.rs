//! FLAC, native and in Ogg.

use crate::audiofile::bytes::{be64, le32, le64};
use crate::audiofile::Facts;
use crate::{Collector, Status, Window};

/// STREAMINFO fields, from its 34 bytes.
pub(crate) fn streaminfo(c: &mut Collector, d: &[u8]) -> Facts {
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

pub(crate) fn flac(w: &mut Window, c: &mut Collector) -> Facts {
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
pub(crate) fn ogg_crc(data: &[u8]) -> u32 {
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

pub(crate) fn ogg_flac(w: &mut Window, c: &mut Collector) -> Facts {
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
