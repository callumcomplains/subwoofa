//! How hard a lossless encoder searches: the speed setting of a job.
//!
//! A lossless encoder can always try one more predictor or one more block size, and the last few tries pay very
//! little. `Effort` says how many to try. It changes how big a lossless file comes out and how long it takes to
//! make; it never changes whether the file is correct. Lossy formats (Dolby Digital, DTS core, High Resolution,
//! Express) are fixed by their bit rate, so the setting does not touch their quality.

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

#[cfg(test)]
mod tests {
    use super::*;

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
