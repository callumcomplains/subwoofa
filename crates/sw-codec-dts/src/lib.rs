//! The DTS family of encoders, written from ETSI TS 102 114 and from watching what real streams contain.
//!
//! * [`dca`]: the core codec (Coherent Acoustics): Digital Surround, ES, 96/24. Everything else is built on its
//!   frame encoder.
//! * [`hra`]: High Resolution Audio, the XBR extension that refines the core.
//! * [`lossless`]: Master Audio, the lossless XLL extension with or without a core underneath.
//! * [`express`]: Express (LBR), a separate low bit rate codec.
//! * [`container`]: extension-substream frames, the `.dtshd` file and the metadata in it.
//!
//! Each family has a `native` module, the file-level driver that reads stems and writes a stream.

pub mod bits;
pub mod container;
pub mod dca;
pub mod express;
pub mod hra;
pub mod lossless;

pub use dca::encoder::{Config, Encoder, Error};

/// ES options for a stream: `es` marks DTS-ES; the mix and phase shift apply to layouts with a separate
/// centre surround (6.x ES Matrix), the rear attenuation to any layout with surrounds.
pub fn es_options(
    es: bool,
    layout: &sw_core::ChannelLayout,
    phase_shift: bool,
    rear_minus_3db: bool,
) -> Option<dca::es::EsOptions> {
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
    Some(dca::es::EsOptions {
        flag: es,
        mix_cs,
        xch,
        phase_shift: es && phase_shift,
        rear_minus_3db,
    })
}
