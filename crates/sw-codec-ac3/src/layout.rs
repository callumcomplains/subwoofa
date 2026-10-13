//! Which AC-3 channels a layout's speakers take, and in what order they are coded.

use crate::Error;
use sw_core::{Channel, ChannelLayout};

/// How a layout maps onto AC-3 channels.
#[derive(Debug, Clone)]
pub struct LayoutMap {
    pub acmod: u8,
    pub lfe: bool,
    pub dolby_surround: bool,
    /// For each AC-3 channel (full-bandwidth channels in bitstream order, LFE last):
    /// the index into `layout.channels`.
    pub source_index: Vec<usize>,
}

pub fn map_layout(layout: &ChannelLayout) -> Result<LayoutMap, Error> {
    use Channel::*;
    let find = |c: &[Channel]| layout.channels.iter().position(|x| c.contains(x));
    let (l, r, c, cs, ls, rs, lfe) = (
        find(&[L, Lt]),
        find(&[R, Rt]),
        find(&[C]),
        find(&[Cs]),
        find(&[Ls]),
        find(&[Rs]),
        find(&[Lfe]),
    );
    let has = |o: Option<usize>| o.is_some();
    let unsupported = || Error::Unsupported(format!("layout '{}' cannot be carried by AC-3", layout.id));
    let (acmod, order): (u8, Vec<Option<usize>>) = match (has(l) && has(r), has(c), has(cs), has(ls) && has(rs)) {
        (false, true, false, false) => (1, vec![c]),
        (true, false, false, false) => (2, vec![l, r]),
        (true, true, false, false) => (3, vec![l, c, r]),
        (true, false, true, false) => (4, vec![l, r, cs]),
        (true, true, true, false) => (5, vec![l, c, r, cs]),
        (true, false, false, true) => (6, vec![l, r, ls, rs]),
        (true, true, false, true) => (7, vec![l, c, r, ls, rs]),
        _ => return Err(unsupported()),
    };
    let mut source_index: Vec<usize> = order
        .into_iter()
        .map(|o| o.ok_or_else(unsupported))
        .collect::<Result<_, _>>()?;
    if layout.channel_count() != source_index.len() + usize::from(lfe.is_some()) {
        return Err(unsupported());
    }
    if let Some(i) = lfe {
        source_index.push(i);
    }
    Ok(LayoutMap {
        acmod,
        lfe: lfe.is_some(),
        dolby_surround: layout.id == "2.0-ltrt",
        source_index,
    })
}

pub(crate) fn fbw_channels(acmod: u8) -> usize {
    [2, 1, 2, 3, 3, 4, 4, 5][acmod as usize]
}

#[cfg(test)]
mod tests {
    use super::*;
    use sw_core::ChannelLayout;

    #[test]
    fn layout_mapping() {
        let m = map_layout(ChannelLayout::find("5.1").unwrap()).unwrap();
        assert_eq!((m.acmod, m.lfe), (7, true));
        // 5.1 layout is L R C LFE Ls Rs -> AC-3 order L C R Ls Rs LFE
        assert_eq!(m.source_index, vec![0, 2, 1, 4, 5, 3]);
        assert_eq!(map_layout(ChannelLayout::find("2.0").unwrap()).unwrap().acmod, 2);
        assert!(map_layout(ChannelLayout::find("7.1-side-rear").unwrap()).is_err());
    }
}
