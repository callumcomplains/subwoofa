use clap::{Parser, Subcommand};
use std::collections::HashMap;
use std::path::PathBuf;
use sw_backend_ffmpeg as ff;
use sw_core::formats::{self, bitrate_range_kbps, Destination, StreamType};
use sw_core::downmix::Matrix;
use sw_core::{autoimport, Channel, ChannelLayout, LAYOUTS};

#[derive(Parser)]
#[command(name = "subwoofa", version, about = "Every channel, every codec.")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List channel layouts.
    Layouts,
    /// List destinations and the stream types each allows.
    Formats,
    /// Show format info for a WAV/AIFF file (and RMS of a span, to cross-check readers).
    Info {
        path: PathBuf,
        #[arg(long)]
        start: Option<f64>,
        #[arg(long)]
        duration: Option<f64>,
    },
    /// Analyse a TrueHD (.thd) stream: structure, coding tools and where the bits go.
    StreamInfo {
        path: PathBuf,
        /// Only analyse this many restart intervals (default: all) ...
        #[arg(long)]
        count: Option<usize>,
        /// ... starting at this restart interval
        #[arg(long, default_value_t = 0)]
        first: usize,
    },
    /// Check a downmix for clipping (three or more consecutive full-scale samples).
    DownmixCheck {
        #[arg(long, default_value = "5.1")]
        layout: String,
        /// Target: `stereo` (Lo/Ro) or `5.1` (from 6.x/7.x).
        #[arg(long, default_value = "stereo")]
        to: String,
        #[arg(long)]
        auto_import: Option<PathBuf>,
        #[arg(long = "input", value_name = "CH=FILE")]
        inputs: Vec<String>,
        /// Extra trim applied to every connected coefficient, in dB (negative attenuates).
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        trim_db: f32,
        /// Apply the trim that guarantees no clipping from full-scale inputs.
        #[arg(long)]
        safe: bool,
        #[arg(long)]
        start: Option<f64>,
        #[arg(long)]
        duration: Option<f64>,
        #[arg(long)]
        ffmpeg: Option<PathBuf>,
    },
    /// Encode per-channel PCM files to a multichannel stream.
    Encode {
        /// Stream type id (ac3, eac3, dts, truehd, mlp, dtshd-ma ...).
        #[arg(long)]
        stream: String,
        /// Layout id (see `subwoofa layouts`).
        #[arg(long, default_value = "5.1")]
        layout: String,
        /// Destination id (see `subwoofa formats`).
        #[arg(long, default_value = "generic")]
        dest: String,
        /// The L (first channel) file; sibling channel files are found automatically.
        #[arg(long)]
        auto_import: Option<PathBuf>,
        /// Explicit channel file, e.g. `--input C=center.wav` (repeatable).
        #[arg(long = "input", value_name = "CH=FILE")]
        inputs: Vec<String>,
        #[arg(long)]
        bitrate: Option<u32>,
        #[arg(long, default_value_t = 48_000)]
        sample_rate: u32,
        #[arg(long)]
        dialnorm: Option<u8>,
        /// Start offset into the source, in seconds.
        #[arg(long)]
        start: Option<f64>,
        /// Length to encode, in seconds (e.g. 60 for a one-minute test).
        #[arg(long)]
        duration: Option<f64>,
        #[arg(short, long)]
        output: PathBuf,
        #[arg(long)]
        ffmpeg: Option<PathBuf>,
        /// Channel order inside a multichannel input file: auto (default), mask or file.
        #[arg(long, default_value = "auto")]
        channel_order: String,
        /// Native TrueHD only: leave out the stereo downmix substream (channels coded directly).
        #[arg(long)]
        no_downmix: bool,
        /// Encoder engine: `ffmpeg` (default) or `native` (Subwoofa's own, AC-3 only for now).
        #[arg(long, default_value = "ffmpeg")]
        engine: String,
    },
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    match Cli::parse().cmd {
        Cmd::Layouts => {
            for l in LAYOUTS {
                println!("{:<22} {:<4} {}", l.id, l.short_label(), l.name);
            }
        }
        Cmd::Formats => {
            for d in Destination::ALL {
                println!("{} ({})", d.display_name(), d.id());
                for s in d.stream_types() {
                    println!("    {:<12} {} [.{}]", s.id(), s.display_name(), d.extension(*s));
                }
            }
        }
        Cmd::Info { path, start, duration } => {
            let mut r = sw_pcm::PcmReader::open(&path).map_err(|e| e.to_string())?;
            let i = r.info().clone();
            println!("{:?}  {} ch  {} Hz  {}-bit{}  {} frames ({:.3} s)", i.container, i.channels, i.sample_rate, i.bits, if i.float { " float" } else { "" }, i.frames, i.duration_secs());
            if let Some(t) = i.start_secs() {
                println!("broadcast start: {t:.3} s since midnight");
            }
            if start.is_some() || duration.is_some() {
                r.seek_frame((start.unwrap_or(0.0) * i.sample_rate as f64) as u64).map_err(|e| e.to_string())?;
                let want = (duration.unwrap_or(10.0) * i.sample_rate as f64) as usize;
                let (mut sum, mut n, mut buf) = (0f64, 0usize, Vec::new());
                while n < want {
                    buf.clear();
                    let got = r.read_f32(&mut buf, (want - n).min(1 << 16)).map_err(|e| e.to_string())?;
                    if got == 0 { break; }
                    sum += buf.iter().map(|v| (*v as f64).powi(2)).sum::<f64>();
                    n += got * i.channels as usize;
                }
                println!("RMS over span: {:.3} dBFS", 10.0 * (sum / n.max(1) as f64).log10());
            }
        }
        Cmd::StreamInfo { path, count, first } => {
            let data = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            print!("{}", sw_codec_truehd::analyze::analyze_range(&data, first, count.unwrap_or(usize::MAX)).summary());
        }
        Cmd::DownmixCheck { layout, to, auto_import, inputs, trim_db, safe, start, duration, ffmpeg } => {
            let layout: &'static ChannelLayout = ChannelLayout::find(&layout).ok_or(format!("unknown layout '{layout}'"))?;
            let mut m = match to.as_str() {
                "stereo" => Matrix::lo_ro(layout),
                "5.1" => Matrix::to_51(layout),
                _ => return Err("--to must be 'stereo' or '5.1'".into()),
            };
            if safe {
                let t = m.safe_trim_db();
                println!("safe trim: {t:.2} dB (worst-case input sum {:.3})", m.worst_case_gain().into_iter().fold(0.0, f32::max));
                m.trim(t);
            }
            if trim_db != 0.0 {
                m.trim(trim_db);
            }
            let map = collect_inputs(layout, auto_import, inputs)?;
            let ffmpeg = ffmpeg.or_else(ff::find_ffmpeg).ok_or("ffmpeg not found on PATH (use --ffmpeg)")?;
            let job = ff::Job { stream: StreamType::Ac3, layout, inputs: map, output: PathBuf::new(), bitrate_kbps: None, sample_rate: None, dialnorm: None, start_secs: start, duration_secs: duration, frames: None, slots: None, ffmpeg };
            let rep = ff::downmix::saturation_check(&job, &m).map_err(|e| e.to_string())?;
            println!("analysed {} frames ({:.1} s at 48 kHz)", rep.frames, rep.frames as f64 / 48_000.0);
            for (c, p) in rep.channels.iter().zip(&rep.peak_db) {
                println!("  {c:<4} peak {p:>7.2} dBFS");
            }
            println!("{} saturation event(s)", rep.event_count);
            for e in rep.events.iter().take(10) {
                println!("  {:<4} at {:.3} s", rep.channels[e.channel], e.frame as f64 / 48_000.0);
            }
        }
        Cmd::Encode { stream, layout, dest, auto_import, inputs, bitrate, sample_rate, dialnorm, start, duration, output, ffmpeg, engine, no_downmix, channel_order } => {
            let order = sw_pcm::ChannelOrder::parse(&channel_order).ok_or("--channel-order must be auto, mask or file")?;
            let stream = StreamType::from_id(&stream).ok_or(format!("unknown stream type '{stream}'"))?;
            let dest = Destination::from_id(&dest).ok_or(format!("unknown destination '{dest}'"))?;
            let layout: &'static ChannelLayout =
                ChannelLayout::find(&layout).ok_or(format!("unknown layout '{layout}'"))?;
            formats::validate(dest, stream, layout, sample_rate).map_err(|e| format!("invalid combination: {e:?}"))?;

            let map = collect_inputs(layout, auto_import, inputs)?;
            let bitrate = bitrate.or_else(|| bitrate_range_kbps(dest, stream, layout, sample_rate).map(|r| r.1.min(default_cap(stream))));
            if engine == "native" {
                if stream == StreamType::TrueHd {
                    let mut stems = sw_pcm::StemSet::open_ordered(layout, &map, order).map_err(|e| e.to_string())?;
                    let req = sw_codec_truehd::native::Request {
                        layout,
                        start_secs: start.unwrap_or(0.0),
                        duration_secs: duration,
                        output: &output,
                        downmix: !no_downmix,
                    };
                    let t0 = std::time::Instant::now();
                    let stats = sw_codec_truehd::native::encode_to_file(&mut stems, &req, &mut |_| {}, &std::sync::atomic::AtomicBool::new(false))
                        .map_err(|e| e.to_string())?;
                    let secs = stats.frames as f64 / stems.sample_rate as f64;
                    let pcm = stats.frames as f64 * layout.channel_count() as f64 * 3.0;
                    println!("wrote {} ({:.1} MB, {:.1}% of PCM) in {:.1} s = {:.0}x real time", output.display(), stats.bytes as f64 / 1e6, 100.0 * stats.bytes as f64 / pcm, t0.elapsed().as_secs_f64(), secs / t0.elapsed().as_secs_f64().max(1e-3));
                    if stats.downmix_overflows > 0 {
                        println!("note: {} stereo-presentation samples exceed 24 bits (a stereo-only decoder will clip them)", stats.downmix_overflows);
                    }
                    return Ok(());
                }
                if stream == StreamType::DtsHdMa {
                    let mut stems = sw_pcm::StemSet::open_ordered(layout, &map, order).map_err(|e| e.to_string())?;
                    let req = sw_codec_dts::ma_native::MaRequest {
                        layout,
                        core_bitrate_kbps: bitrate.filter(|b| *b > 0).or(Some(1509)),
                        dialnorm: { let n = dialnorm.unwrap_or(31) as i32; if n >= 31 { 0 } else { -n } },
                        start_secs: start.unwrap_or(0.0),
                        duration_secs: duration,
                        output: &output,
                        container: output.extension().map_or(false, |e| e.eq_ignore_ascii_case("dtshd")),
                    };
                    let t0 = std::time::Instant::now();
                    let stats = sw_codec_dts::ma_native::encode_to_file(&mut stems, &req, &mut |_| {}, &std::sync::atomic::AtomicBool::new(false)).map_err(|e| e.to_string())?;
                    let secs = (stats.frames * 512) as f64 / stems.sample_rate as f64;
                    println!("wrote {} ({} frames, {:.1} MB, average {:.0} kbps) in {:.1} s = {:.1}x real time", output.display(), stats.frames, stats.bytes as f64 / 1e6, stats.bytes as f64 * 8.0 / secs / 1000.0, t0.elapsed().as_secs_f64(), secs / t0.elapsed().as_secs_f64().max(1e-3));
                    return Ok(());
                }
                if stream == StreamType::DtsDigitalSurround {
                    let mut stems = sw_pcm::StemSet::open_ordered(layout, &map, order).map_err(|e| e.to_string())?;
                    let req = sw_codec_dts::native::Request {
                        layout,
                        bitrate_kbps: bitrate.unwrap_or(1509),
                        dialnorm: { let n = dialnorm.unwrap_or(31) as i32; if n >= 31 { 0 } else { -n } },
                        pcm_bits: (stems.bits as u32).clamp(16, 24),
                        start_secs: start.unwrap_or(0.0),
                        duration_secs: duration,
                        output: &output,
                    };
                    let t0 = std::time::Instant::now();
                    let stats = sw_codec_dts::native::encode_to_file(&mut stems, &req, &mut |_| {}, &std::sync::atomic::AtomicBool::new(false)).map_err(|e| e.to_string())?;
                    let secs = (stats.frames * 512) as f64 / stems.sample_rate as f64;
                    println!("wrote {} ({} frames, {:.1} MB) in {:.1} s = {:.0}x real time", output.display(), stats.frames, stats.bytes as f64 / 1e6, t0.elapsed().as_secs_f64(), secs / t0.elapsed().as_secs_f64().max(1e-3));
                    return Ok(());
                }
                if !matches!(stream, StreamType::Ac3 | StreamType::Eac3) {
                    return Err(format!("the native engine encodes AC-3, E-AC-3, TrueHD, DTS core and DTS-HD MA so far, not {}", stream.display_name()));
                }
                let mut stems = sw_pcm::StemSet::open_ordered(layout, &map, order).map_err(|e| e.to_string())?;
                let req = sw_codec_ac3::native::Request {
                    layout,
                    bitrate_kbps: bitrate.unwrap_or(448),
                    dialnorm: dialnorm.unwrap_or(31),
                    start_secs: start.unwrap_or(0.0),
                    duration_secs: duration,
                    output: &output,
                    eac3: stream == StreamType::Eac3,
                };
                let t0 = std::time::Instant::now();
                let stats = sw_codec_ac3::native::encode_to_file(&mut stems, &req, &mut |_| {}, &std::sync::atomic::AtomicBool::new(false))
                    .map_err(|e| e.to_string())?;
                let secs = stats.input_frames as f64 / stems.sample_rate as f64;
                println!("wrote {} ({} frames, {:.1} MB) in {:.1} s = {:.0}x real time", output.display(), stats.frames, stats.bytes as f64 / 1e6, t0.elapsed().as_secs_f64(), secs / t0.elapsed().as_secs_f64().max(1e-3));
                return Ok(());
            }
            let ffmpeg = ffmpeg.or_else(ff::find_ffmpeg).ok_or("ffmpeg not found on PATH (use --ffmpeg)")?;
            let slots = sw_pcm::slot_map(layout, &map, order).ok();
            let job = ff::Job { stream, layout, inputs: map, output: output.clone(), bitrate_kbps: bitrate, sample_rate: Some(sample_rate), dialnorm, start_secs: start, duration_secs: duration, frames: None, slots, ffmpeg };
            ff::run(&job).map_err(|e| e.to_string())?;
            println!("wrote {}", output.display());
        }
    }
    Ok(())
}

fn collect_inputs(
    layout: &'static ChannelLayout,
    auto_import: Option<PathBuf>,
    inputs: Vec<String>,
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
