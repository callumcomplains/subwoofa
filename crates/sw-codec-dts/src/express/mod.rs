//! DTS-HD Express (LBR): the low bit rate codec, with its own filter bank, tonal components and drivers.

pub mod encoder;
pub mod filterbank;
pub mod lfe;
pub mod lpc;
pub mod native;
#[rustfmt::skip]
pub mod tables;
pub mod tonal;
