use clap::{Parser, Subcommand};
use std::collections::HashMap;
use std::path::PathBuf;
use sw_backend_ffmpeg as ff;
use sw_core::formats::{self, bitrate_range_kbps, Destination, StreamType};
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
        Cmd::Encode { stream, layout, dest, auto_import, inputs, bitrate, sample_rate, dialnorm, output, ffmpeg } => {
            let stream = StreamType::from_id(&stream).ok_or(format!("unknown stream type '{stream}'"))?;
            let dest = Destination::from_id(&dest).ok_or(format!("unknown destination '{dest}'"))?;
            let layout: &'static ChannelLayout =
                ChannelLayout::find(&layout).ok_or(format!("unknown layout '{layout}'"))?;
            formats::validate(dest, stream, layout, sample_rate).map_err(|e| format!("invalid combination: {e:?}"))?;

            let mut map: HashMap<Channel, PathBuf> = HashMap::new();
            if let Some(first) = auto_import {
                map.extend(autoimport::auto_import(&first, layout));
            }
            for spec in inputs {
                let (ch, file) = spec.split_once('=').ok_or("--input must be CH=FILE")?;
                let ch = Channel::from_id(ch).ok_or(format!("unknown channel '{ch}'"))?;
                map.insert(ch, file.into());
            }
            let bitrate = bitrate.or_else(|| bitrate_range_kbps(dest, stream, layout, sample_rate).map(|r| r.1.min(default_cap(stream))));
            let ffmpeg = ffmpeg.or_else(ff::find_ffmpeg).ok_or("ffmpeg not found on PATH (use --ffmpeg)")?;
            let job = ff::Job { stream, layout, inputs: map, output: output.clone(), bitrate_kbps: bitrate, sample_rate: Some(sample_rate), dialnorm, ffmpeg };
            ff::run(&job).map_err(|e| e.to_string())?;
            println!("wrote {}", output.display());
        }
    }
    Ok(())
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
