use crate::layout::ChannelLayout;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Destination {
    BluRayPrimary,
    BluRaySecondary,
    DigitalDelivery,
    DvdVideo,
    DtsMusicDisc,
    /// No disc constraints (Dolby formats, general conversions).
    Generic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum StreamType {
    DtsHdMa,
    DtsHdHra,
    DtsEs,
    Dts9624,
    DtsDigitalSurround,
    /// DTS-HD Express / LBR.
    DtsExpress,
    Ac3,
    Eac3,
    TrueHd,
    Mlp,
}

impl StreamType {
    pub const ALL: &'static [StreamType] = &[
        StreamType::DtsHdMa,
        StreamType::DtsHdHra,
        StreamType::DtsEs,
        StreamType::Dts9624,
        StreamType::DtsDigitalSurround,
        StreamType::DtsExpress,
        StreamType::Ac3,
        StreamType::Eac3,
        StreamType::TrueHd,
        StreamType::Mlp,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            StreamType::DtsHdMa => "dtshd-ma",
            StreamType::DtsHdHra => "dtshd-hra",
            StreamType::DtsEs => "dts-es",
            StreamType::Dts9624 => "dts-96-24",
            StreamType::DtsDigitalSurround => "dts",
            StreamType::DtsExpress => "dts-express",
            StreamType::Ac3 => "ac3",
            StreamType::Eac3 => "eac3",
            StreamType::TrueHd => "truehd",
            StreamType::Mlp => "mlp",
        }
    }

    pub fn from_id(id: &str) -> Option<StreamType> {
        Self::ALL.iter().copied().find(|s| s.id().eq_ignore_ascii_case(id))
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            StreamType::DtsHdMa => "DTS-HD Master Audio",
            StreamType::DtsHdHra => "DTS-HD High Resolution Audio",
            StreamType::DtsEs => "DTS-ES",
            StreamType::Dts9624 => "DTS 96/24",
            StreamType::DtsDigitalSurround => "DTS Digital Surround",
            StreamType::DtsExpress => "DTS-HD Express (LBR)",
            StreamType::Ac3 => "Dolby Digital (AC-3)",
            StreamType::Eac3 => "Dolby Digital Plus (E-AC-3)",
            StreamType::TrueHd => "Dolby TrueHD",
            StreamType::Mlp => "MLP (Meridian Lossless Packing)",
        }
    }

    pub const fn is_dts(self) -> bool {
        !matches!(self, StreamType::Ac3 | StreamType::Eac3 | StreamType::TrueHd | StreamType::Mlp)
    }

    pub const fn is_lossless(self) -> bool {
        matches!(self, StreamType::DtsHdMa | StreamType::TrueHd | StreamType::Mlp)
    }

    /// Sample rates (Hz) the stream type accepts.
    pub const fn sample_rates(self) -> &'static [u32] {
        match self {
            StreamType::DtsHdMa => &[48_000, 96_000, 192_000],
            StreamType::DtsHdHra => &[48_000, 96_000],
            StreamType::Dts9624 => &[96_000],
            StreamType::DtsEs | StreamType::DtsDigitalSurround | StreamType::DtsExpress => &[48_000],
            StreamType::Ac3 => &[32_000, 44_100, 48_000],
            StreamType::Eac3 => &[32_000, 44_100, 48_000],
            StreamType::TrueHd => &[44_100, 48_000, 88_200, 96_000, 176_400, 192_000],
            StreamType::Mlp => &[44_100, 48_000, 88_200, 96_000, 176_400, 192_000],
        }
    }
}

impl Destination {
    pub const ALL: &'static [Destination] = &[
        Destination::BluRayPrimary,
        Destination::BluRaySecondary,
        Destination::DigitalDelivery,
        Destination::DvdVideo,
        Destination::DtsMusicDisc,
        Destination::Generic,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Destination::BluRayPrimary => "bluray-primary",
            Destination::BluRaySecondary => "bluray-secondary",
            Destination::DigitalDelivery => "digital-delivery",
            Destination::DvdVideo => "dvd",
            Destination::DtsMusicDisc => "dts-music-disc",
            Destination::Generic => "generic",
        }
    }

    pub fn from_id(id: &str) -> Option<Destination> {
        Self::ALL.iter().copied().find(|d| d.id().eq_ignore_ascii_case(id))
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Destination::BluRayPrimary => "Blu-ray Disc Primary Audio",
            Destination::BluRaySecondary => "Blu-ray Disc Secondary Audio",
            Destination::DigitalDelivery => "Digital Delivery",
            Destination::DvdVideo => "DVD-Video",
            Destination::DtsMusicDisc => "DTS Music Disc",
            Destination::Generic => "Generic",
        }
    }

    /// Stream types selectable for this destination.
    pub fn stream_types(self) -> &'static [StreamType] {
        use StreamType::*;
        match self {
            Destination::BluRayPrimary | Destination::DigitalDelivery => {
                &[DtsHdMa, DtsHdHra, DtsEs, Dts9624, DtsDigitalSurround]
            }
            Destination::BluRaySecondary => &[DtsExpress],
            Destination::DvdVideo | Destination::DtsMusicDisc => &[DtsEs, Dts9624, DtsDigitalSurround],
            Destination::Generic => &[Ac3, Eac3, TrueHd, Mlp, DtsDigitalSurround],
        }
    }

    /// File extension (without dot) for the produced stream.
    pub const fn extension(self, stream: StreamType) -> &'static str {
        match self {
            Destination::DvdVideo => "cpt",
            Destination::DtsMusicDisc => "wav",
            Destination::Generic => match stream {
                StreamType::Ac3 => "ac3",
                StreamType::Eac3 => "eac3",
                StreamType::TrueHd | StreamType::Mlp => "thd",
                _ => "dts",
            },
            _ => "dtshd",
        }
    }
}

