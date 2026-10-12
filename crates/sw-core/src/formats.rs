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
    /// Plain audio files: WAV, AIFF, CAF and FLAC.
    Files,
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
    /// Uncompressed audio files (the sample format is a separate setting).
    PcmWav,
    PcmAiff,
    PcmCaf,
    Flac,
    /// FLAC frames in an Ogg stream.
    OggFlac,
    /// Apple Lossless in an M4A file.
    Alac,
    /// True Audio.
    Tta,
    /// WavPack, lossless.
    WavPack,
    /// Monkey's Audio, lossless.
    MonkeysAudio,
    /// MPEG-4 Audio Lossless coding.
    MpegAls,
    /// Digital cinema sound: an MXF track file of 24-bit PCM.
    DcpAudio,
    /// DSD (the audio of SACD) in a DSF file.
    DsdDsf,
    /// DSD in a DSDIFF file.
    DsdDff,
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
        StreamType::PcmWav,
        StreamType::PcmAiff,
        StreamType::PcmCaf,
        StreamType::Flac,
        StreamType::OggFlac,
        StreamType::Alac,
        StreamType::Tta,
        StreamType::WavPack,
        StreamType::MonkeysAudio,
        StreamType::MpegAls,
        StreamType::DcpAudio,
        StreamType::DsdDsf,
        StreamType::DsdDff,
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
            StreamType::PcmWav => "wav",
            StreamType::PcmAiff => "aiff",
            StreamType::PcmCaf => "caf",
            StreamType::Flac => "flac",
            StreamType::OggFlac => "oggflac",
            StreamType::Alac => "alac",
            StreamType::Tta => "tta",
            StreamType::WavPack => "wavpack",
            StreamType::MonkeysAudio => "ape",
            StreamType::MpegAls => "als",
            StreamType::DcpAudio => "dcp-audio",
            StreamType::DsdDsf => "dsf",
            StreamType::DsdDff => "dff",
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
            StreamType::PcmWav => "WAV / Broadcast Wave (PCM)",
            StreamType::PcmAiff => "AIFF (PCM)",
            StreamType::PcmCaf => "CAF (PCM)",
            StreamType::Flac => "FLAC",
            StreamType::OggFlac => "FLAC in Ogg",
            StreamType::Alac => "Apple Lossless (ALAC)",
            StreamType::Tta => "TTA (True Audio)",
            StreamType::WavPack => "WavPack",
            StreamType::MonkeysAudio => "Monkey's Audio",
            StreamType::MpegAls => "MPEG-4 ALS",
            StreamType::DcpAudio => "DCP sound (MXF, 24-bit PCM)",
            StreamType::DsdDsf => "DSD (DSF)",
            StreamType::DsdDff => "DSD (DSDIFF)",
        }
    }

    pub const fn is_dts(self) -> bool {
        matches!(self, StreamType::DtsHdMa | StreamType::DtsHdHra | StreamType::DtsEs | StreamType::Dts9624 | StreamType::DtsDigitalSurround | StreamType::DtsExpress)
    }

    pub const fn is_lossless(self) -> bool {
        matches!(self, StreamType::DtsHdMa | StreamType::TrueHd | StreamType::Mlp) || (self.is_audio_file() && !matches!(self, StreamType::DsdDsf | StreamType::DsdDff))
    }

    /// A plain audio file (WAV, AIFF, CAF, FLAC) rather than a codec stream. These have a sample format setting
    /// instead of a bit rate.
    pub const fn is_audio_file(self) -> bool {
        matches!(self, StreamType::PcmWav | StreamType::PcmAiff | StreamType::PcmCaf | StreamType::Flac | StreamType::OggFlac | StreamType::Alac | StreamType::Tta | StreamType::WavPack | StreamType::MonkeysAudio | StreamType::MpegAls | StreamType::DcpAudio | StreamType::DsdDsf | StreamType::DsdDff)
    }

    /// Whether Subwoofa has its own encoder for the stream (every stream type does; the FFmpeg engine is an
    /// optional alternative for some).
    pub const fn is_native(self) -> bool {
        matches!(
            self,
            StreamType::DtsHdMa
                | StreamType::DtsHdHra
                | StreamType::DtsEs
                | StreamType::Dts9624
                | StreamType::DtsDigitalSurround
                | StreamType::DtsExpress
                | StreamType::Ac3
                | StreamType::Eac3
                | StreamType::TrueHd
                | StreamType::Mlp
                | StreamType::PcmWav
                | StreamType::PcmAiff
                | StreamType::PcmCaf
                | StreamType::Flac
                | StreamType::OggFlac
                | StreamType::Alac
                | StreamType::Tta
                | StreamType::WavPack
                | StreamType::MonkeysAudio
                | StreamType::MpegAls
                | StreamType::DcpAudio
                | StreamType::DsdDsf
                | StreamType::DsdDff
        )
    }

    /// Sample rates (Hz) the stream type accepts.
    pub const fn sample_rates(self) -> &'static [u32] {
        match self {
            StreamType::DtsHdMa => &[48_000, 96_000, 192_000],
            StreamType::DtsHdHra => &[48_000, 96_000],
            StreamType::Dts9624 => &[96_000],
            StreamType::DtsEs | StreamType::DtsDigitalSurround => &[44_100, 48_000],
            StreamType::DtsExpress => &[48_000],
            StreamType::Ac3 => &[32_000, 44_100, 48_000],
            StreamType::Eac3 => &[32_000, 44_100, 48_000],
            StreamType::TrueHd => &[44_100, 48_000, 88_200, 96_000, 176_400, 192_000],
            StreamType::Mlp => &[44_100, 48_000, 88_200, 96_000, 176_400, 192_000],
            StreamType::PcmWav | StreamType::PcmAiff | StreamType::PcmCaf | StreamType::Flac | StreamType::OggFlac | StreamType::Alac | StreamType::Tta | StreamType::WavPack | StreamType::MonkeysAudio | StreamType::MpegAls => &[44_100, 48_000, 88_200, 96_000, 176_400, 192_000, 352_800, 384_000],
            StreamType::DcpAudio => &[48_000, 96_000],
            // the PCM rate of the source: the DSD rate is a separate setting
            StreamType::DsdDsf | StreamType::DsdDff => &[44_100, 48_000, 88_200, 96_000, 176_400, 192_000],
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
        Destination::Files,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Destination::BluRayPrimary => "bluray-primary",
            Destination::BluRaySecondary => "bluray-secondary",
            Destination::DigitalDelivery => "digital-delivery",
            Destination::DvdVideo => "dvd",
            Destination::DtsMusicDisc => "dts-music-disc",
            Destination::Generic => "generic",
            Destination::Files => "files",
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
            Destination::Files => "Audio files (PCM / FLAC)",
        }
    }

    /// Stream types selectable for this destination.
    pub fn stream_types(self) -> &'static [StreamType] {
        use StreamType::*;
        match self {
            Destination::BluRayPrimary => &[DtsHdMa, DtsHdHra, DtsEs, Dts9624, DtsDigitalSurround],
            Destination::DigitalDelivery => &[DtsHdMa, DtsHdHra, DtsEs, Dts9624, DtsDigitalSurround, DtsExpress],
            Destination::BluRaySecondary => &[DtsExpress],
            Destination::DvdVideo | Destination::DtsMusicDisc => &[DtsEs, Dts9624, DtsDigitalSurround],
            Destination::Generic => &[Ac3, Eac3, TrueHd, Mlp, DtsDigitalSurround],
            Destination::Files => &[PcmWav, PcmAiff, PcmCaf, Flac, OggFlac, Alac, Tta, WavPack, MonkeysAudio, MpegAls, DcpAudio, DsdDsf, DsdDff],
        }
    }

    /// File extension (without dot) for the produced stream.
    pub const fn extension(self, stream: StreamType) -> &'static str {
        match self {
            Destination::DvdVideo => "cpt",
            Destination::DtsMusicDisc => "wav",
            Destination::Files => match stream {
                StreamType::PcmAiff => "aif",
                StreamType::PcmCaf => "caf",
                StreamType::Flac => "flac",
                StreamType::OggFlac => "oga",
                StreamType::Alac => "m4a",
                StreamType::Tta => "tta",
                StreamType::WavPack => "wv",
                StreamType::MonkeysAudio => "ape",
                StreamType::MpegAls => "als",
                StreamType::DcpAudio => "mxf",
                StreamType::DsdDsf => "dsf",
                StreamType::DsdDff => "dff",
                _ => "wav",
            },
            Destination::Generic => match stream {
                StreamType::Ac3 => "ac3",
                StreamType::Eac3 => "eac3",
                StreamType::TrueHd | StreamType::Mlp => "thd",
                _ => "dts",
            },
            // the core-only DTS streams are written as bare streams, not in a DTS-HD container
            _ => match stream {
                StreamType::DtsDigitalSurround | StreamType::DtsEs | StreamType::Dts9624 => "dts",
                _ => "dtshd",
            },
        }
    }
}

