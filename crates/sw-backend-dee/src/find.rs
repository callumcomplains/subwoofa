//! Finding `dee.exe`.

use std::path::{Path, PathBuf};

/// Where DEE is looked for when nothing is configured.
pub fn find_dee(configured: Option<&Path>) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = configured {
        candidates.push(if p.is_dir() {
            p.join(exe_name())
        } else {
            p.to_path_buf()
        });
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

pub(crate) fn exe_name() -> &'static str {
    if cfg!(windows) {
        "dee.exe"
    } else {
        "dee"
    }
}
