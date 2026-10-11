//! Subwoofa core: channel layouts, channel IDs, and the format capability matrix.
//!
//! The tables here mirror the *behaviour* documented for legacy multichannel
//! encoders (destination -> stream type -> layout -> sample rate -> bit rate),
//! built from public information only.

pub mod version;
pub mod autoimport;
pub mod bitstream;
pub mod channel;
pub mod downmix;
pub mod formats;
pub mod layout;

pub use bitstream::{BitstreamOptions, Downmix};
pub use channel::Channel;
pub use formats::{Destination, StreamType};
pub use layout::{ChannelLayout, LAYOUTS};
