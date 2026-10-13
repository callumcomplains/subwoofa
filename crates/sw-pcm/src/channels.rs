//! Which file or file channel feeds which speaker: channel orders, masks and layout suggestions.

use crate::reader::PcmReader;
use crate::Error;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use sw_core::{Channel, ChannelLayout};

/// How channels inside a multichannel file are assigned to the layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChannelOrder {
    /// Follow the file's speaker mask, except for the standard 7.1 mask (0x63F), which many tools
    /// write over film-ordered audio (L R C LFE Ls Rs Lrs Rrs): that is read in file order.
    #[default]
    Auto,
    /// Always follow the speaker mask (the Windows convention: ..., BL BR, SL SR).
    Mask,
    /// File channel `i` is layout channel `i`.
    File,
}

impl ChannelOrder {
    pub fn parse(s: &str) -> Option<ChannelOrder> {
        match s.to_ascii_lowercase().as_str() {
            "" | "auto" => Some(ChannelOrder::Auto),
            "mask" => Some(ChannelOrder::Mask),
            "file" | "film" => Some(ChannelOrder::File),
            _ => None,
        }
    }
}

/// Windows speaker bit positions used by WAVE_FORMAT_EXTENSIBLE channel masks.
pub(crate) mod spk {
    pub const FL: u32 = 0;
    pub const FR: u32 = 1;
    pub const FC: u32 = 2;
    pub const LFE: u32 = 3;
    pub const BL: u32 = 4;
    pub const BR: u32 = 5;
    pub const FLC: u32 = 6;
    pub const FRC: u32 = 7;
    pub const BC: u32 = 8;
    pub const SL: u32 = 9;
    pub const SR: u32 = 10;
    pub const TC: u32 = 11;
    pub const TFL: u32 = 12;
    pub const TFC: u32 = 13;
    pub const TFR: u32 = 14;
}

/// Channel mask assumed for a multichannel file that does not declare one.
pub(crate) fn default_mask(channels: usize) -> Option<u32> {
    use spk::*;
    let bits: &[u32] = match channels {
        3 => &[FL, FR, FC],
        4 => &[FL, FR, BL, BR],
        5 => &[FL, FR, FC, BL, BR],
        6 => &[FL, FR, FC, LFE, BL, BR],
        7 => &[FL, FR, FC, LFE, BC, SL, SR],
        8 => &[FL, FR, FC, LFE, BL, BR, SL, SR],
        _ => return None,
    };
    Some(bits.iter().fold(0, |m, b| m | 1 << b))
}

/// For each layout channel, its index inside a multichannel file with the given speaker mask.
/// Surround positions are matched flexibly: a layout with both side and rear pairs needs side and
/// back speakers in the file; otherwise Ls/Rs accept either.
pub fn map_channel_mask(layout: &ChannelLayout, mask: u32) -> Option<Vec<usize>> {
    use spk::*;
    let has_rear = layout.channels.iter().any(|c| matches!(c, Channel::Lsr | Channel::Rsr));
    let wants = |c: Channel| -> Vec<u32> {
        match c {
            Channel::L | Channel::Lt => vec![FL],
            Channel::R | Channel::Rt => vec![FR],
            Channel::C => vec![FC],
            Channel::Lfe => vec![LFE],
            Channel::Ls => {
                if has_rear {
                    vec![SL]
                } else {
                    vec![BL, SL]
                }
            }
            Channel::Rs => {
                if has_rear {
                    vec![SR]
                } else {
                    vec![BR, SR]
                }
            }
            Channel::Lss => vec![SL],
            Channel::Rss => vec![SR],
            Channel::Lsr => vec![BL],
            Channel::Rsr => vec![BR],
            Channel::Cs => vec![BC],
            Channel::Lh | Channel::Lhs => vec![TFL],
            Channel::Rh | Channel::Rhs => vec![TFR],
            Channel::Ch => vec![TFC],
            Channel::Oh => vec![TC],
            Channel::Lw => vec![FLC],
            Channel::Rw => vec![FRC],
            Channel::Tfl => vec![TFL],
            Channel::Tfr => vec![TFR],
            Channel::Tfc => vec![TFC],
            Channel::Tc => vec![TC],
            Channel::Lfe2
            | Channel::Tsl
            | Channel::Tsr
            | Channel::Tbl
            | Channel::Tbr
            | Channel::Tbc
            | Channel::Bfc
            | Channel::Bfl
            | Channel::Bfr => vec![],
        }
    };
    let present: Vec<u32> = (0..32).filter(|b| mask >> b & 1 == 1).collect();
    let mut used = Vec::new();
    let mut out = Vec::new();
    for &ch in layout.channels {
        let bit = wants(ch)
            .into_iter()
            .find(|b| present.contains(b) && !used.contains(b))?;
        used.push(bit);
        out.push(present.iter().position(|p| *p == bit)?);
    }
    (used.len() == present.len()).then_some(out)
}