/// Maximum discrete channels the stream type can carry.
fn max_channels(dest: Destination, stream: StreamType, sample_rate: u32) -> usize {
    let base = match stream {
        // immersive layouts (up to NHK 22.2) fit a lossless stream at 44.1 and 48 kHz, up to 12 channels at 96 kHz
        StreamType::DtsHdMa => {
            if sample_rate >= 192_000 { 6 } else if sample_rate >= 96_000 { 12 } else { 24 }
        }
        StreamType::DtsHdHra => {
            if sample_rate >= 192_000 { 6 } else { 8 }
        }
        StreamType::DtsEs | StreamType::Dts9624 | StreamType::DtsDigitalSurround => 7,
        StreamType::DtsExpress => 6,
        StreamType::Ac3 => 6,
        StreamType::Eac3 => 8, // up to 7.1 via independent+dependent substreams
        StreamType::TrueHd | StreamType::Mlp => 8,
        StreamType::Flac | StreamType::OggFlac | StreamType::Alac | StreamType::Tta | StreamType::WavPack | StreamType::MpegAls => 8,
        StreamType::MonkeysAudio => 2,
        StreamType::DcpAudio => 16,
        StreamType::DsdDsf | StreamType::DsdDff => 6,
        StreamType::PcmWav | StreamType::PcmAiff | StreamType::PcmCaf => 32,
    };
    if dest == Destination::DtsMusicDisc && stream == StreamType::Dts9624 {
        return 3; // stereo, with an LFE
    }
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
            // a DTS CD carries the stream at the CD data rate (1411.2 kbit/s) or at half of it
            Some(if dest == Destination::DtsMusicDisc { (754, 1411) } else { (lo, 1509) })
        }
        StreamType::DtsExpress => {
            // the native encoder needs about 56 kbps per channel pair (6 kHz bandwidth), 30 for a lone channel, 3 for the LFE
            let main = layout.main_channels() as u32;
            let lone = main % 2;
            let lo = (main / 2) * 56 + lone * 30 + u32::from(layout.has_lfe()) * 3;
            Some((lo.clamp(48, 510), 510))
        }
        StreamType::Ac3 => Some((32, 640)),
        StreamType::Eac3 => Some((32, if dest == Destination::Generic { 6144 } else { 1024 })),
        StreamType::TrueHd | StreamType::Mlp => None,
        StreamType::PcmWav | StreamType::PcmAiff | StreamType::PcmCaf | StreamType::Flac | StreamType::OggFlac | StreamType::Alac | StreamType::Tta | StreamType::WavPack | StreamType::MonkeysAudio | StreamType::MpegAls | StreamType::DcpAudio | StreamType::DsdDsf | StreamType::DsdDff => None,
    }
}

