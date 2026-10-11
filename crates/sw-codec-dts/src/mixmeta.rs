//! The Bitstream-tab metadata, written the way the original encoder writes it.
//!
//! Everything here was measured by driving the original DTS-HD Master Audio Suite encoder with every setting of
//! the tab and reading the streams back (`docs/ORIGINAL-ENCODER.md`):
//!
//! * **Secondary audio mixing metadata** (DTS Express, Blu-ray): in every extension substream header the stream
//!   says how much to turn the *primary* audio down while it plays (a 6-bit code, new for every frame so that
//!   fades are smooth) and where its mono signal goes in the output speakers (a 6-bit gain code per speaker).
//! * **Program information**: the text goes into the `.dtshd` container's `FILEINFO` chunk, not into the audio.
//! * **Seamless branching**: a `BRANCHPT` chunk lists the access units at which a player may jump.
//! * **Wide 7.1 remapping**: six speaker-remapping sets in the asset descriptor.

use crate::dtshd::{fps_exact, timecode_frames, DtsHdInfo};
use sw_core::bitstream::{MonoMap, Seamless, SecondaryAudio};
use sw_core::BitstreamOptions;

/// The 6-bit code of a gain or attenuation in dB (the same table is used for the primary audio attenuation and
/// for the speaker gains): half-dB steps from 0 dB (code 61) to 15 dB (code 31), one-dB steps to 30 dB (code 16),
/// two-dB steps to 60 dB (code 1). The thresholds are the encoder's own: the half-dB region rounds a little
/// early (a step starts 0.125 dB below its nominal value), the coarser ones round to the nearest step.
pub fn level_code(db: f64) -> u8 {
    let db = db.max(0.0);
    if db < 0.375 {
        61
    } else if db < 14.875 {
        (60 - ((db - 0.375) / 0.5).floor() as i32) as u8
    } else if db < 15.5 {
        31
    } else if db < 29.5 {
        (30 - (db - 15.5).floor() as i32) as u8
    } else if db < 31.0 {
        16
    } else {
        (15 - ((db - 31.0) / 2.0).floor() as i32).max(1) as u8
    }
}

/// The attenuation of the primary audio, in dB, at `t` seconds into a stream `total` seconds long.
///
/// A fade is not linear in dB: the gain follows `1 / (1 + (M - 1) u)^2` with `M = 10^(dB / 40)` while `u` runs
/// from 0 to 1 over the fade, which starts quickly and slows down (more so for deep attenuation).
pub fn attenuation_at(att_db: f64, fade_down: f64, fade_up: f64, t: f64, total: f64) -> f64 {
    if att_db <= 0.0 {
        return 0.0;
    }
    let mut u = 1.0f64;
    if fade_down > 0.0 && t < fade_down {
        u = u.min(t / fade_down);
    }
    if fade_up > 0.0 && t > total - fade_up {
        u = u.min(((total - t) / fade_up).max(0.0));
    }
    let m = 10f64.powf(att_db / 40.0);
    (40.0 * (1.0 + (m - 1.0) * u.clamp(0.0, 1.0)).log10()).min(att_db)
}

/// Mixing metadata of one frame.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameMix {
    /// Code for the attenuation of the primary audio.
    pub scale: u8,
    /// Output speakers (bit 0 C, 1 L, 2 R, 3 Ls, 4 Rs, 5 LFE) that receive the mono secondary, with the gain
    /// code of each in ascending bit order.
    pub speakers: u8,
    pub gains: Vec<u8>,
}

/// Mixing metadata for every frame of a secondary-audio stream.
pub struct SecondaryPlan {
    pub frames: Vec<FrameMix>,
}

impl SecondaryPlan {
    /// `frame_secs` is the duration of one stream frame, `total_secs` the programme length the fades are laid
    /// against, `frames` the number of frames.
    pub fn new(s: &SecondaryAudio, frame_secs: f64, total_secs: f64, frames: usize) -> SecondaryPlan {
        let (speakers, gains) = speaker_gains(&s.mono_map);
        let fading = s.fade_down_secs > 0.0 || s.fade_up_secs > 0.0;
        let mut out = Vec::with_capacity(frames);
        for k in 0..frames {
            let db = if fading {
                attenuation_at(s.attenuation_db, s.fade_down_secs, s.fade_up_secs, k as f64 * frame_secs, total_secs)
            } else {
                s.attenuation_db
            };
            out.push(FrameMix { scale: level_code(db), speakers, gains: gains.clone() });
        }
        // the last frame is always back at full level once a fade up is on
        if s.fade_up_secs > 0.0 {
            if let Some(last) = out.last_mut() {
                last.scale = 61;
            }
        }
        SecondaryPlan { frames: out }
    }
}

fn speaker_gains(m: &MonoMap) -> (u8, Vec<u8>) {
    let all = [m.c, m.l, m.r, m.ls, m.rs, m.lfe];
    let mut mask = 0u8;
    let mut gains = Vec::new();
    for (bit, g) in all.iter().enumerate() {
        if let Some(db) = g {
            mask |= 1 << bit;
            gains.push(level_code(*db));
        }
    }
    (mask, gains)
}

/// Payload of the `FILEINFO` chunk: `0x09`, the text (ISO 8859-1; what it cannot hold becomes `?`), a newline,
/// zero padding to a multiple of four bytes.
pub fn file_info(text: &str) -> Vec<u8> {
    let mut v = vec![0x09u8];
    for c in text.chars().take(2000) {
        v.push(if (c as u32) < 256 { c as u32 as u8 } else { b'?' });
    }
    v.push(b'\n');
    while v.len() % 4 != 0 {
        v.push(0);
    }
    v
}

