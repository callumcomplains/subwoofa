//! The engine's everyday audio jobs, beyond Atmos: measuring loudness the way Dolby does (with dialogue detection),
//! encoding and decoding Dolby Digital and Dolby Digital Plus, transcoding, trimming and joining E-AC-3, correcting
//! loudness and reordering channels. (This engine build has no AAC or AC-4 encoder; its templates for them are for
//! other builds.)
//!
//! A job is a `TaskJob`: where the audio comes from (`Source`), what to do with it (`Task`) and where the result goes.
//! `profile` turns it into the engine's XML job file; `crate::taskrun::run_task` runs it.

use crate::profile::{el, path_str};
use crate::Error;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Where a job's audio comes from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    /// One WAV file with all the channels.
    Wav { path: PathBuf },
    /// One mono WAV per channel. `layout` is `mono`, `stereo`, `5.1` or `7.1`; `files` pairs the engine's channel
    /// names (`L R C LFE LS RS LRS RRS`) with the files.
    WavList {
        layout: String,
        files: Vec<(String, PathBuf)>,
    },
    /// An AC-3 or E-AC-3 stream.
    Stream { path: PathBuf, atmos: bool },
    /// Several streams to trim and join.
    Streams { paths: Vec<PathBuf> },
}

impl Source {
    /// The first file the job reads, which says on which drive its work folder belongs.
    pub fn first_file(&self) -> Option<&Path> {
        match self {
            Source::Wav { path } | Source::Stream { path, .. } => Some(path),
            Source::WavList { files, .. } => files.first().map(|(_, p)| p.as_path()),
            Source::Streams { paths } => paths.first().map(PathBuf::as_path),
        }
    }
}

/// How loudness is treated: only measured (the dialogue level is then written from the measurement) or measured and
/// corrected to a broadcast standard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Loudness {
    MeasureOnly {
        /// `1770-3`, `1770-2`, `1770-1` or `LeqA`.
        metering: String,
        /// Find the dialogue and measure that (what a dialnorm should hold).
        dialogue_intelligence: bool,
    },
    Correct {
        /// `atsc_a85`, `atsc_a85_agile`, `ebu_r128`, `freetv_op59` or `arib_b32`.
        preset: String,
    },
}

impl Default for Loudness {
    fn default() -> Self {
        Loudness::MeasureOnly {
            metering: "1770-3".into(),
            dialogue_intelligence: true,
        }
    }
}

/// Options of the Dolby Digital / Dolby Digital Plus encoder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EncodeOptions {
    /// `dd` (AC-3), `ddp` (E-AC-3) or `ddp71` (E-AC-3 with 7.1).
    pub mode: String,
    pub data_rate_kbps: u32,
    pub loudness: Loudness,
    /// `complete_main`, `music_and_effects`, `visually_impaired`, `hearing_impaired`, `dialogue`, `commentary`,
    /// `emergency` or `voice_over`.
    pub bitstream_mode: String,
    pub lfe: bool,
    pub drc_line: String,
    pub drc_rf: String,
    pub lfe_lowpass: bool,
    pub surround_phase_shift: bool,
    pub surround_3db: bool,
    pub loro_center: String,
    pub loro_surround: String,
    pub ltrt_center: String,
    pub ltrt_surround: String,
    pub preferred_downmix: String,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        EncodeOptions {
            mode: "ddp".into(),
            data_rate_kbps: 448,
            loudness: Loudness::default(),
            bitstream_mode: "complete_main".into(),
            lfe: true,
            drc_line: "film_light".into(),
            drc_rf: "film_light".into(),
            lfe_lowpass: true,
            // the engine's own examples switch these on for broadcast-style mixes; they change the surround channels
            // (a phase shift and a 3 dB cut), which a faithful encode of a finished 5.1 mix should not
            surround_phase_shift: false,
            surround_3db: false,
            loro_center: "-3".into(),
            loro_surround: "-3".into(),
            ltrt_center: "-3".into(),
            ltrt_surround: "-3".into(),
            preferred_downmix: "loro".into(),
        }
    }
}

