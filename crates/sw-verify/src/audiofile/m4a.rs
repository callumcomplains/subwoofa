//! ALAC (and ALS) in an MP4 container.

use crate::audiofile::bytes::{be32, be64, size_check};
use crate::audiofile::Facts;
use crate::{Collector, Status, Window};

/// The boxes of a container, as (name, body start, body size).
pub(crate) fn boxes(w: &mut Window, start: u64, end: u64) -> Vec<([u8; 4], u64, u64)> {
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

pub(crate) fn find(list: &[([u8; 4], u64, u64)], name: &[u8; 4]) -> Option<(u64, u64)> {
    list.iter().find(|b| &b.0 == name).map(|b| (b.1, b.2))
}

pub(crate) fn m4a(w: &mut Window, c: &mut Collector) -> Facts {
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
    let Some((trak_start, trak_size)) = audio_track(w, moov, moov_size) else {
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
            c.kv("Codec", codec_name(&name));
            // an audio sample entry: 8 bytes of reserved and data reference, 8 reserved, then the channel count
            let head = w.get(e, 28).to_vec();
            if head.len() >= 18 && facts.channels == 0 {
                facts.channels = u16::from_be_bytes([head[16], head[17]]) as u32;
            }
            if let Some(why) = not_checked(&name) {
                facts.lossless = false;
                facts.no_decoder = true;
                c.check("Audio", Status::Skipped, why);
            }
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

/// The movie's sound track: the first track whose handler is `soun` (a film may hold a video track first), else the
/// first track of any kind.
fn audio_track(w: &mut Window, moov: u64, moov_size: u64) -> Option<(u64, u64)> {
    let tracks: Vec<(u64, u64)> = boxes(w, moov, moov + moov_size)
        .into_iter()
        .filter(|b| &b.0 == b"trak")
        .map(|b| (b.1, b.2))
        .collect();
    let is_sound = |w: &mut Window, (start, size): (u64, u64)| {
        let in_trak = boxes(w, start, start + size);
        let Some((mdia, mdia_size)) = find(&in_trak, b"mdia") else {
            return false;
        };
        let in_mdia = boxes(w, mdia, mdia + mdia_size);
        // hdlr: version and flags, pre_defined, then the handler type
        find(&in_mdia, b"hdlr").is_some_and(|(hdlr, _)| w.get(hdlr + 8, 4) == b"soun")
    };
    tracks
        .iter()
        .copied()
        .find(|&t| is_sound(w, t))
        .or(tracks.first().copied())
}

/// A sample entry's four-letter code with the format's name, when it is one Subwoofa knows.
fn codec_name(code: &str) -> String {
    let name = match code {
        "alac" => "Apple Lossless",
        "ac-4" => "Dolby AC-4",
        "dtsx" => "DTS-UHD (DTS:X Profile 2)",
        "ec-3" => "Dolby Digital Plus (E-AC-3)",
        "ac-3" => "Dolby Digital (AC-3)",
        "mp4a" => "MPEG-4 audio (AAC, or ALS)",
        _ => return code.to_string(),
    };
    format!("{name} ({code})")
}

/// Why the audio of a codec this check only names is not checked further.
fn not_checked(code: &str) -> Option<&'static str> {
    match code {
        "dtsx" | "ac-4" => Some(
            "the stream is recognised, but no decoder for it is available (FFmpeg has none), so its frames and \
             audio are not checked here; for a frame check, take the stream out of the MP4 \
             (ffmpeg -i FILE -map 0:a -c copy -f data OUT) and verify that",
        ),
        _ => None,
    }
}
