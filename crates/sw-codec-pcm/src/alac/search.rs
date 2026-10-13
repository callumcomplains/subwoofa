//! How hard the encoder looks for a smaller frame.

/// Predictor orders tried for each channel (0 = no prediction).
/// What the encoder tries for each frame. More of each takes longer and saves a little.
#[derive(Clone, Copy)]
pub(crate) struct Search {
    /// Predictor orders tried for each channel (0 is no prediction).
    pub(crate) orders: &'static [usize],
    /// Code a stereo pair every way and keep the smallest; otherwise pick the way that looks best from the signals.
    pub(crate) all_stereo: bool,
}

impl Search {
    pub(crate) fn for_effort(effort: sw_core::Effort) -> Search {
        match effort {
            sw_core::Effort::Best => Search {
                orders: &[0, 4, 8, 16],
                all_stereo: true,
            },
            sw_core::Effort::Balanced => Search {
                orders: &[4, 8, 16],
                all_stereo: false,
            },
            sw_core::Effort::Fast => Search {
                orders: &[8],
                all_stereo: false,
            },
        }
    }
}
