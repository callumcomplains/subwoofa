//! Auto-import: given the file for the L channel, find the sibling channel files.
//!
//! Rules (same convention professional tools use): files live in one directory, names are
//! identical except for a channel ID, and the ID is delimited by `.`, `-`, `_` or space.

use crate::channel::Channel;
use crate::layout::ChannelLayout;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Split a file stem into tokens, keeping the delimiter-separated pieces and their ranges.
fn tokens(stem: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, ch) in stem.char_indices() {
        if matches!(ch, '.' | '-' | '_' | ' ') {
            if i > start {
                out.push((start, i));
            }
            start = i + 1;
        }
    }
    if stem.len() > start {
        out.push((start, stem.len()));
    }
    out
}

/// Remove `chNN` position tokens ("ch01", "Ch12") and descriptive words ("Front-Left") so per-channel file
/// numbering and naming do not defeat matching.
fn strip_numbering(s: &str) -> String {
    let mut out = String::new();
    let mut last = 0;
    for (a, b) in tokens(s) {
        let t = &s[a..b];
        let descriptor = matches!(t.to_ascii_lowercase().as_str(), "front" | "left" | "right" | "center" | "centre" | "surround" | "back" | "rear" | "side" | "low" | "frequency" | "effects" | "channel" | "sub" | "subwoofer" | "height" | "top" | "wide");
        if descriptor || (t.len() > 2 && t[..2].eq_ignore_ascii_case("ch") && t[2..].bytes().all(|c| c.is_ascii_digit())) {
            out.push_str(&s[last..a]);
            last = b;
        }
    }
    out.push_str(&s[last..]);
    // runs of delimiters left behind by the removals compare equal
    let mut collapsed = String::new();
    for ch in out.chars() {
        if matches!(ch, '.' | '-' | '_' | ' ') {
            if !collapsed.ends_with('_') {
                collapsed.push('_');
            }
        } else {
            collapsed.push(ch);
        }
    }
    collapsed.trim_matches('_').to_string()
}

/// File names say `Ls`/`SL` for a side surround even in layouts that call it `Lss`.
fn same_position(a: Channel, b: Channel) -> bool {
    use Channel::*;
    a == b || matches!((a, b), (Ls, Lss) | (Lss, Ls) | (Rs, Rss) | (Rss, Rs))
}

/// Find the channel ID token in `stem` that equals `ch` and return (prefix, suffix) around it.
fn split_around(stem: &str, ch: Channel) -> Option<(String, String)> {
    for (s, e) in tokens(stem) {
        if Channel::from_id(&stem[s..e]).map_or(false, |c| same_position(c, ch)) {
            return Some((strip_numbering(&stem[..s]), strip_numbering(&stem[e..])));
        }
    }
    None
}

/// Given the path to the first channel's file, locate files for every channel in `layout`.
/// Channels whose file cannot be found are omitted.
pub fn auto_import(first: &Path, layout: &ChannelLayout) -> HashMap<Channel, PathBuf> {
    let mut found = HashMap::new();
    let Some(dir) = first.parent() else { return found };
    let Some(ext) = first.extension().and_then(|e| e.to_str()) else { return found };
    let Some(stem) = first.file_stem().and_then(|s| s.to_str()) else { return found };

    // Work out which channel the given file represents.
    let Some(first_ch) = layout.channels.iter().copied().find(|c| split_around(stem, *c).is_some())
    else {
        return found;
    };
    let (prefix, suffix) = split_around(stem, first_ch).unwrap();
    found.insert(first_ch, first.to_path_buf());

    let Ok(entries) = std::fs::read_dir(dir) else { return found };
    let files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    for ch in layout.channels.iter().copied().filter(|c| *c != first_ch) {
        for f in &files {
            if f.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case(ext)) != Some(true) {
                continue;
            }
            let Some(fs) = f.file_stem().and_then(|s| s.to_str()) else { continue };
            if let Some((p, s)) = split_around(fs, ch) {
                if p == prefix && s == suffix {
                    found.insert(ch, f.clone());
                    break;
                }
            }
        }
    }
    found
}

/// One programme found in a folder: a display name and the file for every channel.
#[derive(Debug, Clone)]
pub struct Group {
    pub name: String,
    pub inputs: HashMap<Channel, PathBuf>,
}

/// Find every complete set of per-channel files in `dir` (non-recursive): files are grouped by everything
/// around the channel ID, and only groups that cover the whole layout are returned, sorted by name.
pub fn scan_folder(dir: &Path, layout: &ChannelLayout) -> Vec<Group> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let first = layout.channels[0];
    let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
    files.sort();
    let mut groups: Vec<Group> = Vec::new();
    for f in &files {
        let ext_ok = f.extension().and_then(|e| e.to_str()).map(|e| matches!(e.to_ascii_lowercase().as_str(), "wav" | "w64" | "aif" | "aiff" | "flac" | "bwf")).unwrap_or(false);
        let Some(stem) = f.file_stem().and_then(|s| s.to_str()) else { continue };
        if !ext_ok || split_around(stem, first).is_none() {
            continue;
        }
        let found = auto_import(f, layout);
        if layout.channels.iter().all(|c| found.contains_key(c)) {
            let (prefix, suffix) = split_around(stem, first).unwrap();
            let name = format!("{prefix}{suffix}").trim_matches(|c: char| matches!(c, '.' | '-' | '_' | ' ')).to_string();
            let name = if name.is_empty() { stem.to_string() } else { name };
            if !groups.iter().any(|g| g.inputs.get(&first) == found.get(&first)) {
                groups.push(Group { name, inputs: found });
            }
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_a_folder_into_programmes() {
        let dir = std::env::temp_dir().join(format!("sw_scan_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for prog in ["Reel1", "Reel2"] {
            for ch in ["L", "R"] {
                std::fs::write(dir.join(format!("{prog}_{ch}.wav")), b"x").unwrap();
            }
        }
        std::fs::write(dir.join("Reel3_L.wav"), b"x").unwrap(); // incomplete: no R
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let lay = ChannelLayout::find("2.0").unwrap();
        let groups = scan_folder(&dir, lay);
        let names: Vec<_> = groups.iter().map(|g| g.name.clone()).collect();
        assert_eq!(names, vec!["Reel1", "Reel2"]);
    }

    #[test]
    fn finds_siblings() {
        let dir = std::env::temp_dir().join(format!("sw_ai_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for id in ["L", "R", "C", "LFE", "Ls", "Rs"] {
            std::fs::write(dir.join(format!("Mix_{id}.wav")), b"").unwrap();
        }
        std::fs::write(dir.join("Other_L.wav"), b"").unwrap();
        let lay = ChannelLayout::find("5.1").unwrap();
        let map = auto_import(&dir.join("Mix_L.wav"), lay);
        assert_eq!(map.len(), 6);
        assert!(map[&Channel::Lfe].ends_with("Mix_LFE.wav"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
