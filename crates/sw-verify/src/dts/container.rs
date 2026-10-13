//! The `.dtshd` file container: a series of chunks (an 8-character name, a 64-bit size, the body), one of which,
//! `STRMDATA`, holds the stream. The others describe it: its frame rate and flags, the audio's rate and length, a
//! table of frame offsets, a timecode and a stored silent frame. This module reads them, and checks what they say
//! against the stream that was found.

use super::report::Facts;
use super::walk::{walk, Walk};
use crate::{Collector, Status, Window};

/// A chunk's place in the file.
pub(crate) struct Chunk {
    pub(crate) name: String,
    pub(crate) off: u64,
    pub(crate) size: u64,
}

/// `AUPR-HDR`: the audio presentation the file describes.
pub(crate) struct Aupr {
    pub(crate) rate: u32,
    pub(crate) frames: u32,
    pub(crate) samples_per_frame: u32,
    /// The length of the programme, without the codec delay and the flush frame.
    pub(crate) total_samples: u64,
    pub(crate) delay: u32,
}

/// `CORESSMD`: what the core substream looks like.
pub(crate) struct CoreMeta {
    pub(crate) rate: u32,
    pub(crate) kbps: u32,
    pub(crate) speaker_mask: u32,
    pub(crate) payload_bytes: u32,
}

/// `NAVI-TBL`: the offset of every frame in the stream.
pub(crate) struct NaviTable {
    pub(crate) count: u32,
    pub(crate) offsets: Vec<u64>,
}

/// Everything the chunks say, and where the stream is.
#[derive(Default)]
pub(crate) struct Container {
    pub(crate) chunks: Vec<Chunk>,
    /// The stream (`STRMDATA`) as a range of the file.
    pub(crate) start: u64,
    pub(crate) end: u64,
    pub(crate) stream_found: bool,
    pub(crate) problems: u32,
    pub(crate) aupr: Option<Aupr>,
    pub(crate) navi: Option<NaviTable>,
    /// The stored silent frame (`BLACKOUT`): offset and size.
    pub(crate) blackout: Option<(u64, u64)>,
    pub(crate) fps: u8,
    pub(crate) hd_flags: u8,
    pub(crate) core_meta: Option<CoreMeta>,
    pub(crate) ext_meta: Option<Vec<u8>>,
    pub(crate) timecode: Option<Vec<u8>>,
    pub(crate) build: String,
}

fn be24(d: &[u8]) -> u32 {
    ((d[0] as u32) << 16) | ((d[1] as u32) << 8) | d[2] as u32
}

/// Reads the chunks of a `.dtshd` file, reports on their structure, and finds the stream.
pub(crate) fn read(w: &mut Window, c: &mut Collector) -> Container {
    let len = w.len;
    let mut found = Container {
        end: len,
        ..Default::default()
    };
    let mut pos = 0u64;
    while pos + 16 <= len {
        let header = w.get(pos, 16).to_vec();
        let name = String::from_utf8_lossy(&header[..8]).to_string();
        let size = u64::from_be_bytes(header[8..16].try_into().unwrap());
        if !name.bytes().all(|b| (0x20..0x7F).contains(&b)) {
            found.problems += 1;
            c.finding(Status::Fail, Some(pos), "a container chunk has an unreadable name");
            break;
        }
        let body_off = pos + 16;
        if body_off + size > len {
            found.problems += 1;
            c.finding(
                Status::Fail,
                Some(pos),
                format!("chunk {name} claims {size} bytes but only {} remain", len - body_off),
            );
            if name == "STRMDATA" {
                found.start = body_off;
                found.end = len;
                found.stream_found = true;
            }
            found.chunks.push(Chunk {
                name,
                off: body_off,
                size: len - body_off,
            });
            break;
        }
        // the stream itself and very large chunks are not read into memory
        let body = if size <= 1 << 20 && name != "STRMDATA" {
            w.get(body_off, size as usize).to_vec()
        } else {
            Vec::new()
        };
        found.take_chunk(name.trim_end(), &body, body_off, size);
        found.chunks.push(Chunk {
            name,
            off: body_off,
            size,
        });
        pos = body_off + size;
        if pos == len {
            break;
        }
    }
    found.report_structure(c);
    found
}

