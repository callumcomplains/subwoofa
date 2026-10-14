//! The XML profile that tells the engine what to do.

use crate::input::InputKind;
use crate::job::{AtmosJob, DdpOptions, Target, TrueHdOptions};
use crate::Error;
use std::path::Path;

pub(crate) fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub(crate) fn path_str(p: &Path) -> String {
    // DEE takes Windows paths with either separator; forward slashes avoid every escaping question
    p.to_string_lossy().replace('\\', "/")
}

pub(crate) fn el(name: &str, value: &str) -> String {
    format!("<{name}>{}</{name}>", esc(value))
}

pub(crate) fn valid_rate(v: &str) -> bool {
    [
        "23.976",
        "24",
        "25",
        "29.97",
        "30",
        "48",
        "50",
        "59.94",
        "60",
        "not_indicated",
    ]
    .contains(&v)
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
    if let Some(r) = &job.frame_rate {
        if !valid_rate(r) {
            return Err(Error::Invalid(format!("frame rate '{r}' is not one DEE accepts")));
        }
    }
    let mut x = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<job_config>\n");
    x.push_str(&input_xml(job, kind)?);
    let filter = match &job.target {
        Target::Convert { format } => convert_filter(job, kind, format)?,
        Target::TrueHd(o) => truehd_filter(job, o)?,
        Target::Ddp(o) => ddp_filter(job, kind, o)?,
    };
    x.push_str(&format!("<filter><audio>{filter}</audio></filter>\n"));
    x.push_str(&output_xml(job)?);
    x.push_str(&format!(
        "<misc><temp_dir><clean_temp>true</clean_temp>{}</temp_dir></misc>\n</job_config>\n",
        el("path", &path_str(temp_dir))
    ));
    Ok(x)
}

/// The folder a path is in; the current folder for a bare file name.
fn parent_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

/// A folder as the engine's storage description.
fn storage(dir: &Path) -> String {
    format!("<storage><local>{}</local></storage>", el("path", &path_str(dir)))
}

fn input_xml(job: &AtmosJob, kind: InputKind) -> Result<String, Error> {
    let file = job
        .input
        .file_name()
        .and_then(|f| f.to_str())
        .ok_or_else(|| Error::Invalid("the input has no file name".into()))?;
    // every Dolby-authored master (ADM BWF, DAMF, IAB) goes in as an Atmos mezzanine, which is the input type
    // the Atmos encoders and the converter connect to; only a plain channel bed is a WAV
    let tag = match kind {
        InputKind::Wav => "wav",
        InputKind::Adm | InputKind::Damf | InputKind::AtmosMezz => "atmos_mezz",
    };
    let rate = job
        .frame_rate
        .as_ref()
        .map(|r| el("timecode_frame_rate", r))
        .unwrap_or_default();
    Ok(format!(
        "<input><audio><{tag} version=\"1\">{}{rate}{}</{tag}></audio></input>\n",
        el("file_name", file),
        storage(parent_dir(&job.input))
    ))
}

fn output_xml(job: &AtmosJob) -> Result<String, Error> {
    let file = job
        .output
        .file_name()
        .and_then(|f| f.to_str())
        .ok_or_else(|| Error::Invalid("the output has no file name".into()))?;
    let tag = match job.target {
        Target::TrueHd(_) => "mlp",
        Target::Ddp(_) => "ec3",
        Target::Convert { .. } => "atmos_mezz",
    };
    Ok(format!(
        "<output><{tag} version=\"1\">{}{}</{tag}></output>\n",
        el("file_name", file),
        storage(parent_dir(&job.output))
    ))
}

/// Fields every filter takes: timecode rate, the span to encode and silence to add.
fn common_fields(job: &AtmosJob) -> String {
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
    common
}

/// The metering mode with the dialogue settings that go with it, as the engine's `measure_only` fields.
fn metering_fields(job: &AtmosJob, mode: &str) -> Result<String, Error> {
    if let Some(t) = job.speech_threshold {
        if t > 100 {
            return Err(Error::Invalid("the speech threshold is 0 to 100".into()));
        }
    }
    let mut f = el("metering_mode", mode);
    if let Some(d) = job.dialogue_intelligence {
        f.push_str(&el("dialogue_intelligence", on_off(d)));
    }
    if let Some(t) = job.speech_threshold {
        f.push_str(&el("speech_threshold", &t.to_string()));
    }
    Ok(f)
}

fn on_off(flag: bool) -> &'static str {
    if flag {
        "true"
    } else {
        "false"
    }
}

fn convert_filter(job: &AtmosJob, kind: InputKind, format: &str) -> Result<String, Error> {
    if !["adm", "damf", "mxf_iab"].contains(&format) {
        return Err(Error::Invalid(format!(
            "conversion target '{format}': use adm, damf or mxf_iab"
        )));
    }
    if kind == InputKind::Wav {
        return Err(Error::Invalid("a plain channel WAV is not an Atmos master".into()));
    }
    let f = format!("{}{}", el("target_format", format), common_fields(job));
    Ok(format!("<convert_atmos_mezz version=\"2\">{f}</convert_atmos_mezz>"))
}

