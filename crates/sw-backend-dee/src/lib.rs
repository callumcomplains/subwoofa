//! Dolby Encoding Engine (DEE) backend: Dolby Atmos in TrueHD and Dolby Digital Plus.
//!
//! Subwoofa has no Atmos encoder of its own (the object coding is not public), so Atmos goes through a copy of
//! Dolby's own command-line encoder that the user supplies and is licensed to use. Nothing of DEE is shipped
//! or copied: this crate only writes DEE's job file (an XML profile), runs `dee.exe` and reads what it prints.
//!
//! * **TrueHD Atmos** (`encode_to_dthd`) and **Dolby Digital Plus Atmos** (`encode_to_atmos_ddp`,
//!   `atmos_mezz_to_atmos_ddp`) from an Atmos master written by Dolby's tools (DAMF `.atmos`, ADM BWF `.wav`).
//!   DEE refuses masters that were not authored with Dolby tools, and so does Subwoofa.
//! * **Everyday audio jobs** (`task`): loudness measured with dialogue detection, Dolby Digital and Digital Plus encoding
//!   and decoding, transcoding, trimming and joining E-AC-3, loudness correction and channel reordering.
//! * **Dolby Digital Plus Atmos from channel audio** (`pcm_to_atmos_ddp`): a multichannel WAV bed (up to 7.1.4).

mod alias;
mod find;
mod info;
mod input;
mod job;
mod profile;
mod run;
mod task;
mod taskrun;

pub use find::find_dee;
pub use info::master_info;
pub use input::InputKind;
pub use job::{AtmosJob, DdpOptions, Drc, Presentation, Target, TrueHdOptions};
pub use profile::profile_xml;
pub use run::{run, Finished};
pub use task::{clock, profile, EncodeOptions, Loudness, Measured, Source, Task, TaskJob};
pub use taskrun::{run_task, work_folder, TaskDone};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alias::{has_space, make_alias, needs_alias};
    use crate::run::{parse_progress, relevant};
    use std::path::Path;

    fn ddp_job(kind: InputKind) -> AtmosJob {
        let mut j = AtmosJob::new(
            "D:/masters/film.atmos",
            "D:/out/film.ec3",
            Target::Ddp(DdpOptions::default()),
        );
        j.kind = Some(kind);
        j
    }

    #[test]
    fn ddp_profile_names_the_right_filter_per_input() {
        let temp = Path::new("D:/tmp");
        let wav = profile_xml(&ddp_job(InputKind::Wav), temp).unwrap();
        assert!(
            wav.contains("<pcm_to_atmos_ddp version=\"3\">")
                && wav.contains("<wav version=\"1\">")
                && wav.contains("<ec3 version=\"1\">")
        );
        assert!(wav.contains("<data_rate>768</data_rate>"));
        // masters of every kind go in as Atmos mezzanines
        for kind in [InputKind::Damf, InputKind::Adm, InputKind::AtmosMezz] {
            let x = profile_xml(&ddp_job(kind), temp).unwrap();
            assert!(
                x.contains("<atmos_mezz_to_atmos_ddp version=\"3\">") && x.contains("<atmos_mezz version=\"1\">"),
                "{kind:?}"
            );
        }
        let c = AtmosJob::new("D:/m/a.atmos", "D:/o/a.wav", Target::Convert { format: "adm".into() });
        let x = profile_xml(&c, temp).unwrap();
        assert!(
            x.contains("<convert_atmos_mezz version=\"2\"><target_format>adm</target_format>")
                && x.contains("<atmos_mezz version=\"1\"><file_name>a.wav"),
            "{x}"
        );
    }

    #[test]
    fn truehd_profile_carries_the_presentations() {
        let mut o = TrueHdOptions::default();
        o.spatial_clusters = 16;
        o.presentation_6ch = Some(Presentation {
            drc: Drc::Speech,
            surround_3db: false,
        });
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
        assert!(
            profile_xml(&t, Path::new("t")).is_err(),
            "TrueHD Atmos from a plain WAV"
        );
    }

    #[test]
    fn progress_and_noise_lines() {
        assert_eq!(
            parse_progress(
                "[2026] INFO: framework: Stage: audio filter, Stage progress: 50.0, Overall progress: 37.5."
            ),
            Some(37.5)
        );
        assert!(relevant("[x] INFO: Features enabled by the license:  something").is_none());
        assert!(relevant("[x] INTERNAL_INFO: Loading library").is_none());
        assert!(relevant("[x] ERROR: something broke").is_some());
    }

    #[test]
    fn a_master_with_spaces_gets_a_space_free_alias() {
        let dir = std::env::temp_dir().join(format!("sw_dee_alias_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("out")).unwrap();
        let master = dir.join("My Film - Master.atmos");
        std::fs::write(&master, b"a").unwrap();
        std::fs::write(dir.join("My Film - Master.atmos.metadata"), b"m").unwrap();
        std::fs::write(dir.join("My Film - Master.atmos.audio"), b"x").unwrap();
        assert!(needs_alias(&master));
        let a = make_alias(&master, &dir.join("out").join("o.thd"), &dir).unwrap();
        assert!(
            !has_space(&a.input),
            "the alias folder must have no spaces: {}",
            a.input.display()
        );
        let name = a.input.file_name().unwrap().to_str().unwrap().to_string();
        assert!(!name.contains(' ') && name.ends_with(".atmos"));
        assert_eq!(std::fs::read(&a.input).unwrap(), b"a");
        assert_eq!(std::fs::read(format!("{}.metadata", a.input.display())).unwrap(), b"m");
        assert_eq!(std::fs::read(format!("{}.audio", a.input.display())).unwrap(), b"x");
        a.remove();
        assert!(!a.input.exists());
        assert!(master.exists(), "the master itself is untouched");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn paths_with_spaces_and_markup_are_escaped() {
        let mut j = AtmosJob::new(
            "D:/My Masters/a&b.atmos",
            "D:/o/<x>.ec3",
            Target::Ddp(DdpOptions::default()),
        );
        j.kind = Some(InputKind::Damf);
        let x = profile_xml(&j, Path::new("D:/t")).unwrap();
        assert!(x.contains("a&amp;b.atmos") && x.contains("&lt;x&gt;.ec3") && x.contains("D:/My Masters"));
    }
}
