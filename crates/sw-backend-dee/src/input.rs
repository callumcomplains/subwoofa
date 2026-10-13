//! What kind of master a file is, from its extension and (for a WAV) its chunks.

use crate::Error;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;

/// What kind of file the job reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputKind {
    /// Multichannel WAV holding a channel bed (Dolby Digital Plus Atmos only).
    Wav,
    /// ADM BWF: a WAV/BW64 file with `chna` and `axml` chunks.
    Adm,
    /// Dolby Atmos Master File set (`.atmos` with its `.metadata` and `.audio` files beside it).
    Damf,
    /// Dolby Atmos mezzanine file.
    AtmosMezz,
}

impl InputKind {
    /// Work out the kind from the file name and, for `.wav`/`.bw64`, from the chunks inside.
    pub fn detect(path: &Path) -> Result<InputKind, Error> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "atmos" => Ok(InputKind::Damf),
            "wav" | "bw64" | "rf64" | "bwf" => Ok(if has_adm_chunks(path)? {
                InputKind::Adm
            } else {
                InputKind::Wav
            }),
            "mxf" | "ec3" | "ac3" | "mp4" => Err(Error::Invalid(format!("'{ext}' files are not Atmos masters"))),
            _ => Ok(InputKind::AtmosMezz),
        }
    }
}

/// Whether a RIFF/BW64 file has an `axml` chunk (looking only at the chunk headers).
pub(crate) fn has_adm_chunks(path: &Path) -> Result<bool, Error> {
    use std::io::{Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    let mut head = [0u8; 12];
    f.read_exact(&mut head)?;
    if !(&head[..4] == b"RIFF" || &head[..4] == b"BW64" || &head[..4] == b"RF64") {
        return Ok(false);
    }
    // sizes of chunks that do not fit 32 bits are in ds64; the audio chunk is the only large one, and skipping
    // it by its stated size works for ordinary files and for BW64 whose data size is in ds64
    let len = f.metadata()?.len();
    let mut pos = 12u64;
    let mut ds64_data: Option<u64> = None;
    while pos + 8 <= len {
        f.seek(SeekFrom::Start(pos))?;
        let mut h = [0u8; 8];
        f.read_exact(&mut h)?;
        let id = [h[0], h[1], h[2], h[3]];
        let mut size = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as u64;
        if &id == b"axml" {
            return Ok(true);
        }
        if &id == b"ds64" {
            let mut b = [0u8; 16];
            f.read_exact(&mut b)?;
            ds64_data = Some(u64::from_le_bytes(b[8..16].try_into().unwrap()));
        }
        if &id == b"data" && size == 0xFFFF_FFFF {
            size = ds64_data.unwrap_or(0);
        }
        pos += 8 + size + (size & 1);
    }
    Ok(false)
}