/// What the engine can do with a job's audio.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "task", rename_all = "snake_case")]
pub enum Task {
    /// Measure loudness; the result is the manifest, which `Measured::from_manifest` reads.
    Measure {
        metering: String,
        dialogue_intelligence: bool,
        /// How sure the speech detector must be, 0 to 100.
        speech_threshold: u8,
    },
    /// PCM to Dolby Digital or Dolby Digital Plus.
    Encode(Box<EncodeOptions>),
    /// PCM (up to 7.1) to ordinary lossless Dolby TrueHD, with the engine's own encoder.
    Truehd {
        /// `1770-4`, `1770-3`, `1770-2`, `1770-1` or `LeqA`: how the dialogue level written into the stream is measured.
        metering: String,
        /// The 6 and 8 channel presentations lower the surrounds by 3 dB unless told not to, which would change a
        /// finished mix; false (the default) leaves the audio as it is.
        surround_3db: bool,
    },
    /// Dolby Digital (Plus) to a WAV file; `drc` is `none`, `line` or `rf`.
    Decode { drc: String },
    /// Dolby Digital (Plus) to Dolby Digital Plus, with optional loudness correction (`None` leaves it alone).
    Transcode {
        data_rate_kbps: Option<u32>,
        correct: Option<String>,
    },
    /// Cut and join E-AC-3 streams without re-encoding. `start` and `end` are times (`0:0:0.0`, `end_of_file`).
    Edit {
        start: String,
        end: String,
        invert_trim: bool,
    },
    /// Correct PCM loudness to a preset (`atsc_a85`, `ebu_r128`, ...).
    Correct { preset: String },
    /// Put the channels of a WAV in the order the engine expects.
    Reorder,
}

/// One job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskJob {
    pub source: Source,
    /// The file the job writes (for `Measure` the manifest, an XML file).
    pub output: PathBuf,
    pub task: Task,
    /// Work on only this part of the programme, as the engine writes times (`0:1:30.5`, see `clock`); measuring,
    /// encoding, decoding and correcting take it.
    #[serde(default)]
    pub start: Option<String>,
    #[serde(default)]
    pub end: Option<String>,
}

/// Seconds as the engine writes a time: hours, minutes and seconds with a fraction.
pub fn clock(secs: f64) -> String {
    let total = secs.max(0.0);
    let (h, m) = ((total / 3600.0) as u64, ((total % 3600.0) / 60.0) as u64);
    format!("{h}:{m}:{:.3}", total % 60.0)
}

const PRESETS: [&str; 5] = ["atsc_a85", "atsc_a85_agile", "ebu_r128", "freetv_op59", "arib_b32"];
const RATES: [u32; 45] = [
    32, 40, 48, 56, 64, 72, 80, 88, 96, 104, 112, 120, 128, 144, 160, 176, 192, 200, 208, 216, 224, 232, 240, 248, 256,
    272, 288, 304, 320, 336, 352, 368, 384, 400, 448, 512, 576, 640, 704, 768, 832, 896, 960, 1008, 1024,
];
const DRC: [&str; 5] = ["film_standard", "film_light", "music_standard", "music_light", "speech"];
const CENTER: [&str; 8] = ["+3", "+1.5", "0", "-1.5", "-3", "-4.5", "-6", "-inf"];
const SURROUND: [&str; 5] = ["-1.5", "-3", "-4.5", "-6", "-inf"];
const MODES: [&str; 8] = [
    "complete_main",
    "music_and_effects",
    "visually_impaired",
    "hearing_impaired",
    "dialogue",
    "commentary",
    "emergency",
    "voice_over",
];

fn pick<'a>(what: &str, value: &str, allowed: &[&'a str]) -> Result<&'a str, Error> {
    allowed
        .iter()
        .find(|a| **a == value)
        .copied()
        .ok_or_else(|| Error::Invalid(format!("{what} '{value}': use {}", allowed.join(", "))))
}

fn flag(on: bool) -> &'static str {
    if on {
        "true"
    } else {
        "false"
    }
}

/// The name of the loudness manifest that jobs which measure as they go write beside their output (the engine wants
/// it connected even when nobody reads it).
pub(crate) fn manifest_name(output: &Path) -> String {
    let stem = output.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
    format!("{stem}_manifest.xml")
}

pub(crate) fn writes_manifest(task: &Task) -> bool {
    matches!(task, Task::Decode { .. } | Task::Correct { .. })
}

