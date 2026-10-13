//! The engine cannot open paths with spaces: such files are reached through a short alias in a work folder.

use crate::Error;
use std::path::{Path, PathBuf};

/// DEE reads a master's file name and folder as lists separated by white space, so a name or a folder with spaces
/// is taken for several files and the encoder refuses to connect or cannot find the folder. Such a master is given
/// an alias for the job in a folder whose path has no spaces: a hard link (no copy) beside the output or the master,
/// else a copy in the work folder. A DAMF master keeps its companion files (`.metadata`, `.audio`) under the same
/// alias.
pub(crate) struct Alias {
    pub(crate) input: PathBuf,
    pub(crate) made: Vec<PathBuf>,
}

impl Alias {
    pub(crate) fn remove(&self) {
        for p in &self.made {
            let _ = std::fs::remove_file(p);
        }
    }
}

pub(crate) fn has_space(path: &Path) -> bool {
    path.to_string_lossy().chars().any(char::is_whitespace)
}

pub(crate) fn needs_alias(path: &Path) -> bool {
    has_space(path)
}

pub(crate) fn make_alias(input: &Path, output: &Path, work: &Path) -> Result<Alias, Error> {
    let name = input.file_name().and_then(|f| f.to_str()).unwrap_or("master");
    let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("wav");
    let stem = format!("subwoofa_in_{}", std::process::id());
    let alias_name = format!("{stem}.{ext}");
    // companions of a DAMF master: <name>.metadata and <name>.audio
    let mut sources: Vec<(PathBuf, String)> = vec![(input.to_path_buf(), alias_name.clone())];
    if ext.eq_ignore_ascii_case("atmos") {
        for suffix in ["metadata", "audio"] {
            let c = input.with_file_name(format!("{name}.{suffix}"));
            if c.is_file() {
                sources.push((c, format!("{alias_name}.{suffix}")));
            }
        }
    }
    // the output's folder first: it is writable, and usually on the master's drive
    let dirs: Vec<PathBuf> = [output.parent(), input.parent()]
        .into_iter()
        .flatten()
        .filter(|d| !has_space(d))
        .map(Path::to_path_buf)
        .collect();
    'dirs: for dir in &dirs {
        let mut made: Vec<PathBuf> = Vec::new();
        for (src, target) in &sources {
            let dst = dir.join(target);
            let _ = std::fs::remove_file(&dst);
            if std::fs::hard_link(src, &dst).is_err() {
                for m in &made {
                    let _ = std::fs::remove_file(m);
                }
                continue 'dirs;
            }
            made.push(dst);
        }
        return Ok(Alias {
            input: dir.join(&alias_name),
            made,
        });
    }
    // no hard link possible (another volume, or a file system without them): a copy in the work folder
    if has_space(work) {
        return Err(Error::Invalid(format!(
            "DEE cannot read a master whose path has spaces ({}); move or rename it to a folder without spaces",
            input.display()
        )));
    }
    let mut made = Vec::new();
    for (src, target) in &sources {
        let dst = work.join(target);
        std::fs::copy(src, &dst)?;
        made.push(dst);
    }
    Ok(Alias {
        input: work.join(&alias_name),
        made,
    })
}
