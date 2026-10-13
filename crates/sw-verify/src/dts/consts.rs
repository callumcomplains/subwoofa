//! The constants of the DTS bitstream: sync words, and the tables the headers index into.

pub(crate) const CORE_SYNC: u32 = 0x7FFE_8001;

pub(crate) const EXSS_SYNC: u32 = 0x6458_2025;

pub(crate) const SYNC_XCH: u32 = 0x5A5A_5A5A;

pub(crate) const SYNC_XXCH: u32 = 0x4700_4A03;

pub(crate) const SYNC_X96: u32 = 0x1D95_F262;

pub(crate) const SYNC_XBR: u32 = 0x655E_315E;

pub(crate) const SYNC_LBR: u32 = 0x0A80_1921;

pub(crate) const SYNC_XLL: u32 = 0x41A2_9547;

pub(crate) const SYNC_EXSS_CORE: u32 = 0x02B0_9261;

/// DTS:X object data trails the lossless audio of an XLL frame, announced by one of these words.
pub(crate) const SYNC_XLL_X: u32 = 0x0200_0850;

pub(crate) const SYNC_XLL_X_IMAX: u32 = 0xF140_00D0;

pub(crate) const SAMPLE_RATES: [u32; 16] = [
    0, 8000, 16000, 32000, 0, 0, 11025, 22050, 44100, 0, 0, 12000, 24000, 48000, 0, 0,
];

pub(crate) const BIT_RATES: [u32; 32] = [
    32, 56, 64, 96, 112, 128, 192, 224, 256, 320, 384, 448, 512, 576, 640, 768, 896, 1024, 1152, 1280, 1344, 1408,
    1411, 1472, 1536, 1920, 2048, 3072, 3840, 0, 0, 0,
];

pub(crate) const AMODE_CH: [u32; 16] = [1, 2, 2, 2, 2, 3, 3, 4, 4, 5, 6, 6, 6, 7, 8, 8];

pub(crate) const AMODE_NAME: [&str; 16] = [
    "C",
    "C+C (dual mono)",
    "L R",
    "L+R (sum/difference)",
    "Lt Rt",
    "C L R",
    "L R S",
    "C L R S",
    "L R Ls Rs",
    "C L R Ls Rs",
    "CL CR L R Ls Rs",
    "C L R Ls Rs Lh Rh",
    "CL CR L R Ls Rs",
    "CL C CR L R Ls Rs",
    "CL CR L R Ls C Rs Rsr",
    "CL C CR L R Ls Rs Rsr",
];

pub(crate) const PCM_BITS: [u32; 8] = [16, 16, 20, 20, 0, 24, 24, 0];

pub(crate) const EXT_AUDIO: [&str; 8] = [
    "XCH (channel extension)",
    "reserved",
    "X96 (96 kHz extension)",
    "XCH + X96",
    "reserved",
    "reserved",
    "XXCH (extra channels)",
    "reserved",
];

/// Speaker activity mask of the extension substream: one bit per speaker or pair of speakers.
pub(crate) const SPEAKERS: [(&str, u32); 16] = [
    ("C", 1),
    ("L R", 2),
    ("Ls Rs", 2),
    ("LFE", 1),
    ("Cs", 1),
    ("Lh Rh", 2),
    ("Lsr Rsr", 2),
    ("Ch", 1),
    ("Oh", 1),
    ("Lc Rc", 2),
    ("Lw Rw", 2),
    ("Lss Rss", 2),
    ("LFE2", 1),
    ("Lhs Rhs", 2),
    ("Chr", 1),
    ("Lhr Rhr", 2),
];

pub(crate) const SAMPLE_RATES_EXSS: [u32; 16] = [
    8000, 16000, 32000, 64000, 128000, 22050, 44100, 88200, 176400, 352800, 12000, 24000, 48000, 96000, 192000, 384000,
];
