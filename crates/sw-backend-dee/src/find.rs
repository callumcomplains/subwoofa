//! Finding `dee.exe`.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// The engine the user chose in the app; `find_dee` looks there when it is given nothing else.
static CHOSEN: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Remembers the engine chosen in the app (`None` goes back to the usual places).
pub fn set_dee_override(path: Option<PathBuf>) {
    *CHOSEN.write().unwrap() = path;
}

/// Where DEE is looked for when nothing is configured.
pub fn find_dee(configured: Option<&Path>) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    let chosen = CHOSEN.read().unwrap().clone();
    if let Some(p) = configured.map(Path::to_path_buf).or(chosen) {
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
