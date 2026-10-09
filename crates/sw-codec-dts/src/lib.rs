//! Native DTS core (Digital Surround) encoder, written from ETSI TS 102 114.

pub mod bits;
pub mod dmix_table;
pub mod core_exact;
pub mod encoder;
pub mod dts96;
pub mod dtshd;
pub mod es;
pub mod hra;
pub mod lbr_fb;
mod lbr_tables;
mod lbr_lfe;
mod lbr_tonal;
pub mod lbr;
pub mod lbr_native;
pub mod exss;
pub mod ma;
pub mod ma_core;
pub mod ma_native;
pub mod native;
pub mod native96;
pub mod native_xxch;
pub mod qmf;
pub mod qmf64;
pub mod tables;
mod tables_fixed;
pub mod xbr;
pub mod x96;
pub mod xll;
pub mod xll_bands;
mod xll_table;

pub use encoder::{Config, Encoder, Error};

/// ES options for a stream: `es` marks DTS-ES; the mix and phase shift apply to layouts with a separate
/// centre surround (6.x ES Matrix), the rear attenuation to any layout with surrounds.
pub fn es_options(es: bool, layout: &sw_core::ChannelLayout, phase_shift: bool, rear_minus_3db: bool) -> Option<es::EsOptions> {
    let has_cs = layout.channels.contains(&sw_core::Channel::Cs);
    let xch = es && has_cs && layout.kind == sw_core::layout::LayoutKind::Discrete;
    let mix_cs = es && has_cs && matches!(layout.kind, sw_core::layout::LayoutKind::EsMatrix | sw_core::layout::LayoutKind::Discrete);
    if !es && !rear_minus_3db {
        return None;
    }
    Some(es::EsOptions { flag: es, mix_cs, xch, phase_shift: es && phase_shift, rear_minus_3db })
}
