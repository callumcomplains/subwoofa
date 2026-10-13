//! What a walk of a DTS stream turns into: the facts about the stream (rate, length, channels, which codec it is),
//! the key/value lines of the report, the pass/fail checks on the structure, and the listings of a deep
//! verification.

use super::consts::{AMODE_CH, AMODE_NAME, PCM_BITS};
use super::container::Aupr;
use super::core_header::core_rows;
use super::exss::{exss_rows, speakers};
use super::walk::Walk;
use super::xll::xll_rows;
use crate::{Collector, Probe, Status};

/// What the stream is, worked out from the frames it holds and, if there is one, the container's `AUPR-HDR`.
pub(crate) struct Facts {
    /// Sample rate of the decoded audio.
    pub(crate) rate: u32,
    pub(crate) core_rate: u32,
    /// Samples (at that rate) every frame holds.
    pub(crate) unit_samples: Vec<u32>,
    pub(crate) duration_secs: f64,
    pub(crate) channels: u32,
}

impl Facts {
    pub(crate) fn new(s: &Walk, aupr: Option<&Aupr>) -> Facts {
        let core = s.core_hdr.as_ref();
        // The rate of the decoded audio: the core's, doubled by an X96 extension, or what the extension substream /
        // container states when that is higher (a 96 kHz lossless stream over a 48 kHz core).
        let core_rate = core.map(|k| k.rate).unwrap_or(0);
        let x96 = core.is_some_and(|k| k.ext_present && (k.ext_id == 2 || k.ext_id == 3));
        let mut rate = if x96 { core_rate * 2 } else { core_rate };
        rate = rate.max(s.exss_first.as_ref().map_or(0, |e| e.max_rate));
        if let Some(a) = aupr {
            if s.saw_core && a.rate > rate && core_rate > 0 && a.rate % core_rate == 0 {
                rate = a.rate;
            }
        }
        if !s.saw_core {
            rate = aupr
                .map(|a| a.rate)
                .or(s.exss_first.as_ref().map(|e| e.max_rate))
                .unwrap_or(0);
        }
        let ratio = if s.saw_core && core_rate > 0 {
            (rate / core_rate).max(1)
        } else {
            1
        };
        // frames of an extension-only stream have the length the container states; otherwise a frame is its core
        // frame's length scaled up by the rate ratio, and a frame without a core takes the container's
        let stated = aupr.map(|a| a.samples_per_frame);
        let unit_samples: Vec<u32> = match stated {
            Some(n) if n > 0 && !s.saw_core => s.frame_samples.iter().map(|_| n).collect(),
            _ if s.frame_samples.iter().all(|x| *x > 0) => s.frame_samples.iter().map(|x| x * ratio).collect(),
            Some(n) => s
                .frame_samples
                .iter()
                .map(|x| if *x == 0 { n } else { *x * ratio })
                .collect(),
            None => s.frame_samples.clone(),
        };
        let total_samples: u64 = unit_samples.iter().map(|x| *x as u64).sum();
        let channels_core = core
            .map(|k| AMODE_CH[k.amode as usize] + u32::from(k.lff != 0))
            .unwrap_or(0);
        let channels_exss = s.exss_first.as_ref().map(|e| e.nchannels).unwrap_or(0);
        Facts {
            rate,
            core_rate,
            unit_samples,
            duration_secs: if rate > 0 {
                total_samples as f64 / rate as f64
            } else {
                0.0
            },
            channels: channels_core.max(channels_exss),
        }
    }

    /// What the file-level `Probe` takes from the stream.
    pub(crate) fn fill_probe(&self, s: &Walk, p: &mut Probe) {
        p.sample_rate = self.rate;
        p.has_core = s.saw_core;
        p.core_rate = self.core_rate;
        p.lossless = s.saw_xll;
        p.frame_bytes = s.frame_bytes.clone();
        p.frame_samples = self.unit_samples.clone();
        p.duration_secs = self.duration_secs;
        p.channels = self.channels;
    }
}

/// The name of the format a stream is, from the parts it was found to have.
fn format_name(s: &Walk) -> &'static str {
    if s.saw_xll {
        "DTS-HD Master Audio"
    } else if s.saw_lbr {
        "DTS Express"
    } else if s.exss > 0 && s.ext_kinds.contains("XBR") || s.ext_kinds.contains("XXCH") && s.exss > 0 {
        "DTS-HD High Resolution Audio"
    } else if s.ext_kinds.contains("X96") {
        "DTS 96/24"
    } else if s.ext_kinds.contains("XCH") {
        "DTS-ES"
    } else {
        "DTS Digital Surround"
    }
}

