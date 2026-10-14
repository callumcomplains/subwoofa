//! `subwoofa encode`: turns the command line into the same job the app builds and hands it to the job runner.

use crate::args::EncodeArgs;
use crate::progress::Progress;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use sw_backend_ffmpeg as ff;
use sw_core::formats::{self, bitrate_range_kbps, Destination, StreamType};
use sw_core::{autoimport, Channel, ChannelLayout};
use sw_queue::run::{execute, EncodeSpec, Failure};

pub(crate) fn run_encode(args: EncodeArgs) -> Result<(), String> {
    if let Some(path) = args.ffmpeg.clone() {
        ff::set_ffmpeg_override(Some(path));
    }
    let spec = build_spec(&args)?;
    if let Some(path) = &args.save_job {
        crate::jobs::save_job(path, &spec)?;
        println!(
            "job saved to {} (run it with: subwoofa run {})",
            path.display(),
            path.display()
        );
        return Ok(());
    }
    let mut progress = Progress::new();
    let cancel = AtomicBool::new(false);
    let out = execute(&spec, false, &cancel, &mut |p, _| progress.tick(p)).map_err(|e| match e {
        Failure::Canceled => "canceled".to_string(),
        Failure::Error(message) => message,
    })?;
    println!(
        "wrote {} ({:.2} MB) in {:.1} s = {:.0}x real time",
        args.output.display(),
        out.bytes as f64 / 1e6,
        out.elapsed_secs,
        out.span_secs / out.elapsed_secs.max(1e-3)
    );
    if !out.message.is_empty() {
        println!("{}", out.message);
    }
    Ok(())
}

/// The job the options describe, after checking that the combination exists.
fn build_spec(args: &EncodeArgs) -> Result<EncodeSpec, String> {
    if sw_core::Effort::from_id(&args.speed).is_none() {
        return Err(format!("unknown speed '{}' (use fast, balanced or best)", args.speed));
    }
    let stream = StreamType::from_id(&args.stream).ok_or(format!("unknown stream type '{}'", args.stream))?;
    let dest = Destination::from_id(&args.dest).ok_or(format!("unknown destination '{}'", args.dest))?;
    // the audio-file streams live under their own destination: do not make the user say so
    let dest = if stream.is_audio_file() && dest == Destination::Generic {
        Destination::Files
    } else {
        dest
    };
    let layout = find_layout(&args.layout)?;
    formats::validate(dest, stream, layout, args.sample_rate).map_err(|e| format!("invalid combination: {e:?}"))?;
    let source = match args.source_layout.as_deref().filter(|s| !s.is_empty()) {
        Some(id) => find_layout(id)?,
        None => layout,
    };
    let inputs = collect_inputs(source, args.auto_import.clone(), &args.inputs)?;
    let dialnorm = if args.auto_dialnorm {
        Some(measured_dialnorm(source, &inputs, args)?)
    } else {
        args.dialnorm
    };
    let bitrate = args
        .bitrate
        .or_else(|| bitrate_range_kbps(dest, stream, layout, args.sample_rate).map(|r| r.1.min(default_cap(stream))));
    Ok(EncodeSpec {
        dest: dest.id().to_string(),
        stream: stream.id().to_string(),
        layout: layout.id.to_string(),
        sample_rate: args.sample_rate,
        bitrate,
        dialnorm,
        start: args.start,
        duration: args.duration,
        inputs: inputs
            .iter()
            .map(|(c, p)| (c.short().to_string(), p.to_string_lossy().into_owned()))
            .collect(),
        output: args.output.to_string_lossy().into_owned(),
        // the combination was checked above, and the input rate may differ from --sample-rate
        unlocked: true,
        engine: args.engine.clone(),
        downmix: !args.no_downmix,
        channel_order: args.channel_order.clone(),
        es_phase_shift: args.es_phase_shift,
        rear_minus_3db: args.rear_3db,
        source_layout: (source.id != layout.id).then(|| source.id.to_string()),
        core_x96: args.core_96_24,
        pcm_format: args.pcm_format.clone().unwrap_or_default(),
        bitstream: args.bitstream.options()?,
        speed: args.speed.clone(),
        threads: args.threads,
        normalize_lufs: args.normalize,
        write_log: false,
    })
}

pub(crate) fn find_layout(id: &str) -> Result<&'static ChannelLayout, String> {
    ChannelLayout::find(id).ok_or(format!("unknown layout '{id}'"))
}

/// The channel files named by `--auto-import` and `--input CH=FILE`.
pub(crate) fn collect_inputs(
    layout: &'static ChannelLayout,
    auto_import: Option<PathBuf>,
    inputs: &[String],
) -> Result<HashMap<Channel, PathBuf>, String> {
    let mut map: HashMap<Channel, PathBuf> = HashMap::new();
    if let Some(first) = auto_import {
        // a single file holding every channel (e.g. an 8-channel WAV) feeds the whole layout
        match sw_pcm::multichannel_inputs(layout, &first) {
            Some(all) => map.extend(all),
            None => map.extend(autoimport::auto_import(&first, layout)),
        }
    }
    for spec in inputs {
        let (ch, file) = spec.split_once('=').ok_or("--input must be CH=FILE")?;
        let ch = Channel::from_id(ch).ok_or(format!("unknown channel '{ch}'"))?;
        map.insert(ch, file.into());
    }
    Ok(map)
}

/// Sensible default ceilings where the format's absolute maximum is rarely wanted.
fn default_cap(stream: StreamType) -> u32 {
    match stream {
        StreamType::Ac3 => 640,
        StreamType::Eac3 => 1024,
        StreamType::DtsDigitalSurround => 1509,
        _ => u32::MAX,
    }
}

/// The dialnorm that matches the integrated loudness of the input, measured now.
fn measured_dialnorm(
    layout: &'static ChannelLayout,
    inputs: &HashMap<Channel, PathBuf>,
    args: &EncodeArgs,
) -> Result<u8, String> {
    let order = sw_pcm::ChannelOrder::parse(&args.channel_order).ok_or("--channel-order must be auto, mask or file")?;
    let loudness = crate::loudness::measure_inputs(layout, inputs, order, args.start, args.duration)?;
    let dialnorm = loudness
        .dialnorm()
        .ok_or("cannot set the dialnorm from the input: it is silent")?;
    eprintln!(
        "measured {:.1} LUFS: dialnorm -{dialnorm} dBFS",
        loudness.integrated.unwrap_or_default()
    );
    Ok(dialnorm)
}
