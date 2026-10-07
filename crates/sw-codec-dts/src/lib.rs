//! Native DTS core (Digital Surround) encoder, written from ETSI TS 102 114.

pub mod bits;
pub mod encoder;
pub mod exss;
pub mod ma;
pub mod native;
pub mod qmf;
pub mod tables;
pub mod xll;
mod xll_table;

pub use encoder::{Config, Encoder, Error};