/// Resolved source of each layout channel: file index and channel inside the file.
pub(crate) fn resolve_sources(
    layout: &'static ChannelLayout,
    inputs: &HashMap<Channel, PathBuf>,
    order: ChannelOrder,
) -> Result<(Vec<PcmReader>, Vec<Slot>, Vec<PathBuf>), Error> {
    let mut readers: Vec<PcmReader> = Vec::new();
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut by_path: HashMap<PathBuf, usize> = HashMap::new();
    let mut slots = Vec::new();
    let mut multi: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, ch) in layout.channels.iter().enumerate() {
        let path = inputs
            .get(ch)
            .ok_or_else(|| Error::Mismatch(format!("no input file for channel {}", ch.short())))?;
        let idx = match by_path.get(path) {
            Some(&i) => i,
            None => {
                let r = PcmReader::open(path)?;
                readers.push(r);
                paths.push(path.clone());
                by_path.insert(path.clone(), readers.len() - 1);
                readers.len() - 1
            }
        };
        let n = readers[idx].info.channels as usize;
        let ch_in_file = match n {
            1 => 0,
            2 => i % 2,
            _ => {
                if n != layout.channel_count() {
                    return Err(Error::Mismatch(format!(
                        "{}: has {n} channels but layout '{}' needs {}",
                        path.display(),
                        layout.id,
                        layout.channel_count()
                    )));
                }
                if let std::collections::hash_map::Entry::Vacant(e) = multi.entry(idx) {
                    let info = &readers[idx].info;
                    let mask = info.channel_mask.or_else(|| default_mask(n));
                    let positional = || (0..n).collect::<Vec<usize>>();
                    let map = match (order, mask) {
                        (ChannelOrder::File, _) => positional(),
                        (ChannelOrder::Auto, None) => positional(),
                        // the standard 7.1 mask is routinely written over film-ordered audio
                        (ChannelOrder::Auto, Some(0x63F)) | (ChannelOrder::Auto, Some(0x3F)) => positional(),
                        (_, Some(mask)) => map_channel_mask(layout, mask).ok_or_else(|| {
                            Error::Mismatch(format!(
                                "{}: its speaker layout (mask {mask:#x}) does not fit '{}'",
                                path.display(),
                                layout.id
                            ))
                        })?,
                        (ChannelOrder::Mask, None) => {
                            return Err(Error::Mismatch(format!("{}: has no speaker mask", path.display())))
                        }
                    };
                    e.insert(map);
                }
                multi[&idx][i]
            }
        };
        slots.push(Slot {
            file: idx,
            ch: ch_in_file,
        });
    }
    Ok((readers, slots, paths))
}

/// If `path` is a single file holding every channel of `layout` (more than two), return the input
/// map that points each layout channel at it. The channels are separated by `StemSet`/`slot_map`
/// using the file's speaker mask.
pub fn multichannel_inputs(layout: &'static ChannelLayout, path: &Path) -> Option<HashMap<Channel, PathBuf>> {
    let r = PcmReader::open(path).ok()?;
    let n = r.info().channels as usize;
    (n > 2 && n == layout.channel_count()).then(|| layout.channels.iter().map(|c| (*c, path.to_path_buf())).collect())
}

/// For one multichannel file: its channel count and speaker mask, and the first of `candidates` whose
/// channels the file can fill (same channel count, and a speaker mask the layout fits; files with no mask or
/// the common film-order masks are read in file order, so the layout with the film order wins there).
pub fn suggest_layout(
    path: &Path,
    candidates: &[&'static ChannelLayout],
) -> Result<(usize, Option<u32>, Option<&'static ChannelLayout>), Error> {
    let r = PcmReader::open(path)?;
    let n = r.info().channels as usize;
    let mask = r.info().channel_mask.or_else(|| default_mask(n));
    let positional = n <= 2 || matches!(mask, None | Some(0x63F) | Some(0x3F));
    let mut fits: Vec<&'static ChannelLayout> = candidates
        .iter()
        .copied()
        .filter(|l| l.channel_count() == n && (positional || mask.is_some_and(|m| map_channel_mask(l, m).is_some())))
        .collect();
    // the plain layouts before the ES matrix ones that list the same channels
    fits.sort_by_key(|l| l.id.contains("premixed") || l.id.contains("matrix"));
    let film: &[Channel] = match n {
        2 => &[Channel::L, Channel::R],
        3 => &[Channel::L, Channel::R, Channel::C],
        4 => &[Channel::L, Channel::R, Channel::Ls, Channel::Rs],
        5 => &[Channel::L, Channel::R, Channel::C, Channel::Ls, Channel::Rs],
        6 => &[
            Channel::L,
            Channel::R,
            Channel::C,
            Channel::Lfe,
            Channel::Ls,
            Channel::Rs,
        ],
        8 => &[
            Channel::L,
            Channel::R,
            Channel::C,
            Channel::Lfe,
            Channel::Ls,
            Channel::Rs,
            Channel::Lsr,
            Channel::Rsr,
        ],
        _ => &[],
    };
    let pick = if positional {
        fits.iter()
            .copied()
            .find(|l| l.channels == film)
            .or_else(|| fits.first().copied())
    } else {
        fits.first().copied()
    };
    Ok((n, mask, pick))
}

/// Per layout channel: the file and channel index inside it that supplies the channel.
pub fn slot_map(
    layout: &'static ChannelLayout,
    inputs: &HashMap<Channel, PathBuf>,
    order: ChannelOrder,
) -> Result<Vec<(PathBuf, usize)>, Error> {
    let (_, slots, paths) = resolve_sources(layout, inputs, order)?;
    Ok(slots.into_iter().map(|s| (paths[s.file].clone(), s.ch)).collect())
}

/// One layout channel's source: which file and which channel inside it.
#[derive(Debug, Clone)]
pub(crate) struct Slot {
    pub(crate) file: usize,
    pub(crate) ch: usize,
}
