//! Dolby Encoding Engine (DEE) backend: Dolby Atmos in TrueHD and Dolby Digital Plus.
//!
//! Subwoofa has no Atmos encoder of its own (the object coding is not public), so Atmos goes through a copy of
//! Dolby's own command-line encoder that the user supplies and is licensed to use. Nothing of DEE is shipped
//! or copied: this crate only writes DEE's job file (an XML profile), runs `dee.exe` and reads what it prints.
//!
//! * **TrueHD Atmos** (`encode_to_dthd`) and **Dolby Digital Plus Atmos** (`encode_to_atmos_ddp`,
//!   `atmos_mezz_to_atmos_ddp`) from an Atmos master written by Dolby's tools (DAMF `.atmos`, ADM BWF `.wav`).
//!   DEE refuses masters that were not authored with Dolby tools, and so does Subwoofa.
//! * **Dolby Digital Plus Atmos from channel audio** (`pcm_to_atmos_ddp`): a multichannel WAV bed (up to 7.1.4).

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the Dolby Encoding Engine was not found; set its location in the settings (dee.exe)")]
    NotFound,
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Invalid(String),
    #[error("DEE failed (exit code {code}):\n{messages}")]
    Failed { code: i32, messages: String },
    #[error("canceled")]
    Canceled,
}

/// Where DEE is looked for when nothing is configured.
pub fn find_dee(configured: Option<&Path>) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = configured {
        candidates.push(if p.is_dir() { p.join(exe_name()) } else { p.to_path_buf() });
    }
    if let Some(p) = std::env::var_os("SUBWOOFA_DEE") {
        let p = PathBuf::from(p);
        candidates.push(if p.is_dir() { p.join(exe_name()) } else { p });
    }
    for var in ["ProgramFiles", "ProgramW6432"] {
        if let Some(pf) = std::env::var_os(var) {
            candidates.push(PathBuf::from(pf).join("Dolby Encoding Engine").join(exe_name()));
        }
    }
    candidates.push(PathBuf::from(r"C:\Program Files\Dolby Encoding Engine").join(exe_name()));
    candidates.into_iter().find(|p| p.is_file())
}

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "dee.exe"
    } else {
        "dee"
    }
}

/// What kind of file the job reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputKind {
    /// Multichannel WAV holding a channel bed (Dolby Digital Plus Atmos only).
    Wav,
    /// ADM BWF: a WAV/BW64 file with `chna` and `axml` chunks.
    Adm,
    /// Dolby Atmos Master File set (`.atmos` with its `.metadata` and `.audio` files beside it).
    Damf,
    /// Dolby Atmos mezzanine file.
    AtmosMezz,
}

impl InputKind {
    /// Work out the kind from the file name and, for `.wav`/`.bw64`, from the chunks inside.
    pub fn detect(path: &Path) -> Result<InputKind, Error> {
        let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).unwrap_or_default();
        match ext.as_str() {
            "atmos" => Ok(InputKind::Damf),
            "wav" | "bw64" | "rf64" | "bwf" => Ok(if has_adm_chunks(path)? { InputKind::Adm } else { InputKind::Wav }),
            "mxf" | "ec3" | "ac3" | "mp4" => Err(Error::Invalid(format!("'{ext}' files are not Atmos masters"))),
            _ => Ok(InputKind::AtmosMezz),
        }
    }
}

