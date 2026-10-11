//! The version Subwoofa shows to people: `b0.1.0 (2026-10-11 14:30:00 UTC)`.
//!
//! The number comes from the workspace `Cargo.toml`. The timestamp is the moment the release was tagged and is
//! given to the build by `SUBWOOFA_BUILD_STAMP`; a local build has none and says `dev build`.

/// The "b" marks the public beta; it goes away at 1.0.
pub const NUMBER: &str = concat!("b", env!("CARGO_PKG_VERSION"));

/// Build timestamp in UTC, if this is a release build.
pub fn stamp() -> Option<&'static str> {
    option_env!("SUBWOOFA_BUILD_STAMP").filter(|s| !s.is_empty())
}

/// `b0.1.0 (2026-10-11 14:30:00 UTC)` or `b0.1.0 (dev build)`.
pub fn display() -> String {
    match stamp() {
        Some(s) => format!("{NUMBER} ({s} UTC)"),
        None => format!("{NUMBER} (dev build)"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shows_number_and_build() {
        let d = super::display();
        assert!(d.starts_with("b0."));
        assert!(d.contains("UTC)") || d.ends_with("(dev build)"));
    }
}
