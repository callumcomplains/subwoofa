//! DTS-HD Express (LBR): the low bit rate codec, with its own filter bank, tonal components and drivers.

pub mod bits;
mod channel;
pub mod encoder;
pub mod filterbank;
mod levels;
pub mod lfe;
pub mod lpc;
pub mod native;
mod pair;
#[rustfmt::skip]
pub mod tables;
pub mod tonal;