/// Common bit rates (kbps) worth offering as one-click presets, clipped to the legal range.
/// Users can still type any value; these are shortcuts, not limits.
pub fn bitrate_presets(
    dest: Destination,
    stream: StreamType,
    layout: &ChannelLayout,
    sample_rate: u32,
) -> Vec<u32> {
    let Some((lo, hi)) = bitrate_range_kbps(dest, stream, layout, sample_rate) else {
        return Vec::new();
    };
    let common: &[u32] = match stream {
        StreamType::Ac3 => &[96, 128, 192, 224, 256, 320, 384, 448, 512, 576, 640],
        StreamType::Eac3 => &[192, 256, 384, 448, 640, 768, 1024, 1536, 2048, 3024, 6144],
        StreamType::DtsDigitalSurround | StreamType::DtsEs | StreamType::Dts9624 | StreamType::DtsHdMa => {
            &[192, 256, 384, 448, 512, 768, 1024, 1411, 1509]
        }
        StreamType::DtsHdHra => &[2046, 2500, 3018, 3500, 4000, 4224, 4500, 5000, 5376, 5760],
        StreamType::DtsExpress => &[48, 64, 96, 128, 192, 256, 320, 384, 510],
        _ => &[],
    };
    common.iter().copied().filter(|b| (lo..=hi).contains(b)).collect()
}

#[derive(Debug, PartialEq, Eq)]
pub enum Invalid {
    StreamNotInDestination,
    LayoutTooWide { max: usize },
    SampleRateUnsupported,
    LfeOnlyDvd,
    LayoutNotSupported,
}

/// Sample rates the stream type accepts on a given destination: a DTS Music Disc is a CD (44.1 kHz), the
/// video discs and delivery formats use 48 kHz for the core formats.
pub fn sample_rates_for(dest: Destination, stream: StreamType) -> &'static [u32] {
    match (dest, stream) {
        (Destination::DtsMusicDisc, StreamType::DtsEs | StreamType::DtsDigitalSurround) => &[44_100],
        // a DTS CD holds 96/24 as 88.2 kHz stereo (the original encoder offers no other rate there)
        (Destination::DtsMusicDisc, StreamType::Dts9624) => &[88_200],
        (_, StreamType::DtsEs | StreamType::DtsDigitalSurround) if dest != Destination::Generic => &[48_000],
        _ => stream.sample_rates(),
    }
}

