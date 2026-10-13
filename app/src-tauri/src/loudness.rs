//! Loudness of the input files, for the Measure button beside Dialnorm.

use crate::blocking;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use sw_core::{Channel, ChannelLayout};
use sw_queue::measure::{self, Measure};

/// Measures the files the page has chosen for `layout` (over the span, if one is set).
#[tauri::command]
pub async fn measure_loudness(
    layout: String,
    inputs: HashMap<String, String>,
    start: Option<f64>,
    duration: Option<f64>,
    channel_order: String,
) -> Result<sw_loudness::Loudness, String> {
    blocking(move || {
        let layout = ChannelLayout::find(&layout).ok_or("unknown layout")?;
        let order = sw_pcm::ChannelOrder::parse(&channel_order).ok_or("channel order must be auto, mask or file")?;
        let files: HashMap<Channel, PathBuf> = inputs
            .iter()
            .filter_map(|(k, v)| Channel::from_id(k).map(|c| (c, PathBuf::from(v))))
            .collect();
        let request = Measure {
            layout,
            inputs: &files,
            order,
            start,
            duration,
        };
        measure::loudness(&request, &AtomicBool::new(false), &mut |_| {}).map_err(|e| e.to_string())
    })
    .await
}
