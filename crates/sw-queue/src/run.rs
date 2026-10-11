//! Executes one encode job and writes its log.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;
use sw_backend_ffmpeg as ff;
use sw_core::formats::{validate, Destination, StreamType};
use sw_core::{Channel, ChannelLayout};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncodeSpec {
    pub dest: String,
    pub stream: String,
    pub layout: String,
    pub sample_rate: u32,
    pub bitrate: Option<u32>,
    pub dialnorm: Option<u8>,
    pub start: Option<f64>,
    pub duration: Option<f64>,
    pub inputs: HashMap<String, String>,
    pub output: String,
    /// Skip spec validation (custom bit rates, sample rates, unusual combinations).
    #[serde(default)]
    pub unlocked: bool,
    /// "ffmpeg" (default) or "native" (Subwoofa's own encoder, AC-3 only for now).
    #[serde(default)]
    pub engine: String,
    /// Native TrueHD: include the stereo downmix substream (default on).
    #[serde(default = "yes")]
    pub downmix: bool,
    /// Multichannel-file channel order: "auto" (default), "mask" or "file".
    #[serde(default)]
    pub channel_order: String,
    /// DTS: rotate the surrounds by +/-45 degrees before the centre surround is added (6.x ES).
    #[serde(default)]
    pub es_phase_shift: bool,
    /// DTS: attenuate the rear channels by 3 dB first.
    #[serde(default)]
    pub rear_minus_3db: bool,
    /// Layout of the input files when it is richer than the output layout (e.g. "22.2"): the inputs are
    /// mixed down to `layout` by constant-power panning. Native engine only.
    #[serde(default)]
    pub source_layout: Option<String>,
    /// DTS-HD MA at 96 kHz: the core carries an X96 extension (a 96/24 core).
    #[serde(default)]
    pub core_x96: bool,
    /// Audio files (WAV, AIFF, CAF, FLAC): the sample format, one of "s16", "s24" (default), "s32", "f32", "f64".
    #[serde(default)]
    pub pcm_format: String,
}

fn yes() -> bool {
    true
}

impl EncodeSpec {
    pub fn display_name(&self) -> String {
        let file = self.output.rsplit(['\\', '/']).next().unwrap_or(&self.output);
        file.to_string()
    }

    /// `<output without extension>_log.txt`
    pub fn log_path(&self) -> String {
        let p = std::path::Path::new(&self.output);
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("encode");
        p.with_file_name(format!("{stem}_log.txt")).to_string_lossy().into_owned()
    }
}

pub struct Outcome {
    pub message: String,
    pub verified: bool,
}

pub enum Failure {
    Canceled,
    Error(String),
}

impl From<String> for Failure {
    fn from(s: String) -> Self {
        Failure::Error(s)
    }
}
impl From<&str> for Failure {
    fn from(s: &str) -> Self {
        Failure::Error(s.to_string())
    }
}
impl From<ff::Error> for Failure {
    fn from(e: ff::Error) -> Self {
        match e {
            ff::Error::Canceled => Failure::Canceled,
            other => Failure::Error(other.to_string()),
        }
    }
}

fn fmt_secs(s: f64) -> String {
    format!("{:02}:{:02}:{:06.3}", (s / 3600.0) as u64, ((s % 3600.0) / 60.0) as u64, s % 60.0)
}

pub fn execute(
    spec: &EncodeSpec,
    verify: bool,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(f32, bool),
) -> Result<Outcome, Failure> {
    let t0 = Instant::now();
    let mut log = String::new();
    let result = execute_inner(spec, verify, cancel, on_progress, &mut log, t0);
    // Always leave a log next to the output, whatever happened.
    let status = match &result {
        Ok(o) => format!("RESULT: {}", o.message),
        Err(Failure::Canceled) => "RESULT: canceled".to_string(),
        Err(Failure::Error(m)) => format!("RESULT: FAILED\n{m}"),
    };
    log.push_str(&format!("\n{status}\nTotal time: {:.1} s\n", t0.elapsed().as_secs_f64()));
    let _ = std::fs::write(spec.log_path(), log);
    result
}

