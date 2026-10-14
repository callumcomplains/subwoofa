//! One function per encoder family, called by the job runner once the sources are open, and the check of the finished file.

use crate::run::spec::{dts_dialnorm, EncodeSpec, Failure, Outcome};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use sw_backend_ffmpeg as ff;
use sw_core::formats::{Destination, StreamType};
use sw_core::ChannelLayout;
use sw_pcm::StemSet;

/// What every encoder needs to know about the job, whichever codec runs it.
pub(crate) struct Run<'a> {
    pub(crate) spec: &'a EncodeSpec,
    pub(crate) stream: StreamType,
    pub(crate) dest: Destination,
    pub(crate) layout: &'static ChannelLayout,
    pub(crate) effort: sw_core::Effort,
    /// Where in the source the encode starts, and how many seconds it covers.
    pub(crate) start: f64,
    pub(crate) span: f64,
}

impl Run<'_> {
    fn output(&self) -> &Path {
        Path::new(&self.spec.output)
    }
}

/// Progress callback of the native encoders: the fraction done.
pub(crate) type Progress<'a> = &'a mut dyn FnMut(f32);

pub(crate) fn is_dts_core_family(stream: StreamType) -> bool {
    matches!(
        stream,
        StreamType::DtsDigitalSurround | StreamType::DtsEs | StreamType::DtsHdHra | StreamType::Dts9624
    )
}

/// WAV, FLAC, DSD and the other audio files.
pub(crate) fn encode_audio_file(
    run: &Run,
    stems: &mut StemSet,
    progress: Progress,
    cancel: &AtomicBool,
    log: &mut String,
) -> Result<(), Failure> {
    let spec = run.spec;
    // for DSD the setting is the DSD rate ("dsd64", "dsd128"); for the rest the sample format
    let is_dsd = matches!(run.stream, StreamType::DsdDsf | StreamType::DsdDff);
    let dsd_multiplier = if is_dsd {
        match spec.pcm_format.as_str() {
            "" | "dsd64" => 64,
            "dsd128" => 128,
            other => return Err(format!("unknown DSD rate '{other}' (use dsd64 or dsd128)").into()),
        }
    } else {
        0
    };
    let is_dcp = run.stream == StreamType::DcpAudio;
    let format = if is_dsd || is_dcp || spec.pcm_format.is_empty() {
        Some(sw_codec_pcm::SampleFormat::Int24)
    } else {
        sw_codec_pcm::SampleFormat::from_id(&spec.pcm_format)
    }
    .ok_or_else(|| {
        format!(
            "unknown sample format '{}' (use s16, s24, s32, f32 or f64)",
            spec.pcm_format
        )
    })?;
    if is_dsd {
        log.push_str(&format!(
            "DSD rate: DSD{} ({} Hz), one-bit, fifth-order sigma-delta\n",
            dsd_multiplier,
            44_100 * dsd_multiplier
        ));
    } else {
        log.push_str(&format!(
            "Sample format: {} ({}-bit {})\n",
            format.id(),
            format.bits(),
            if format.is_float() { "floating point" } else { "integer" }
        ));
    }
    let dcp = if is_dcp {
        sw_codec_pcm::DcpOptions::parse(&spec.pcm_format)?
    } else {
        Default::default()
    };
    let req = sw_codec_pcm::Request {
        layout: run.layout,
        container: audio_container(run.stream),
        format,
        start_secs: run.start,
        duration_secs: Some(run.span),
        output: run.output(),
        dsd_multiplier,
        dcp,
        effort: run.effort,
    };
    sw_codec_pcm::encode_to_file(stems, &req, progress, cancel)
        .map(|_| ())
        .map_err(|e| match e {
            sw_codec_pcm::PcmError::Canceled => Failure::Canceled,
            other => Failure::Error(other.to_string()),
        })
}

/// The file container an audio-file stream type is written in.
pub(crate) fn audio_container(stream: StreamType) -> sw_codec_pcm::Container {
    use sw_codec_pcm::Container;
    match stream {
        StreamType::PcmWav => Container::Wav,
        StreamType::PcmAiff => Container::Aiff,
        StreamType::PcmCaf => Container::Caf,
        StreamType::OggFlac => Container::OggFlac,
        StreamType::Alac => Container::Alac,
        StreamType::Tta => Container::Tta,
        StreamType::WavPack => Container::WavPack,
        StreamType::MonkeysAudio => Container::MonkeysAudio,
        StreamType::MpegAls => Container::MpegAls,
        StreamType::DcpAudio => Container::DcpAudio,
        StreamType::DsdDsf => Container::Dsf,
        StreamType::DsdDff => Container::Dff,
        _ => Container::Flac,
    }
}

