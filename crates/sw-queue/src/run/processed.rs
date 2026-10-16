//! The lossless check for a job whose audio Subwoofa changed on the way in: Normalise's gain, a delay.
//!
//! The usual check hashes FFmpeg's read of the input files, which knows nothing of either change, so it can never
//! match. Here the source side is read again through a `StemSet` set up exactly as the encoder's was, rounded to the
//! grid the encoder codes at, and hashed the way FFmpeg hashes the decode: interleaved 24-bit little-endian samples
//! in the decoder's channel order. Pieces: `Edit` (what was changed), `verify` (the check), `source_md5`.

use crate::run::sources::open_sources;
use crate::run::spec::{EncodeSpec, Failure, Outcome};
use sw_backend_ffmpeg as ff;
use sw_core::formats::StreamType;
use sw_core::ChannelLayout;
use sw_pcm::StemSet;

/// Both sides are hashed as 24-bit samples; a source coded at fewer bits is shifted up, as FFmpeg does.
const HASH_FORMAT: &str = "pcm_s24le";
const HASH_BITS: u32 = 24;
/// Frames read per block while hashing.
const BLOCK: usize = 16_384;

/// What Subwoofa changed in the audio as it was read.
#[derive(Clone, Copy, Default)]
pub(crate) struct Edit {
    /// Frames of silence in front (positive) or of the start left out (negative).
    pub(crate) delay_frames: i64,
    /// Normalise's gain, in dB.
    pub(crate) gain_db: Option<f64>,
}

impl Edit {
    pub(crate) fn changes_audio(&self) -> bool {
        self.delay_frames != 0 || self.gain_db.is_some()
    }

    fn apply(&self, stems: &mut StemSet) {
        if self.delay_frames != 0 {
            stems.set_delay_frames(self.delay_frames);
        }
        if let Some(db) = self.gain_db {
            stems.set_gain_db(db);
        }
    }
}

/// Checks that the lossless file decodes to exactly the edited source over `(start, span)`.
pub(crate) fn verify(
    spec: &EncodeSpec,
    stream: StreamType,
    layout: &'static ChannelLayout,
    job: &ff::Job,
    (start, span): (f64, f64),
    edit: Edit,
    log: &mut String,
) -> Result<Outcome, Failure> {
    let src = source_md5(spec, stream, layout, (start, span), edit)?;
    let dec = ff::decoded_md5(job, HASH_FORMAT)?;
    if src != dec {
        let detail =
            format!("decoded audio differs from the source with the gain/delay applied (source {src}, decoded {dec})");
        log.push_str(&format!("Verify: FAILED: {detail}\n"));
        return Err(Failure::Error(detail));
    }
    let message = format!("lossless verified against the source with the gain/delay applied, MD5 {src}");
    log.push_str(&format!("Verify: {message}\n"));
    Ok(Outcome {
        message,
        verified: true,
        ..Default::default()
    })
}

/// MD5 of the edited source as the encoder saw it, rounded to its coding grid.
fn source_md5(
    spec: &EncodeSpec,
    stream: StreamType,
    layout: &'static ChannelLayout,
    (start, span): (f64, f64),
    edit: Edit,
) -> Result<String, Failure> {
    let mut stems = open_sources(spec, layout)?.stems;
    edit.apply(&mut stems);
    let total = stems.select_span(start, Some(span)).map_err(|e| e.to_string())?;
    let bits = coded_bits(stream, stems.bits);
    let by_decoded = ff::decode_order(layout);
    let mut md5 = md5::Context::new();
    let mut bytes = Vec::new();
    stems
        .read_ahead(BLOCK, total, |planar, frames| {
            bytes.clear();
            for f in 0..frames {
                for &ch in &by_decoded {
                    let v = quantise(planar[ch][f], bits) << (HASH_BITS - bits);
                    bytes.extend_from_slice(&v.to_le_bytes()[..3]);
                }
            }
            md5.consume(&bytes);
            Ok::<(), sw_pcm::Error>(())
        })
        .map_err(|e| e.to_string())?;
    Ok(format!("{:x}", md5.finalize()))
}

/// The resolution the encoder rounds samples to.
fn coded_bits(stream: StreamType, source_bits: u16) -> u32 {
    match stream {
        StreamType::DtsHdMa => sw_codec_dts::lossless::native::coded_bits(source_bits),
        _ => sw_codec_truehd::native::CODED_BITS,
    }
}

/// The encoders' rounding: nearest step of a `bits`-bit grid, kept in range.
fn quantise(x: f32, bits: u32) -> i32 {
    let scale = (1i64 << (bits - 1)) as f32;
    let (lo, hi) = (-(1i64 << (bits - 1)), (1i64 << (bits - 1)) - 1);
    ((x * scale).round() as i64).clamp(lo, hi) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_like_encoders() {
        assert_eq!(quantise(0.5, 24), 4_194_304);
        assert_eq!(quantise(1.0, 24), 8_388_607);
        assert_eq!(quantise(-1.5, 16), -32_768);
        assert_eq!(quantise(1.5 / 32_768.0, 16), 2);
    }
}
