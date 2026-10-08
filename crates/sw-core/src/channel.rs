use serde::{Deserialize, Serialize};

/// A single speaker position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Channel {
    L,
    R,
    C,
    Lfe,
    Ls,
    Rs,
    /// Center surround (a.k.a. `S`).
    Cs,
    /// Surround rear.
    Lsr,
    Rsr,
    /// Surround side.
    Lss,
    Rss,
    /// Wide fronts.
    Lw,
    Rw,
    /// Heights.
    Lh,
    Rh,
    Lhs,
    Rhs,
    Ch,
    /// Overhead ("voice of god").
    Oh,
    /// Left/Right total (matrix-encoded stereo).
    Lt,
    Rt,
    /// Second low-frequency channel (22.2).
    Lfe2,
    /// Top layer (22.2): front centre/left/right, side left/right, centre, back left/right/centre.
    Tfc,
    Tfl,
    Tfr,
    Tsl,
    Tsr,
    Tc,
    Tbl,
    Tbr,
    Tbc,
    /// Bottom layer (22.2): front centre/left/right.
    Bfc,
    Bfl,
    Bfr,
}

impl Channel {
    pub const fn short(self) -> &'static str {
        use Channel::*;
        match self {
            L => "L",
            R => "R",
            C => "C",
            Lfe => "LFE",
            Ls => "Ls",
            Rs => "Rs",
            Cs => "Cs",
            Lsr => "Lsr",
            Rsr => "Rsr",
            Lss => "Lss",
            Rss => "Rss",
            Lw => "Lw",
            Rw => "Rw",
            Lh => "Lh",
            Rh => "Rh",
            Lhs => "Lhs",
            Rhs => "Rhs",
            Ch => "Ch",
            Oh => "Oh",
            Lt => "Lt",
            Rt => "Rt",
            Lfe2 => "LFE2",
            Tfc => "Tfc",
            Tfl => "Tfl",
            Tfr => "Tfr",
            Tsl => "Tsl",
            Tsr => "Tsr",
            Tc => "Tc",
            Tbl => "Tbl",
            Tbr => "Tbr",
            Tbc => "Tbc",
            Bfc => "Bfc",
            Bfl => "Bfl",
            Bfr => "Bfr",
        }
    }

    /// Parse a channel ID as it appears in file names (`LFE`, `LF`, `SW` all mean LFE;
    /// `S` and `Cs` both mean center surround). Case-insensitive.
    pub fn from_id(id: &str) -> Option<Channel> {
        use Channel::*;
        Some(match id.to_ascii_lowercase().as_str() {
            "l" => L,
            "r" => R,
            "c" => C,
            "lfe" | "lf" | "sw" => Lfe,
            "ls" => Ls,
            "rs" => Rs,
            "cs" | "s" => Cs,
            "lsr" => Lsr,
            "rsr" => Rsr,
            "lss" => Lss,
            "rss" => Rss,
            "lw" => Lw,
            "rw" => Rw,
            "lh" => Lh,
            "rh" => Rh,
            "lhs" => Lhs,
            "rhs" => Rhs,
            "ch" => Ch,
            "oh" => Oh,
            "lt" => Lt,
            "rt" => Rt,
            "lfe2" | "lf2" | "sw2" => Lfe2,
            "tfc" => Tfc,
            "tfl" => Tfl,
            "tfr" => Tfr,
            "tsl" => Tsl,
            "tsr" => Tsr,
            "tc" => Tc,
            "tbl" => Tbl,
            "tbr" => Tbr,
            "tbc" => Tbc,
            "bfc" => Bfc,
            "bfl" => Bfl,
            "bfr" => Bfr,
            "lb" => Lsr,
            "rb" => Rsr,
            "cb" => Cs,
            "lfe1" => Lfe,
            "fl" => L,
            "fr" => R,
            "fc" => C,
            "sl" => Ls,
            "sr" => Rs,
            "bl" => Lsr,
            "br" => Rsr,
            _ => return None,
        })
    }

    /// True for speakers in a different vertical plane (drawn differently in the GUI).
    pub const fn is_height(self) -> bool {
        matches!(
            self,
            Channel::Lh | Channel::Rh | Channel::Lhs | Channel::Rhs | Channel::Ch | Channel::Oh | Channel::Tfc | Channel::Tfl | Channel::Tfr | Channel::Tsl | Channel::Tsr | Channel::Tc | Channel::Tbl | Channel::Tbr | Channel::Tbc
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_aliases() {
        assert_eq!(Channel::from_id("SW"), Some(Channel::Lfe));
        assert_eq!(Channel::from_id("s"), Some(Channel::Cs));
        assert_eq!(Channel::from_id("nope"), None);
    }
}