pub(crate) fn encode_truehd(
    run: &Run,
    stems: &mut StemSet,
    progress: Progress,
    cancel: &AtomicBool,
    log: &mut String,
) -> Result<(), Failure> {
    use sw_codec_truehd::native::{encode_to_file, NativeError, Request};
    let req = Request {
        layout: run.layout,
        start_secs: run.start,
        duration_secs: Some(run.span),
        output: run.output(),
        downmix: run.spec.downmix,
        mlp: run.stream == StreamType::Mlp,
        effort: run.effort,
    };
    let stats = encode_to_file(stems, &req, progress, cancel).map_err(|e| match e {
        NativeError::Canceled => Failure::Canceled,
        other => Failure::Error(other.to_string()),
    })?;
    let has_downmix = run.spec.downmix && run.layout.channel_count() > 2;
    log.push_str(&format!(
        "TrueHD stereo downmix substream: {}\n",
        if has_downmix { "yes" } else { "no" }
    ));
    if stats.downmix_overflows > 0 {
        log.push_str(&format!(
            "Warning: {} stereo-presentation samples exceed 24 bits; stereo-only decoders will clip them\n",
            stats.downmix_overflows
        ));
    }
    Ok(())
}

pub(crate) fn encode_express(
    run: &Run,
    stems: &mut StemSet,
    progress: Progress,
    cancel: &AtomicBool,
) -> Result<(), Failure> {
    use sw_codec_dts::express::native::{encode_to_file, LbrError, LbrRequest};
    let req = LbrRequest {
        layout: run.layout,
        bitrate_kbps: run.spec.bitrate.unwrap_or(128),
        start_secs: run.start,
        duration_secs: Some(run.span),
        output: run.output(),
        secondary: run.dest == Destination::BluRaySecondary,
        bitstream: run.spec.bitstream.clone(),
    };
    encode_to_file(stems, &req, progress, cancel)
        .map(|_| ())
        .map_err(|e| match e {
            LbrError::Canceled => Failure::Canceled,
            other => Failure::Error(other.to_string()),
        })
}

pub(crate) fn encode_dts_ma(
    run: &Run,
    stems: &mut StemSet,
    progress: Progress,
    cancel: &AtomicBool,
    log: &mut String,
) -> Result<(), Failure> {
    use sw_codec_dts::lossless::native::{encode_to_file, MaError, MaRequest};
    let spec = run.spec;
    if spec.sample_rate > 96_000 && run.layout.channels.len() > 6 && spec.bitrate != Some(0) {
        log.push_str(
            "Note: above 96 kHz with more than 6 channels the stream is lossless only (no DTS core is written).\n",
        );
    }
    let req = MaRequest {
        layout: run.layout,
        core_bitrate_kbps: match spec.bitrate {
            Some(0) => None,
            Some(b) => Some(b),
            None => Some(1509),
        },
        dialnorm: dts_dialnorm(spec.dialnorm),
        start_secs: run.start,
        duration_secs: Some(run.span),
        output: run.output(),
        container: spec.output.to_ascii_lowercase().ends_with(".dtshd"),
        x96: spec.core_x96,
        bitstream: spec.bitstream.clone(),
        effort: run.effort,
    };
    encode_to_file(stems, &req, progress, cancel)
        .map(|_| ())
        .map_err(|e| match e {
            MaError::Canceled => Failure::Canceled,
            other => Failure::Error(other.to_string()),
        })
}

/// DTS core, ES, 96/24 and High Resolution Audio.
pub(crate) fn encode_dts_core(
    run: &Run,
    stems: &mut StemSet,
    progress: Progress,
    cancel: &AtomicBool,
) -> Result<(), Failure> {
    use sw_codec_dts::dca::native::{encode_to_file, NativeError, Request};
    let (spec, stream) = (run.spec, run.stream);
    let hra = stream == StreamType::DtsHdHra;
    let is_es = stream == StreamType::DtsEs
        || (matches!(stream, StreamType::DtsHdHra | StreamType::Dts9624) && run.layout.id.starts_with("6."));
    let req = Request {
        layout: run.layout,
        bitrate_kbps: if hra { 1509 } else { spec.bitrate.unwrap_or(1509) },
        x96: stream == StreamType::Dts9624,
        bitstream: spec.bitstream.clone(),
        dvd: run.dest == Destination::DvdVideo,
        hra_kbps: hra.then(|| spec.bitrate.unwrap_or(3018)),
        dialnorm: dts_dialnorm(spec.dialnorm),
        pcm_bits: (stems.bits as u32).clamp(16, 24),
        start_secs: run.start,
        duration_secs: Some(run.span),
        output: run.output(),
        es: sw_codec_dts::es_options(is_es, run.layout, spec.es_phase_shift, spec.rear_minus_3db),
    };
    encode_to_file(stems, &req, progress, cancel)
        .map(|_| ())
        .map_err(|e| match e {
            NativeError::Canceled => Failure::Canceled,
            other => Failure::Error(other.to_string()),
        })
}

