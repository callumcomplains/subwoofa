//! Changing the dialogue level (dialnorm) of a finished AC-3 or E-AC-3 stream without re-encoding it.
//!
//! The level is a five-bit field in each frame's bit stream information. Writing it means finding it (the fields in
//! front of it come and go with the channel mode), changing five bits, and putting the CRCs right again: AC-3 has two
//! (`crc1` over the first five eighths of the frame, `crc2` over all of it), E-AC-3 one at the end. The audio is not
//! touched, so the stream plays exactly as before apart from the level a decoder applies.

use crate::bits::{crc16, crc1_field};
use crate::tables::FRAME_SIZE_WORDS;

/// What an edit did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edited {
    pub frames: usize,
    /// Frames whose level was different and has been rewritten.
    pub changed: usize,
    pub eac3: bool,
    /// The level of the first frame before the edit.
    pub previous: u8,
}

fn read_bits(data: &[u8], pos: usize, n: usize) -> u32 {
    (0..n).fold(0, |v, i| {
        let bit = pos + i;
        (v << 1) | ((data[bit / 8] >> (7 - bit % 8)) & 1) as u32
    })
}

fn write_bits(data: &mut [u8], pos: usize, n: usize, value: u32) {
    for i in 0..n {
        let bit = pos + i;
        let mask = 0x80u8 >> (bit % 8);
        if (value >> (n - 1 - i)) & 1 == 1 {
            data[bit / 8] |= mask;
        } else {
            data[bit / 8] &= !mask;
        }
    }
}

/// Where the dialnorm fields of a frame sit (bit offsets from the sync word), and the frame's length in bytes.
struct Layout {
    size: usize,
    eac3: bool,
    fields: Vec<usize>,
}

/// The fields after `dialnorm` that lead to `dialnorm2` of a dual-mono frame: `compre`, `langcode` and (AC-3 only)
/// `audprodie`, each a flag followed by its payload.
fn skip_optional(data: &[u8], pos: &mut usize, payload: usize) {
    let present = read_bits(data, *pos, 1) == 1;
    *pos += 1 + if present { payload } else { 0 };
}

fn layout_of(frame: &[u8]) -> Result<Layout, String> {
    if frame.len() < 8 || frame[0] != 0x0B || frame[1] != 0x77 {
        return Err("a frame does not start with the AC-3 sync word".into());
    }
    let ac3_bsid = frame[5] >> 3;
    if ac3_bsid <= 10 {
        let (fscod, frmsizecod) = ((frame[4] >> 6) as usize, (frame[4] & 0x3F) as usize);
        if fscod == 3 || frmsizecod >= 38 {
            return Err("a frame has a reserved sample rate or frame size code".into());
        }
        let size = FRAME_SIZE_WORDS[frmsizecod * 3 + fscod] as usize * 2;
        let mut pos = 48;
        let acmod = read_bits(frame, pos, 3);
        pos += 3;
        if acmod & 1 != 0 && acmod != 1 {
            pos += 2; // cmixlev
        }
        if acmod & 4 != 0 {
            pos += 2; // surmixlev
        }
        if acmod == 2 {
            pos += 2; // dsurmod
        }
        pos += 1; // lfeon
        let mut fields = vec![pos];
        pos += 5;
        if acmod == 0 {
            skip_optional(frame, &mut pos, 8); // compre
            skip_optional(frame, &mut pos, 8); // langcode
            skip_optional(frame, &mut pos, 7); // audprodie: mixlevel and roomtyp
            fields.push(pos);
        }
        return Ok(Layout {
            size,
            eac3: false,
            fields,
        });
    }
    // E-AC-3: strmtyp(2) substreamid(3) frmsiz(11) fscod(2) numblkscod(2) or fscod2(2), acmod(3) lfeon(1) bsid(5)
    let frmsiz = read_bits(frame, 21, 11) as usize;
    // fscod, then numblkscod (or fscod2 when fscod is 3): two bits each either way
    let mut pos = 36;
    let acmod = read_bits(frame, pos, 3);
    pos += 4;
    let bsid = read_bits(frame, pos, 5);
    if !(11..=16).contains(&bsid) {
        return Err(format!(
            "a frame has bit stream id {bsid}, which is neither AC-3 nor E-AC-3"
        ));
    }
    pos += 5;
    let mut fields = vec![pos];
    pos += 5;
    if acmod == 0 {
        skip_optional(frame, &mut pos, 8); // compre
        fields.push(pos);
    }
    Ok(Layout {
        size: (frmsiz + 1) * 2,
        eac3: true,
        fields,
    })
}

