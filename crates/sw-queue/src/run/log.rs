//! The job log: its header, times and the peak bit rate of the result.

use crate::run::spec::EncodeSpec;
use std::collections::HashMap;
use std::path::PathBuf;
use sw_core::formats::{Destination, StreamType};
use sw_core::{Channel, ChannelLayout};
use sw_pcm::StemSet;

pub(crate) fn fmt_secs(s: f64) -> String {
    format!(
        "{:02}:{:02}:{:06.3}",
        (s / 3600.0) as u64,
        ((s % 3600.0) / 60.0) as u64,
        s % 60.0
    )
}

/// The first part of the log: what is encoded, from what, and where.
pub(crate) fn log_header(
    log: &mut String,
    spec: &EncodeSpec,
    (dest, stream, layout, src_layout): (Destination, StreamType, &ChannelLayout, &ChannelLayout),
    stems: &StemSet,
    inputs: &HashMap<Channel, PathBuf>,
    (start, span): (f64, f64),
) {
    let total_src = stems.frames as f64 / stems.sample_rate as f64;
    log.push_str("Subwoofa encode log\n");
    log.push_str(&format!("Output:       {}\n", spec.output));
    log.push_str(&format!(
        "Format:       {} / {} / {}\n",
        dest.display_name(),
        stream.display_name(),
        layout.name
    ));
    log.push_str(&format!(
        "Sample rate:  {} Hz   Source bit depth: {}-bit   Bit rate: {}\n",
        spec.sample_rate,
        stems.bits,
        spec.bitrate
            .map(|b| format!("{b} kbps"))
            .unwrap_or_else(|| "variable".into())
    ));
    log.push_str(&format!(
        "Dialnorm:     {}\n",
        spec.dialnorm
            .map(|d| format!("-{d} dBFS"))
            .unwrap_or_else(|| "n/a".into())
    ));
    log.push_str(&format!(
        "Span:         start {}  length {}  (source {})\n",
        fmt_secs(start),
        fmt_secs(span),
        fmt_secs(total_src)
    ));
    if let Some(t) = stems.time_reference {
        log.push_str(&format!(
            "Broadcast time reference: {} samples ({})\n",
            t,
            fmt_secs(t as f64 / stems.sample_rate as f64)
        ));
    }
    log.push_str(&format!("Unlocked:     {}\n", spec.unlocked));
    for ch in src_layout.channels {
        if let Some(p) = inputs.get(ch) {
            log.push_str(&format!("  {:<4} {}\n", ch.short(), p.display()));
        }
    }
    for line in sw_codec_dts::container::mixmeta::describe(
        &spec.bitstream,
        span,
        stream == StreamType::DtsExpress && dest == Destination::BluRaySecondary,
    ) {
        log.push_str(&line);
        log.push('\n');
    }
}

/// Peak bit rate of a finished stream (the figure disc authoring checks), or nothing if the file is not a
/// stream StreamTools can index.
pub(crate) fn peak_bit_rate_note(path: &str) -> String {
    let Ok(st) = sw_tools::Stream::open(std::path::Path::new(path)) else {
        return String::new();
    };
    let win = st.pbr(1.0);
    let (Some(top), Some(busiest)) = (
        win.iter().max_by(|a, b| a.peak_kbps.total_cmp(&b.peak_kbps)),
        win.iter().max_by(|a, b| a.mean_kbps.total_cmp(&b.mean_kbps)),
    ) else {
        return String::new();
    };
    format!(
        "Peak bit rate: {:.0} kbps in a single frame (at {}), busiest second {:.0} kbps (at {}), {} frames.\n",
        top.peak_kbps,
        fmt_secs(top.secs),
        busiest.mean_kbps,
        fmt_secs(busiest.secs),
        st.frames.len()
    )
}
