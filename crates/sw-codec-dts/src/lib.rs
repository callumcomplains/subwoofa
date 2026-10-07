//! Native DTS core (Digital Surround) encoder, written from ETSI TS 102 114.

pub mod bits;
pub mod encoder;
pub mod native;
pub mod qmf;
pub mod tables;

pub use encoder::{Config, Encoder, Error};