impl Container {
    /// Keeps what a chunk says.
    fn take_chunk(&mut self, name: &str, body: &[u8], body_off: u64, size: u64) {
        match name {
            "DTSHDHDR" if body.len() >= 12 => {
                self.fps = body[9];
                self.hd_flags = body[11];
            }
            "CORESSMD" if body.len() >= 11 => {
                self.core_meta = Some(CoreMeta {
                    rate: be24(body),
                    kbps: u16::from_be_bytes([body[3], body[4]]) as u32,
                    speaker_mask: u16::from_be_bytes([body[5], body[6]]) as u32,
                    payload_bytes: be24(&body[8..]),
                })
            }
            "EXTSS_MD" => self.ext_meta = Some(body.to_vec()),
            "AUPR-HDR" if body.len() >= 21 => {
                self.aupr = Some(Aupr {
                    rate: be24(&body[3..]),
                    frames: u32::from_be_bytes(body[6..10].try_into().unwrap()),
                    samples_per_frame: u16::from_be_bytes([body[10], body[11]]) as u32,
                    total_samples: ((u32::from_be_bytes(body[12..16].try_into().unwrap()) as u64) << 8)
                        | body[16] as u64,
                    delay: u16::from_be_bytes([body[19], body[20]]) as u32,
                });
            }
            "STRMDATA" => {
                self.start = body_off;
                self.end = body_off + size;
                self.stream_found = true;
            }
            "NAVI-TBL" if body.len() >= 7 => {
                let count = u32::from_be_bytes(body[..4].try_into().unwrap());
                let width = body[6] as usize;
                let mut offsets = Vec::new();
                if (1..=4).contains(&width) {
                    for i in 0..count as usize {
                        let at = 7 + i * width;
                        if at + width > body.len() {
                            break;
                        }
                        offsets.push(body[at..at + width].iter().fold(0u64, |v, b| (v << 8) | *b as u64));
                    }
                }
                self.navi = Some(NaviTable { count, offsets });
            }
            "TIMECODE" => self.timecode = Some(body.to_vec()),
            "BUILDVER" => {
                self.build = String::from_utf8_lossy(body)
                    .replace(['\0', '\r'], "")
                    .replace('\n', "; ")
                    .trim()
                    .to_string()
            }
            "BLACKOUT" => self.blackout = Some((body_off, size)),
            _ => {}
        }
    }

    /// The two checks on the chunk structure itself.
    fn report_structure(&self, c: &mut Collector) {
        let sound = self.problems == 0 && self.stream_found;
        c.check(
            "Container chunks",
            if sound { Status::Pass } else { Status::Fail },
            if sound {
                format!("{} chunks, sizes consistent with the file", self.chunks.len())
            } else if !self.stream_found {
                "there is no STRMDATA chunk".to_string()
            } else {
                "a chunk is damaged or runs past the end of the file".to_string()
            },
        );
        let need = ["DTSHDHDR", "AUPR-HDR", "STRMDATA", "TIMECODE"];
        let missing: Vec<&str> = need
            .iter()
            .copied()
            .filter(|n| !self.chunks.iter().any(|c| c.name.trim_end() == *n))
            .collect();
        c.check(
            "Required chunks",
            if missing.is_empty() { Status::Pass } else { Status::Fail },
            if missing.is_empty() {
                "DTSHDHDR, AUPR-HDR, STRMDATA and TIMECODE are present".to_string()
            } else {
                format!("missing: {}", missing.join(", "))
            },
        );
    }

    /// What the chunks say about the stream, checked against what the walk found in it.
    pub(crate) fn cross_check(&self, w: &mut Window, c: &mut Collector, s: &Walk, facts: &Facts) {
        if let Some(aupr) = &self.aupr {
            check_aupr(c, aupr, s, facts);
        }
        self.check_navi(c, s);
        if let Some(tc) = &self.timecode {
            check_timecode(c, tc);
        }
        self.check_blackout(w, c);
    }

    /// The offset table must list every frame at the offset it was found at.
    fn check_navi(&self, c: &mut Collector, s: &Walk) {
        if let Some(navi) = &self.navi {
            let matches = navi.offsets.len() as u64 == s.aus
                && navi
                    .offsets
                    .iter()
                    .zip(&s.frame_starts)
                    .all(|(a, b)| *a == b - self.start);
            c.check(
                "Frame offset table (NAVI-TBL)",
                if matches && navi.count as u64 == s.aus {
                    Status::Pass
                } else {
                    Status::Fail
                },
                if matches {
                    format!("{} offsets match the frames found", navi.count)
                } else {
                    format!(
                        "table lists {} frames; the stream has {}, offsets differ",
                        navi.count, s.aus
                    )
                },
            );
        } else if self.hd_flags & 0x04 != 0 || self.hd_flags & 0x01 != 0 {
            c.check(
                "Frame offset table (NAVI-TBL)",
                Status::Warn,
                "the stream is marked variable rate but has no offset table",
            );
        }
    }

