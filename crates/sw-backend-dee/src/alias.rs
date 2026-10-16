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

/// The text of a DAMF `.atmos` file with the names of its companion files changed to `metadata` and `audio`. The file
/// is a small text listing and names its companions (`metadata: Film.atmos.metadata`, `audio: Film.atmos.audio`), so
/// companions given an alias name are only found if the listing says so.
pub(crate) fn retarget_damf(text: &str, metadata: &str, audio: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        let ending = &line[body.len()..];
        let trimmed = body.trim_start();
        let indent = &body[..body.len() - trimmed.len()];
        let replaced = if let Some(rest) = trimmed.strip_prefix("metadata:") {
            (!rest.trim().is_empty()).then(|| format!("{indent}metadata: {metadata}"))
        } else if let Some(rest) = trimmed.strip_prefix("audio:") {
            (!rest.trim().is_empty()).then(|| format!("{indent}audio: {audio}"))
        } else {
            None
        };
        out.push_str(replaced.as_deref().unwrap_or(body));
        out.push_str(ending);
    }
    out
}

/// Puts `src` at `dst` as a hard link, or (when `text` is given) as a file holding that text.
fn place(src: &Path, dst: &Path, text: Option<&str>) -> std::io::Result<()> {
    let _ = std::fs::remove_file(dst);
    match text {
        Some(t) => std::fs::write(dst, t),
        None => std::fs::hard_link(src, dst),
    }
}

pub(crate) fn make_alias(input: &Path, output: &Path, work: &Path) -> Result<Alias, Error> {
    let name = input.file_name().and_then(|f| f.to_str()).unwrap_or("master");
    let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("wav");
    let stem = format!("subwoofa_in_{}", std::process::id());
    let alias_name = format!("{stem}.{ext}");
    // companions of a DAMF master: <name>.metadata and <name>.audio
    let mut sources: Vec<(PathBuf, String)> = vec![(input.to_path_buf(), alias_name.clone())];
    let listing = ext
        .eq_ignore_ascii_case("atmos")
        .then(|| std::fs::read_to_string(input))
        .transpose()?
        .map(|t| retarget_damf(&t, &format!("{alias_name}.metadata"), &format!("{alias_name}.audio")));
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
            let text = (src == input).then_some(listing.as_deref()).flatten();
            if place(src, &dst, text).is_err() {
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
        match (src == input).then_some(listing.as_deref()).flatten() {
            Some(text) => std::fs::write(&dst, text)?,
            None => {
                std::fs::copy(src, &dst)?;
            }
        }
        made.push(dst);
    }
    Ok(Alias {
        input: work.join(&alias_name),
        made,
    })
}