/// The key/value lines at the top of the report: format, rate, channels, resolution, length and bit rate.
/// `stream_bytes` is the size of the stream itself (the file without a container's chunks).
pub(crate) fn describe(c: &mut Collector, s: &Walk, facts: &Facts, stream_bytes: u64) {
    let mut parts: Vec<String> = Vec::new();
    if s.saw_core {
        parts.push("core".to_string());
    }
    for k in &s.ext_kinds {
        parts.push((*k).to_string());
    }
    if s.saw_xll {
        parts.push("XLL (lossless)".to_string());
    }
    if s.saw_lbr {
        parts.push("LBR (Express)".to_string());
    }
    c.kv("Format", format!("{} ({})", format_name(s), parts.join(" + ")));
    c.kv("Sample rate", format!("{} Hz", facts.rate));
    if s.band_total.len() > 1 {
        let all: u64 = s.band_total.iter().sum();
        c.kv(
            "Lossless data per frequency band",
            s.band_total
                .iter()
                .enumerate()
                .map(|(i, b)| {
                    format!(
                        "band {i}: {:.1} MB ({:.0}%)",
                        *b as f64 / 1e6,
                        100.0 * *b as f64 / all.max(1) as f64
                    )
                })
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    if s.saw_xll {
        describe_dtsx(c, s);
    }
    if let Some(k) = &s.core_hdr {
        c.kv(
            "Core channels",
            format!(
                "{} {}",
                AMODE_NAME[k.amode as usize],
                if k.lff != 0 { "+ LFE" } else { "" }
            ),
        );
    }
    if let Some(e) = &s.exss_first {
        if e.static_fields {
            c.kv(
                "Extension channels",
                format!("{} ({})", e.nchannels, speakers(e.spk_mask)),
            );
            c.kv("Source resolution", format!("{} bits", e.pcm_bits));
        }
    } else if let Some(k) = &s.core_hdr {
        c.kv("Source resolution", format!("{} bits", PCM_BITS[k.pcmr]));
    }
    c.kv("Access units", s.aus.to_string());
    c.kv("Duration", format!("{:.3} s", facts.duration_secs));
    if facts.duration_secs > 0.0 {
        c.kv(
            "Average bit rate",
            format!("{:.1} kbps", stream_bytes as f64 * 8.0 / facts.duration_secs / 1000.0),
        );
    }
}

/// Whether DTS:X (or IMAX Enhanced) object data follows the lossless audio. A chance match in audio data is possible
/// in a long file, so the marker must be there in most frames.
fn describe_dtsx(c: &mut Collector, s: &Walk) {
    let (hits, kind) = if s.imax_frames > s.x_frames {
        (s.imax_frames, "IMAX Enhanced")
    } else {
        (s.x_frames, "DTS:X")
    };
    if hits * 2 >= s.xll_checked && hits > 0 {
        c.kv(
            "DTS:X",
            format!(
                "yes: {kind} object data follows the lossless audio (marker in {hits} of {} lossless frames)",
                s.xll_checked
            ),
        );
        c.check("DTS:X signalling", Status::Info, format!("{kind} marker found in {hits} of {} lossless frames; the object data is not decoded by FFmpeg, so the audio check covers the channel bed", s.xll_checked));
    } else {
        c.kv("DTS:X", "no");
        if hits > 0 {
            c.check("DTS:X signalling", Status::Info, format!("a DTS:X-like marker appeared in {hits} of {} lossless frames, too few to call it DTS:X (most likely a chance match in audio data)", s.xll_checked));
        }
    }
}

/// A pass when `bad` is zero, a fail otherwise.
fn pass_if_none(bad: u64) -> Status {
    if bad == 0 {
        Status::Pass
    } else {
        Status::Fail
    }
}

/// The checks on the structure of the stream: that frames follow one another, that their headers hold legal values
/// and checksums, and that the stream ends on a frame boundary.
pub(crate) fn check_structure(c: &mut Collector, s: &Walk) {
    c.check(
        "Frame structure",
        pass_if_none(s.junk_events),
        if s.junk_events == 0 {
            format!(
                "{} access units, each substream starts where the previous one ends{}",
                s.aus,
                if s.pad > 0 {
                    format!(" ({} padding bytes)", s.pad)
                } else {
                    String::new()
                }
            )
        } else {
            format!(
                "{} stretch(es) of data that is not a frame ({} bytes)",
                s.junk_events, s.junk
            )
        },
    );
    if s.cores > 0 {
        check_core_frames(c, s);
    }
    if s.exss > 0 {
        check_extension_frames(c, s);
    }
    if s.saw_core && !s.saw_xll {
        c.check("Audio data integrity", Status::Info, "the DTS core format has no checksum over its audio data, so damage inside the audio can only be found by decoding (deep verification)");
    }
    if s.trailing > 0 {
        c.check(
            "Complete file",
            Status::Fail,
            format!("the stream ends with {} bytes of an unfinished frame", s.trailing),
        );
    } else {
        c.check("Complete file", Status::Pass, "the stream ends on a frame boundary");
    }
}

fn check_core_frames(c: &mut Collector, s: &Walk) {
    c.check(
        "Core headers",
        pass_if_none(s.core_bad),
        if s.core_bad == 0 {
            format!("{} core frames hold legal values", s.cores)
        } else {
            format!("{} invalid core header(s)", s.core_bad)
        },
    );
    if s.core_hdr.as_ref().is_some_and(|k| k.crc_present) {
        c.check(
            "Core header checksum",
            if s.core_hcrc_bad == 0 {
                Status::Pass
            } else {
                Status::Warn
            },
            if s.core_hcrc_bad == 0 {
                "matches".to_string()
            } else {
                format!(
                    "{} frame(s) do not match the range assumed here (not necessarily an error)",
                    s.core_hcrc_bad
                )
            },
        );
    }
    if s.ext_checked > 0 {
        c.check(
            "Core extension sync words",
            pass_if_none(s.ext_missing),
            if s.ext_missing == 0 {
                format!("{} extension headers found where announced", s.ext_checked)
            } else {
                format!(
                    "{} of {} frames lack the announced extension",
                    s.ext_missing, s.ext_checked
                )
            },
        );
    }
    c.check(
        "Stable core parameters",
        if s.core_changes == 0 {
            Status::Pass
        } else {
            Status::Warn
        },
        if s.core_changes == 0 {
            "rate, channels and frame length never change".to_string()
        } else {
            format!("parameters change {} time(s)", s.core_changes)
        },
    );
}

fn check_extension_frames(c: &mut Collector, s: &Walk) {
    let headers_ok = s.exss_bad == 0 && s.exss_hdr_crc_bad == 0;
    c.check(
        "Extension substream headers",
        if headers_ok { Status::Pass } else { Status::Fail },
        if headers_ok {
            format!("{} headers parsed, checksums ok", s.exss)
        } else {
            format!("{} unreadable, {} with a bad checksum", s.exss_bad, s.exss_hdr_crc_bad)
        },
    );
    if s.comp_checked > 0 {
        c.check(
            "Extension component sync words",
            pass_if_none(s.comp_missing),
            if s.comp_missing == 0 {
                format!("{} components start with their sync word", s.comp_checked)
            } else {
                format!(
                    "{} of {} components lack their sync word",
                    s.comp_missing, s.comp_checked
                )
            },
        );
    }
    if s.xll_checked > 0 {
        let xll_ok = s.xll_bad == 0 && s.xll_chset_bad == 0;
        c.check(
            "Lossless (XLL) frame headers",
            if xll_ok { Status::Pass } else { Status::Fail },
            if xll_ok {
                format!("{} frames: common and channel set header checksums ok", s.xll_checked)
            } else {
                format!(
                    "{} frame(s) with a bad common header, {} with bad channel set headers",
                    s.xll_bad, s.xll_chset_bad
                )
            },
        );
    }
}

/// The listings of a deep verification: the first frame's headers and how the lossless predictors were used.
pub(crate) fn deep_sections(c: &mut Collector, s: &Walk) {
    if let Some(k) = &s.core_hdr {
        c.section("Core substream header (first frame)", core_rows(k));
    }
    if let Some(e) = &s.exss_first {
        c.section("Extension substream header (first frame)", exss_rows(e));
    }
    if let Some(x) = &s.xll_first {
        c.section("Lossless (XLL) frame header (first frame)", xll_rows(x));
    }
    if s.pred_frames > 0 {
        c.section("Lossless coding tools (all frames)", prediction_rows(s));
    }
}

/// How the lossless predictors were used, per frequency band.
fn prediction_rows(s: &Walk) -> Vec<(String, String)> {
    let mut rows = vec![("frames analysed".to_string(), s.pred_frames.to_string())];
    for (i, hist) in s.pred_adapt.iter().enumerate() {
        let total: u64 = hist.iter().sum::<u64>().max(1);
        let mean = hist.iter().enumerate().map(|(o, n)| o as u64 * n).sum::<u64>() as f64 / total as f64;
        let common: Vec<String> = hist
            .iter()
            .enumerate()
            .filter(|(_, n)| **n * 50 >= total)
            .map(|(o, n)| format!("{o}:{:.0}%", 100.0 * *n as f64 / total as f64))
            .collect();
        rows.push((
            format!("band {i} adaptive predictor order"),
            format!("mean {mean:.1} ({})", common.join(" ")),
        ));
        let fixed_total: u64 = s.pred_fixed[i].iter().sum::<u64>().max(1);
        rows.push((
            format!("band {i} fixed order (when no adaptive)"),
            s.pred_fixed[i]
                .iter()
                .enumerate()
                .map(|(o, n)| format!("{o}:{:.0}%", 100.0 * *n as f64 / fixed_total as f64))
                .collect::<Vec<_>>()
                .join(" "),
        ));
        rows.push((
            format!("band {i} channel decorrelation"),
            format!(
                "on in {:.0}% of frames",
                100.0 * s.pred_decor[i] as f64 / s.pred_frames as f64
            ),
        ));
    }
    rows
}
