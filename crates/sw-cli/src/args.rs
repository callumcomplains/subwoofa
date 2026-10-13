//! The command line as `clap` sees it: every command, its options and what they mean.

use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

/// `subwoofa`: the program as a whole.
#[derive(Parser)]
#[command(
    name = "subwoofa",
    about = "Every channel, every codec.",
    disable_version_flag = true
)]
pub(crate) struct Cli {
    /// Print the version and exit
    #[allow(dead_code)]
    #[arg(short = 'V', long, global = true)]
    pub(crate) version: bool,
    #[command(subcommand)]
    pub(crate) cmd: Cmd,
}

#[derive(Subcommand)]
pub(crate) enum ToolsCmd {
    /// Format, rate, channels, length, bit rates.
    Info { file: PathBuf },
    /// Bit rate over time (mean and peak per window).
    Pbr {
        file: PathBuf,
        #[arg(long, default_value_t = 1.0)]
        window: f64,
    },
    /// Keep the part between two times (on frame boundaries).
    Trim {
        file: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        #[arg(long, default_value_t = 0.0)]
        start: f64,
        #[arg(long)]
        end: Option<f64>,
    },
    /// Cut at the given times (seconds) into numbered files.
    Split {
        file: PathBuf,
        /// Output path without the number and extension, e.g. `D:/out/part`
        #[arg(short, long)]
        output: PathBuf,
        #[arg(long, required = true, num_args = 1..)]
        at: Vec<f64>,
    },
    /// Join streams of the same format end to end.
    Append {
        #[arg(required = true, num_args = 2..)]
        inputs: Vec<PathBuf>,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Replace the stretch of the first stream that the second covers (Join/Replace), starting at --at seconds.
    Replace {
        base: PathBuf,
        with: PathBuf,
        #[arg(long)]
        at: f64,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Insert the second stream into the first at --at seconds without removing anything.
    Insert {
        base: PathBuf,
        with: PathBuf,
        #[arg(long)]
        at: f64,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Relabel the start timecode and frame rate of a .dtshd file (the audio is untouched).
    Restripe {
        file: PathBuf,
        /// New start time, hh:mm:ss:ff
        #[arg(long, default_value = "00:00:00:00")]
        start: String,
        /// New frame rate: 23.976, 24, 25, 29.97, "29.97 drop", 30 or "30 drop"
        #[arg(long, default_value = "24")]
        fps: String,
        /// Write here instead of changing the file in place
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Add silence to the head and/or tail of a .dtshd file written by Subwoofa or the DTS encoder.
    AddSilence {
        file: PathBuf,
        /// Silence at the head, seconds
        #[arg(long, default_value_t = 0.0)]
        head: f64,
        /// Silence at the tail, seconds
        #[arg(long, default_value_t = 0.0)]
        tail: f64,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Same as `subwoofa verify`.
    Verify {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        #[arg(long)]
        deep: bool,
        #[arg(long)]
        ffmpeg: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub(crate) enum AtmosCmd {
    /// What the Dolby Encoding Engine's validator says about an Atmos master (beds, objects, length).
    Info {
        file: PathBuf,
        /// Path of dee.exe (found automatically in the usual place)
        #[arg(long)]
        dee: Option<PathBuf>,
    },
    /// Convert an Atmos master between ADM BWF (.wav), DAMF (.atmos) and IAB (.mxf).
    Convert {
        input: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        /// adm, damf or mxf_iab
        #[arg(long, default_value = "adm")]
        format: String,
        #[arg(long)]
        dee: Option<PathBuf>,
    },
    /// Encode an Atmos master to TrueHD Atmos or Dolby Digital Plus Atmos with the Dolby Encoding Engine.
    /// A multichannel WAV channel bed (up to 7.1.4) also works for Dolby Digital Plus Atmos.
    Encode(AtmosEncodeArgs),
}

// the command line is parsed once, so the size of the largest variant does not matter
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
pub(crate) enum Cmd {
    /// Dolby Atmos through the Dolby Encoding Engine (a copy you are licensed to use).
    Atmos {
        #[command(subcommand)]
        op: AtmosCmd,
    },
    /// Check stream or audio files: structure, checksums and (with --deep) a full decode, statistics and header dumps.
    /// Several files may be given; a summary follows when there is more than one.
    Verify {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Also decode the audio and dump every header
        #[arg(long)]
        deep: bool,
        /// Print the report as JSON
        #[arg(long)]
        json: bool,
        #[arg(long)]
        ffmpeg: Option<PathBuf>,
    },
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
    /// StreamTools: inspect, cut and join DTS / DTS-HD and TrueHD / MLP streams.
    Tools {
        #[command(subcommand)]
        op: ToolsCmd,
    },
    /// Check a downmix for clipping (three or more consecutive full-scale samples).
    DownmixCheck(DownmixArgs),
    /// Encode per-channel PCM files to a multichannel stream.
    Encode(EncodeArgs),
}

/// The Bitstream tab of the original encoder.
#[derive(Args, Debug, Clone, Default)]
pub(crate) struct BitstreamArgs {
    /// Programme information stored in the .dtshd file (Latin-1, up to 2000 characters).
    #[arg(long)]
    pub(crate) program_info: Option<String>,
    /// Seamless branching:  for a clip with no branch points inside, or a CSV file of branch point timecodes
    /// (HH:MM:SS:FF, one per line).
    #[arg(long)]
    pub(crate) seamless: Option<String>,
    /// Blu-ray secondary audio: how many dB the primary audio is turned down while this stream plays (default 6).
    #[arg(long)]
    pub(crate) primary_attenuation: Option<f64>,
    /// Blu-ray secondary audio: seconds to reach that attenuation at the start (0 to 9).
    #[arg(long)]
    pub(crate) fade_down: Option<f64>,
    /// Blu-ray secondary audio: seconds to release it again at the end (0 to 9).
    #[arg(long)]
    pub(crate) fade_up: Option<f64>,
    /// DTS Express dialog mode: only the lower half of the band is coded.
    #[arg(long)]
    pub(crate) dialog_mode: bool,
    /// Blu-ray secondary audio, mono: where it goes, as dB of attenuation per speaker, e.g.  (the default) or
    /// . Speakers: C, L, R, Ls, Rs, LFE.
    #[arg(long)]
    pub(crate) mono_map: Option<String>,
    /// Wide 7.1: add the speaker remapping sets for side-rear and 5.1 playback.
    #[arg(long)]
    pub(crate) wide_remap: bool,
    /// Frame rate stored in the .dtshd file: 23.976, 24 (default), 25, 29.97, "29.97 drop", 30 or "30 drop".
    #[arg(long)]
    pub(crate) fps: Option<String>,
    /// Start timecode stored in the .dtshd file (HH:MM:SS:FF).
    #[arg(long)]
    pub(crate) timecode_start: Option<String>,
    /// Reference timecode stored in the .dtshd file (HH:MM:SS:FF); the start timecode when not given.
    #[arg(long)]
    pub(crate) timecode_reference: Option<String>,
    /// Master Audio: carry a stereo downmix as a channel set of its own (needs a layout with a surround pair).
    #[arg(long)]
    pub(crate) embed_stereo: bool,
    /// Master Audio: write the stereo downmix coefficients into the stream as information.
    #[arg(long)]
    pub(crate) stereo_matrix: bool,
    /// Label the stereo downmix as matrix encoded (Lt/Rt) instead of Lo/Ro.
    #[arg(long)]
    pub(crate) stereo_lt_rt: bool,
    /// Downmix to 5.1: gain of the 5.1 channels, e.g. "L=3,R=3,C=3,Ls=3,Rs=3,LFE=3" (dB; "inf" is off).
    #[arg(long)]
    pub(crate) dmx51_scale: Option<String>,
    /// Downmix to 5.1: level at which the first extra channel of a 7.x layout is added to each 5.1 channel
    /// (default "Ls=3").
    #[arg(long)]
    pub(crate) dmx51_a: Option<String>,
    /// Downmix to 5.1: the same for the second extra channel (default "Rs=3").
    #[arg(long)]
    pub(crate) dmx51_b: Option<String>,
    /// Downmix to stereo: level at which each channel goes to the left output (default "L=3,C=6,Ls=6").
    #[arg(long)]
    pub(crate) dmx20_a: Option<String>,
    /// Downmix to stereo: the same for the right output (default "R=3,C=6,Rs=6").
    #[arg(long)]
    pub(crate) dmx20_b: Option<String>,
}

impl BitstreamArgs {
    pub(crate) fn options(&self) -> Result<sw_core::BitstreamOptions, String> {
        use sw_core::bitstream::{MonoMap, Seamless};
        let mut o = sw_core::BitstreamOptions::default();
        o.program_info = self.program_info.clone().unwrap_or_default();
        o.wide_remap = self.wide_remap;
        o.timecode.rate = self.fps.clone().unwrap_or_default();
        o.timecode.start = self.timecode_start.clone().unwrap_or_default();
        o.timecode.reference = self.timecode_reference.clone().unwrap_or_default();
        o.secondary.dialog_mode = self.dialog_mode;
        o.downmix.embed_stereo = self.embed_stereo;
        o.downmix.stereo_matrix = self.stereo_matrix;
        o.downmix.lt_rt = self.stereo_lt_rt;
        // channels named on the command line replace their default level; the others keep it
        for (text, table) in [
            (&self.dmx51_scale, &mut o.downmix.primary),
            (&self.dmx51_a, &mut o.downmix.extra_a),
            (&self.dmx51_b, &mut o.downmix.extra_b),
            (&self.dmx20_a, &mut o.downmix.stereo_a),
            (&self.dmx20_b, &mut o.downmix.stereo_b),
        ] {
            if let Some(t) = text {
                sw_core::Downmix::parse_levels(table, t).map_err(|e| format!("downmix levels: {e}"))?;
            }
        }
        if let Some(v) = self.primary_attenuation {
            o.secondary.attenuation_db = v.clamp(0.0, 60.0);
        }
        o.secondary.fade_down_secs = self.fade_down.unwrap_or(0.0).clamp(0.0, 9.0);
        o.secondary.fade_up_secs = self.fade_up.unwrap_or(0.0).clamp(0.0, 9.0);
        if let Some(m) = &self.mono_map {
            let mut map = MonoMap::default();
            for part in m.split(',').filter(|p| !p.trim().is_empty()) {
                let (name, db) = part
                    .split_once('=')
                    .ok_or(format!("--mono-map: '{part}' is not SPEAKER=dB"))?;
                let db: f64 = db
                    .trim()
                    .parse()
                    .map_err(|_| format!("--mono-map: '{db}' is not a number"))?;
                let slot = match name.trim().to_ascii_lowercase().as_str() {
                    "c" => &mut map.c,
                    "l" => &mut map.l,
                    "r" => &mut map.r,
                    "ls" => &mut map.ls,
                    "rs" => &mut map.rs,
                    "lfe" => &mut map.lfe,
                    other => return Err(format!("--mono-map: unknown speaker '{other}' (C, L, R, Ls, Rs, LFE)")),
                };
                *slot = Some(db);
            }
            o.secondary.mono_map = map;
        }
        o.seamless = match self.seamless.as_deref() {
            None | Some("") | Some("off") => Seamless::Off,
            Some("single") => Seamless::SingleClip,
            Some(path) => {
                let text = std::fs::read_to_string(path).map_err(|e| format!("--seamless: cannot read {path}: {e}"))?;
                Seamless::Timecodes(
                    text.lines()
                        .map(|l| l.split(',').next().unwrap_or("").trim().to_string())
                        .filter(|l| !l.is_empty())
                        .collect(),
                )
            }
        };
        Ok(o)
    }
}

/// The options of `subwoofa atmos encode`.
#[derive(Args)]
pub(crate) struct AtmosEncodeArgs {
    /// The master: .atmos (DAMF), an ADM BWF .wav, or a channel-bed .wav
    pub(crate) input: PathBuf,
    /// truehd or ddp
    #[arg(long, default_value = "truehd")]
    pub(crate) format: String,
    #[arg(short, long)]
    pub(crate) output: PathBuf,
    /// Dolby Digital Plus data rate in kbps (384, 448, 576, 640, 768, 1024)
    #[arg(long, default_value_t = 768)]
    pub(crate) bitrate: u32,
    /// TrueHD spatial clusters (12, 14, 16)
    #[arg(long, default_value_t = 12)]
    pub(crate) clusters: u8,
    /// Dynamic range preset: film_standard, film_light, music_standard, music_light, speech, none
    #[arg(long, default_value = "film_light")]
    pub(crate) drc: String,
    /// Dialogue level override, -31 to 0
    #[arg(long)]
    pub(crate) dialnorm: Option<i32>,
    /// Timecode frame rate of the master (23.976, 24, 25, 29.97, 30, 48, 50, 59.94, 60)
    #[arg(long)]
    pub(crate) frame_rate: Option<String>,
    #[arg(long)]
    pub(crate) start: Option<String>,
    #[arg(long)]
    pub(crate) end: Option<String>,
    /// Also write the 8-channel presentation (TrueHD)
    #[arg(long)]
    pub(crate) presentation_8ch: bool,
    /// Also write the 6-channel presentation (TrueHD)
    #[arg(long)]
    pub(crate) presentation_6ch: bool,
    /// Measure loudness (1770-4, 1770-3, 1770-2, 1770-1, LeqA)
    #[arg(long)]
    pub(crate) metering: Option<String>,
    #[arg(long)]
    pub(crate) dee: Option<PathBuf>,
}

/// The options of `subwoofa downmix-check`.
#[derive(Args)]
pub(crate) struct DownmixArgs {
    #[arg(long, default_value = "5.1")]
    pub(crate) layout: String,
    /// Target: `stereo` (Lo/Ro) or `5.1` (from 6.x/7.x).
    #[arg(long, default_value = "stereo")]
    pub(crate) to: String,
    #[arg(long)]
    pub(crate) auto_import: Option<PathBuf>,
    #[arg(long = "input", value_name = "CH=FILE")]
    pub(crate) inputs: Vec<String>,
    /// Extra trim applied to every connected coefficient, in dB (negative attenuates).
    #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
    pub(crate) trim_db: f32,
    /// Apply the trim that guarantees no clipping from full-scale inputs.
    #[arg(long)]
    pub(crate) safe: bool,
    #[arg(long)]
    pub(crate) start: Option<f64>,
    #[arg(long)]
    pub(crate) duration: Option<f64>,
    #[arg(long)]
    pub(crate) ffmpeg: Option<PathBuf>,
}

/// The options of `subwoofa encode`.
#[derive(Args)]
pub(crate) struct EncodeArgs {
    /// Stream type id (ac3, eac3, dts, truehd, mlp, dtshd-ma ...).
    #[arg(long)]
    pub(crate) stream: String,
    /// Layout id (see `subwoofa layouts`).
    #[arg(long, default_value = "5.1")]
    pub(crate) layout: String,
    /// Destination id (see `subwoofa formats`).
    #[arg(long, default_value = "generic")]
    pub(crate) dest: String,
    /// The L (first channel) file; sibling channel files are found automatically.
    #[arg(long)]
    pub(crate) auto_import: Option<PathBuf>,
    /// Explicit channel file, e.g. `--input C=center.wav` (repeatable).
    #[arg(long = "input", value_name = "CH=FILE")]
    pub(crate) inputs: Vec<String>,
    #[arg(long)]
    pub(crate) bitrate: Option<u32>,
    #[arg(long, default_value_t = 48_000)]
    pub(crate) sample_rate: u32,
    #[arg(long)]
    pub(crate) dialnorm: Option<u8>,
    /// Start offset into the source, in seconds.
    #[arg(long)]
    pub(crate) start: Option<f64>,
    /// Length to encode, in seconds (e.g. 60 for a one-minute test).
    #[arg(long)]
    pub(crate) duration: Option<f64>,
    #[arg(short, long)]
    pub(crate) output: PathBuf,
    #[arg(long)]
    pub(crate) ffmpeg: Option<PathBuf>,
    /// Channel order inside a multichannel input file: auto (default), mask or file.
    #[arg(long, default_value = "auto")]
    pub(crate) channel_order: String,
    /// Native TrueHD only: leave out the stereo downmix substream (channels coded directly).
    #[arg(long)]
    pub(crate) no_downmix: bool,
    /// Encoder engine: `native` (Subwoofa's own, the default) or `ffmpeg` (only for AC-3, E-AC-3, DTS core,
    /// TrueHD and MLP).
    #[arg(long, default_value = "native")]
    pub(crate) engine: String,
    /// Native DTS-ES: rotate the surrounds by +/-45 degrees before the centre surround is added.
    #[arg(long)]
    pub(crate) es_phase_shift: bool,
    /// Native DTS: attenuate the rear channels by 3 dB first.
    #[arg(long)]
    pub(crate) rear_3db: bool,
    /// Layout of the input files when richer than --layout (e.g. 22.2): mixed down to --layout. Native engine.
    #[arg(long)]
    pub(crate) source_layout: Option<String>,
    /// DTS-HD MA at 96 kHz: give the core an X96 extension (a 96/24 core).
    #[arg(long)]
    pub(crate) core_96_24: bool,
    /// Audio files (wav, aiff, caf, flac ...): the sample format: s16, s24 (default), s32, f32 or f64. DSD: dsd64 or dsd128.
    /// DCP sound: the frame rate (24, 23.976, 25, 30, 48, 50, 60), then `-interop` for MXF Interop labels, e.g. 25-interop.
    #[arg(long)]
    pub(crate) pcm_format: Option<String>,
    /// Lossless search effort: fast, balanced (default) or best. Lossy formats ignore it.
    #[arg(long, default_value = "")]
    pub(crate) speed: String,
    /// Threads to use; 0 (default) uses every core.
    #[arg(long, default_value_t = 0)]
    pub(crate) threads: u32,
    #[command(flatten)]
    pub(crate) bitstream: BitstreamArgs,
}