/// Maximum discrete channels the stream type can carry.
fn max_channels(dest: Destination, stream: StreamType, sample_rate: u32) -> usize {
    let base = match stream {
        StreamType::DtsHdMa | StreamType::DtsHdHra => {
            if sample_rate >= 192_000 { 6 } else { 8 }
        }
        StreamType::DtsEs | StreamType::Dts9624 | StreamType::DtsDigitalSurround => 7,
        StreamType::DtsExpress => 6,
        StreamType::Ac3 => 6,
        StreamType::Eac3 => 8, // up to 7.1 via independent+dependent substreams
        StreamType::TrueHd | StreamType::Mlp => 8,
    };
    if matches!(dest, Destination::DvdVideo | Destination::DtsMusicDisc) { base.min(7) } else { base }
}

/// Inclusive supported constant bit-rate range (kbps) for the (core) stream, or `None` for
/// pure-VBR/lossless-only streams.
pub fn bitrate_range_kbps(
    dest: Destination,
    stream: StreamType,
    layout: &ChannelLayout,
    sample_rate: u32,
) -> Option<(u32, u32)> {
    let n = layout.channel_count();
    match stream {
        StreamType::DtsHdMa => Some((if n >= 6 { 768 } else { 192 }, 1509)),
        StreamType::DtsHdHra => Some((2046, if sample_rate >= 96_000 { 5760 } else { 5376 })),
        StreamType::DtsEs | StreamType::Dts9624 | StreamType::DtsDigitalSurround => {
            let lo = if layout.main_channels() >= 5 { 754 } else { 255 };
            Some((lo, 1509))
        }
        StreamType::DtsExpress => Some((48, 510)),
        StreamType::Ac3 => Some((32, 640)),
        StreamType::Eac3 => Some((32, if dest == Destination::Generic { 6144 } else { 1024 })),
        StreamType::TrueHd | StreamType::Mlp => None,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Invalid {
    StreamNotInDestination,
    LayoutTooWide { max: usize },
    SampleRateUnsupported,
    LfeOnlyDvd,
    LayoutNotSupported,
}

/// Layouts expressible as AC-3 audio coding modes (acmod) with optional LFE.
const AC3_LAYOUTS: &[&str] = &[
    "5.1", "5.0", "4.1-lcrs", "4.1-quad", "4.0-quad", "4.0-lcrs", "3.1", "3.1-lrs", "3.0", "3.0-lrs",
    "2.1", "2.0", "2.0-ltrt", "1.0",
];

/// Validate a destination/stream/layout/sample-rate combination.
pub fn validate(
    dest: Destination,
    stream: StreamType,
    layout: &ChannelLayout,
    sample_rate: u32,
) -> Result<(), Invalid> {
    if !dest.stream_types().contains(&stream) {
        return Err(Invalid::StreamNotInDestination);
    }
    if !stream.sample_rates().contains(&sample_rate) {
        return Err(Invalid::SampleRateUnsupported);
    }
    let max = max_channels(dest, stream, sample_rate);
    if layout.channel_count() > max {
        return Err(Invalid::LayoutTooWide { max });
    }
    if layout.id == "1.1" && dest != Destination::DvdVideo {
        return Err(Invalid::LfeOnlyDvd);
    }
    // AC-3 only defines the standard front/surround channel modes (up to 5.1); anything with
    // 6.x/7.x, heights or wides needs E-AC-3 or a lossless format.
    if stream == StreamType::Ac3 && !AC3_LAYOUTS.contains(&layout.id) {
        return Err(Invalid::LayoutNotSupported);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(id: &str) -> &'static ChannelLayout {
        ChannelLayout::find(id).unwrap()
    }

    #[test]
    fn ma_71_ok_at_48k_but_not_192k() {
        let lay = l("7.1-side-rear");
        assert!(validate(Destination::BluRayPrimary, StreamType::DtsHdMa, lay, 48_000).is_ok());
        assert_eq!(
            validate(Destination::BluRayPrimary, StreamType::DtsHdMa, lay, 192_000),
            Err(Invalid::LayoutTooWide { max: 6 })
        );
    }

    #[test]
    fn dvd_has_no_ma() {
        assert_eq!(
            validate(Destination::DvdVideo, StreamType::DtsHdMa, l("5.1"), 48_000),
            Err(Invalid::StreamNotInDestination)
        );
    }

    #[test]
    fn hra_5760_needs_96k() {
        let lay = l("5.1");
        assert_eq!(bitrate_range_kbps(Destination::BluRayPrimary, StreamType::DtsHdHra, lay, 96_000), Some((2046, 5760)));
        assert_eq!(bitrate_range_kbps(Destination::BluRayPrimary, StreamType::DtsHdHra, lay, 48_000), Some((2046, 5376)));
    }

    #[test]
    fn ac3_rejects_six_zero() {
        assert_eq!(
            validate(Destination::Generic, StreamType::Ac3, l("6.0-discrete"), 48_000),
            Err(Invalid::LayoutNotSupported)
        );
        assert!(validate(Destination::Generic, StreamType::Ac3, l("5.1"), 48_000).is_ok());
    }

    #[test]
    fn extensions() {
        assert_eq!(Destination::DvdVideo.extension(StreamType::DtsDigitalSurround), "cpt");
        assert_eq!(Destination::BluRayPrimary.extension(StreamType::DtsHdMa), "dtshd");
    }
}
