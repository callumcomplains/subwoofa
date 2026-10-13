//! What the engine's validator says about a master.

use crate::Error;
use std::path::Path;
use std::process::Command;

/// What DEE's `atmos_info` tool says about an Atmos master (beds, objects, length, whether it passes validation).
pub fn master_info(dee: &Path, file: &Path) -> Result<String, Error> {
    let tool = dee.with_file_name(if cfg!(windows) { "atmos_info.exe" } else { "atmos_info" });
    if !tool.is_file() {
        return Err(Error::Invalid("atmos_info was not found next to dee".into()));
    }
    let mut cmd = Command::new(&tool);
    cmd.current_dir(dee.parent().unwrap_or(Path::new(".")))
        .arg("--input")
        .arg(std::path::absolute(file)?);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
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
