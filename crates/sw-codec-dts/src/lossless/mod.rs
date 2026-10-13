//! DTS-HD Master Audio: the lossless extension (XLL) and the drivers that put it together with a core.

pub mod ma;
pub mod ma_core;
pub mod native;
pub mod xll;
pub mod xll_bands;
#[rustfmt::skip]
pub mod xll_table;
