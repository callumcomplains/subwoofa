//! Native DTS core (Digital Surround) encoder, written from ETSI TS 102 114.

pub mod bits;
pub mod core_exact;
#[rustfmt::skip]
pub mod dmix_table;
pub mod dts96;
pub mod dtshd;
pub mod encoder;
pub mod es;
pub mod exss;
pub mod hra;
pub mod hra96;
pub mod lbr;
pub mod lbr_fb;
mod lbr_lfe;
mod lbr_lpc;
pub mod lbr_native;
#[rustfmt::skip]
mod lbr_tables;
mod lbr_tonal;
pub mod ma;
pub mod ma_core;
pub mod ma_native;
pub mod mixmeta;
pub mod native;
pub mod native96;
pub mod native_xxch;
pub mod qmf;
pub mod qmf64;
#[rustfmt::skip]
pub mod tables;
#[rustfmt::skip]
mod tables_fixed;
pub mod x96;
pub mod xbr;
pub mod xll;
pub mod xll_bands;
#[rustfmt::skip]
mod xll_table;

pub use encoder::{Config, Encoder, Error};

/// ES options for a stream: `es` marks DTS-ES; the mix and phase shift apply to layouts with a separate
/// centre surround (6.x ES Matrix), the rear attenuation to any layout with surrounds.
pub fn es_options(
    es: bool,
    layout: &sw_core::ChannelLayout,
    phase_shift: bool,
    rear_minus_3db: bool,
) -> Option<es::EsOptions> {
    let has_cs = layout.channels.contains(&sw_core::Channel::Cs);
    let xch = es && has_cs && layout.kind == sw_core::layout::LayoutKind::Discrete;
    let mix_cs = es
        && has_cs
        && matches!(
            layout.kind,
            sw_core::layout::LayoutKind::EsMatrix | sw_core::layout::LayoutKind::Discrete
        );
    if !es && !rear_minus_3db {
        return None;
    }
    Some(es::EsOptions {
        flag: es,
        mix_cs,
        xch,
        phase_shift: es && phase_shift,
        rear_minus_3db,
    })
}
