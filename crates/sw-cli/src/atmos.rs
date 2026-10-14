//! `subwoofa atmos`: driving the Dolby Encoding Engine.

use crate::args::{AtmosCmd, AtmosEncodeArgs};
use std::path::PathBuf;

pub(crate) fn run_atmos(op: AtmosCmd) -> Result<(), String> {
    use sw_backend_dee as dee;
    let find = |p: Option<PathBuf>| dee::find_dee(p.as_deref()).ok_or_else(|| dee::Error::NotFound.to_string());
    match op {
        AtmosCmd::Info { file, dee: path } => {
            let d = find(path)?;
            println!("{}", dee::master_info(&d, &file).map_err(|e| e.to_string())?);
        }
        AtmosCmd::Convert {
            input,
            output,
            format,
            dee: path,
        } => {
            let d = find(path)?;
            let job = dee::AtmosJob::new(&input, &output, dee::Target::Convert { format });
            let work = std::env::temp_dir().join(format!("subwoofa_dee_{}", std::process::id()));
            let done = dee::run(
                &d,
                &job,
                &work,
                &mut |_| {},
                &mut |_| {},
                &std::sync::atomic::AtomicBool::new(false),
            )
            .map_err(|e| e.to_string())?;
            let _ = std::fs::remove_dir_all(&work);
            println!("wrote {} in {:.1} s", done.output.display(), done.seconds);
        }
        AtmosCmd::Encode(args) => atmos_encode(&find, args)?,
    }
    Ok(())
}

/// Encode an Atmos master with the Dolby Encoding Engine, showing a percentage on stderr.
fn atmos_encode(
    find: &dyn Fn(Option<PathBuf>) -> Result<PathBuf, String>,
    args: AtmosEncodeArgs,
) -> Result<(), String> {
    use sw_backend_dee as dee;
    let AtmosEncodeArgs {
        input,
        format,
        output,
        bitrate,
        clusters,
        drc,
        dialnorm,
        frame_rate,
        start,
        end,
        presentation_8ch,
        presentation_6ch,
        metering,
        no_dialogue,
        speech_threshold,
        surround_trim,
        height_trim,
        dee: path,
    } = args;
    let d = find(path)?;
    let drc: dee::Drc = drc.parse()?;
    let target = match format.as_str() {
        "truehd" | "thd" => {
            let pres = |on: bool| {
                on.then_some(dee::Presentation {
                    drc,
                    surround_3db: true,
                })
            };
            dee::Target::TrueHd(dee::TrueHdOptions {
                drc,
                spatial_clusters: clusters,
                presentation_8ch: pres(presentation_8ch),
                presentation_6ch: pres(presentation_6ch),
                ..Default::default()
            })
        }
        "ddp" | "ec3" | "eac3" => dee::Target::Ddp(dee::DdpOptions {
            data_rate_kbps: bitrate,
            drc_line: drc,
            drc_rf: drc,
            surround_trim_5_1: surround_trim,
            height_trim_5_1: height_trim,
            ..Default::default()
        }),
        other => return Err(format!("unknown format '{other}': use truehd or ddp")),
    };
    let mut job = dee::AtmosJob::new(&input, &output, target);
    job.custom_dialnorm = dialnorm;
    job.frame_rate = frame_rate;
    job.start = start;
    job.end = end;
    job.metering = metering;
    job.dialogue_intelligence = no_dialogue.then_some(false);
    job.speech_threshold = speech_threshold;
    let work = std::env::temp_dir().join(format!("subwoofa_dee_{}", std::process::id()));
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut shown = -1i32;
    let done = dee::run(
        &d,
        &job,
        &work,
        &mut |p| {
            let pct = (p * 100.0) as i32;
            if pct != shown {
                shown = pct;
                eprint!("\r{pct:3}%");
            }
        },
        &mut |_| {},
        &cancel,
    )
    .map_err(|e| e.to_string())?;
    eprintln!();
    let _ = std::fs::remove_dir_all(&work);
    println!("wrote {} in {:.1} s", done.output.display(), done.seconds);
    for m in done.messages.iter().filter(|m| m.contains("WARNING")) {
        println!("{m}");
    }
    Ok(())
}
