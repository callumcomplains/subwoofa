//! The XML profile that tells the engine what to do.

use crate::input::InputKind;
use crate::job::{AtmosJob, Target};
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
    let dir = job
        .input
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let file = job
        .input
        .file_name()
        .and_then(|f| f.to_str())
        .ok_or_else(|| Error::Invalid("the input has no file name".into()))?;
    let storage = |d: &Path| format!("<storage><local>{}</local></storage>", el("path", &path_str(d)));
    if let Some(r) = &job.frame_rate {
        if !valid_rate(r) {
            return Err(Error::Invalid(format!("frame rate '{r}' is not one DEE accepts")));
        }
    }
    let mut x = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<job_config>\n");
    // ---- input
    // every Dolby-authored master (ADM BWF, DAMF, IAB) goes in as an Atmos mezzanine, which is the input type
    // the Atmos encoders and the converter connect to; only a plain channel bed is a WAV
    let (tag, extra) = match kind {
        InputKind::Wav => ("wav", String::new()),
        InputKind::Adm | InputKind::Damf | InputKind::AtmosMezz => ("atmos_mezz", String::new()),
    };
    let fname = el("file_name", file);
    let rate = job
        .frame_rate
        .as_ref()
        .map(|r| el("timecode_frame_rate", r))
        .unwrap_or_default();
    x.push_str(&format!(
        "<input><audio><{tag} version=\"1\">{fname}{rate}{extra}{}</{tag}></audio></input>\n",
        storage(dir)
    ));
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
        Target::Convert { format } => {
            if !["adm", "damf", "mxf_iab"].contains(&format.as_str()) {
                return Err(Error::Invalid(format!(
                    "conversion target '{format}': use adm, damf or mxf_iab"
                )));
            }
            if kind == InputKind::Wav {
                return Err(Error::Invalid("a plain channel WAV is not an Atmos master".into()));
            }
            let mut f = String::new();
            f.push_str(&el("target_format", format));
            f.push_str(&common);
            format!("<convert_atmos_mezz version=\"2\">{f}</convert_atmos_mezz>")
        }
        Target::TrueHd(o) => {
            if ![12u8, 14, 16].contains(&o.spatial_clusters) {
                return Err(Error::Invalid("spatial clusters must be 12, 14 or 16".into()));
            }
            let mut f = String::new();
            if let Some(m) = &job.metering {
                f.push_str(&format!(
                    "<loudness_measurement>{}</loudness_measurement>",
                    el("metering_mode", m)
                ));
            }
            f.push_str(&common);
            f.push_str(&format!(
                "<atmos_presentation>{}{}{}</atmos_presentation>",
                el("drc_profile", o.drc.tag()),
                el("spatial_clusters", &o.spatial_clusters.to_string()),
                el(
                    "legacy_authoring_compatibility",
                    if o.legacy_authoring { "true" } else { "false" }
                )
            ));
            for (name, p) in [
                ("presentation_8ch", &o.presentation_8ch),
                ("presentation_6ch", &o.presentation_6ch),
            ] {
                if let Some(p) = p {
                    f.push_str(&format!(
                        "<{name}>{}{}</{name}>",
                        el("drc_profile", p.drc.tag()),
                        el(
                            "surround_3db_attenuation",
                            if p.surround_3db { "true" } else { "false" }
                        )
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
                    el("drc_default_on", if *default_on { "true" } else { "false" }),
                    el("format", format)
                ));
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
                return Err(Error::Invalid(
                    "Dolby Digital Plus Atmos data rate must be 384, 448, 576, 640, 768 or 1024 kbps".into(),
                ));
            }
            let mut f = String::new();
            if let Some(m) = &job.metering {
                f.push_str(&format!(
                    "<loudness><measure_only>{}</measure_only></loudness>",
                    el("metering_mode", m)
                ));
            }
            f.push_str(&el("data_rate", &o.data_rate_kbps.to_string()));
            f.push_str(&common);
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
            if let Some(d) = job.custom_dialnorm {
                f.push_str(&el("custom_dialnorm", &d.clamp(-31, 0).to_string()));
            }
            match kind {
                InputKind::Wav => format!("<pcm_to_atmos_ddp version=\"3\">{f}</pcm_to_atmos_ddp>"),
                _ => format!("<atmos_mezz_to_atmos_ddp version=\"3\">{f}</atmos_mezz_to_atmos_ddp>"),
            }
        }
    };
    x.push_str(&format!("<filter><audio>{filter}</audio></filter>\n"));
    // ---- output
    let out_dir = job
        .output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let out_file = job
        .output
        .file_name()
        .and_then(|f| f.to_str())
        .ok_or_else(|| Error::Invalid("the output has no file name".into()))?;
    let out_tag = match job.target {
        Target::TrueHd(_) => "mlp",
        Target::Ddp(_) => "ec3",
        Target::Convert { .. } => "atmos_mezz",
    };
    x.push_str(&format!(
        "<output><{out_tag} version=\"1\">{}{}</{out_tag}></output>\n",
        el("file_name", out_file),
        storage(out_dir)
    ));
    x.push_str(&format!(
        "<misc><temp_dir><clean_temp>true</clean_temp>{}</temp_dir></misc>\n</job_config>\n",
        el("path", &path_str(temp_dir))
    ));
    Ok(x)
}
