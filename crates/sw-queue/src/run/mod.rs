//! Executes one encode job and writes its log.

mod encoders;
mod log;
mod sources;
mod spec;

pub use spec::{dts_dialnorm, EncodeSpec, Failure, Outcome};

use encoders::{
    encode_ac3, encode_audio_file, encode_dts_core, encode_dts_ma, encode_express, encode_truehd, is_dts_core_family,
    verify_output, Run,
};
use log::{fmt_secs, log_header, peak_bit_rate_note};
use sources::{open_sources, Sources};

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;
use sw_backend_ffmpeg as ff;
use sw_core::formats::{validate, Destination, StreamType};
use sw_core::ChannelLayout;

pub fn execute(
    spec: &EncodeSpec,
    verify: bool,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(f32, bool),
) -> Result<Outcome, Failure> {
    let t0 = Instant::now();
    let mut log = String::new();
    let mut span_secs = 0.0;
    let result = crate::threads::with_threads(spec.threads, || {
        execute_inner(spec, verify, cancel, on_progress, &mut log, t0, &mut span_secs)
    })
    .map(|outcome| Outcome {
        bytes: std::fs::metadata(&spec.output).map_or(0, |m| m.len()),
        span_secs,
        elapsed_secs: t0.elapsed().as_secs_f64(),
        ..outcome
    });
    // Always leave a log next to the output, whatever happened.
    let status = match &result {
        Ok(o) => format!("RESULT: {}", o.message),
        Err(Failure::Canceled) => "RESULT: canceled".to_string(),
        Err(Failure::Error(m)) => format!("RESULT: FAILED\n{m}"),
    };
    log.push_str(&format!(
        "\n{status}\nTotal time: {:.1} s\n",
        t0.elapsed().as_secs_f64()
    ));
    if spec.write_log {
        let _ = std::fs::write(spec.log_path(), log);
    }
    result
}

fn execute_inner(
    spec: &EncodeSpec,
    verify: bool,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(f32, bool),
    log: &mut String,
    t0: Instant,
    span_secs: &mut f64,
) -> Result<Outcome, Failure> {
    let dest = Destination::from_id(&spec.dest).ok_or("unknown destination")?;
    let stream = StreamType::from_id(&spec.stream).ok_or("unknown stream type")?;
    let layout: &'static ChannelLayout = ChannelLayout::find(&spec.layout).ok_or("unknown layout")?;
    if !spec.unlocked {
        validate(dest, stream, layout, spec.sample_rate).map_err(|e| format!("invalid combination: {e:?}"))?;
    }
    let Sources {
        mut stems,
        inputs,
        order,
        src_layout,
        downmixed,
    } = open_sources(spec, layout)?;
    let effort = sw_core::Effort::from_id(&spec.speed)
        .ok_or_else(|| format!("unknown speed '{}' (use fast, balanced or best)", spec.speed))?;
    let start = spec.start.unwrap_or(0.0);
    let total_src = stems.frames as f64 / stems.sample_rate as f64;
    let span = spec
        .duration
        .unwrap_or(total_src - start)
        .min(total_src - start)
        .max(0.0);
    *span_secs = span;
    if span <= 0.0 {
        return Err("nothing to encode: the start time is at or past the end of the input".into());
    }

    log_header(
        log,
        spec,
        (dest, stream, layout, src_layout),
        &stems,
        &inputs,
        (start, span),
    );
    // audio files only have a native writer
    let ffmpeg = ff::find_ffmpeg();
    // with no FFmpeg, a format Subwoofa can encode itself simply uses its own encoder
    let native = spec.engine == "native" || stream.is_audio_file() || (ffmpeg.is_none() && stream.is_native());
    if native && !stream.is_native() {
        return Err(format!("the native engine cannot encode {} yet", stream.display_name()).into());
    }
    if ffmpeg.is_none() && !native {
        return Err("FFmpeg was not found. Set it up under Settings > Engines (see docs/ENGINES.md).".into());
    }
    let slots = if downmixed {
        Vec::new()
    } else {
        sw_pcm::slot_map(layout, &inputs, order).map_err(|e| e.to_string())?
    };
    let job = ff::Job {
        stream,
        layout,
        inputs,
        output: PathBuf::from(&spec.output),
        bitrate_kbps: spec.bitrate,
        sample_rate: Some(spec.sample_rate),
        dialnorm: spec.dialnorm,
        start_secs: spec.start,
        duration_secs: (spec.start.is_some() || spec.duration.is_some()).then_some(span),
        frames: Some((span * stems.sample_rate as f64).round() as u64),
        slots: Some(slots),
        ffmpeg: ffmpeg.clone().unwrap_or_default(),
    };
    log.push_str(&format!(
        "Engine:       {}\n",
        if native { "Subwoofa native encoder" } else { "FFmpeg" }
    ));

    let ctx = Run {
        spec,
        stream,
        dest,
        layout,
        effort,
        start,
        span,
    };
    let mut progress = |p: f32| on_progress(p, false);
    if stream.is_audio_file() {
        encode_audio_file(&ctx, &mut stems, &mut progress, cancel, log)?;
    } else if native && matches!(stream, StreamType::TrueHd | StreamType::Mlp) {
        encode_truehd(&ctx, &mut stems, &mut progress, cancel, log)?;
    } else if native && stream == StreamType::DtsExpress {
        encode_express(&ctx, &mut stems, &mut progress, cancel)?;
    } else if native && stream == StreamType::DtsHdMa {
        encode_dts_ma(&ctx, &mut stems, &mut progress, cancel, log)?;
    } else if native && is_dts_core_family(stream) {
        encode_dts_core(&ctx, &mut stems, &mut progress, cancel)?;
    } else if native {
        encode_ac3(&ctx, &mut stems, &mut progress, cancel)?;
    } else {
        ff::run_progress(&job, span, cancel, &mut progress)?;
    }
    let enc_secs = t0.elapsed().as_secs_f64();
    let size = std::fs::metadata(&spec.output).map(|m| m.len()).unwrap_or(0);
    let avg_kbps = size as f64 * 8.0 / span / 1000.0;
    // audio files have no frames to chart, and opening one as a stream scans the whole file
    let peak_note = if stream.is_audio_file() {
        String::new()
    } else {
        peak_bit_rate_note(&spec.output)
    };
    log.push_str(&format!(
        "\nEncoded {} in {:.1} s ({:.0}x real time). Output {:.1} MB, average {:.0} kbps.\n",
        fmt_secs(span),
        enc_secs,
        span / enc_secs.max(0.001),
        size as f64 / 1e6,
        avg_kbps
    ));
    log.push_str(&peak_note);

    if !verify || downmixed {
        return Ok(Outcome {
            message: format!("encoded {:.1} MB", size as f64 / 1e6),
            verified: false,
            ..Default::default()
        });
    }
    if ffmpeg.is_none() && !stream.is_audio_file() {
        log.push_str(
            "Verify: skipped (ffmpeg not available to decode the result)
",
        );
        return Ok(Outcome {
            message: format!(
                "encoded {:.1} MB (not verified: no decoder available)",
                size as f64 / 1e6
            ),
            verified: false,
            ..Default::default()
        });
    }
    on_progress(1.0, true);
    verify_output(spec, stream, ffmpeg.as_deref(), &job, stems.bits, span, log)
}