/// The name of the file the job writes, and the engine's tag for it.
fn output_tag(task: &Task) -> &'static str {
    match task {
        Task::Measure { .. } => "manifest",
        Task::Truehd { .. } => "mlp",
        Task::Encode(o) if o.mode == "dd" => "ac3",
        Task::Encode(_) | Task::Transcode { .. } | Task::Edit { .. } => "ec3",
        Task::Decode { .. } | Task::Correct { .. } | Task::Reorder => "wav",
    }
}

fn storage(dir: &Path) -> String {
    format!("<storage><local>{}</local></storage>", el("path", &path_str(dir)))
}

fn dir_of(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

fn file_of(path: &Path) -> Result<&str, Error> {
    path.file_name()
        .and_then(|f| f.to_str())
        .ok_or_else(|| Error::Invalid(format!("{} has no file name", path.display())))
}

fn input_xml(job: &TaskJob) -> Result<String, Error> {
    let body = match &job.source {
        Source::Wav { path } => format!(
            "<wav version=\"1\">{}{}</wav>",
            el("file_name", file_of(path)?),
            storage(dir_of(path))
        ),
        Source::WavList { layout, files } => {
            let layout = pick("channel configuration", layout, &["mono", "stereo", "5.1", "7.1"])?;
            let mut names = String::new();
            for (channel, path) in files {
                let channel = pick("channel", channel, &["L", "R", "C", "LFE", "LS", "RS", "LRS", "RRS"])?;
                names.push_str(&el(&format!("file_name_{channel}"), file_of(path)?));
            }
            let dir = files
                .first()
                .map(|(_, p)| dir_of(p))
                .ok_or_else(|| Error::Invalid("no channel files".into()))?;
            format!(
                "<wav_list version=\"1\">{names}{}{}</wav_list>",
                el("channel_configuration", layout),
                storage(dir)
            )
        }
        Source::Stream { path, atmos } => {
            let tag = match path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("ac3") => "ac3",
                Some("ec3" | "eac3") => "ec3",
                other => {
                    return Err(Error::Invalid(format!(
                        "a Dolby Digital stream is .ac3 or .ec3, not .{}",
                        other.unwrap_or("")
                    )))
                }
            };
            // only an E-AC-3 stream says whether it carries Atmos
            let stream_type = match (tag, atmos) {
                ("ec3", true) => el("stream_type", "atmos"),
                ("ec3", false) => el("stream_type", "non_atmos"),
                _ => String::new(),
            };
            format!(
                "<{tag} version=\"1\">{stream_type}{}{}</{tag}>",
                el("file_name", file_of(path)?),
                storage(dir_of(path))
            )
        }
        Source::Streams { paths } => {
            let first = paths.first().ok_or_else(|| Error::Invalid("no streams".into()))?;
            let names: Vec<&str> = paths.iter().map(|p| file_of(p)).collect::<Result<_, _>>()?;
            format!(
                "<generic version=\"1\">{}{}</generic>",
                el("file_name", &names.join(" ")),
                storage(dir_of(first))
            )
        }
    };
    Ok(format!("<input><audio>{body}</audio></input>\n"))
}

fn loudness_xml(l: &Loudness) -> Result<String, Error> {
    Ok(match l {
        Loudness::MeasureOnly {
            metering,
            dialogue_intelligence,
        } => format!(
            "<loudness><measure_only>{}{}</measure_only></loudness>",
            el(
                "metering_mode",
                pick("metering mode", metering, &["1770-3", "1770-2", "1770-1", "LeqA"])?
            ),
            el("dialogue_intelligence", flag(*dialogue_intelligence))
        ),
        Loudness::Correct { preset } => format!(
            "<loudness><measure_and_correct>{}</measure_and_correct></loudness>",
            el("preset", pick("loudness preset", preset, &PRESETS)?)
        ),
    })
}

