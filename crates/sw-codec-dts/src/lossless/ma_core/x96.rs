//! The X96 part of a 96/24 core: 64-band analysis of the programme, aligned with the core's subbands.

use crate::dca::encoder::Encoder as CoreEncoder;
use crate::dca::qmf64::Analysis64;
use crate::dca::x96::X96Encoder;

/// The X96 part of a 96/24 core: 64-band analysis of the programme, aligned with the core's subbands.
pub(crate) struct X96State {
    pub(crate) ext: Vec<Analysis64>,
    pub(crate) enc: X96Encoder,
    pub(crate) bytes: usize,
    pub(crate) delay: usize,
}

impl X96State {
    /// Sets the core up to reserve room for the X96 extension and keep its residual.
    pub(crate) fn new(core: &mut CoreEncoder, nprim: usize) -> X96State {
        let bytes = (core.frame_bytes() / 8).max(96) & !3;
        core.keep_residual = true;
        core.x96_reserve = bytes;
        let ext: Vec<Analysis64> = (0..nprim).map(|_| Analysis64::new()).collect();
        let delay = ext[0].delay();
        X96State {
            ext,
            enc: X96Encoder::new(16),
            bytes,
            delay,
        }
    }
}
