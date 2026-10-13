//! Which layout channels go where in a 96 kHz High Resolution stream.

use sw_core::{Channel, ChannelLayout};

/// What the stream carries besides the 5.x core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hra96Kind {
    /// 5.1 or 5.0.
    Plain,
    /// 6.x with the centre surround matrixed into the surrounds (the core carries the ES flag).
    Matrix,
    /// 6.x with the centre surround also sent as the XCH extension of the core.
    Discrete,
    /// 7.x: the rear pair in an XXCH extension.
    Rear,
}

impl Hra96Kind {
    /// Channels coded beside the core's five (and LFE): Cs for discrete, the rears for 7.x.
    pub(crate) fn extra_channels(self) -> usize {
        match self {
            Hra96Kind::Plain | Hra96Kind::Matrix => 0,
            Hra96Kind::Discrete => 1,
            Hra96Kind::Rear => 2,
        }
    }
}

/// How a layout's channels feed the stream.
pub struct Hra96Map {
    pub kind: Hra96Kind,
    /// Layout index of C, L, R, Ls, Rs.
    pub core: Vec<usize>,
    /// DTS channel arrangement of the core.
    pub amode: u8,
    pub lfe: Option<usize>,
    /// Layout index of the centre surround (6.x) or of the rear pair (7.x).
    pub extra: Vec<usize>,
}

/// `Some` for the layouts this stream type carries: 5.x, 6.x (matrix or discrete) and 7.x with a rear pair.
pub fn hra96_map(layout: &ChannelLayout) -> Option<Hra96Map> {
    use sw_core::layout::LayoutKind;
    if let Some(m) = crate::dca::native_xxch::xxch_map(layout) {
        return Some(Hra96Map {
            kind: Hra96Kind::Rear,
            core: m.core.to_vec(),
            amode: 9,
            lfe: m.lfe,
            extra: m.rear.to_vec(),
        });
    }
    let pos = |list: &[Channel]| layout.channels.iter().position(|c| list.contains(c));
    let lfe = pos(&[Channel::Lfe]);
    let cs = pos(&[Channel::Cs]);
    let five = (|| {
        Some([
            pos(&[Channel::C])?,
            pos(&[Channel::L])?,
            pos(&[Channel::R])?,
            pos(&[Channel::Ls, Channel::Lss])?,
            pos(&[Channel::Rs, Channel::Rss])?,
        ])
    })();
    if let Some(core) = five {
        if layout.channels.len() != 5 + usize::from(lfe.is_some()) + usize::from(cs.is_some()) {
            return None;
        }
        let kind = match cs {
            None => Hra96Kind::Plain,
            Some(_) if layout.kind == LayoutKind::Discrete => Hra96Kind::Discrete,
            Some(_) => Hra96Kind::Matrix,
        };
        return Some(Hra96Map {
            kind,
            core: core.to_vec(),
            amode: 9,
            lfe,
            extra: cs.into_iter().collect(),
        });
    }
    // fewer channels: the core carries them as they are
    let lm = crate::dca::native::map_layout(layout).ok()?;
    Some(Hra96Map {
        kind: Hra96Kind::Plain,
        core: lm.order,
        amode: lm.amode,
        lfe: lm.lfe,
        extra: Vec::new(),
    })
}
