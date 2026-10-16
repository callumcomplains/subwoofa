//! Executes one encode job and writes its log.

mod closeness;
mod encoders;
mod log;
mod processed;
mod sources;
mod spec;

pub use spec::{dts_dialnorm, EncodeSpec, Failure, Outcome};

use encoders::{
    encode_ac3, encode_audio_file, encode_dolby, encode_dts_core, encode_dts_ma, encode_express, encode_truehd,
    is_dts_core_family, verify_output, Run,
};
use log::{fmt_secs, log_header, peak_bit_rate_note};
pub(crate) use sources::{open_sources, Sources};

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;
use sw_backend_ffmpeg as ff;
use sw_core::formats::{validate, Destination, StreamType};
use sw_core::ChannelLayout;
use sw_pcm::StemSet;

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
    let mut edit = processed::Edit {
        delay_frames: (spec.delay_ms as f64 * stems.sample_rate as f64 / 1000.0).round() as i64,
        gain_db: None,
    };
    if edit.delay_frames != 0 {
        stems.set_delay_frames(edit.delay_frames);
    }
    let effort = sw_core::Effort::from_id(&spec.speed)
        .ok_or_else(|| format!("unknown speed '{}' (use fast, balanced or best)", spec.speed))?;
    let (start, span) = span_of(spec, &stems);
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
    let ffmpeg = ff::find_ffmpeg();
    let native = choose_engine(spec, stream, ffmpeg.is_some())?;
    if let Some(target) = spec.normalize_lufs {
        edit.gain_db = Some(normalise(&mut stems, layout, (start, span), target, cancel, log)?);
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
        if spec.engine == "dolby" {
            "Dolby Encoding Engine (Dolby's own encoder)"
        } else if native {
            "Subwoofa native encoder"
        } else {
            "FFmpeg"
        }
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
    run_encoder(&ctx, native, &job, &mut stems, &mut progress, cancel, log)?;
    let size = log_result(log, spec, stream, span, t0.elapsed().as_secs_f64());

    if !verify || downmixed {
        return Ok(not_verified(format!("encoded {:.1} MB", size as f64 / 1e6)));
    }
    if ffmpeg.is_none() && !stream.is_audio_file() {
        log.push_str("Verify: skipped (ffmpeg not available to decode the result)\n");
        return Ok(not_verified(format!(
            "encoded {:.1} MB (not verified: no decoder available)",
            size as f64 / 1e6
        )));
    }
    on_progress(1.0, true);
    if edit.changes_audio() && stream.is_lossless() && !stream.is_audio_file() {
        // the usual check reads the files as they are, without the gain or delay
        return processed::verify(spec, stream, layout, &job, (start, span), edit, log);
    }
    let outcome = verify_output(spec, stream, ffmpeg.as_deref(), &job, stems.bits, span, log)?;
    if stream.is_lossless() || stream.is_audio_file() {
        return Ok(outcome);
    }
    // decoding cleanly says little about a lossy file: it must also sound like its source
    let message = closeness::verify(spec, layout, &job.inputs, &job.ffmpeg, (start, span), edit, log)?;
    Ok(Outcome { message, ..outcome })
}

fn not_verified(message: String) -> Outcome {
    Outcome {
        message,
        verified: false,
        ..Default::default()
    }
}

/// Where the encode starts and how many seconds it covers: the job's span, kept inside the audio.
fn span_of(spec: &EncodeSpec, stems: &StemSet) -> (f64, f64) {
    let start = spec.start.unwrap_or(0.0);
    let total = stems.frames as f64 / stems.sample_rate as f64;
    let span = spec.duration.unwrap_or(total - start).min(total - start).max(0.0);
    (start, span)
}

/// Whether Subwoofa's own encoder does the job, for a job that may need it. A delay and a gain are applied as the
/// samples are read, which only Subwoofa's own encoders do.
fn choose_engine(spec: &EncodeSpec, stream: StreamType, have_ffmpeg: bool) -> Result<bool, Failure> {
    let native = use_native_engine(spec, stream, have_ffmpeg)?;
    if spec.normalize_lufs.is_none() && spec.delay_ms == 0 {
        return Ok(native);
    }
    if spec.engine == "dolby" {
        return Err("a delay or normalising is not available with the Dolby Encoding Engine".into());
    }
    if !native && !stream.is_native() {
        return Err(format!(
            "a delay or normalising needs a format Subwoofa encodes itself; {} goes through FFmpeg",
            stream.display_name()
        )
        .into());
    }
    Ok(true)
}