/// Whether a RIFF/BW64 file has an `axml` chunk (looking only at the chunk headers).
fn has_adm_chunks(path: &Path) -> Result<bool, Error> {
    use std::io::{Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    let mut head = [0u8; 12];
    f.read_exact(&mut head)?;
    if !(&head[..4] == b"RIFF" || &head[..4] == b"BW64" || &head[..4] == b"RF64") {
        return Ok(false);
    }
    // sizes of chunks that do not fit 32 bits are in ds64; the audio chunk is the only large one, and skipping
    // it by its stated size works for ordinary files and for BW64 whose data size is in ds64
    let len = f.metadata()?.len();
    let mut pos = 12u64;
    let mut ds64_data: Option<u64> = None;
    while pos + 8 <= len {
        f.seek(SeekFrom::Start(pos))?;
        let mut h = [0u8; 8];
        f.read_exact(&mut h)?;
        let id = [h[0], h[1], h[2], h[3]];
        let mut size = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as u64;
        if &id == b"axml" {
            return Ok(true);
        }
        if &id == b"ds64" {
            let mut b = [0u8; 16];
            f.read_exact(&mut b)?;
            ds64_data = Some(u64::from_le_bytes(b[8..16].try_into().unwrap()));
        }
        if &id == b"data" && size == 0xFFFF_FFFF {
            size = ds64_data.unwrap_or(0);
        }
        pos += 8 + size + (size & 1);
    }
    Ok(false)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Drc {
    FilmStandard,
    FilmLight,
    MusicStandard,
    MusicLight,
    Speech,
    /// Dolby Digital Plus only.
    None,
}

impl Drc {
    fn tag(self) -> &'static str {
        match self {
            Drc::FilmStandard => "film_standard",
            Drc::FilmLight => "film_light",
            Drc::MusicStandard => "music_standard",
            Drc::MusicLight => "music_light",
            Drc::Speech => "speech",
            Drc::None => "none",
        }
    }
}

/// One of TrueHD's lower presentations (8, 6 or 2 channels), derived from the Atmos mix.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Presentation {
    pub drc: Drc,
    /// 3 dB attenuation of the surround channels (8 and 6 channel presentations).
    pub surround_3db: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrueHdOptions {
    pub drc: Drc,
    /// Spatial clusters of the Atmos presentation: 12, 14 or 16.
    pub spatial_clusters: u8,
    /// Keep compatibility with older Blu-ray authoring tools (a little higher peak data rate).
    pub legacy_authoring: bool,
    pub presentation_8ch: Option<Presentation>,
    pub presentation_6ch: Option<Presentation>,
    /// Stereo presentation: its DRC and format (`stereo`, `dolby_surround_encoded`, `dolby_headphone_encoded`).
    pub presentation_2ch: Option<(Drc, bool, String)>,
    pub optimize_data_rate: bool,
}

impl Default for TrueHdOptions {
    fn default() -> Self {
        TrueHdOptions { drc: Drc::FilmLight, spatial_clusters: 12, legacy_authoring: true, presentation_8ch: None, presentation_6ch: None, presentation_2ch: None, optimize_data_rate: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DdpOptions {
    /// 384, 448, 576, 640, 768 or 1024 kbps.
    pub data_rate_kbps: u32,
    pub drc_line: Drc,
    pub drc_rf: Drc,
    /// Lo/Ro and Lt/Rt centre and surround mix levels in dB (as DEE spells them, e.g. "-3").
    pub loro_center: String,
    pub loro_surround: String,
    pub ltrt_center: String,
    pub ltrt_surround: String,
    /// `loro`, `ltrt`, `ltrt-pl2` or `not_indicated`.
    pub preferred_downmix: String,
}

impl Default for DdpOptions {
    fn default() -> Self {
        DdpOptions { data_rate_kbps: 768, drc_line: Drc::FilmLight, drc_rf: Drc::FilmLight, loro_center: "-3".into(), loro_surround: "-3".into(), ltrt_center: "-3".into(), ltrt_surround: "-3".into(), preferred_downmix: "loro".into() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "format", rename_all = "snake_case")]
pub enum Target {
    TrueHd(TrueHdOptions),
    Ddp(DdpOptions),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtmosJob {
    pub input: PathBuf,
    /// Detected from the file when `None`.
    pub kind: Option<InputKind>,
    pub output: PathBuf,
    pub target: Target,
    /// Frame rate of the master's timecode (`23.976`, `24`, `25`, `29.97`, `30`, `48`, `50`, `59.94`, `60`).
    pub frame_rate: Option<String>,
    /// Encode only this span (`HH:MM:SS:FF` or `HH:MM:SS.xx`).
    pub start: Option<String>,
    pub end: Option<String>,
    /// Measure loudness with this standard (`1770-4`...) so the dialogue level is set from it.
    pub metering: Option<String>,
    /// Dialogue level override, -31 to 0 (0 keeps the measured one).
    pub custom_dialnorm: Option<i32>,
    pub prepend_silence: Option<String>,
    pub append_silence: Option<String>,
}

impl AtmosJob {
    pub fn new(input: impl Into<PathBuf>, output: impl Into<PathBuf>, target: Target) -> AtmosJob {
        AtmosJob { input: input.into(), kind: None, output: output.into(), target, frame_rate: None, start: None, end: None, metering: None, custom_dialnorm: None, prepend_silence: None, append_silence: None }
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn path_str(p: &Path) -> String {
    // DEE takes Windows paths with either separator; forward slashes avoid every escaping question
    p.to_string_lossy().replace('\\', "/")
}

fn el(name: &str, value: &str) -> String {
    format!("<{name}>{}</{name}>", esc(value))
}

fn valid_rate(v: &str) -> bool {
    ["23.976", "24", "25", "29.97", "30", "48", "50", "59.94", "60", "not_indicated"].contains(&v)
}

/// DEE's job file for `job`, with `temp_dir` as its scratch folder.
pub fn profile_xml(job: &AtmosJob, temp_dir: &Path) -> Result<String, Error> {
    let kind = match job.kind {
        Some(k) => k,
        None => InputKind::detect(&job.input)?,
    };
    if kind == InputKind::Wav && matches!(job.target, Target::TrueHd(_)) {
        return Err(Error::Invalid("TrueHD Atmos needs an Atmos master (DAMF or ADM BWF); a plain channel WAV only makes Dolby Digital Plus Atmos".into()));
    }
    let dir = job.input.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let file = job.input.file_name().and_then(|f| f.to_str()).ok_or_else(|| Error::Invalid("the input has no file name".into()))?;
    let storage = |d: &Path| format!("<storage><local>{}</local></storage>", el("path", &path_str(d)));
    if let Some(r) = &job.frame_rate {
        if !valid_rate(r) {
            return Err(Error::Invalid(format!("frame rate '{r}' is not one DEE accepts")));
        }
    }
    let mut x = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<job_config>\n");
    // ---- input
    let (tag, extra) = match kind {
        InputKind::Wav => ("wav", String::new()),
        InputKind::Adm => ("adm", String::new()),
        InputKind::Damf => ("damf", String::new()),
        InputKind::AtmosMezz => ("atmos_mezz", String::new()),
    };
    let mut fname = el("file_name", file);
    if kind == InputKind::Adm || kind == InputKind::AtmosMezz {
        // these two take a list of files
        fname = el("file_name", file);
    }
    let rate = job.frame_rate.as_ref().map(|r| el("timecode_frame_rate", r)).unwrap_or_default();
    x.push_str(&format!("<input><audio><{tag} version=\"1\">{fname}{rate}{extra}{}</{tag}></audio></input>\n", storage(dir)));
    // ---- filter
    let mut common = String::new();
    if let Some(r) = &job.frame_rate {
        common.push_str(&el("timecode_frame_rate", r));
    }
    if let Some(s) = &job.start {
        common.push_str(&el("start", s));
    }
    if let Some(s) = &job.end {
        common.push_str(&el("end", s));
    }
    if let Some(s) = &job.prepend_silence {
        common.push_str(&el("prepend_silence_duration", s));
    }
    if let Some(s) = &job.append_silence {
        common.push_str(&el("append_silence_duration", s));
    }
    let filter = match &job.target {
        Target::TrueHd(o) => {
            if ![12u8, 14, 16].contains(&o.spatial_clusters) {
                return Err(Error::Invalid("spatial clusters must be 12, 14 or 16".into()));
            }
            let mut f = String::new();
            if let Some(m) = &job.metering {
                f.push_str(&format!("<loudness_measurement>{}</loudness_measurement>", el("metering_mode", m)));
            }
            f.push_str(&common);
            f.push_str(&format!(
                "<atmos_presentation>{}{}{}</atmos_presentation>",
                el("drc_profile", o.drc.tag()),
                el("spatial_clusters", &o.spatial_clusters.to_string()),
                el("legacy_authoring_compatibility", if o.legacy_authoring { "true" } else { "false" })
            ));
            for (name, p) in [("presentation_8ch", &o.presentation_8ch), ("presentation_6ch", &o.presentation_6ch)] {
                if let Some(p) = p {
                    f.push_str(&format!("<{name}>{}{}</{name}>", el("drc_profile", p.drc.tag()), el("surround_3db_attenuation", if p.surround_3db { "true" } else { "false" })));
                }
            }
            if let Some((drc, default_on, format)) = &o.presentation_2ch {
                if !["stereo", "dolby_surround_encoded", "dolby_headphone_encoded"].contains(&format.as_str()) {
                    return Err(Error::Invalid(format!("stereo format '{format}' is not one DEE accepts")));
                }
                f.push_str(&format!("<presentation_2ch>{}{}{}</presentation_2ch>", el("drc_profile", drc.tag()), el("drc_default_on", if *default_on { "true" } else { "false" }), el("format", format)));
            }
            if o.optimize_data_rate {
                f.push_str(&el("optimize_data_rate", "true"));
            }
            if let Some(d) = job.custom_dialnorm {
                f.push_str(&el("custom_dialnorm", &d.clamp(-31, 0).to_string()));
            }
            format!("<encode_to_dthd version=\"1\">{f}</encode_to_dthd>")
        }
        Target::Ddp(o) => {
            if ![384u32, 448, 576, 640, 768, 1024].contains(&o.data_rate_kbps) {
                return Err(Error::Invalid("Dolby Digital Plus Atmos data rate must be 384, 448, 576, 640, 768 or 1024 kbps".into()));
            }
            let mut f = String::new();
            if let Some(m) = &job.metering {
                f.push_str(&format!("<loudness><measure_only>{}</measure_only></loudness>", el("metering_mode", m)));
            }
            f.push_str(&el("data_rate", &o.data_rate_kbps.to_string()));
            f.push_str(&common);
            f.push_str(&format!("<drc>{}{}</drc>", el("line_mode_drc_profile", o.drc_line.tag()), el("rf_mode_drc_profile", o.drc_rf.tag())));
            f.push_str(&format!(
                "<downmix>{}{}{}{}{}</downmix>",
                el("loro_center_mix_level", &o.loro_center),
                el("loro_surround_mix_level", &o.loro_surround),
                el("ltrt_center_mix_level", &o.ltrt_center),
                el("ltrt_surround_mix_level", &o.ltrt_surround),
                el("preferred_downmix_mode", &o.preferred_downmix)
            ));
            if let Some(d) = job.custom_dialnorm {
                f.push_str(&el("custom_dialnorm", &d.clamp(-31, 0).to_string()));
            }
            // the three filters share one syntax; which one reads which input is DEE's rule
            match kind {
                InputKind::Wav => format!("<pcm_to_atmos_ddp version=\"3\">{f}</pcm_to_atmos_ddp>"),
                InputKind::AtmosMezz => format!("<atmos_mezz_to_atmos_ddp version=\"3\">{f}</atmos_mezz_to_atmos_ddp>"),
                InputKind::Adm | InputKind::Damf => format!("<encode_to_atmos_ddp version=\"1\">{f}</encode_to_atmos_ddp>"),
            }
        }
    };
    x.push_str(&format!("<filter><audio>{filter}</audio></filter>\n"));
    // ---- output
    let out_dir = job.output.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let out_file = job.output.file_name().and_then(|f| f.to_str()).ok_or_else(|| Error::Invalid("the output has no file name".into()))?;
    let out_tag = match job.target {
        Target::TrueHd(_) => "mlp",
        Target::Ddp(_) => "ec3",
    };
    x.push_str(&format!("<output><{out_tag} version=\"1\">{}{}</{out_tag}></output>\n", el("file_name", out_file), storage(out_dir)));
    x.push_str(&format!("<misc><temp_dir><clean_temp>true</clean_temp>{}</temp_dir></misc>\n</job_config>\n", el("path", &path_str(temp_dir))));
    Ok(x)
}

/// What a finished job leaves behind.
#[derive(Debug, Clone, Serialize)]
pub struct Finished {
    pub output: PathBuf,
    /// DEE's own report next to the output (`<output>.log`), if it wrote one.
    pub report: Option<PathBuf>,
    pub seconds: f64,
    /// Everything DEE said that is worth showing (warnings and errors).
    pub messages: Vec<String>,
}

/// Run the job. `progress` receives 0.0 to 1.0, `line` every log line worth showing; `cancel` stops the engine.
pub fn run(dee: &Path, job: &AtmosJob, work_dir: &Path, progress: &mut dyn FnMut(f32), line: &mut dyn FnMut(&str), cancel: &AtomicBool) -> Result<Finished, Error> {
    if !job.input.is_file() {
        return Err(Error::Invalid(format!("{} does not exist", job.input.display())));
    }
    std::fs::create_dir_all(work_dir)?;
    let temp = work_dir.join("temp");
    std::fs::create_dir_all(&temp)?;
    if let Some(d) = job.output.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(d)?;
    }
    let xml = profile_xml(job, &temp)?;
    let profile = work_dir.join("job.xml");
    std::fs::write(&profile, xml)?;
    let log = work_dir.join("dee.log");
    let mut cmd = Command::new(dee);
    cmd.current_dir(dee.parent().unwrap_or(Path::new(".")))
        .arg("--xml")
        .arg(&profile)
        .arg("--progress")
        .arg("--log-file")
        .arg(&log)
        .arg("--stdout")
        .arg("--verbose")
        .arg("info")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // no console window
    }
    let started = std::time::Instant::now();
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        for l in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(l).is_err() {
                break;
            }
        }
    });
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let mut messages: Vec<String> = Vec::new();
    let mut last = 0.0f32;
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Canceled);
        }
        match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(l) => {
                if let Some(p) = parse_progress(&l) {
                    if p > last {
                        last = p;
                        progress(p / 100.0);
                    }
                }
                if let Some(m) = relevant(&l) {
                    if l.contains("ERROR") || l.contains("WARNING") {
                        messages.push(m.clone());
                    }
                    line(&m);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Ok(Some(_)) = child.try_wait() {
                    // drain what is left
                    while let Ok(l) = rx.try_recv() {
                        if let Some(m) = relevant(&l) {
                            if l.contains("ERROR") || l.contains("WARNING") {
                                messages.push(m.clone());
                            }
                            line(&m);
                        }
                    }
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child.wait()?;
    let _ = reader.join();
    let err = err_thread.join().unwrap_or_default();
    if !err.trim().is_empty() {
        messages.push(err.trim().to_string());
    }
    if !status.success() {
        let errors: Vec<String> = messages.iter().filter(|m| m.contains("ERROR")).cloned().collect();
        return Err(Error::Failed { code: status.code().unwrap_or(-1), messages: if errors.is_empty() { messages.join("\n") } else { errors.join("\n") } });
    }
    progress(1.0);
    if !job.output.is_file() {
        return Err(Error::Failed { code: 0, messages: "DEE reported success but wrote no output file".into() });
    }
    let report = {
        let mut name = job.output.as_os_str().to_os_string();
        name.push(".log");
        let p = PathBuf::from(name);
        p.is_file().then_some(p)
    };
    Ok(Finished { output: job.output.clone(), report, seconds: started.elapsed().as_secs_f64(), messages })
}

/// "Overall progress: 37.5." in a framework line.
fn parse_progress(l: &str) -> Option<f32> {
    let i = l.find("Overall progress:")?;
    l[i + "Overall progress:".len()..].trim().trim_end_matches('.').parse().ok()
}

/// The lines worth showing: not the component loading chatter, not resource sampling, not the licence's
/// feature list (it names the licence, which is nobody else's business).
fn relevant(l: &str) -> Option<String> {
    let t = l.trim();
    if t.is_empty() || t.contains("INTERNAL_INFO") || t.contains("CPU:") || t.contains("Features enabled by the license") || t.contains("Max MEM") || t.contains("Average CPU") || t.contains("Average MEM") {
        return None;
    }
    Some(t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ddp_job(kind: InputKind) -> AtmosJob {
        let mut j = AtmosJob::new("D:/masters/film.atmos", "D:/out/film.ec3", Target::Ddp(DdpOptions::default()));
        j.kind = Some(kind);
        j
    }

    #[test]
    fn ddp_profile_names_the_right_filter_per_input() {
        let temp = Path::new("D:/tmp");
        let wav = profile_xml(&ddp_job(InputKind::Wav), temp).unwrap();
        assert!(wav.contains("<pcm_to_atmos_ddp version=\"3\">") && wav.contains("<wav version=\"1\">") && wav.contains("<ec3 version=\"1\">"));
        assert!(wav.contains("<data_rate>768</data_rate>"));
        let damf = profile_xml(&ddp_job(InputKind::Damf), temp).unwrap();
        assert!(damf.contains("<encode_to_atmos_ddp version=\"1\">") && damf.contains("<damf version=\"1\">"));
        let mezz = profile_xml(&ddp_job(InputKind::AtmosMezz), temp).unwrap();
        assert!(mezz.contains("<atmos_mezz_to_atmos_ddp version=\"3\">"));
    }

    #[test]
    fn truehd_profile_carries_the_presentations() {
        let mut o = TrueHdOptions::default();
        o.spatial_clusters = 16;
        o.presentation_6ch = Some(Presentation { drc: Drc::Speech, surround_3db: false });
        o.presentation_2ch = Some((Drc::FilmStandard, true, "dolby_surround_encoded".into()));
        let mut j = AtmosJob::new("D:/m/film.atmos", "D:/o/film.thd", Target::TrueHd(o));
        j.kind = Some(InputKind::Damf);
        j.custom_dialnorm = Some(-27);
        j.metering = Some("1770-4".into());
        let x = profile_xml(&j, Path::new("D:/tmp")).unwrap();
        for want in ["<encode_to_dthd version=\"1\">", "<spatial_clusters>16</spatial_clusters>", "<presentation_6ch><drc_profile>speech</drc_profile><surround_3db_attenuation>false</surround_3db_attenuation>", "<format>dolby_surround_encoded</format>", "<custom_dialnorm>-27</custom_dialnorm>", "<mlp version=\"1\"><file_name>film.thd</file_name>", "<metering_mode>1770-4</metering_mode>"] {
            assert!(x.contains(want), "missing {want} in {x}");
        }
    }

    #[test]
    fn bad_settings_are_refused_before_dee_runs() {
        let mut j = ddp_job(InputKind::Wav);
        if let Target::Ddp(o) = &mut j.target {
            o.data_rate_kbps = 500;
        }
        assert!(profile_xml(&j, Path::new("t")).is_err());
        let mut t = AtmosJob::new("a.wav", "b.thd", Target::TrueHd(TrueHdOptions::default()));
        t.kind = Some(InputKind::Wav);
        assert!(profile_xml(&t, Path::new("t")).is_err(), "TrueHD Atmos from a plain WAV");
    }

    #[test]
    fn progress_and_noise_lines() {
        assert_eq!(parse_progress("[2026] INFO: framework: Stage: audio filter, Stage progress: 50.0, Overall progress: 37.5."), Some(37.5));
        assert!(relevant("[x] INFO: Features enabled by the license:  something").is_none());
        assert!(relevant("[x] INTERNAL_INFO: Loading library").is_none());
        assert!(relevant("[x] ERROR: something broke").is_some());
    }

    #[test]
    fn paths_with_spaces_and_markup_are_escaped() {
        let mut j = AtmosJob::new("D:/My Masters/a&b.atmos", "D:/o/<x>.ec3", Target::Ddp(DdpOptions::default()));
        j.kind = Some(InputKind::Damf);
        let x = profile_xml(&j, Path::new("D:/t")).unwrap();
        assert!(x.contains("a&amp;b.atmos") && x.contains("&lt;x&gt;.ec3") && x.contains("D:/My Masters"));
    }
}