/// Access unit number of a branch point `secs` into the stream: the encoder counts in 512-sample units of the
/// 48 kHz core whatever the stream's rate is, and the first audio sits three units in.
pub fn branch_unit(secs: f64) -> u32 {
    (secs * 48_000.0 / 512.0).floor() as u32 + 3
}

/// Payload of the `BRANCHPT` chunk: the number of points (the start of the clip always counts as one), the byte
/// width of an entry (1, or 2 when any unit exceeds 255), then the units, padded to a multiple of four bytes.
/// `units` may include points outside the stream; they are dropped by the caller.
pub fn branch_points(units: &[u32]) -> Vec<u8> {
    let mut all = vec![branch_unit(0.0)];
    all.extend_from_slice(units);
    all.sort_unstable();
    all.dedup();
    let wide = all.iter().any(|u| *u > 255);
    let mut v = Vec::new();
    v.extend_from_slice(&(all.len() as u32).to_be_bytes());
    v.push(if wide { 2 } else { 1 });
    for u in &all {
        if wide {
            v.extend_from_slice(&(*u as u16).to_be_bytes());
        } else {
            v.push(*u as u8);
        }
    }
    while v.len() % 4 != 0 {
        v.push(0);
    }
    v
}

/// Seconds into the programme of a timecode `HH:MM:SS:FF` (`;` or `.` before the frames mean drop frame, which
/// only matters at 29.97 and 30 frames per second), for a stream whose first sample is at `start` seconds.
pub fn timecode_to_secs(tc: &str, fps_code: u8, start_secs: f64) -> Option<f64> {
    let parts: Vec<&str> = tc.trim().split([':', ';', '.']).collect();
    if parts.len() != 4 {
        return None;
    }
    let n: Vec<u32> = parts.iter().map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    let code = if tc.contains(';') && fps_code == crate::dtshd::FPS_29_97 {
        crate::dtshd::FPS_29_97_DROP
    } else if tc.contains(';') && fps_code == crate::dtshd::FPS_30 {
        crate::dtshd::FPS_30_DROP
    } else {
        fps_code
    };
    let frames = timecode_frames(code, n[0], n[1], n[2], n[3]);
    let (num, den) = fps_exact(code);
    Some(frames as f64 * den as f64 / num as f64 - start_secs)
}

/// Put the programme information and the branch points into the container description. `span` is the length
/// of the encode in seconds. Returns notes for the log.
pub fn apply_container_options(info: &mut DtsHdInfo, bs: &BitstreamOptions, span: f64) -> Vec<String> {
    let mut notes = Vec::new();
    if !bs.program_info.is_empty() {
        info.file_info = file_info(&bs.program_info);
    }
    match &bs.seamless {
        Seamless::Off => {}
        Seamless::SingleClip => info.branch_points = Some(branch_points(&[])),
        Seamless::Timecodes(list) => {
            let start = info.start.samples as f64 / 48_000.0;
            let mut units = Vec::new();
            let mut used = Vec::new();
            for tc in list {
                match timecode_to_secs(tc, info.fps_code, start) {
                    Some(t) if t > 0.0 && t < span => {
                        units.push(branch_unit(t));
                        used.push(tc.clone());
                    }
                    Some(t) if t == 0.0 => {}
                    Some(_) => notes.push(format!("branch point {tc} is outside the encode and was not used")),
                    None => notes.push(format!("branch point '{tc}' is not a timecode (HH:MM:SS:FF) and was ignored")),
                }
            }
            info.branch_points = Some(branch_points(&units));
        }
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_codes_match_the_original_table() {
        // values read off the original encoder's output
        for (db, code) in [(0.0, 61), (0.3, 61), (0.4, 60), (6.0, 49), (12.0, 37), (14.8, 32), (14.9, 31), (15.4, 31), (15.6, 30), (18.0, 28), (29.4, 17), (29.6, 16), (30.9, 16), (31.1, 15), (45.0, 8), (59.0, 1), (60.0, 1)] {
            assert_eq!(level_code(db), code, "{db} dB");
        }
    }

    #[test]
    fn fade_down_is_fast_at_first() {
        // thirty dB over two seconds: 3 dB, then 5.5 dB at the first two frames (85.3 ms each)
        let f = 4096.0 / 48_000.0;
        let codes: Vec<u8> = (0..4).map(|k| level_code(attenuation_at(30.0, 2.0, 0.0, k as f64 * f, 10.0))).collect();
        assert_eq!(codes, vec![61, 55, 50, 45]);
    }

    #[test]
    fn branch_chunks() {
        assert_eq!(branch_points(&[]), vec![0, 0, 0, 1, 1, 3, 0, 0]);
        assert_eq!(branch_points(&[branch_unit(1.0), branch_unit(2.0)]), vec![0, 0, 0, 3, 1, 3, 0x60, 0xbe]);
        assert_eq!(branch_points(&[96, 190, 378, 753]), hex("00000005020003006000be017a02f100"));
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn file_info_padding() {
        assert_eq!(file_info(""), vec![9, 10, 0, 0]);
        assert_eq!(file_info("Subwoofa test ABC 123").len(), 24);
        assert_eq!(file_info("Café")[4], 0xe9);
    }
}
