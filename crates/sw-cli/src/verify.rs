//! `subwoofa verify`: checking stream and audio files.

use crate::args::VerifyArgs;

pub(crate) fn verify_cmd(args: VerifyArgs) -> Result<(), String> {
    let VerifyArgs {
        files,
        deep,
        json,
        ffmpeg,
        report,
    } = args;
    if report.is_some() && files.len() != 1 {
        return Err("--report writes one page: give one file".into());
    }
    let opt = sw_verify::Options {
        // a QC report holds the decoded audio's loudness, so it needs the decode
        deep: deep || report.is_some(),
        analyse: report.is_some(),
        ffmpeg: ffmpeg.or_else(sw_backend_ffmpeg::find_ffmpeg),
        reference_verifier: None,
    };
    let mut reports = Vec::new();
    for file in &files {
        match sw_verify::verify(file, &opt) {
            Ok(r) => reports.push(r),
            Err(e) => {
                // a file that cannot even be opened is reported like a failed check, and the others still run
                eprintln!("{}: {e}", file.display());
                reports.push(sw_verify::Report {
                    file: file.display().to_string(),
                    format: "unreadable".into(),
                    ..Default::default()
                });
            }
        }
    }
    if json {
        // one file: the report itself, as before; several: a list
        let text = if reports.len() == 1 {
            serde_json::to_string_pretty(&reports[0])
        } else {
            serde_json::to_string_pretty(&reports)
        };
        println!("{}", text.map_err(|e| e.to_string())?);
    } else {
        for r in &reports {
            print!("{}", r.to_text());
        }
        if reports.len() > 1 {
            println!("\nSummary of {} files", reports.len());
            for r in &reports {
                let failed = r.checks.iter().filter(|c| c.status == sw_verify::Status::Fail).count();
                let warned = r.checks.iter().filter(|c| c.status == sw_verify::Status::Warn).count();
                let verdict = if r.passed {
                    "passed".to_string()
                } else if failed > 0 {
                    format!("FAILED ({failed} check{} failed)", if failed == 1 { "" } else { "s" })
                } else {
                    "FAILED".to_string()
                };
                println!(
                    "  {verdict:<26} {}{}",
                    r.file,
                    if warned > 0 {
                        format!("  [{warned} warning{}]", if warned == 1 { "" } else { "s" })
                    } else {
                        String::new()
                    }
                );
            }
        }
    }
    if let Some(path) = report {
        std::fs::write(&path, reports[0].to_html()).map_err(|e| format!("{}: {e}", path.display()))?;
        eprintln!("QC report written to {}", path.display());
    }
    let bad = reports.iter().filter(|r| !r.passed).count();
    if bad == 0 {
        Ok(())
    } else if reports.len() == 1 {
        Err("verification failed".into())
    } else {
        Err(format!("{bad} of {} files failed verification", reports.len()))
    }
}
