//! Opening the input files as one multichannel source.

use crate::run::spec::{EncodeSpec, Failure};
use std::collections::HashMap;
use std::path::PathBuf;
use sw_core::{Channel, ChannelLayout};
use sw_pcm::StemSet;

/// The input files, opened as one multichannel source.
pub(crate) struct Sources {
    pub(crate) stems: StemSet,
    pub(crate) inputs: HashMap<Channel, PathBuf>,
    pub(crate) order: sw_pcm::ChannelOrder,
    /// The layout of the files, which is richer than the output layout when they are mixed down.
    pub(crate) src_layout: &'static ChannelLayout,
    pub(crate) downmixed: bool,
}

/// Opens the inputs natively (rate, bit depth, length, timecode) and sets up a mixdown when the files hold a richer
/// layout than the output.
pub(crate) fn open_sources(spec: &EncodeSpec, layout: &'static ChannelLayout) -> Result<Sources, Failure> {
    let inputs: HashMap<Channel, PathBuf> = spec
        .inputs
        .iter()
        .filter_map(|(k, v)| Channel::from_id(k).map(|c| (c, PathBuf::from(v))))
        .collect();

    // Read the real format of the inputs natively (rate, bit depth, length, timecode).
    let order = sw_pcm::ChannelOrder::parse(&spec.channel_order).ok_or("channel order must be auto, mask or file")?;
    let src_layout: &'static ChannelLayout = match &spec.source_layout {
        Some(id) if !id.is_empty() => ChannelLayout::find(id).ok_or("unknown source layout")?,
        _ => layout,
    };
    if src_layout.id != layout.id && spec.engine != "native" {
        return Err("mixing a richer input layout down needs the native engine".into());
    }
    let mut stems = sw_pcm::StemSet::open_ordered(src_layout, &inputs, order).map_err(|e| e.to_string())?;
    let downmixed = src_layout.id != layout.id;
    if downmixed {
        if spec.engine != "native" {
            return Err("mixing a richer input layout down needs the native engine".into());
        }
        stems.set_downmix(sw_core::downmix::Matrix::pan(src_layout.channels, layout.channels));
    }
    if !spec.unlocked && stems.sample_rate != spec.sample_rate {
        return Err(format!(
            "input files are {} Hz but the encode is set to {} Hz",
            stems.sample_rate, spec.sample_rate
        )
        .into());
    }
    Ok(Sources {
        stems,
        inputs,
        order,
        src_layout,
        downmixed,
    })
}
