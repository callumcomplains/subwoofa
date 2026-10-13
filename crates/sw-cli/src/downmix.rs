//! `subwoofa downmix-check`: does a downmix of the files clip?

use crate::args::DownmixArgs;
use crate::encode::{collect_inputs, find_layout};
use std::path::PathBuf;
use sw_backend_ffmpeg as ff;
use sw_core::downmix::Matrix;
use sw_core::formats::StreamType;

pub(crate) fn run_downmix_check(args: DownmixArgs) -> Result<(), String> {
    let layout = find_layout(&args.layout)?;
    let mut m = match args.to.as_str() {
        "stereo" => Matrix::lo_ro(layout),
        "5.1" => Matrix::to_51(layout),
        _ => return Err("--to must be 'stereo' or '5.1'".into()),
    };
    if args.safe {
        let t = m.safe_trim_db();
        println!(
            "safe trim: {t:.2} dB (worst-case input sum {:.3})",
            m.worst_case_gain().into_iter().fold(0.0, f32::max)
        );
        m.trim(t);
    }
    if args.trim_db != 0.0 {
        m.trim(args.trim_db);
    }
    let inputs = collect_inputs(layout, args.auto_import, &args.inputs)?;
    let ffmpeg = args
        .ffmpeg
        .or_else(ff::find_ffmpeg)
        .ok_or("ffmpeg not found on PATH (use --ffmpeg)")?;
    let job = ff::Job {
        stream: StreamType::Ac3,
        layout,
        inputs,
        output: PathBuf::new(),
        bitrate_kbps: None,
        sample_rate: None,
        dialnorm: None,
        start_secs: args.start,
        duration_secs: args.duration,
        frames: None,
        slots: None,
        ffmpeg,
    };
    let report = ff::downmix::saturation_check(&job, &m).map_err(|e| e.to_string())?;
    println!(
        "analysed {} frames ({:.1} s at 48 kHz)",
        report.frames,
        report.frames as f64 / 48_000.0
    );
    for (c, p) in report.channels.iter().zip(&report.peak_db) {
        println!("  {c:<4} peak {p:>7.2} dBFS");
    }
    println!("{} saturation event(s)", report.event_count);
    for e in report.events.iter().take(10) {
        println!(
            "  {:<4} at {:.3} s",
            report.channels[e.channel],
            e.frame as f64 / 48_000.0
        );
    }
    Ok(())
}
