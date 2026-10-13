//! The core encoder and the extension substream description a Master Audio stream with a core starts from.

use crate::container::exss::{exss_frame, ExssParams};
use crate::dca::encoder::Config as CoreConfig;
use crate::dca::encoder::Encoder as CoreEncoder;
use crate::dca::native::LayoutMap;
use crate::lossless::ma::MaMap;
use crate::lossless::ma_core::{Job, MaCoreConfig};
use crate::lossless::xll::{XllConfig, XllEncoder};

pub(crate) fn core_encoder(
    cfg: &MaCoreConfig,
    cmap: &LayoutMap,
    nprim: usize,
    xch: bool,
    ratio: usize,
) -> Result<CoreEncoder, crate::Error> {
    let mut core = CoreEncoder::new(CoreConfig {
        sample_rate: cfg.sample_rate / ratio as u32,
        amode: cmap.amode,
        channels: nprim,
        lfe: cmap.lfe.is_some(),
        bitrate: cfg.core_bitrate,
        dialnorm: cfg.dialnorm,
        pcm_bits: cfg.bit_res,
        // the commercial encoder flags the core of a 6.x or Ls/Rs + Lsr/Rsr stream as ES (its surrounds carry more)
        es: cfg.layout.id.starts_with("6.") || cfg.layout.id == "7.1-ls-rs-lsr-rsr",
        xch,
        perfect: false,
        blocks: 16,
    })?;
    core.set_aux_downmix(crate::lossless::ma::core_aux(
        &cfg.downmix,
        cfg.layout,
        cmap.amode,
        cmap.lfe.is_some(),
        xch,
    ));
    Ok(core)
}

/// The extension substream's description of the stream.
pub(crate) fn exss_params(cfg: &MaCoreConfig, xmap: &MaMap, x96: bool, xch: bool) -> ExssParams {
    ExssParams {
        sample_rate: cfg.sample_rate,
        channels: xmap.order.len(),
        bit_res: cfg.bit_res,
        spk_mask: xmap.exss_mask,
        with_core: true,
        dialnorm: (-cfg.dialnorm) as u32,
        frame_duration_code: 0,
        x96_in_core: x96,
        xch_in_core: xch,
        secondary: false,
        extras: crate::container::exss::ExssExtras {
            min_header: 32,
            emb_5_1: xmap.hier.as_ref().is_some_and(|h| h.extras),
            emb_stereo: xmap.hier.as_ref().is_some_and(|h| h.stereo),
            wide_remap: cfg.wide_remap,
            repr_type: (cfg.layout.kind == sw_core::layout::LayoutKind::LtRt).then_some(2),
            ..Default::default()
        },
    }
}

/// Lossless coding of a batch of independent frames on the rayon pool; returns the finished stream frames.
pub(crate) fn xll_stage(jobs: Vec<Job>, cfg: XllConfig, exss: ExssParams) -> Vec<Vec<u8>> {
    use rayon::prelude::*;
    let threads = rayon::current_num_threads().max(1);
    let per = jobs.len().div_ceil(threads).max(1);
    jobs.par_chunks(per)
        .flat_map_iter(|chunk| {
            let mut xll = XllEncoder::new(cfg.clone());
            chunk
                .iter()
                .map(|(core_bytes, resid, core0)| {
                    let data = xll.encode_frame_core(resid, core0.as_deref());
                    let mut frame = core_bytes.clone();
                    frame.extend_from_slice(&exss_frame(&exss, &data));
                    frame
                })
                .collect::<Vec<_>>()
        })
        .collect()
}