/// Dolby Digital and Dolby Digital Plus.
pub(crate) fn encode_ac3(
    run: &Run,
    stems: &mut StemSet,
    progress: Progress,
    cancel: &AtomicBool,
) -> Result<(), Failure> {
    use sw_codec_ac3::native::{encode_to_file, NativeError, Request};
    let req = Request {
        layout: run.layout,
        bitrate_kbps: run.spec.bitrate.unwrap_or(448),
        dialnorm: run.spec.dialnorm.unwrap_or(31),
        start_secs: run.start,
        duration_secs: Some(run.span),
        output: run.output(),
        eac3: run.stream == StreamType::Eac3,
    };
    encode_to_file(stems, &req, progress, cancel)
        .map(|_| ())
        .map_err(|e| match e {
            NativeError::Canceled => Failure::Canceled,
            other => Failure::Error(other.to_string()),
        })
}

/// Checks the finished file: a structure check for audio files, otherwise a decode with FFmpeg compared with the
/// source. Returns what to tell the user.
pub(crate) fn verify_output(
    spec: &EncodeSpec,
    stream: StreamType,
    ffmpeg: Option<&Path>,
    job: &ff::Job,
    bits: u16,
    span: f64,
    log: &mut String,
) -> Result<Outcome, Failure> {
    if stream.is_audio_file() {
        // the FFmpeg check compares a decode with the source sample for sample, which cannot hold for files that
        // reorder channels, round to a smaller sample format or are one-bit: use the native file check instead
        let options = sw_verify::Options {
            deep: ffmpeg.is_some(),
            ffmpeg: ffmpeg.map(Path::to_path_buf),
            reference_verifier: None,
            ..Default::default()
        };
        let report = sw_verify::verify(Path::new(&spec.output), &options).map_err(|e| Failure::Error(e.to_string()))?;
        if !report.passed {
            let failed: Vec<String> = report
                .checks
                .iter()
                .filter(|c| c.status == sw_verify::Status::Fail)
                .map(|c| format!("{}: {}", c.name, c.detail))
                .collect();
            log.push_str(&format!("Verify: FAILED: {}\n", failed.join("; ")));
            return Err(Failure::Error(format!(
                "the encoded file failed verification: {}",
                failed.join("; ")
            )));
        }
        let message = if ffmpeg.is_some() {
            "file structure and a full decode check out"
        } else {
            "file structure checks out (not decoded: no ffmpeg)"
        };
        log.push_str(&format!("Verify: {message}\n"));
        return Ok(Outcome {
            message: message.to_string(),
            verified: true,
            ..Default::default()
        });
    }
    let message = ff::verify(job, bits, span)?;
    log.push_str(&format!("Verify: {message}\n"));
    Ok(Outcome {
        message,
        verified: true,
        ..Default::default()
    })
}

/// Dolby Digital or Dolby Digital Plus made by Dolby's own encoder, through the Dolby Encoding Engine. The engine
/// measures the loudness itself and writes the dialnorm from it, so the job's dialnorm is not used.
pub(crate) fn encode_dolby(
    run: &Run,
    job: &ff::Job,
    progress: Progress,
    cancel: &AtomicBool,
    log: &mut String,
) -> Result<(), Failure> {
    use sw_backend_dee::{clock, run_task, EncodeOptions, Task, TaskJob};
    if run
        .spec
        .source_layout
        .as_deref()
        .is_some_and(|s| !s.is_empty() && s != run.layout.id)
    {
        return Err("the Dolby encoder cannot mix a richer layout down; use Subwoofa's own encoder for that".into());
    }
    let dee = crate::dolby::find_engine()?;
    let eac3 = run.stream == StreamType::Eac3;
    let mode = match (eac3, run.layout.channels.len()) {
        (false, 8) => return Err("Dolby Digital has no 7.1; use Dolby Digital Plus".into()),
        (false, _) => "dd",
        (true, 8) => "ddp71",
        (true, _) => "ddp",
    };
    let options = EncodeOptions {
        mode: mode.into(),
        data_rate_kbps: run.spec.bitrate.unwrap_or(448),
        lfe: run.layout.channels.contains(&sw_core::Channel::Lfe),
        ..Default::default()
    };
    let task = TaskJob {
        source: crate::dolby::engine_source(run.layout, &job.inputs)?,
        output: run.output().to_path_buf(),
        task: Task::Encode(Box::new(options)),
        start: Some(clock(run.start)),
        end: Some(clock(run.start + run.span)),
    };
    let work = crate::dolby::job_folder("encode")?;
    log.push_str("Engine:       Dolby Encoding Engine (Dolby's own encoder)\n");
    if let Some(d) = run.spec.dialnorm {
        log.push_str(&format!(
            "Dialnorm:     the engine measures it; the {d} given is not used\n"
        ));
    }
    let done = run_task(&dee, &task, &work, progress, &mut |_| {}, cancel).map_err(|e| match e {
        sw_backend_dee::Error::Canceled => Failure::Canceled,
        other => Failure::Error(other.to_string()),
    })?;
    for m in done.messages {
        log.push_str(&format!("Engine said:  {m}\n"));
    }
    let _ = std::fs::remove_dir_all(&work);
    Ok(())
}
