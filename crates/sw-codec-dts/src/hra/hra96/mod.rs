//! DTS-HD High Resolution Audio at 96 kHz, in the layout of the commercial encoder's streams.
//!
//! The core substream holds a plain 48 kHz core (1509 kbps, 5.1 or 5.0). The extension substream holds an X96
//! extension carrying what the 48 kHz coding cannot: the upper half of the spectrum, and a refinement of the lower
//! half. What else goes where depends on the layout:
//!
//! | layout            | core                                  | extension substream               |
//! |-------------------|---------------------------------------|-----------------------------------|
//! | 5.1 / 5.0         | the channels                          | X96 (one channel set)             |
//! | 6.1 matrix        | Cs mixed into the surrounds (ES flag) | X96 (one channel set)             |
//! | 6.1 discrete      | Cs mixed in, and sent again in XCH    | X96 (the core channels, then Cs)  |
//! | 7.1 / 7.0         | rears folded into the sides           | XXCH (the rear pair) and X96      |

mod driver;
mod encoder;
mod map;

pub use driver::encode_to_file;
pub use map::{hra96_map, Hra96Kind, Hra96Map};

/// Bytes of the XXCH extension in a 96 kHz 7.x stream until X96 is full (the original encoder's split, measured at every legal rate).
const XXCH_FIXED_BYTES: usize = 512;
/// The largest X96 component an extension substream header can describe (a 12-bit size, rounded down to words).
const MAX_X96_BYTES: usize = 4092;
/// Gain of the centre surround where it is mixed into the surrounds.
const CS_MIX: f64 = std::f64::consts::FRAC_1_SQRT_2;