fn encode_filter(o: &EncodeOptions) -> Result<String, Error> {
    let mode = pick("encoder mode", &o.mode, &["dd", "ddp", "ddp71"])?;
    if !RATES.contains(&o.data_rate_kbps) {
        return Err(Error::Invalid(format!(
            "data rate {} kbps is not one the encoder offers",
            o.data_rate_kbps
        )));
    }
    // Dolby Digital's loudness may only be measured (the dialogue level goes into the stream); Digital Plus may also
    // be corrected
    if mode == "dd" && matches!(o.loudness, Loudness::Correct { .. }) {
        return Err(Error::Invalid(
            "Dolby Digital can only measure loudness, not correct it".into(),
        ));
    }
    let mut f = loudness_xml(&o.loudness)?;
    f.push_str(&el("encoder_mode", mode));
    f.push_str(&el(
        "bitstream_mode",
        pick("bitstream mode", &o.bitstream_mode, &MODES)?,
    ));
    f.push_str(&el("data_rate", &o.data_rate_kbps.to_string()));
    f.push_str(&el("lfe_on", flag(o.lfe)));
    f.push_str(&format!(
        "<drc>{}{}</drc>",
        el("line_mode_drc_profile", pick("DRC profile", &o.drc_line, &DRC)?),
        el("rf_mode_drc_profile", pick("DRC profile", &o.drc_rf, &DRC)?)
    ));
    f.push_str(&el("lfe_lowpass_filter", flag(o.lfe_lowpass)));
    f.push_str(&el("surround_90_degree_phase_shift", flag(o.surround_phase_shift)));
    f.push_str(&el("surround_3db_attenuation", flag(o.surround_3db)));
    f.push_str(&format!(
        "<downmix>{}{}{}{}{}</downmix>",
        el("loro_center_mix_level", pick("mix level", &o.loro_center, &CENTER)?),
        el(
            "loro_surround_mix_level",
            pick("mix level", &o.loro_surround, &SURROUND)?
        ),
        el("ltrt_center_mix_level", pick("mix level", &o.ltrt_center, &CENTER)?),
        el(
            "ltrt_surround_mix_level",
            pick("mix level", &o.ltrt_surround, &SURROUND)?
        ),
        el(
            "preferred_downmix_mode",
            pick(
                "downmix mode",
                &o.preferred_downmix,
                &["loro", "ltrt", "ltrt-pl2", "not_indicated"]
            )?
        )
    ));
    Ok(format!("<pcm_to_ddp version=\"3\">{f}</pcm_to_ddp>"))
}

fn filter_xml(task: &Task) -> Result<String, Error> {
    Ok(match task {
        Task::Measure {
            metering,
            dialogue_intelligence,
            speech_threshold,
        } => {
            let l = Loudness::MeasureOnly {
                metering: metering.clone(),
                dialogue_intelligence: *dialogue_intelligence,
            };
            // the speech threshold sits inside measure_only, after the other two
            let inner = loudness_xml(&l)?.replace(
                "</measure_only>",
                &format!(
                    "{}</measure_only>",
                    el("speech_threshold", &(*speech_threshold).min(100).to_string())
                ),
            );
            format!("<measure_loudness version=\"1\">{inner}</measure_loudness>")
        }
        Task::Encode(o) => encode_filter(o)?,
        Task::Truehd { metering, surround_3db } => {
            let metering = pick(
                "metering mode",
                metering,
                &["1770-4", "1770-3", "1770-2", "1770-1", "LeqA"],
            )?;
            let presentation = |name: &str| {
                format!(
                    "<{name}>{}{}</{name}>",
                    el("drc_profile", "film_light"),
                    el("surround_3db_attenuation", flag(*surround_3db))
                )
            };
            format!(
                "<encode_to_dthd version=\"1\"><loudness_measurement>{}</loudness_measurement>{}{}</encode_to_dthd>",
                el("metering_mode", metering),
                presentation("presentation_8ch"),
                presentation("presentation_6ch")
            )
        }
        Task::Decode { drc } => format!(
            "<ddp_decode version=\"1\">{}</ddp_decode>",
            el("drc", pick("DRC", drc, &["none", "line", "rf"])?)
        ),
        Task::Transcode {
            data_rate_kbps,
            correct,
        } => {
            let mut f = String::new();
            if let Some(preset) = correct {
                f.push_str(&loudness_xml(&Loudness::Correct { preset: preset.clone() })?);
            }
            match data_rate_kbps {
                Some(r) if RATES.contains(r) => f.push_str(&el("data_rate", &r.to_string())),
                Some(r) => {
                    return Err(Error::Invalid(format!(
                        "data rate {r} kbps is not one the encoder offers"
                    )))
                }
                None => f.push_str(&el("data_rate", "auto")),
            }
            format!("<transcode_to_ddp version=\"1\">{f}</transcode_to_ddp>")
        }
        Task::Edit {
            start,
            end,
            invert_trim,
        } => format!(
            "<edit_ddp version=\"1\">{}{}{}{}</edit_ddp>",
            el("start", start),
            el("end", end),
            el("order", "trim_join"),
            el("invert_trim", flag(*invert_trim))
        ),
        Task::Correct { preset } => format!(
            "<pcm_loudness_correction version=\"1\">{}</pcm_loudness_correction>",
            loudness_xml(&Loudness::Correct { preset: preset.clone() })?
        ),
        Task::Reorder => "<pcm_channel_order version=\"1\"/>".to_string(),
    })
}