/// Puts the CRCs of a frame right after its contents changed.
fn fix_crcs(frame: &mut [u8], eac3: bool) {
    let size = frame.len();
    if !eac3 {
        // crc1 covers the first five eighths and must be in place before crc2, which covers everything after the sync
        let five_eighths = ((size >> 2) + (size >> 4)) << 1;
        let after = crc16(&frame[4..five_eighths]);
        let crc1 = crc1_field(after, five_eighths - 2);
        frame[2..4].copy_from_slice(&crc1.to_be_bytes());
    }
    frame[size - 2..].fill(0);
    let mut crc = crc16(&frame[2..size - 2]);
    if crc == 0x0B77 {
        // never let the CRC imitate a sync word: flip the reserved bit before it, which leaves the residual unchanged
        frame[size - 3] ^= 1;
        crc ^= 0x8005;
    }
    frame[size - 2..].copy_from_slice(&crc.to_be_bytes());
}

/// The level of the first frame and how many frames the stream has (`changed` is zero), without changing anything.
pub fn inspect(stream: &[u8]) -> Result<Edited, String> {
    let (mut at, mut frames, mut eac3, mut previous) = (0, 0, false, 0);
    while at < stream.len() {
        let layout = layout_of(&stream[at..]).map_err(|e| format!("at byte {at}: {e}"))?;
        if layout.size > stream.len() - at || layout.size < 8 {
            return Err(format!("the stream ends in the middle of a frame (byte {at})"));
        }
        if frames == 0 {
            eac3 = layout.eac3;
            previous = read_bits(&stream[at..], layout.fields[0], 5) as u8;
        }
        frames += 1;
        at += layout.size;
    }
    if frames == 0 {
        return Err("there are no frames in the stream".into());
    }
    Ok(Edited {
        frames,
        changed: 0,
        eac3,
        previous,
    })
}

