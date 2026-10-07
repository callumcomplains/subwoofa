//! Subwoofa core: channel layouts, channel IDs, and the format capability matrix.
//!
//! The tables here mirror the *behaviour* documented for legacy multichannel
//! encoders (destination -> stream type -> layout -> sample rate -> bit rate),
//! built from public information only.

pub mod autoimport;
pub mod channel;
pub mod formats;
pub mod layout;

pub use channel::Channel;
pub use formats::{Destination, StreamType};
pub use layout::{ChannelLayout, LAYOUTS};