/// The engine's job file for `job`, with `temp_dir` as scratch space.
pub fn profile(job: &TaskJob, temp_dir: &Path) -> Result<String, Error> {
    let tag = output_tag(&job.task);
    let mut x = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<job_config>\n");
    x.push_str(&input_xml(job)?);
    x.push_str(&format!("<filter><audio>{}</audio></filter>\n", filter_xml(&job.task)?));
    let manifest = if writes_manifest(&job.task) {
        format!(
            "<manifest version=\"1\">{}{}</manifest>",
            el("file_name", &manifest_name(&job.output)),
            storage(dir_of(&job.output))
        )
    } else {
        String::new()
    };
    x.push_str(&format!(
        "<output><{tag} version=\"1\">{}{}</{tag}>{manifest}</output>\n",
        el("file_name", file_of(&job.output)?),
        storage(dir_of(&job.output))
    ));
    x.push_str(&format!(
        "<misc><temp_dir><clean_temp>true</clean_temp>{}</temp_dir></misc>\n</job_config>\n",
        el("path", &path_str(temp_dir))
    ));
    Ok(x)
}

/// What the engine measured: the numbers of its loudness manifest.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Measured {
    /// Integrated loudness in LUFS, measured on the dialogue when dialogue detection was on (`speech`) and on the
    /// whole programme otherwise.
    pub integrated: Option<f64>,
    /// `speech` or `absolute`/`relative`: what the integrated figure was gated on.
    pub gating: String,
    /// Share of the programme that was speech, in per cent.
    pub speech_percent: Option<f64>,
    pub range: Option<f64>,
    pub max_momentary: Option<f64>,
    pub max_short_term: Option<f64>,
    /// Per channel, in dBFS / dBTP, in the order of the file.
    pub sample_peak: Vec<f64>,
    pub true_peak: Vec<f64>,
    pub channels: usize,
}

impl Measured {
    /// Reads the manifest the engine writes.
    pub fn from_manifest(xml: &str) -> Result<Measured, Error> {
        let text = |tag: &str| -> Option<String> {
            let open = format!("<{tag}>");
            let start = xml.find(&open)? + open.len();
            let end = xml[start..].find(&format!("</{tag}>"))? + start;
            Some(xml[start..end].trim().to_string())
        };
        let number =
            |tag: &str| text(tag).and_then(|t| t.split_whitespace().next().and_then(|n| n.parse::<f64>().ok()));
        let list = |tag: &str| -> Vec<f64> {
            text(tag)
                .map(|t| t.split_whitespace().filter_map(|n| n.parse().ok()).collect())
                .unwrap_or_default()
        };
        if !xml.contains("<loudness_results>") {
            return Err(Error::Invalid("the manifest has no loudness results".into()));
        }
        Ok(Measured {
            integrated: number("integrated_loudness"),
            gating: text("integrated_loudness_gating").unwrap_or_default(),
            speech_percent: number("integrated_speech_percentage"),
            range: number("integrated_loudness_range"),
            max_momentary: number("max_momentary_loudness"),
            max_short_term: number("max_short_term_3s_loudness"),
            sample_peak: list("sample_peak"),
            true_peak: list("true_peak"),
            channels: number("channel_count").map_or(0, |n| n as usize),
        })
    }

