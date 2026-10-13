//! How hard a lossless encoder searches: the speed setting of a job.
//!
//! A lossless encoder can always try one more predictor or one more block size, and the last few tries pay very
//! little. `Effort` says how many to try. It changes how big a lossless file comes out and how long it takes to
//! make; it never changes whether the file is correct. Lossy formats (Dolby Digital, DTS core, High Resolution,
//! Express) are fixed by their bit rate, so the setting does not touch their quality.

use crate::formats::StreamType;

/// The speed setting of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Effort {
    /// The first predictors that work. Several times quicker; lossless files come out a few per cent larger.
    Fast,
    /// Skips what rarely pays. Close to `Best` in size (usually within a fraction of a per cent) and much quicker.
    #[default]
    Balanced,
    /// Tries everything the encoder knows. The smallest files and the longest wait.
    Best,
}

impl Effort {
    pub const ALL: [Effort; 3] = [Effort::Fast, Effort::Balanced, Effort::Best];

    pub fn id(self) -> &'static str {
        match self {
            Effort::Fast => "fast",
            Effort::Balanced => "balanced",
            Effort::Best => "best",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Effort::Fast => "Fast",
            Effort::Balanced => "Balanced",
            Effort::Best => "Best",
        }
    }

    /// Reads an id; an empty string means the default.
    pub fn from_id(id: &str) -> Option<Effort> {
        match id.trim().to_ascii_lowercase().as_str() {
            "" => Some(Effort::default()),
            "fast" => Some(Effort::Fast),
            "balanced" => Some(Effort::Balanced),
            "best" => Some(Effort::Best),
            _ => None,
        }
    }
}

/// What a lower speed setting costs in file size for a format, measured against `Best` on film and music at 48 and
/// 96 kHz (see docs/SPEED.md), or `None` when the setting does not change the format at all: lossy formats are
/// fixed by their bit rate, and the other lossless formats have a single level.
pub fn size_cost(stream: StreamType, effort: Effort) -> Option<&'static str> {
    use StreamType::*;
    let (balanced, fast) = match stream {
        TrueHd | Mlp => ("up to 1% larger", "1% to 5% larger"),
        DtsHdMa => ("up to 0.3% larger", "up to 0.3% larger"),
        Flac | OggFlac => ("no larger", "0.2% to 6% larger"),
        Alac => ("under 0.1% larger", "under 1% larger"),
        MpegAls => ("under 0.1% larger", "0.1% to 6% larger"),
        _ => return None,
    };
    match effort {
        Effort::Best => Some("the smallest file"),
        Effort::Balanced => Some(balanced),
        Effort::Fast => Some(fast),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_formats_with_levels_have_a_size_cost() {
        assert!(size_cost(StreamType::TrueHd, Effort::Fast).is_some());
        assert!(size_cost(StreamType::Flac, Effort::Balanced).is_some());
        assert!(size_cost(StreamType::Ac3, Effort::Fast).is_none());
        assert!(size_cost(StreamType::WavPack, Effort::Fast).is_none());
    }

    #[test]
    fn ids_round_trip() {
        for e in Effort::ALL {
            assert_eq!(Effort::from_id(e.id()), Some(e));
        }
        assert_eq!(Effort::from_id(""), Some(Effort::Balanced));
        assert_eq!(Effort::from_id(" FAST "), Some(Effort::Fast));
        assert_eq!(Effort::from_id("turbo"), None);
    }
}