/// Sets the dialnorm (1 to 31, the attenuation in dB below full scale) of every frame of an AC-3 or E-AC-3 stream.
pub fn set_dialnorm(stream: &mut [u8], dialnorm: u8) -> Result<Edited, String> {
    if !(1..=31).contains(&dialnorm) {
        return Err("the dialnorm must be between 1 and 31 (-1 to -31 dBFS)".into());
    }
    let (mut at, mut frames, mut changed) = (0, 0, 0);
    let (mut eac3, mut previous) = (false, 0);
    while at < stream.len() {
        let rest = &stream[at..];
        let layout = layout_of(rest).map_err(|e| format!("at byte {at}: {e}"))?;
        if layout.size > rest.len() || layout.size < 8 {
            return Err(format!("the stream ends in the middle of a frame (byte {at})"));
        }
        let frame = &mut stream[at..at + layout.size];
        if frames == 0 {
            eac3 = layout.eac3;
            previous = read_bits(frame, layout.fields[0], 5) as u8;
        }
        let mut touched = false;
        for &field in &layout.fields {
            if read_bits(frame, field, 5) != dialnorm as u32 {
                write_bits(frame, field, 5, dialnorm as u32);
                touched = true;
            }
        }
        if touched {
            fix_crcs(frame, layout.eac3);
            changed += 1;
        }
        frames += 1;
        at += layout.size;
    }
    if frames == 0 {
        return Err("there are no frames in the stream".into());
    }
    Ok(Edited {
        frames,
        changed,
        eac3,
        previous,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, Encoder, SAMPLES_PER_FRAME};

    fn frames(eac3: bool, acmod: u8, dialnorm: u8) -> Vec<u8> {
        let cfg = Config {
            acmod,
            lfe: acmod != 1 && acmod != 2,
            sample_rate: 48_000,
            bitrate_kbps: 384,
            dialnorm,
            dolby_surround: false,
            bandwidth_hz: None,
            eac3,
        };
        let channels = [2, 1, 2, 3, 3, 4, 4, 5][acmod as usize] + cfg.lfe as usize;
        let mut enc = Encoder::new(cfg).unwrap();
        let mut out = Vec::new();
        for n in 0..4 {
            let sig: Vec<f32> = (0..SAMPLES_PER_FRAME)
                .map(|i| ((i + n * SAMPLES_PER_FRAME) as f32 * 0.05).sin() * 0.4)
                .collect();
            let planes: Vec<&[f32]> = (0..channels).map(|_| &sig[..]).collect();
            out.extend(enc.encode_frame(&planes));
        }
        out
    }

    fn crcs_hold(stream: &[u8]) {
        let mut at = 0;
        while at < stream.len() {
            let layout = layout_of(&stream[at..]).unwrap();
            let frame = &stream[at..at + layout.size];
            assert_eq!(crc16(&frame[2..]), 0, "the last CRC of the frame at {at}");
            if !layout.eac3 {
                let five_eighths = ((frame.len() >> 2) + (frame.len() >> 4)) << 1;
                assert_eq!(crc16(&frame[2..five_eighths]), 0, "crc1 of the frame at {at}");
            }
            at += layout.size;
        }
    }

    #[test]
    fn ac3_levels_change_and_the_crcs_stay_right() {
        for acmod in [2, 7, 1] {
            let mut stream = frames(false, acmod, 31);
            let before = stream.clone();
            let edited = set_dialnorm(&mut stream, 24).unwrap();
            assert_eq!(edited.frames, 4);
            assert_eq!(edited.changed, 4);
            assert_eq!(edited.previous, 31);
            assert!(!edited.eac3);
            assert_ne!(stream, before);
            crcs_hold(&stream);
            // only the level and the two checksums differ: every other byte is the original audio
            let differing = stream.iter().zip(&before).filter(|(a, b)| a != b).count();
            assert!(differing <= 4 * 4, "{differing} bytes differ for acmod {acmod}");
            // putting it back gives the original stream
            set_dialnorm(&mut stream, 31).unwrap();
            assert_eq!(stream, before);
        }
    }

    #[test]
    fn eac3_levels_change_and_the_crc_stays_right() {
        let mut stream = frames(true, 7, 27);
        let before = stream.clone();
        let edited = set_dialnorm(&mut stream, 20).unwrap();
        assert!(edited.eac3 && edited.changed == 4 && edited.previous == 27);
        crcs_hold(&stream);
        set_dialnorm(&mut stream, 27).unwrap();
        assert_eq!(stream, before);
    }

    #[test]
    fn an_unchanged_level_changes_nothing() {
        let mut stream = frames(false, 7, 27);
        let before = stream.clone();
        assert_eq!(set_dialnorm(&mut stream, 27).unwrap().changed, 0);
        assert_eq!(stream, before);
    }

    #[test]
    fn bad_input_is_refused() {
        assert!(set_dialnorm(&mut [0u8; 64], 24).is_err());
        let mut stream = frames(false, 2, 31);
        assert!(set_dialnorm(&mut stream, 0).is_err());
        assert!(set_dialnorm(&mut stream, 32).is_err());
        let cut = stream.len() - 10;
        assert!(set_dialnorm(&mut stream[..cut], 24).is_err());
    }
}
