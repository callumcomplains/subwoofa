//! Native DTS core (Digital Surround) encoder, written from ETSI TS 102 114.

pub mod bits;
pub mod dmix_table;
pub mod core_exact;
pub mod encoder;
pub mod exss;
pub mod ma;
pub mod ma_core;
pub mod ma_native;
pub mod native;
pub mod qmf;
pub mod tables;
mod tables_fixed;
pub mod xll;
mod xll_table;

pub use encoder::{Config, Encoder, Error};