    /// The dialnorm this measurement calls for: its distance below full scale, rounded, 1 to 31.
    pub fn dialnorm(&self) -> Option<u8> {
        self.integrated.map(|l| (-l).round().clamp(1.0, 31.0) as u8)
    }

    /// The highest true peak of any channel, in dBTP.
    pub fn highest_true_peak(&self) -> Option<f64> {
        self.true_peak.iter().copied().reduce(f64::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav_list(layout: &str) -> Source {
        Source::WavList {
            layout: layout.into(),
            files: vec![
                ("L".into(), "D:/in/a_L.wav".into()),
                ("R".into(), "D:/in/a_R.wav".into()),
                ("C".into(), "D:/in/a_C.wav".into()),
            ],
        }
    }

    fn job(source: Source, output: &str, task: Task) -> TaskJob {
        TaskJob {
            source,
            output: output.into(),
            task,
            start: None,
            end: None,
        }
    }

    const TEMP: &str = "D:/tmp";

    #[test]
    fn measuring_writes_only_the_manifest_and_names_each_channel_file() {
        let j = job(
            wav_list("5.1"),
            "D:/out/m.xml",
            Task::Measure {
                metering: "1770-3".into(),
                dialogue_intelligence: true,
                speech_threshold: 15,
            },
        );
        let x = profile(&j, Path::new(TEMP)).unwrap();
        for want in [
            "<wav_list version=\"1\"><file_name_L>a_L.wav</file_name_L>",
            "<channel_configuration>5.1</channel_configuration>",
            "<measure_loudness version=\"1\"><loudness><measure_only>",
            "<dialogue_intelligence>true</dialogue_intelligence><speech_threshold>15</speech_threshold></measure_only>",
            "<manifest version=\"1\"><file_name>m.xml</file_name>",
        ] {
            assert!(x.contains(want), "missing {want} in {x}");
        }
        assert!(!x.contains("<wav version=\"1\"><file_name>m"), "no audio output wanted");
    }

    #[test]
    fn encoding_picks_the_output_type_from_the_mode() {
        let mut o = EncodeOptions::default();
        let x = profile(
            &job(
                Source::Wav {
                    path: "D:/in/a.wav".into(),
                },
                "D:/o/a.ec3",
                Task::Encode(Box::new(o.clone())),
            ),
            Path::new(TEMP),
        )
        .unwrap();
        assert!(x.contains("<ec3 version=\"1\">") && x.contains("<encoder_mode>ddp</encoder_mode>"));
        o.mode = "dd".into();
        let x = profile(
            &job(
                Source::Wav {
                    path: "D:/in/a.wav".into(),
                },
                "D:/o/a.ac3",
                Task::Encode(Box::new(o.clone())),
            ),
            Path::new(TEMP),
        )
        .unwrap();
        assert!(x.contains("<ac3 version=\"1\">") && x.contains("<measure_only>"));
        // Dolby Digital cannot correct loudness
        o.loudness = Loudness::Correct {
            preset: "ebu_r128".into(),
        };
        assert!(profile(
            &job(Source::Wav { path: "a.wav".into() }, "a.ac3", Task::Encode(Box::new(o))),
            Path::new(TEMP)
        )
        .is_err());
    }

    #[test]
    fn bad_values_are_refused_before_the_engine_runs() {
        let mut o = EncodeOptions::default();
        o.data_rate_kbps = 500;
        assert!(profile(
            &job(Source::Wav { path: "a.wav".into() }, "a.ec3", Task::Encode(Box::new(o))),
            Path::new(TEMP)
        )
        .is_err());
        let bad_channel = Source::WavList {
            layout: "5.1".into(),
            files: vec![("Q".into(), "a.wav".into())],
        };
        assert!(profile(&job(bad_channel, "m.xml", Task::Reorder), Path::new(TEMP)).is_err());
        assert!(profile(
            &job(
                Source::Stream {
                    path: "a.mp3".into(),
                    atmos: false
                },
                "a.wav",
                Task::Decode { drc: "none".into() }
            ),
            Path::new(TEMP)
        )
        .is_err());
        assert!(profile(
            &job(
                Source::Wav { path: "a.wav".into() },
                "a.wav",
                Task::Correct { preset: "loud".into() }
            ),
            Path::new(TEMP)
        )
        .is_err());
    }

    #[test]
    fn streams_and_edits_are_described_the_way_the_engine_reads_them() {
        let x = profile(
            &job(
                Source::Stream {
                    path: "D:/in/a.ac3".into(),
                    atmos: false,
                },
                "D:/o/a.wav",
                Task::Decode { drc: "line".into() },
            ),
            Path::new(TEMP),
        )
        .unwrap();
        // an AC-3 stream has no stream type to give; an E-AC-3 stream says whether it holds Atmos
        assert!(x.contains("<ac3 version=\"1\"><file_name>a.ac3</file_name>") && !x.contains("stream_type"));
        assert!(
            x.contains("<drc>line</drc>")
                && x.contains("<wav version=\"1\">")
                && x.contains("<manifest version=\"1\">"),
            "{x}"
        );
        let e = profile(
            &job(
                Source::Stream {
                    path: "D:/in/a.ec3".into(),
                    atmos: true,
                },
                "D:/o/a.wav",
                Task::Decode { drc: "none".into() },
            ),
            Path::new(TEMP),
        )
        .unwrap();
        assert!(e.contains("<ec3 version=\"1\"><stream_type>atmos</stream_type>"));
        let x = profile(
            &job(
                Source::Streams {
                    paths: vec!["D:/in/a.ec3".into(), "D:/in/b.ec3".into()],
                },
                "D:/o/j.ec3",
                Task::Edit {
                    start: "0:0:5.0".into(),
                    end: "end_of_file".into(),
                    invert_trim: false,
                },
            ),
            Path::new(TEMP),
        )
        .unwrap();
        assert!(
            x.contains("<generic version=\"1\"><file_name>a.ec3 b.ec3</file_name>")
                && x.contains("<order>trim_join</order>")
        );
    }

    #[test]
    fn truehd_is_encoded_without_the_surround_cut_unless_asked() {
        let make = |cut: bool| {
            profile(
                &job(
                    wav_list("5.1"),
                    "D:/o/a.thd",
                    Task::Truehd {
                        metering: "1770-4".into(),
                        surround_3db: cut,
                    },
                ),
                Path::new(TEMP),
            )
            .unwrap()
        };
        let plain = make(false);
        for want in [
            "<encode_to_dthd version=\"1\">",
            "<mlp version=\"1\"><file_name>a.thd</file_name>",
            "<presentation_8ch><drc_profile>film_light</drc_profile><surround_3db_attenuation>false</surround_3db_attenuation></presentation_8ch>",
            "<presentation_6ch><drc_profile>film_light</drc_profile><surround_3db_attenuation>false</surround_3db_attenuation></presentation_6ch>",
        ] {
            assert!(plain.contains(want), "missing {want} in {plain}");
        }
        assert!(make(true).contains("<surround_3db_attenuation>true</surround_3db_attenuation>"));
    }

    #[test]
    fn markup_in_names_is_escaped() {
        let x = profile(
            &job(
                Source::Wav {
                    path: "D:/in/a&b.wav".into(),
                },
                "D:/o/<x>.wav",
                Task::Reorder,
            ),
            Path::new(TEMP),
        )
        .unwrap();
        assert!(x.contains("a&amp;b.wav") && x.contains("&lt;x&gt;.wav"));
    }

    #[test]
    fn a_manifest_is_read_into_numbers() {
        let xml = "<manifest><loudness_results><integrated_loudness>\n -25.751657\n</integrated_loudness>\
            <integrated_loudness_gating>speech</integrated_loudness_gating>\
            <integrated_speech_percentage>66.5</integrated_speech_percentage>\
            <true_peak>\n -27.0\n -1.42\n -80.3\n</true_peak><channel_count>3</channel_count></loudness_results></manifest>";
        let m = Measured::from_manifest(xml).unwrap();
        assert_eq!(m.integrated, Some(-25.751657));
        assert_eq!(m.dialnorm(), Some(26));
        assert_eq!(m.gating, "speech");
        assert_eq!(m.true_peak.len(), 3);
        assert_eq!(m.highest_true_peak(), Some(-1.42));
        assert_eq!(m.channels, 3);
        assert!(Measured::from_manifest("<manifest/>").is_err());
    }
}