/// Measures the span and sets the gain that brings its integrated loudness to `target` LUFS, less if that would push the
/// true peak above -1 dBTP. Returns the gain in dB.
fn normalise(
    stems: &mut StemSet,
    layout: &'static ChannelLayout,
    (start, span): (f64, f64),
    target: f64,
    cancel: &AtomicBool,
    log: &mut String,
) -> Result<f64, Failure> {
    let measured = crate::measure::measure_stems(stems, layout.channels, Some(start), Some(span), cancel, &mut |_| {})
        .map_err(|e| match e {
            crate::measure::MeasureError::Canceled => Failure::Canceled,
            other => Failure::Error(other.to_string()),
        })?;
    let integrated = measured
        .integrated
        .ok_or("the input is silent, so there is nothing to normalise")?;
    let mut gain = target - integrated;
    let mut note = String::new();
    if let Some(peak) = measured.true_peak() {
        if peak + gain > -1.0 {
            gain = -1.0 - peak;
            note = format!(" (limited by the true peak: {peak:.1} dBTP before, -1.0 dBTP after)");
        }
    }
    stems.set_gain_db(gain);
    log.push_str(&format!(
        "Normalise:    {integrated:.1} LUFS measured, {gain:+.1} dB applied, aiming for {target:.1} LUFS{note}
"
    ));
    Ok(gain)
}

/// Whether Subwoofa's own encoder does the job (otherwise FFmpeg does).
fn use_native_engine(spec: &EncodeSpec, stream: StreamType, have_ffmpeg: bool) -> Result<bool, Failure> {
    if spec.engine == "dolby" {
        // Dolby's own encoder, run through the Dolby Encoding Engine: neither of ours
        if !matches!(stream, StreamType::Ac3 | StreamType::Eac3 | StreamType::TrueHd) {
            return Err(format!(
                "the Dolby Encoding Engine encodes Dolby Digital, Digital Plus and TrueHD here, not {}",
                stream.display_name()
            )
            .into());
        }
        return Ok(false);
    }
    // audio files only have a native writer; with no FFmpeg, a format Subwoofa can encode itself simply uses its own
    let native = spec.engine == "native" || stream.is_audio_file() || (!have_ffmpeg && stream.is_native());
    if native && !stream.is_native() {
        return Err(format!("the native engine cannot encode {} yet", stream.display_name()).into());
    }
    if !have_ffmpeg && !native {
        return Err("FFmpeg was not found. Set it up under Settings > Engines (see docs/ENGINES.md).".into());
    }
    Ok(native)
}

/// Runs the encoder for the stream type: Subwoofa's own, or FFmpeg through `job`.
fn run_encoder(
    ctx: &Run,
    native: bool,
    job: &ff::Job,
    stems: &mut StemSet,
    progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
    log: &mut String,
) -> Result<(), Failure> {
    let stream = ctx.stream;
    if ctx.spec.engine == "dolby" {
        encode_dolby(ctx, job, progress, cancel, log)
    } else if stream.is_audio_file() {
        encode_audio_file(ctx, stems, progress, cancel, log)
    } else if native && matches!(stream, StreamType::TrueHd | StreamType::Mlp) {
        encode_truehd(ctx, stems, progress, cancel, log)
    } else if native && stream == StreamType::DtsExpress {
        encode_express(ctx, stems, progress, cancel)
    } else if native && stream == StreamType::DtsHdMa {
        encode_dts_ma(ctx, stems, progress, cancel, log)
    } else if native && is_dts_core_family(stream) {
        encode_dts_core(ctx, stems, progress, cancel)
    } else if native {
        encode_ac3(ctx, stems, progress, cancel)
    } else {
        ff::run_progress(job, ctx.span, cancel, progress)?;
        Ok(())
    }
}

/// Writes the size, speed and peak bit rate of the finished file into the log; returns the size in bytes.
fn log_result(log: &mut String, spec: &EncodeSpec, stream: StreamType, span: f64, enc_secs: f64) -> u64 {
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
    size
}
