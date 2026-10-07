use crate::channel::Channel::{self, *};
use serde::Serialize;

/// How the layout is realised in the bitstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum LayoutKind {
    Discrete,
    /// Center surround is matrixed into Ls/Rs (6.x ES Matrix).
    EsMatrix,
    /// Ls/Rs already carry the matrixed center surround.
    EsMatrixPremixed,
    /// Lt/Rt matrix-encoded stereo.
    LtRt,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ChannelLayout {
    /// Stable machine id, also used in settings files and the CLI.
    pub id: &'static str,
    pub name: &'static str,
    pub channels: &'static [Channel],
    pub kind: LayoutKind,
}

impl ChannelLayout {
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }
    pub fn has_lfe(&self) -> bool {
        self.channels.contains(&Lfe)
    }
    /// Full-bandwidth channels (everything except LFE), e.g. 5 for 5.1.
    pub fn main_channels(&self) -> usize {
        self.channels.iter().filter(|c| **c != Lfe).count()
    }
    pub fn has_heights(&self) -> bool {
        self.channels.iter().any(|c| c.is_height())
    }
    /// "5.1", "7.0", "2.0" style label.
    pub fn short_label(&self) -> String {
        format!("{}.{}", self.main_channels(), u8::from(self.has_lfe()))
    }
    pub fn find(id: &str) -> Option<&'static ChannelLayout> {
        LAYOUTS.iter().find(|l| l.id.eq_ignore_ascii_case(id))
    }
}

macro_rules! lay {
    ($id:expr, $name:expr, $kind:ident, [$($ch:ident),+]) => {
        ChannelLayout { id: $id, name: $name, channels: &[$($ch),+], kind: LayoutKind::$kind }
    };
}

/// All primary-audio layouts, richest first (the order shown in the GUI drop-down).
pub static LAYOUTS: &[ChannelLayout] = &[
    lay!("7.1-side-rear", "7.1 L R C LFE Lss Rss Lsr Rsr", Discrete, [L, R, C, Lfe, Lss, Rss, Lsr, Rsr]),
    lay!("7.1-ls-rs-lsr-rsr", "7.1 L R C LFE Ls Rs Lsr Rsr", Discrete, [L, R, C, Lfe, Ls, Rs, Lsr, Rsr]),
    lay!("7.1-height", "7.1 L R C LFE Ls Rs Lh Rh", Discrete, [L, R, C, Lfe, Ls, Rs, Lh, Rh]),
    lay!("7.1-height-side", "7.1 L R C LFE Ls Rs Lhs Rhs", Discrete, [L, R, C, Lfe, Ls, Rs, Lhs, Rhs]),
    lay!("7.1-cs-ch", "7.1 L R C LFE Ls Rs Cs Ch", Discrete, [L, R, C, Lfe, Ls, Rs, Cs, Ch]),
    lay!("7.1-cs-oh", "7.1 L R C LFE Ls Rs Cs Oh", Discrete, [L, R, C, Lfe, Ls, Rs, Cs, Oh]),
    lay!("7.1-wide", "7.1 L R C LFE Ls Rs Lw Rw", Discrete, [L, R, C, Lfe, Ls, Rs, Lw, Rw]),
    lay!("7.0-side-rear", "7.0 L R C Lss Rss Lsr Rsr", Discrete, [L, R, C, Lss, Rss, Lsr, Rsr]),
    lay!("6.1-discrete", "6.1 Discrete L R C LFE Ls Rs Cs", Discrete, [L, R, C, Lfe, Ls, Rs, Cs]),
    lay!("6.0-discrete", "6.0 Discrete L R C Ls Rs Cs", Discrete, [L, R, C, Ls, Rs, Cs]),
    lay!("6.1-matrix", "6.1 Matrix L R C LFE Ls Rs Cs", EsMatrix, [L, R, C, Lfe, Ls, Rs, Cs]),
    lay!("6.0-matrix", "6.0 Matrix L R C Ls Rs Cs", EsMatrix, [L, R, C, Ls, Rs, Cs]),
    lay!("6.1-matrix-premixed", "6.1 Matrix (pre-mixed) L R C LFE Ls Rs", EsMatrixPremixed, [L, R, C, Lfe, Ls, Rs]),
    lay!("6.0-matrix-premixed", "6.0 Matrix (pre-mixed) L R C Ls Rs", EsMatrixPremixed, [L, R, C, Ls, Rs]),
    lay!("5.1", "5.1 L R C LFE Ls Rs", Discrete, [L, R, C, Lfe, Ls, Rs]),
    lay!("5.0", "5.0 L R C Ls Rs", Discrete, [L, R, C, Ls, Rs]),
    lay!("4.1-lcrs", "4.1 L C R S LFE", Discrete, [L, C, R, Cs, Lfe]),
    lay!("4.1-quad", "4.1 L R Ls Rs LFE", Discrete, [L, R, Ls, Rs, Lfe]),
    lay!("4.0-quad", "4.0 L R Ls Rs", Discrete, [L, R, Ls, Rs]),
    lay!("4.0-lcrs", "4.0 L C R S", Discrete, [L, C, R, Cs]),
    lay!("3.1", "3.1 L C R LFE", Discrete, [L, C, R, Lfe]),
    lay!("3.1-lrs", "3.1 L R S LFE", Discrete, [L, R, Cs, Lfe]),
    lay!("3.0", "3.0 L C R", Discrete, [L, C, R]),
    lay!("3.0-lrs", "3.0 L R S", Discrete, [L, R, Cs]),
    lay!("2.1", "2.1 L R LFE", Discrete, [L, R, Lfe]),
    lay!("2.0", "2.0 L R", Discrete, [L, R]),
    lay!("2.0-ltrt", "2.0 Lt/Rt", LtRt, [Lt, Rt]),
    lay!("1.1", "1.1 C LFE (DVD only)", Discrete, [C, Lfe]),
    lay!("1.0", "1.0 C (mono)", Discrete, [C]),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sane() {
        assert_eq!(LAYOUTS.len(), 29);
        let mut ids: Vec<_> = LAYOUTS.iter().map(|l| l.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), LAYOUTS.len(), "layout ids must be unique");
        for l in LAYOUTS {
            assert!(l.channel_count() <= 8, "{} exceeds 8 channels", l.id);
        }
    }

    #[test]
    fn labels() {
        assert_eq!(ChannelLayout::find("5.1").unwrap().short_label(), "5.1");
        assert_eq!(ChannelLayout::find("7.0-side-rear").unwrap().short_label(), "7.0");
        assert!(ChannelLayout::find("7.1-height").unwrap().has_heights());
    }
}