fn truehd_filter(job: &AtmosJob, o: &TrueHdOptions) -> Result<String, Error> {
    if ![12u8, 14, 16].contains(&o.spatial_clusters) {
        return Err(Error::Invalid("spatial clusters must be 12, 14 or 16".into()));
    }
    let mut f = String::new();
    if let Some(m) = &job.metering {
        f.push_str(&format!(
            "<loudness_measurement>{}</loudness_measurement>",
            metering_fields(job, m)?
        ));
    }
    f.push_str(&common_fields(job));
    f.push_str(&format!(
        "<atmos_presentation>{}{}{}</atmos_presentation>",
        el("drc_profile", o.drc.tag()),
        el("spatial_clusters", &o.spatial_clusters.to_string()),
        el("legacy_authoring_compatibility", on_off(o.legacy_authoring))
    ));
    for (name, p) in [
        ("presentation_8ch", &o.presentation_8ch),
        ("presentation_6ch", &o.presentation_6ch),
    ] {
        if let Some(p) = p {
            f.push_str(&format!(
                "<{name}>{}{}</{name}>",
                el("drc_profile", p.drc.tag()),
                el("surround_3db_attenuation", on_off(p.surround_3db))
            ));
        }
    }
    if let Some((drc, default_on, format)) = &o.presentation_2ch {
        if !["stereo", "dolby_surround_encoded", "dolby_headphone_encoded"].contains(&format.as_str()) {
            return Err(Error::Invalid(format!(
                "stereo format '{format}' is not one DEE accepts"
            )));
        }
        f.push_str(&format!(
            "<presentation_2ch>{}{}{}</presentation_2ch>",
            el("drc_profile", drc.tag()),
            el("drc_default_on", on_off(*default_on)),
            el("format", format)
        ));
    }
    if o.optimize_data_rate {
        f.push_str(&el("optimize_data_rate", "true"));
    }
    if let Some(d) = job.custom_dialnorm {
        f.push_str(&el("custom_dialnorm", &d.clamp(-31, 0).to_string()));
    }
    Ok(format!("<encode_to_dthd version=\"1\">{f}</encode_to_dthd>"))
}

fn ddp_filter(job: &AtmosJob, kind: InputKind, o: &DdpOptions) -> Result<String, Error> {
    if ![384u32, 448, 576, 640, 768, 1024].contains(&o.data_rate_kbps) {
        return Err(Error::Invalid(
            "Dolby Digital Plus Atmos data rate must be 384, 448, 576, 640, 768 or 1024 kbps".into(),
        ));
    }
    let mut f = String::new();
    if let Some(m) = &job.metering {
        f.push_str(&format!(
            "<loudness><measure_only>{}</measure_only></loudness>",
            metering_fields(job, m)?
        ));
    }
    f.push_str(&el("data_rate", &o.data_rate_kbps.to_string()));
    f.push_str(&common_fields(job));
    f.push_str(&format!(
        "<drc>{}{}</drc>",
        el("line_mode_drc_profile", o.drc_line.tag()),
        el("rf_mode_drc_profile", o.drc_rf.tag())
    ));
    f.push_str(&format!(
        "<downmix>{}{}{}{}{}</downmix>",
        el("loro_center_mix_level", &o.loro_center),
        el("loro_surround_mix_level", &o.loro_surround),
        el("ltrt_center_mix_level", &o.ltrt_center),
        el("ltrt_surround_mix_level", &o.ltrt_surround),
        el("preferred_downmix_mode", &o.preferred_downmix)
    ));
    if o.surround_trim_5_1 != "auto" || o.height_trim_5_1 != "auto" {
        let pick = |what: &str, v: &str, allowed: &[&str]| -> Result<String, Error> {
            if allowed.contains(&v) {
                Ok(v.to_string())
            } else {
                Err(Error::Invalid(format!("{what} '{v}': use {}", allowed.join(", "))))
            }
        };
        f.push_str(&format!(
            "<custom_trims>{}{}</custom_trims>",
            el(
                "surround_trim_5_1",
                &pick("surround trim", &o.surround_trim_5_1, &["0", "-3", "-6", "-9", "auto"])?
            ),
            el(
                "height_trim_5_1",
                &pick("height trim", &o.height_trim_5_1, &["-3", "-6", "-9", "-12", "auto"])?
            )
        ));
    }
    if let Some(d) = job.custom_dialnorm {
        f.push_str(&el("custom_dialnorm", &d.clamp(-31, 0).to_string()));
    }
    Ok(match kind {
        InputKind::Wav => format!("<pcm_to_atmos_ddp version=\"3\">{f}</pcm_to_atmos_ddp>"),
        _ => format!("<atmos_mezz_to_atmos_ddp version=\"3\">{f}</atmos_mezz_to_atmos_ddp>"),
    })
}
