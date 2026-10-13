//! `subwoofa loudness`: how loud a programme is, and the dialnorm that goes with it.

use crate::args::LoudnessArgs;
use crate::encode::{collect_inputs, find_layout};
use crate::progress::Progress;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use sw_core::{Channel, ChannelLayout};
use sw_loudness::Loudness;
use sw_pcm::ChannelOrder;
use sw_queue::measure::{self, Measure};

pub(crate) fn run_loudness(args: LoudnessArgs) -> Result<(), String> {
    let layout = find_layout(&args.layout)?;
    let order = ChannelOrder::parse(&args.channel_order).ok_or("--channel-order must be auto, mask or file")?;
    let inputs = collect_inputs(layout, args.auto_import, &args.inputs)?;
    let loudness = measure_inputs(layout, &inputs, order, args.start, args.duration)?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&loudness).map_err(|e| e.to_string())?
        );
    } else {
        print!("{}", describe(&loudness));
    }
    Ok(())
}

/// Reads the input files and measures them, with a progress line on a terminal.
pub(crate) fn measure_inputs(
    layout: &'static ChannelLayout,
    inputs: &HashMap<Channel, PathBuf>,
    order: ChannelOrder,
    start: Option<f64>,
    duration: Option<f64>,
) -> Result<Loudness, String> {
    let mut progress = Progress::new();
    let request = Measure {
        layout,
        inputs,
        order,
        start,
        duration,
    };
    measure::loudness(&request, &AtomicBool::new(false), &mut |p| progress.tick(p)).map_err(|e| e.to_string())
}

fn level(value: Option<f64>, unit: &str) -> String {
    match value {
        Some(v) => format!("{v:>7.1} {unit}"),
        None => format!("{:>7} {unit}", "-inf"),
    }
}

/// The measurement as text, one line each.
pub(crate) fn describe(l: &Loudness) -> String {
    let mut text = format!(
        "Integrated loudness  {}\nLoudness range       {}\nShort-term maximum   {}\nMomentary maximum    {}\n",
        level(l.integrated, "LUFS"),
        level(l.range, "LU"),
        level(l.short_term_max, "LUFS"),
        level(l.momentary_max, "LUFS"),
    );
    text.push_str(&format!("True peak            {}\n", level(l.true_peak(), "dBTP")));
    for c in &l.channels {
        text.push_str(&format!(
            "  {:<5} true {}  sample {}\n",
            c.name,
            level(c.true_peak, "dBTP"),
            level(c.sample_peak, "dBFS")
        ));
    }
    match l.dialnorm() {
        Some(d) => text.push_str(&format!("Matching dialnorm    {:>7} dBFS\n", -(d as i32))),
        None => text.push_str("Matching dialnorm    (the programme is silent)\n"),
    }
    text
}
