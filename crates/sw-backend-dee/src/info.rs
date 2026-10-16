//! What the engine's validator says about a master.

use crate::Error;
use std::path::Path;
use sw_core::process::command;

/// What DEE's `atmos_info` tool says about an Atmos master (beds, objects, length, whether it passes validation).
pub fn master_info(dee: &Path, file: &Path) -> Result<String, Error> {
    let tool = dee.with_file_name(if cfg!(windows) { "atmos_info.exe" } else { "atmos_info" });
    if !tool.is_file() {
        return Err(Error::Invalid("atmos_info was not found next to dee".into()));
    }
    let mut cmd = command(&tool);
    cmd.current_dir(dee.parent().unwrap_or(Path::new(".")))
        .arg("--input")
        .arg(std::path::absolute(file)?);
    let out = cmd.output()?;
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok(text
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with("AtmosInfo Tool"))
        .collect::<Vec<_>>()
        .join(
            "
",
        ))
}