    /// The stored silent frame must itself be a valid frame (it is what Add silence inserts).
    fn check_blackout(&self, w: &mut Window, c: &mut Collector) {
        let Some((off, size)) = self.blackout else {
            c.check(
                "Silent frame (BLACKOUT)",
                Status::Info,
                "none; the DTS tools cannot add silence to this file",
            );
            return;
        };
        let mut scratch = Collector {
            report: Default::default(),
        };
        let b = walk(w, off, off + size, &mut scratch, false);
        let problems = b.junk_events + b.core_bad + b.exss_bad + b.exss_hdr_crc_bad + b.comp_missing + b.xll_bad;
        let valid = b.aus >= 1 && problems == 0;
        c.check(
            "Silent frame (BLACKOUT)",
            if valid { Status::Pass } else { Status::Fail },
            if valid {
                format!("{size} bytes, a valid frame (lets Add silence work)")
            } else {
                "the stored silent frame does not parse".to_string()
            },
        );
    }

    /// The listings of a deep verification: every chunk, and what the metadata chunks hold.
    pub(crate) fn deep_sections(&self, c: &mut Collector) {
        let rows: Vec<(String, String)> = self
            .chunks
            .iter()
            .map(|ch| {
                (
                    ch.name.trim_end().to_string(),
                    format!("{} bytes at offset {}", ch.size, ch.off),
                )
            })
            .collect();
        c.section("Container chunks", rows);
        let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ");
        let mut rows = vec![
            ("frame rate code".to_string(), self.fps.to_string()),
            ("stream flags".to_string(), format!("0x{:02X}", self.hd_flags)),
        ];
        if let Some(m) = &self.core_meta {
            rows.push((
                "CORESSMD".to_string(),
                format!(
                    "{} Hz, {} kbps, speaker mask 0x{:X}, frame payload {} bytes",
                    m.rate, m.kbps, m.speaker_mask, m.payload_bytes
                ),
            ));
        }
        if let Some(e) = &self.ext_meta {
            rows.push(("EXTSS_MD".to_string(), hex(e)));
        }
        if let Some(tc) = &self.timecode {
            rows.push(("TIMECODE".to_string(), hex(tc)));
        }
        if !self.build.is_empty() {
            rows.push(("BUILDVER".to_string(), self.build.clone()));
        }
        c.section("Container metadata", rows);
    }
}

/// `AUPR-HDR` against the stream: frame count, rate, programme length and frame length.
fn check_aupr(c: &mut Collector, aupr: &Aupr, s: &Walk, facts: &Facts) {
    let Aupr {
        rate: stated_rate,
        frames,
        samples_per_frame,
        total_samples,
        delay,
    } = *aupr;
    c.check(
        "Frame count (AUPR-HDR)",
        if frames as u64 == s.aus {
            Status::Pass
        } else {
            Status::Fail
        },
        format!("header says {frames}, the stream holds {}", s.aus),
    );
    if facts.rate != 0 {
        c.check(
            "Sample rate (AUPR-HDR)",
            if stated_rate == facts.rate {
                Status::Pass
            } else {
                Status::Fail
            },
            format!("header says {stated_rate} Hz, the stream says {} Hz", facts.rate),
        );
    }
    let expect = frames as u64 * samples_per_frame as u64;
    // the stream carries the codec delay and a flush frame beyond the programme
    let slack = delay as u64 + 2 * samples_per_frame as u64;
    c.check(
        "Programme length (AUPR-HDR)",
        if total_samples <= expect && total_samples + slack >= expect {
            Status::Pass
        } else {
            Status::Warn
        },
        format!(
            "{total_samples} samples stated; {frames} frames of {samples_per_frame} samples hold {expect} (codec delay {delay})"
        ),
    );
    if facts.duration_secs > 0.0 && samples_per_frame > 0 {
        let actual = facts.unit_samples.iter().filter(|x| **x == samples_per_frame).count();
        if actual as u64 != s.aus {
            c.check(
                "Frame length (AUPR-HDR)",
                Status::Warn,
                format!(
                    "header says {samples_per_frame} samples per frame; {} of {} frames have that length",
                    actual, s.aus
                ),
            );
        }
    }
    c.kv("Codec delay (container)", format!("{delay} samples"));
    c.kv("Programme length (container)", format!("{total_samples} samples"));
}

/// The timecode chunk: a clock rate and a frame rate code.
fn check_timecode(c: &mut Collector, tc: &[u8]) {
    let clock = if tc.len() >= 4 {
        u32::from_be_bytes(tc[..4].try_into().unwrap())
    } else {
        0
    };
    let sane = tc.len() >= 8
        && (clock == 48_000 || clock >= 44_100 && clock % 44_100 == 0 || clock % 48_000 == 0)
        && (1..=7).contains(&tc[4]);
    c.check(
        "Timecode chunk",
        if sane { Status::Pass } else { Status::Warn },
        format!("clock {clock} Hz, frame rate code {}", tc.get(4).copied().unwrap_or(0)),
    );
}