/// Layouts expressible as AC-3 audio coding modes (acmod) with optional LFE.
const AC3_LAYOUTS: &[&str] = &[
    "5.1", "5.0", "4.1-lcrs", "4.1-quad", "4.0-quad", "4.0-lcrs", "3.1", "3.1-lrs", "3.0", "3.0-lrs",
    "2.1", "2.0", "2.0-ltrt", "1.0",
];

/// Layouts E-AC-3 carries beyond the AC-3 modes: side/rear surround pairs in a dependent substream.
const EAC3_EXTENDED_LAYOUTS: &[&str] = &["7.1-side-rear", "7.1-ls-rs-lsr-rsr", "7.0-side-rear"];

/// Layouts Apple Lossless has a channel arrangement for.
const ALAC_LAYOUTS: &[&str] = &["1.0", "2.0", "3.0", "4.0-lcrs", "5.0", "5.1", "6.1-discrete", "7.1-wide"];

/// Layouts the DSD file formats define a channel arrangement for.
const DSD_LAYOUTS: &[&str] = &["1.0", "2.0", "3.0", "3.1", "4.0-quad", "5.0", "5.1"];

/// Layouts DTS Express can carry: front L R / C, surrounds Ls Rs, optional LFE.
const EXPRESS_LAYOUTS: &[&str] = &["5.1", "5.0", "4.1-quad", "4.0-quad", "3.1", "3.0", "2.1", "2.0", "1.0"];

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
    if !sample_rates_for(dest, stream).contains(&sample_rate) {
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
    // E-AC-3 adds 7.1 / 7.0 with the rear pair in a dependent substream
    if stream == StreamType::Eac3 && !AC3_LAYOUTS.contains(&layout.id) && !EAC3_EXTENDED_LAYOUTS.contains(&layout.id) {
        return Err(Invalid::LayoutNotSupported);
    }
    // top-layer speakers (7.1.4, 9.1.6, 22.2...) are carried by the lossless DTS-HD stream only for now
    if layout.channels.iter().any(|c| c.is_top_layer()) && stream != StreamType::DtsHdMa && !stream.is_audio_file() {
        return Err(Invalid::LayoutNotSupported);
    }
    if matches!(stream, StreamType::DsdDsf | StreamType::DsdDff) && !DSD_LAYOUTS.contains(&layout.id) {
        return Err(Invalid::LayoutNotSupported);
    }
    if stream == StreamType::Alac && !ALAC_LAYOUTS.contains(&layout.id) {
        return Err(Invalid::LayoutNotSupported);
    }
    if stream == StreamType::DtsExpress && !EXPRESS_LAYOUTS.contains(&layout.id) {
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
    fn audio_files_take_wide_layouts_but_flac_stops_at_eight_channels() {
        assert!(validate(Destination::Files, StreamType::PcmWav, l("22.2"), 96_000).is_ok());
        assert!(validate(Destination::Files, StreamType::Flac, l("7.1.4"), 48_000).is_err());
        assert!(validate(Destination::Files, StreamType::Flac, l("7.1-side-rear"), 192_000).is_ok());
        assert_eq!(Destination::Files.extension(StreamType::PcmAiff), "aif");
        assert!(StreamType::Flac.is_lossless() && !StreamType::Flac.is_dts());
    }

    #[test]
    fn core_only_dts_is_a_bare_dts_stream_and_the_rest_a_container() {
        assert_eq!(Destination::BluRayPrimary.extension(StreamType::DtsDigitalSurround), "dts");
        assert_eq!(Destination::DigitalDelivery.extension(StreamType::DtsEs), "dts");
        assert_eq!(Destination::BluRayPrimary.extension(StreamType::DtsHdMa), "dtshd");
        assert_eq!(Destination::BluRayPrimary.extension(StreamType::DtsHdHra), "dtshd");
        assert_eq!(Destination::DigitalDelivery.extension(StreamType::DtsExpress), "dtshd");
    }

    #[test]
    fn every_stream_type_is_native() {
        assert!(StreamType::ALL.iter().all(|s| s.is_native()));
    }

    #[test]
    fn presets_stay_in_range() {
        let p = bitrate_presets(Destination::Generic, StreamType::Ac3, l("5.1"), 48_000);
        assert!(p.contains(&448) && p.contains(&640) && p.contains(&96));
        assert!(p.iter().all(|b| (32..=640).contains(b)));
        assert!(bitrate_presets(Destination::Generic, StreamType::TrueHd, l("5.1"), 48_000).is_empty());
    }

    #[test]
    fn extensions() {
        assert_eq!(Destination::DvdVideo.extension(StreamType::DtsDigitalSurround), "cpt");
        assert_eq!(Destination::BluRayPrimary.extension(StreamType::DtsHdMa), "dtshd");
    }
}
