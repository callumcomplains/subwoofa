//! Frame rates and start timecodes of a `.dtshd` file, and relabelling them without touching the audio.

use crate::container::chunk_offsets;
use crate::{Error, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Frame rate codes of the `.dtshd` timecode.
pub const FPS_23_976: u8 = 1;

pub const FPS_24: u8 = 2;

pub const FPS_25: u8 = 3;

pub const FPS_29_97_DROP: u8 = 4;

pub const FPS_29_97: u8 = 5;

pub const FPS_30_DROP: u8 = 6;

pub const FPS_30: u8 = 7;

/// Frame rate code for a name such as "23.976", "29.97 drop" or "30".
pub fn fps_code(name: &str) -> Option<u8> {
    let n = name.trim().to_ascii_lowercase().replace(' ', "");
    Some(match n.as_str() {
        "23.976" | "23.98" => FPS_23_976,
        "24" => FPS_24,
        "25" => FPS_25,
        "29.97drop" | "29.97df" => FPS_29_97_DROP,
        "29.97" => FPS_29_97,
        "30drop" | "30df" => FPS_30_DROP,
        "30" => FPS_30,
        _ => return None,
    })
}

pub(crate) fn fps_exact(code: u8) -> (u64, u64) {
    match code {
        FPS_23_976 => (24_000, 1001),
        FPS_25 => (25, 1),
        FPS_29_97 | FPS_29_97_DROP => (30_000, 1001),
        FPS_30 | FPS_30_DROP => (30, 1),
        _ => (24, 1),
    }
}

/// Position of `hh:mm:ss:ff` at a frame rate code, as the file stores it: whole samples at 48 kHz and a
/// remainder in units of 1/(frame rate numerator) sample. Drop-frame rates skip two frame numbers at the
/// start of every minute that is not a tenth minute.
pub fn timecode_position(code: u8, h: u32, m: u32, s: u32, f: u32) -> (u32, u32) {
    let nominal: u64 = match code {
        FPS_23_976 | FPS_24 => 24,
        FPS_25 => 25,
        _ => 30,
    };
    let minutes = (h * 60 + m) as u64;
    let mut frames = (minutes * 60 + s as u64) * nominal + f as u64;
    if code == FPS_29_97_DROP || code == FPS_30_DROP {
        frames -= 2 * (minutes - minutes / 10);
    }
    let (num, den) = fps_exact(code);
    let scaled = frames as u128 * 48_000 * den as u128;
    ((scaled / num as u128) as u32, (scaled % num as u128) as u32)
}

/// Parse `hh:mm:ss:ff` (also with semicolons).
pub fn parse_timecode(text: &str) -> Option<(u32, u32, u32, u32)> {
    let parts: Vec<u32> = text
        .split([':', ';'])
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    (parts.len() == 4).then(|| (parts[0], parts[1], parts[2], parts[3]))
}

/// What a restripe reads: the start time and frame rate in a `.dtshd` file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stripe {
    pub fps_code: u8,
    pub start_samples: u32,
    pub reference_samples: u32,
}

pub fn read_stripe(path: &Path) -> Result<Stripe> {
    let chunks = chunk_offsets(path)?;
    let mut f = File::open(path)?;
    let (_, at, size) = chunks
        .iter()
        .find(|(n, _, _)| n == b"TIMECODE")
        .ok_or_else(|| Error::Format("the file has no TIMECODE chunk (not a .dtshd written by an encoder)".into()))?;
    if *size < 32 {
        return Err(Error::Format("TIMECODE chunk too short".into()));
    }
    f.seek(SeekFrom::Start(*at))?;
    let mut b = [0u8; 32];
    f.read_exact(&mut b)?;
    Ok(Stripe {
        fps_code: b[4],
        start_samples: u32::from_be_bytes(b[9..13].try_into().unwrap()),
        reference_samples: u32::from_be_bytes(b[21..25].try_into().unwrap()),
    })
}

/// Relabel the start time and frame rate of a `.dtshd` file (the header and the timecode chunk; the reference
/// time is kept). Writes `out`, or patches `input` in place when they are the same path.
pub fn restripe(input: &Path, out: &Path, fps_code: u8, start: (u32, u32, u32, u32)) -> Result<()> {
    if input != out {
        std::fs::copy(input, out)?;
    }
    let chunks = chunk_offsets(out)?;
    let (samples, fraction) = timecode_position(fps_code, start.0, start.1, start.2, start.3);
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(out)?;
    let mut patched = 0;
    for (name, at, size) in &chunks {
        if name == b"DTSHDHDR" && *size >= 10 {
            f.seek(SeekFrom::Start(*at + 5))?;
            f.write_all(&samples.to_be_bytes())?;
            f.write_all(&[fps_code])?;
            patched += 1;
        } else if name == b"TIMECODE" && *size >= 32 {
            f.seek(SeekFrom::Start(*at + 4))?;
            f.write_all(&[fps_code])?;
            f.seek(SeekFrom::Start(*at + 8))?;
            f.write_all(&[0])?;
            f.write_all(&samples.to_be_bytes())?;
            f.write_all(&fraction.to_be_bytes())?;
            patched += 1;
        }
    }
    if patched < 2 {
        return Err(Error::Format(
            "the file has no DTSHDHDR and TIMECODE chunks to restripe".into(),
        ));
    }
    Ok(())
}
