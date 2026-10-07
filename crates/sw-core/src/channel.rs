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
            _ => return None,
        })
    }

    /// True for speakers in a different vertical plane (drawn differently in the GUI).
    pub const fn is_height(self) -> bool {
        matches!(
            self,
            Channel::Lh | Channel::Rh | Channel::Lhs | Channel::Rhs | Channel::Ch | Channel::Oh
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
