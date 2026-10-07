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
            let job = ff::Job { stream: StreamType::Ac3, layout, inputs: map, output: PathBuf::new(), bitrate_kbps: None, sample_rate: None, dialnorm: None, start_secs: start, duration_secs: duration, ffmpeg };
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
        Cmd::Encode { stream, layout, dest, auto_import, inputs, bitrate, sample_rate, dialnorm, start, duration, output, ffmpeg } => {
            let stream = StreamType::from_id(&stream).ok_or(format!("unknown stream type '{stream}'"))?;
            let dest = Destination::from_id(&dest).ok_or(format!("unknown destination '{dest}'"))?;
            let layout: &'static ChannelLayout =
                ChannelLayout::find(&layout).ok_or(format!("unknown layout '{layout}'"))?;
            formats::validate(dest, stream, layout, sample_rate).map_err(|e| format!("invalid combination: {e:?}"))?;

            let map = collect_inputs(layout, auto_import, inputs)?;
            let bitrate = bitrate.or_else(|| bitrate_range_kbps(dest, stream, layout, sample_rate).map(|r| r.1.min(default_cap(stream))));
            let ffmpeg = ffmpeg.or_else(ff::find_ffmpeg).ok_or("ffmpeg not found on PATH (use --ffmpeg)")?;
            let job = ff::Job { stream, layout, inputs: map, output: output.clone(), bitrate_kbps: bitrate, sample_rate: Some(sample_rate), dialnorm, start_secs: start, duration_secs: duration, ffmpeg };
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
        map.extend(autoimport::auto_import(&first, layout));
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
