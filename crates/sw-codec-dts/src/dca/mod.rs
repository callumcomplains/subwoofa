//! DTS Coherent Acoustics: the core codec (Digital Surround, ES, 96/24), its subband filter banks and the
//! frame encoder, and the file-level drivers that feed it.

pub mod core_exact;
#[rustfmt::skip]
pub mod dmix_table;
pub mod dts96;
pub mod encoder;
pub mod es;
pub mod native;
pub mod native96;
pub mod native_xxch;
pub mod qmf;
pub mod qmf64;
#[rustfmt::skip]
pub mod tables;
#[rustfmt::skip]
pub mod tables_fixed;
pub mod x96;
