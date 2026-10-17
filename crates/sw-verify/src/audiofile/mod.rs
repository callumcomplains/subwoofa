//! Structure checks for plain audio files: WAV / RF64, AIFF, CAF, FLAC (also in Ogg), ALAC / ALS in MP4, TTA,
//! WavPack, Monkey's Audio, raw ALS, MXF sound and DSD (DSF and DSDIFF). Each check reads the headers the way a
//! player would and compares what they say with each other and with the size of the file. Deep verification then
//! decodes the audio (see `deep.rs`). One module per format family; this file recognises the kind and dispatches.

mod als;
mod bytes;
mod dsd;
mod flac;
mod m4a;
mod monkeys;
mod mxf;
mod riff;
mod tta;
mod wavpack;

use crate::{Collector, Probe, Status, Window};
use als::als;
use dsd::{dff, dsf};
use flac::{flac, ogg_flac};
use m4a::m4a;
use monkeys::monkeys;
use mxf::mxf;
use riff::{aiff, caf, wav};
use tta::tta;
use wavpack::wavpack;

/// The file kinds this module understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AudioKind {
    Wav,
    Aiff,
    Caf,
    Flac,
    OggFlac,
    M4a,
    Tta,
    WavPack,
    Monkeys,
    Als,
    Mxf,
    Dsf,
    Dff,
}

impl AudioKind {
    pub fn name(self) -> &'static str {
        match self {
            AudioKind::Wav => "WAV / Broadcast Wave",
            AudioKind::Aiff => "AIFF",
            AudioKind::Caf => "CAF (Core Audio Format)",
            AudioKind::Flac => "FLAC",
            AudioKind::OggFlac => "FLAC in Ogg",
            AudioKind::M4a => "M4A (MPEG-4 audio)",
            AudioKind::Tta => "TTA (True Audio)",
            AudioKind::WavPack => "WavPack",
            AudioKind::Monkeys => "Monkey's Audio",
            AudioKind::Als => "MPEG-4 ALS",
            AudioKind::Mxf => "MXF (digital cinema sound)",
            AudioKind::Dsf => "DSD (DSF)",
            AudioKind::Dff => "DSD (DSDIFF)",
        }
    }
}

/// Recognises the file kinds by their first bytes.
pub(crate) fn sniff(h: &[u8]) -> Option<AudioKind> {
    if h.len() >= 12 {
        if (h.starts_with(b"RIFF") || h.starts_with(b"RF64")) && &h[8..12] == b"WAVE" {
            return Some(AudioKind::Wav);
        }
        if h.starts_with(b"FORM") && (&h[8..12] == b"AIFF" || &h[8..12] == b"AIFC") {
            return Some(AudioKind::Aiff);
        }
        if h.starts_with(b"FRM8") && h.len() >= 16 && &h[12..16] == b"DSD " {
            return Some(AudioKind::Dff);
        }
        if &h[4..8] == b"ftyp" {
            return Some(AudioKind::M4a);
        }
    }
    if h.starts_with(b"caff") {
        return Some(AudioKind::Caf);
    }
    if h.starts_with(b"fLaC") {
        return Some(AudioKind::Flac);
    }
    if h.starts_with(b"OggS") && h.len() >= 33 && h[28..33] == [0x7F, b'F', b'L', b'A', b'C'] {
        return Some(AudioKind::OggFlac);
    }
    if h.starts_with(b"TTA1") {
        return Some(AudioKind::Tta);
    }
    if h.starts_with(&[
        0x06, 0x0e, 0x2b, 0x34, 0x02, 0x05, 0x01, 0x01, 0x0d, 0x01, 0x02, 0x01, 0x01, 0x02,
    ]) {
        return Some(AudioKind::Mxf);
    }
    if h.starts_with(b"ALS\0") {
        return Some(AudioKind::Als);
    }
    if h.starts_with(b"MAC ") {
        return Some(AudioKind::Monkeys);
    }
    if h.starts_with(b"wvpk") {
        return Some(AudioKind::WavPack);
    }
    if h.starts_with(b"DSD ") {
        return Some(AudioKind::Dsf);
    }
    None
}

/// What the headers say about the audio.
#[derive(Default, Clone)]
struct Facts {
    rate: u32,
    channels: u32,
    bits: u32,
    /// Samples per channel, when the headers give it.
    frames: Option<u64>,
    lossless: bool,
    /// No decoder exists for the codec inside (AC-4 or DTS-UHD in an MP4).
    no_decoder: bool,
}

pub(crate) fn verify(w: &mut Window, c: &mut Collector, kind: AudioKind) -> Probe {
    c.check("Recognised file", Status::Pass, kind.name());
    let facts = match kind {
        AudioKind::Wav => wav(w, c),
        AudioKind::Aiff => aiff(w, c),
        AudioKind::Caf => caf(w, c),
        AudioKind::Flac => flac(w, c),
        AudioKind::OggFlac => ogg_flac(w, c),
        AudioKind::M4a => m4a(w, c),
        AudioKind::Tta => tta(w, c),
        AudioKind::WavPack => wavpack(w, c),
        AudioKind::Monkeys => monkeys(w, c),
        AudioKind::Als => als(w, c),
        AudioKind::Mxf => mxf(w, c),
        AudioKind::Dsf => dsf(w, c),
        AudioKind::Dff => dff(w, c),
    };
    let duration = match (facts.frames, facts.rate) {
        (Some(f), r) if r > 0 => f as f64 / r as f64,
        _ => 0.0,
    };
    if facts.rate > 0 {
        c.kv("Sample rate", format!("{} Hz", facts.rate));
    }
    if facts.channels > 0 {
        c.kv("Channels", facts.channels.to_string());
    }
    if facts.bits > 0 {
        c.kv(
            "Sample format",
            if kind == AudioKind::Dsf || kind == AudioKind::Dff {
                "1-bit DSD".to_string()
            } else {
                format!("{}-bit", facts.bits)
            },
        );
    }
    if let Some(f) = facts.frames {
        c.kv("Length", format!("{f} samples per channel ({duration:.3} s)"));
    }
    Probe {
        duration_secs: duration,
        sample_rate: facts.rate,
        channels: facts.channels,
        lossless: facts.lossless,
        no_decoder: facts.no_decoder,
        ..Default::default()
    }
}