fn execute_inner(
    spec: &EncodeSpec,
    verify: bool,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(f32, bool),
    log: &mut String,
    t0: Instant,
) -> Result<Outcome, Failure> {
    let dest = Destination::from_id(&spec.dest).ok_or("unknown destination")?;
    let stream = StreamType::from_id(&spec.stream).ok_or("unknown stream type")?;
    let layout: &'static ChannelLayout = ChannelLayout::find(&spec.layout).ok_or("unknown layout")?;
    if !spec.unlocked {
        validate(dest, stream, layout, spec.sample_rate).map_err(|e| format!("invalid combination: {e:?}"))?;
    }
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
        return Err(format!("input files are {} Hz but the encode is set to {} Hz", stems.sample_rate, spec.sample_rate).into());
    }
    let start = spec.start.unwrap_or(0.0);
    let total_src = stems.frames as f64 / stems.sample_rate as f64;
    let span = spec.duration.unwrap_or(total_src - start).min(total_src - start).max(0.0);
    if span <= 0.0 {
        return Err("nothing to encode: the start time is at or past the end of the input".into());
    }

    log.push_str("Subwoofa encode log\n");
    log.push_str(&format!("Output:       {}\n", spec.output));
    log.push_str(&format!("Format:       {} / {} / {}\n", dest.display_name(), stream.display_name(), layout.name));
    log.push_str(&format!("Sample rate:  {} Hz   Source bit depth: {}-bit   Bit rate: {}\n", spec.sample_rate, stems.bits, spec.bitrate.map(|b| format!("{b} kbps")).unwrap_or_else(|| "variable".into())));
    log.push_str(&format!("Dialnorm:     {}\n", spec.dialnorm.map(|d| format!("-{d} dBFS")).unwrap_or_else(|| "n/a".into())));
    log.push_str(&format!("Span:         start {}  length {}  (source {})\n", fmt_secs(start), fmt_secs(span), fmt_secs(total_src)));
    if let Some(t) = stems.time_reference {
        log.push_str(&format!("Broadcast time reference: {} samples ({})\n", t, fmt_secs(t as f64 / stems.sample_rate as f64)));
    }
    log.push_str(&format!("Unlocked:     {}\n", spec.unlocked));
    for ch in src_layout.channels {
        if let Some(p) = inputs.get(ch) {
            log.push_str(&format!("  {:<4} {}\n", ch.short(), p.display()));
        }
    }
    // audio files only have a native writer
    let native = spec.engine == "native" || stream.is_audio_file();
    if native && !stream.is_native() {
        return Err(format!("the native engine cannot encode {} yet", stream.display_name()).into());
    }


    let ffmpeg = ff::find_ffmpeg();
    if ffmpeg.is_none() && !native {
        return Err("ffmpeg not found on PATH".into());
    }
    let slots = if downmixed { Vec::new() } else { sw_pcm::slot_map(layout, &inputs, order).map_err(|e| e.to_string())? };
    let job = ff::Job {
        stream,
        layout,
        inputs,
        output: PathBuf::from(&spec.output),
        bitrate_kbps: spec.bitrate,
        sample_rate: Some(spec.sample_rate),
        dialnorm: spec.dialnorm,
        start_secs: spec.start,
        duration_secs: Some(span).filter(|_| spec.start.is_some() || spec.duration.is_some()),
        frames: Some((span * stems.sample_rate as f64).round() as u64),
        slots: Some(slots),
        ffmpeg: ffmpeg.clone().unwrap_or_default(),
    };
    log.push_str(&format!("Engine:       {}\n", if native { "Subwoofa native encoder" } else { "FFmpeg" }));

    if stream.is_audio_file() {
        // for DSD the setting is the DSD rate ("dsd64", "dsd128"); for the rest the sample format
        let is_dsd = matches!(stream, StreamType::DsdDsf | StreamType::DsdDff);
        let dsd_multiplier = if is_dsd { match spec.pcm_format.as_str() { "" | "dsd64" => 64, "dsd128" => 128, other => return Err(format!("unknown DSD rate '{other}' (use dsd64 or dsd128)").into()) } } else { 0 };
        let format = if is_dsd || spec.pcm_format.is_empty() { Some(sw_codec_pcm::SampleFormat::Int24) } else { sw_codec_pcm::SampleFormat::from_id(&spec.pcm_format) }.ok_or_else(|| format!("unknown sample format '{}' (use s16, s24, s32, f32 or f64)", spec.pcm_format))?;
        let container = match stream {
            StreamType::PcmWav => sw_codec_pcm::Container::Wav,
            StreamType::PcmAiff => sw_codec_pcm::Container::Aiff,
            StreamType::PcmCaf => sw_codec_pcm::Container::Caf,
            StreamType::OggFlac => sw_codec_pcm::Container::OggFlac,
            StreamType::Alac => sw_codec_pcm::Container::Alac,
            StreamType::Tta => sw_codec_pcm::Container::Tta,
            StreamType::DsdDsf => sw_codec_pcm::Container::Dsf,
            StreamType::DsdDff => sw_codec_pcm::Container::Dff,
            _ => sw_codec_pcm::Container::Flac,
        };
        if is_dsd {
            log.push_str(&format!("DSD rate: DSD{} ({} Hz), one-bit, fifth-order sigma-delta
", dsd_multiplier, 44_100 * dsd_multiplier));
        } else {
            log.push_str(&format!("Sample format: {} ({}-bit {})
", format.id(), format.bits(), if format.is_float() { "floating point" } else { "integer" }));
        }
        let req = sw_codec_pcm::Request { layout, container, format, start_secs: start, duration_secs: Some(span), output: std::path::Path::new(&spec.output), dsd_multiplier };
        sw_codec_pcm::encode_to_file(&mut stems, &req, &mut |p| on_progress(p, false), cancel).map_err(|e| match e {
            sw_codec_pcm::PcmError::Canceled => Failure::Canceled,
            other => Failure::Error(other.to_string()),
        })?;
    } else if native && matches!(stream, StreamType::TrueHd | StreamType::Mlp) {
        let req = sw_codec_truehd::native::Request {
            layout,
            start_secs: start,
            duration_secs: Some(span),
            output: std::path::Path::new(&spec.output),
            downmix: spec.downmix,
            mlp: stream == StreamType::Mlp,
        };
        let stats = sw_codec_truehd::native::encode_to_file(&mut stems, &req, &mut |p| on_progress(p, false), cancel).map_err(|e| match e {
            sw_codec_truehd::native::NativeError::Canceled => Failure::Canceled,
            other => Failure::Error(other.to_string()),
        })?;
        log.push_str(&format!("TrueHD stereo downmix substream: {}\n", if spec.downmix && layout.channel_count() > 2 { "yes" } else { "no" }));
        if stats.downmix_overflows > 0 {
            log.push_str(&format!("Warning: {} stereo-presentation samples exceed 24 bits; stereo-only decoders will clip them\n", stats.downmix_overflows));
        }
    } else if native && stream == StreamType::DtsExpress {
        let req = sw_codec_dts::lbr_native::LbrRequest {
            layout,
            bitrate_kbps: spec.bitrate.unwrap_or(128),
            start_secs: start,
            duration_secs: Some(span),
            output: std::path::Path::new(&spec.output),
            secondary: dest == Destination::BluRaySecondary,
        };
        sw_codec_dts::lbr_native::encode_to_file(&mut stems, &req, &mut |p| on_progress(p, false), cancel).map_err(|e| match e {
            sw_codec_dts::lbr_native::LbrError::Canceled => Failure::Canceled,
            other => Failure::Error(other.to_string()),
        })?;
    } else if native && stream == StreamType::DtsHdMa {
        if spec.sample_rate > 96_000 && layout.channels.len() > 6 && spec.bitrate != Some(0) {
            log.push_str("Note: above 96 kHz with more than 6 channels the stream is lossless only (no DTS core is written).
");
        }
        let req = sw_codec_dts::ma_native::MaRequest {
            layout,
            core_bitrate_kbps: match spec.bitrate { Some(0) => None, Some(b) => Some(b), None => Some(1509) },
            dialnorm: { let n = spec.dialnorm.unwrap_or(31) as i32; if n >= 31 { 0 } else { -n } },
            start_secs: start,
            duration_secs: Some(span),
            output: std::path::Path::new(&spec.output),
            container: spec.output.to_ascii_lowercase().ends_with(".dtshd"),
            x96: spec.core_x96,
        };
        sw_codec_dts::ma_native::encode_to_file(&mut stems, &req, &mut |p| on_progress(p, false), cancel).map_err(|e| match e {
            sw_codec_dts::ma_native::MaError::Canceled => Failure::Canceled,
            other => Failure::Error(other.to_string()),
        })?;
    } else if native && matches!(stream, StreamType::DtsDigitalSurround | StreamType::DtsEs | StreamType::DtsHdHra | StreamType::Dts9624) {
        let req = sw_codec_dts::native::Request {
            layout,
            bitrate_kbps: if stream == StreamType::DtsHdHra { 1509 } else { spec.bitrate.unwrap_or(1509) },
            x96: stream == StreamType::Dts9624,
            hra_kbps: if stream == StreamType::DtsHdHra { Some(spec.bitrate.unwrap_or(3018)) } else { None },
            dialnorm: { let n = spec.dialnorm.unwrap_or(31) as i32; if n >= 31 { 0 } else { -n } },
            pcm_bits: (stems.bits as u32).clamp(16, 24),
            start_secs: start,
            duration_secs: Some(span),
            output: std::path::Path::new(&spec.output),
            es: sw_codec_dts::es_options(stream == StreamType::DtsEs, layout, spec.es_phase_shift, spec.rear_minus_3db),
        };
        sw_codec_dts::native::encode_to_file(&mut stems, &req, &mut |p| on_progress(p, false), cancel).map_err(|e| match e {
            sw_codec_dts::native::NativeError::Canceled => Failure::Canceled,
            other => Failure::Error(other.to_string()),
        })?;
    } else if native {
        let req = sw_codec_ac3::native::Request {
            layout,
            bitrate_kbps: spec.bitrate.unwrap_or(448),
            dialnorm: spec.dialnorm.unwrap_or(31),
            start_secs: start,
            duration_secs: Some(span),
            output: std::path::Path::new(&spec.output),
            eac3: stream == StreamType::Eac3,
        };
        sw_codec_ac3::native::encode_to_file(&mut stems, &req, &mut |p| on_progress(p, false), cancel).map_err(|e| match e {
            sw_codec_ac3::native::NativeError::Canceled => Failure::Canceled,
            other => Failure::Error(other.to_string()),
        })?;
    } else {
        ff::run_progress(&job, span, cancel, &mut |p| on_progress(p, false))?;
    }
    let enc_secs = t0.elapsed().as_secs_f64();
    let size = std::fs::metadata(&spec.output).map(|m| m.len()).unwrap_or(0);
    let avg_kbps = size as f64 * 8.0 / span / 1000.0;
    let peak_note = peak_bit_rate_note(&spec.output);
    log.push_str(&format!("\nEncoded {} in {:.1} s ({:.0}x real time). Output {:.1} MB, average {:.0} kbps.\n", fmt_secs(span), enc_secs, span / enc_secs.max(0.001), size as f64 / 1e6, avg_kbps));
    log.push_str(&peak_note);

    if !verify || downmixed {
        return Ok(Outcome { message: format!("encoded {:.1} MB", size as f64 / 1e6), verified: false });
    }
    if stream.is_audio_file() {
        // the FFmpeg check compares a decode with the source sample for sample, which cannot hold for files that
        // reorder channels, round to a smaller sample format or are one-bit: use the native file check instead
        on_progress(1.0, true);
        let options = sw_verify::Options { deep: ffmpeg.is_some(), ffmpeg: ffmpeg.clone(), reference_verifier: None };
        let report = sw_verify::verify(std::path::Path::new(&spec.output), &options).map_err(|e| Failure::Error(e.to_string()))?;
        let failed: Vec<String> = report.checks.iter().filter(|c| c.status == sw_verify::Status::Fail).map(|c| format!("{}: {}", c.name, c.detail)).collect();
        if !report.passed {
            log.push_str(&format!("Verify: FAILED: {}
", failed.join("; ")));
            return Err(Failure::Error(format!("the encoded file failed verification: {}", failed.join("; "))));
        }
        let msg = if ffmpeg.is_some() { "file structure and a full decode check out".to_string() } else { "file structure checks out (not decoded: no ffmpeg)".to_string() };
        log.push_str(&format!("Verify: {msg}
"));
        return Ok(Outcome { message: msg, verified: true });
    }
    if ffmpeg.is_none() {
        log.push_str("Verify: skipped (ffmpeg not available to decode the result)\n");
        return Ok(Outcome { message: format!("encoded {:.1} MB (not verified: no decoder available)", size as f64 / 1e6), verified: false });
    }
    on_progress(1.0, true);
    let bits = stems.bits;
    match ff::verify(&job, bits, span) {
        Ok(msg) => {
            log.push_str(&format!("Verify: {msg}\n"));
            Ok(Outcome { message: msg, verified: true })
        }
        Err(e) => Err(Failure::from(e)),
    }
}

/// Peak bit rate of a finished stream (the figure disc authoring checks), or nothing if the file is not a
/// stream StreamTools can index.
fn peak_bit_rate_note(path: &str) -> String {
    let Ok(st) = sw_tools::Stream::open(std::path::Path::new(path)) else { return String::new() };
    let win = st.pbr(1.0);
    let (Some(top), Some(busiest)) = (win.iter().max_by(|a, b| a.peak_kbps.total_cmp(&b.peak_kbps)), win.iter().max_by(|a, b| a.mean_kbps.total_cmp(&b.mean_kbps))) else { return String::new() };
    format!(
        "Peak bit rate: {:.0} kbps in a single frame (at {}), busiest second {:.0} kbps (at {}), {} frames.\n",
        top.peak_kbps,
        fmt_secs(top.secs),
        busiest.mean_kbps,
        fmt_secs(busiest.secs),
        st.frames.len()
    )
}
