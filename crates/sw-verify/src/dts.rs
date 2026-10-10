//! DTS: the core substream, the extension substream (DTS-HD) and the `.dtshd` container.

use crate::bits::Bits;
use crate::{be32, crc, Collector, Probe, Status, Window};

const CORE_SYNC: u32 = 0x7FFE_8001;
const EXSS_SYNC: u32 = 0x6458_2025;
const SYNC_XCH: u32 = 0x5A5A_5A5A;
const SYNC_XXCH: u32 = 0x4700_4A03;
const SYNC_X96: u32 = 0x1D95_F262;
const SYNC_XBR: u32 = 0x655E_315E;
const SYNC_LBR: u32 = 0x0A80_1921;
const SYNC_XLL: u32 = 0x41A2_9547;
const SYNC_EXSS_CORE: u32 = 0x02B0_9261;
/// DTS:X object data trails the lossless audio of an XLL frame, announced by one of these words.
const SYNC_XLL_X: u32 = 0x0200_0850;
const SYNC_XLL_X_IMAX: u32 = 0xF140_00D0;

const SAMPLE_RATES: [u32; 16] = [0, 8000, 16000, 32000, 0, 0, 11025, 22050, 44100, 0, 0, 12000, 24000, 48000, 0, 0];
const BIT_RATES: [u32; 32] = [32, 56, 64, 96, 112, 128, 192, 224, 256, 320, 384, 448, 512, 576, 640, 768, 896, 1024, 1152, 1280, 1344, 1408, 1411, 1472, 1536, 1920, 2048, 3072, 3840, 0, 0, 0];
const AMODE_CH: [u32; 16] = [1, 2, 2, 2, 2, 3, 3, 4, 4, 5, 6, 6, 6, 7, 8, 8];
const AMODE_NAME: [&str; 16] = ["C", "C+C (dual mono)", "L R", "L+R (sum/difference)", "Lt Rt", "C L R", "L R S", "C L R S", "L R Ls Rs", "C L R Ls Rs", "CL CR L R Ls Rs", "C L R Ls Rs Lh Rh", "CL CR L R Ls Rs", "CL C CR L R Ls Rs", "CL CR L R Ls C Rs Rsr", "CL C CR L R Ls Rs Rsr"];
const PCM_BITS: [u32; 8] = [16, 16, 20, 20, 0, 24, 24, 0];
const EXT_AUDIO: [&str; 8] = ["XCH (channel extension)", "reserved", "X96 (96 kHz extension)", "XCH + X96", "reserved", "reserved", "XXCH (extra channels)", "reserved"];
/// Speaker activity mask of the extension substream: one bit per speaker or pair of speakers.
const SPEAKERS: [(&str, u32); 16] = [("C", 1), ("L R", 2), ("Ls Rs", 2), ("LFE", 1), ("Cs", 1), ("Lh Rh", 2), ("Lsr Rsr", 2), ("Ch", 1), ("Oh", 1), ("Lss Rss", 2), ("Lw Rw", 2), ("Lhs Rhs", 2), ("Chr", 1), ("Lhr Rhr", 2), ("Lc Rc", 2), ("LFE2", 1)];

#[derive(Debug, Clone, Default, PartialEq)]
struct Core {
    normal: bool,
    short: u32,
    crc_present: bool,
    nblks: u32,
    fsize: usize,
    amode: u32,
    rate: u32,
    rate_code: u32,
    br_code: usize,
    dynf: bool,
    timef: bool,
    auxf: bool,
    hdcd: bool,
    ext_id: u32,
    ext_present: bool,
    aspf: bool,
    lff: u32,
    hflag: bool,
    filts: bool,
    vernum: u32,
    chist: u32,
    pcmr: usize,
    sumf: bool,
    sums: bool,
    dialnorm: u32,
    hcrc_ok: bool,
}

fn parse_core(h: &[u8]) -> Result<Core, String> {
    if h.len() < 16 {
        return Err("header cut short".into());
    }
    let mut b = Bits::new(&h[4..]);
    let mut c = Core { normal: b.flag(), short: b.get(5), crc_present: b.flag(), nblks: b.get(7) + 1, fsize: b.get(14) as usize + 1, amode: b.get(6), rate_code: 0, ..Default::default() };
    c.rate_code = b.get(4);
    c.rate = SAMPLE_RATES[c.rate_code as usize];
    c.br_code = b.get(5) as usize;
    let fixed = b.get(1);
    c.dynf = b.flag();
    c.timef = b.flag();
    c.auxf = b.flag();
    c.hdcd = b.flag();
    c.ext_id = b.get(3);
    c.ext_present = b.flag();
    c.aspf = b.flag();
    c.lff = b.get(2);
    c.hflag = b.flag();
    if c.crc_present {
        let stored = b.get(16);
        // the checksum covers the header bytes after the sync word, its own two bytes included
        c.hcrc_ok = crc::dts(&h[4..4 + 7 + 2]) == 0;
        let _ = stored;
    }
    c.filts = b.flag();
    c.vernum = b.get(4);
    c.chist = b.get(2);
    c.pcmr = b.get(3) as usize;
    c.sumf = b.flag();
    c.sums = b.flag();
    c.dialnorm = b.get(4);
    if c.nblks < 6 {
        return Err(format!("{} sample blocks per frame (the minimum is 6)", c.nblks));
    }
    if c.fsize < 96 || c.fsize > 16_384 {
        return Err(format!("frame size {} bytes is out of range", c.fsize));
    }
    if c.rate == 0 {
        return Err(format!("invalid sampling frequency code {}", c.rate_code));
    }
    if c.amode >= 16 {
        return Err(format!("unsupported channel arrangement {}", c.amode));
    }
    if c.lff == 3 {
        return Err("invalid LFE flag".into());
    }
    if fixed != 0 {
        return Err("a reserved bit is set".into());
    }
    if PCM_BITS[c.pcmr] == 0 {
        return Err("invalid source PCM resolution".into());
    }
    Ok(c)
}

fn core_rows(c: &Core) -> Vec<(String, String)> {
    let mut v = vec![
        ("frame type (FTYPE)".to_string(), if c.normal { "normal".to_string() } else { format!("termination, {} deficit samples", c.short) }),
        ("header checksum present (CPF)".to_string(), c.crc_present.to_string()),
        ("sample blocks (NBLKS)".to_string(), format!("{} ({} samples)", c.nblks, c.nblks * 32)),
        ("frame size (FSIZE)".to_string(), format!("{} bytes", c.fsize)),
        ("channel arrangement (AMODE)".to_string(), format!("{} = {}", c.amode, AMODE_NAME[c.amode as usize])),
        ("sampling frequency (SFREQ)".to_string(), format!("{} = {} Hz", c.rate_code, c.rate)),
        ("transmission bit rate (RATE)".to_string(), match BIT_RATES[c.br_code] {
            0 => ["open", "variable", "lossless"].get(c.br_code.wrapping_sub(29)).copied().unwrap_or("?").to_string(),
            k => format!("{} = {k} kbps", c.br_code),
        }),
        ("dynamic range coefficients (DYNF)".to_string(), c.dynf.to_string()),
        ("time stamp (TIMEF)".to_string(), c.timef.to_string()),
        ("auxiliary data (AUXF)".to_string(), c.auxf.to_string()),
        ("HDCD (HDCD)".to_string(), c.hdcd.to_string()),
        ("extension (EXT_AUDIO_ID)".to_string(), if c.ext_present { EXT_AUDIO[c.ext_id as usize].to_string() } else { "none".to_string() }),
        ("LFE (LFF)".to_string(), ["none", "128x interpolation", "64x interpolation", "invalid"][c.lff as usize].to_string()),
        ("multirate filter bank (FILTS)".to_string(), if c.filts { "perfect reconstruction".to_string() } else { "non-perfect".to_string() }),
        ("encoder version (VERNUM)".to_string(), c.vernum.to_string()),
        ("source PCM resolution (PCMR)".to_string(), format!("{} bits{}", PCM_BITS[c.pcmr], if c.pcmr & 1 == 1 { " (ES)" } else { "" })),
        ("front sum/difference (SUMF)".to_string(), c.sumf.to_string()),
        ("surround sum/difference (SUMS)".to_string(), c.sums.to_string()),
        ("dialogue normalization (DIALNORM)".to_string(), format!("{}", c.dialnorm)),
    ];
    if c.crc_present {
        v.push(("header checksum matches".to_string(), c.hcrc_ok.to_string()));
    }
    v
}

// ------------------------------------------------------------------------------------------------
// extension substream

#[derive(Debug, Clone, Default)]
struct Exss {
    index: u32,
    wide: bool,
    header_size: usize,
    size: usize,
    hdr_crc_ok: bool,
    static_fields: bool,
    ref_clock: u32,
    frame_dur: u32,
    npresent: u32,
    nassets: u32,
    mix_metadata: bool,
    asset_size: usize,
    pcm_bits: u32,
    max_rate: u32,
    nchannels: u32,
    spk_mask: u32,
    one_to_one: bool,
    embedded_stereo: bool,
    embedded_6ch: bool,
    coding_mode: u32,
    ext_mask: u32,
    core_size: usize,
    xbr_size: usize,
    xxch_size: usize,
    x96_size: usize,
    lbr_size: usize,
    xll_size: usize,
    xll_sync_present: bool,
    xll_delay: u32,
    xll_sync_offset: usize,
    xll_pbr: u32,
    hd_stream_id: u32,
    dialnorm: Option<u32>,
    drc: Option<u32>,
    core_offset: usize,
    xbr_offset: usize,
    xxch_offset: usize,
    x96_offset: usize,
    lbr_offset: usize,
    xll_offset: usize,
}

fn mask_count(mut m: u32) -> u32 {
    let mut n = 0;
    while m != 0 {
        n += m & 1;
        m >>= 1;
    }
    n
}

fn parse_exss(d: &[u8]) -> Result<Exss, String> {
    if d.len() < 12 {
        return Err("header cut short".into());
    }
    let mut b = Bits::new(d);
    b.skip(32 + 8);
    let mut e = Exss { index: b.get(2), wide: b.flag(), ..Default::default() };
    let wide = e.wide as usize;
    e.header_size = b.get(8 + 4 * wide) as usize + 1;
    if e.header_size > d.len() {
        return Err("header larger than the data given".into());
    }
    e.hdr_crc_ok = crc::dts(&d[5..e.header_size]) == 0;
    let nbits = 16 + 4 * wide;
    e.size = b.get(nbits) as usize + 1;
    e.static_fields = b.flag();
    let mut mix_out = Vec::new();
    if e.static_fields {
        e.ref_clock = b.get(2);
        e.frame_dur = b.get(3);
        if b.flag() {
            b.skip(36);
        }
        e.npresent = b.get(3) + 1;
        e.nassets = b.get(3) + 1;
        let mut active = Vec::new();
        for _ in 0..e.npresent {
            active.push(b.get(e.index as usize + 1));
        }
        for m in &active {
            b.skip(mask_count(*m) as usize * 8);
        }
        e.mix_metadata = b.flag();
        if e.mix_metadata {
            b.skip(2);
            let nb = (b.get(2) as usize + 1) << 2;
            let n = b.get(2) + 1;
            for _ in 0..n {
                mix_out.push(mask_count(b.get(nb)));
            }
        }
    } else {
        e.npresent = 1;
        e.nassets = 1;
    }
    if e.nassets > 1 {
        return Err(format!("{} audio assets (only one is understood)", e.nassets));
    }
    e.asset_size = b.get(nbits) as usize + 1;
    // asset descriptor
    let descr_pos = b.pos;
    let descr_size = b.get(9) as usize + 1;
    b.get(3);
    if e.static_fields {
        if b.flag() {
            b.skip(4);
        }
        if b.flag() {
            b.skip(24);
        }
        if b.flag() {
            let n = b.get(10) as usize + 1;
            b.skip(n * 8);
        }
        e.pcm_bits = b.get(5) + 1;
        e.max_rate = SAMPLE_RATES_EXSS[b.get(4) as usize];
        e.nchannels = b.get(8) + 1;
        e.one_to_one = b.flag();
        if e.one_to_one {
            e.embedded_stereo = e.nchannels > 2 && b.flag();
            e.embedded_6ch = e.nchannels > 6 && b.flag();
            let mut nbits_mask = 0usize;
            if b.flag() {
                nbits_mask = (b.get(2) as usize + 1) << 2;
                e.spk_mask = b.get(nbits_mask);
            }
            let nsets = b.get(3) as usize;
            if nsets > 0 && nbits_mask == 0 {
                return Err("speaker mask disabled yet there are remapping sets".into());
            }
            let mut counts = Vec::new();
            for _ in 0..nsets {
                counts.push(mask_count(b.get(nbits_mask)) as usize);
            }
            for n in counts {
                let nch = b.get(5) as usize + 1;
                for _ in 0..n {
                    let m = b.get(nch);
                    b.skip(mask_count(m) as usize * 5);
                }
            }
        } else {
            b.get(3);
        }
    }
    let drc = b.flag();
    if drc {
        e.drc = Some(b.get(8));
    }
    if b.flag() {
        e.dialnorm = Some(b.get(5));
    }
    if drc && e.embedded_stereo {
        b.skip(8);
    }
    if e.mix_metadata && b.flag() {
        b.skip(1 + 6);
        if b.get(2) == 3 {
            b.skip(8);
        } else {
            b.skip(3);
        }
        if b.flag() {
            for n in &mix_out {
                b.skip(6 * *n as usize);
            }
        } else {
            b.skip(6 * mix_out.len());
        }
        let mut nd = e.nchannels as usize;
        if e.embedded_6ch {
            nd += 6;
        }
        if e.embedded_stereo {
            nd += 2;
        }
        for n in &mix_out {
            if *n == 0 {
                return Err("invalid speaker layout mask for a mixing configuration".into());
            }
            for _ in 0..nd {
                let m = b.get(*n as usize);
                b.skip(mask_count(m) as usize * 6);
            }
        }
    }
    e.coding_mode = b.get(2);
    match e.coding_mode {
        0 => {
            e.ext_mask = b.get(12);
            if e.ext_mask & 0x10 != 0 {
                e.core_size = b.get(14) as usize + 1;
                if b.flag() {
                    b.skip(2);
                }
            }
            if e.ext_mask & 0x20 != 0 {
                e.xbr_size = b.get(14) as usize + 1;
            }
            if e.ext_mask & 0x40 != 0 {
                e.xxch_size = b.get(14) as usize + 1;
            }
            if e.ext_mask & 0x80 != 0 {
                e.x96_size = b.get(12) as usize + 1;
            }
            if e.ext_mask & 0x100 != 0 {
                e.lbr_size = b.get(14) as usize + 1;
                if b.flag() {
                    b.skip(2);
                }
            }
            if e.ext_mask & 0x200 != 0 {
                parse_xll_params(&mut b, nbits, &mut e);
            }
            if e.ext_mask & 0x400 != 0 {
                b.skip(16);
            }
            if e.ext_mask & 0x800 != 0 {
                b.skip(16);
            }
        }
        1 => {
            e.ext_mask = 0x200;
            parse_xll_params(&mut b, nbits, &mut e);
        }
        2 => {
            e.ext_mask = 0x100;
            e.lbr_size = b.get(14) as usize + 1;
            if b.flag() {
                b.skip(2);
            }
        }
        _ => {
            b.skip(14 + 8);
            if b.flag() {
                b.skip(3);
            }
        }
    }
    if e.ext_mask & 0x200 != 0 {
        e.hd_stream_id = b.get(3);
    }
    if b.over {
        return Err("the header runs past the data given".into());
    }
    if descr_pos + descr_size * 8 > e.header_size * 8 {
        return Err("asset descriptor runs past the header".into());
    }
    // component offsets inside the asset
    let mut off = e.header_size;
    let mut left = e.asset_size;
    let mut place = |present: bool, size: usize, offset: &mut usize| -> Result<(), String> {
        if present {
            *offset = off;
            if size > left {
                return Err(format!("a component ({size} bytes) is larger than the {left} bytes left in the asset (asset {} bytes, ext mask 0x{:X}, header {} bytes)", e.asset_size, e.ext_mask, e.header_size));
            }
            off += size;
            left -= size;
        }
        Ok(())
    };
    place(e.ext_mask & 0x10 != 0, e.core_size, &mut e.core_offset)?;
    place(e.ext_mask & 0x20 != 0, e.xbr_size, &mut e.xbr_offset)?;
    place(e.ext_mask & 0x40 != 0, e.xxch_size, &mut e.xxch_offset)?;
    place(e.ext_mask & 0x80 != 0, e.x96_size, &mut e.x96_offset)?;
    place(e.ext_mask & 0x100 != 0, e.lbr_size, &mut e.lbr_offset)?;
    place(e.ext_mask & 0x200 != 0, e.xll_size, &mut e.xll_offset)?;
    Ok(e)
}

const SAMPLE_RATES_EXSS: [u32; 16] = [8000, 16000, 32000, 64000, 128000, 22050, 44100, 88200, 176400, 352800, 12000, 24000, 48000, 96000, 192000, 384000];

fn parse_xll_params(b: &mut Bits, nbits: usize, e: &mut Exss) {
    e.xll_size = b.get(nbits) as usize + 1;
    e.xll_sync_present = b.flag();
    if e.xll_sync_present {
        e.xll_pbr = b.get(4);
        let n = b.get(5) as usize + 1;
        e.xll_delay = b.get(n);
        e.xll_sync_offset = b.get(nbits) as usize;
    }
}

fn exss_rows(e: &Exss) -> Vec<(String, String)> {
    let mut v = vec![
        ("substream index".to_string(), e.index.to_string()),
        ("header".to_string(), format!("{} bytes ({}), checksum {}", e.header_size, if e.wide { "wide" } else { "short" }, if e.hdr_crc_ok { "ok" } else { "BAD" })),
        ("substream size".to_string(), format!("{} bytes", e.size)),
    ];
    if e.static_fields {
        v.push(("reference clock".to_string(), ["32 kHz", "44.1 kHz", "48 kHz", "reserved"][e.ref_clock as usize].to_string()));
        v.push(("frame duration".to_string(), format!("code {} ({} samples at the clock)", e.frame_dur, (e.frame_dur + 1) * 512)));
        v.push(("presentations / assets".to_string(), format!("{} / {}", e.npresent, e.nassets)));
        v.push(("mixing metadata".to_string(), e.mix_metadata.to_string()));
        v.push(("PCM resolution".to_string(), format!("{} bits", e.pcm_bits)));
        v.push(("maximum sample rate".to_string(), format!("{} Hz", e.max_rate)));
        v.push(("total channels".to_string(), e.nchannels.to_string()));
        if e.one_to_one {
            v.push(("embedded downmixes".to_string(), format!("stereo {}, 5.1 {}", e.embedded_stereo, e.embedded_6ch)));
            v.push(("speaker activity mask".to_string(), format!("0x{:X} = {}", e.spk_mask, speakers(e.spk_mask))));
        }
    }
    v.push(("asset size".to_string(), format!("{} bytes", e.asset_size)));
    v.push(("coding mode".to_string(), ["several components", "lossless only", "low bit rate only", "auxiliary"][e.coding_mode as usize].to_string()));
    let mut comps = Vec::new();
    for (bit, name, size) in [(0x10, "core", e.core_size), (0x20, "XBR", e.xbr_size), (0x40, "XXCH", e.xxch_size), (0x80, "X96", e.x96_size), (0x100, "LBR", e.lbr_size), (0x200, "XLL", e.xll_size)] {
        if e.ext_mask & bit != 0 {
            comps.push(format!("{name} {size} B"));
        }
    }
    v.push(("components in this substream".to_string(), if comps.is_empty() { "none".to_string() } else { comps.join(", ") }));
    if e.ext_mask & 0xF != 0 {
        let low: Vec<&str> = [(1, "core"), (4, "X96")].iter().filter(|(b, _)| e.ext_mask & b != 0).map(|(_, n)| *n).collect();
        v.push(("components kept in the core substream".to_string(), format!("mask 0x{:X}{}", e.ext_mask & 0xF, if low.is_empty() { String::new() } else { format!(" ({})", low.join(", ")) })));
    }
    if e.ext_mask & 0x200 != 0 {
        v.push(("XLL sync".to_string(), if e.xll_sync_present { format!("at +{}, decoding delay {} frames, peak bit rate buffer code {}", e.xll_sync_offset, e.xll_delay, e.xll_pbr) } else { "not signalled".to_string() }));
        v.push(("hd stream id".to_string(), e.hd_stream_id.to_string()));
    }
    if let Some(d) = e.dialnorm {
        v.push(("dialogue normalization".to_string(), format!("-{d} dBFS")));
    }
    if let Some(d) = e.drc {
        v.push(("dynamic range code".to_string(), d.to_string()));
    }
    v
}

fn speakers(mask: u32) -> String {
    let names: Vec<&str> = (0..16).filter(|b| mask >> b & 1 == 1).map(|b| SPEAKERS[b].0).collect();
    if names.is_empty() {
        "none".to_string()
    } else {
        names.join(" + ")
    }
}

fn speaker_count(mask: u32) -> u32 {
    (0..16).filter(|b| mask >> b & 1 == 1).map(|b| SPEAKERS[b].1).sum()
}

// ------------------------------------------------------------------------------------------------
// XLL

#[derive(Debug, Default, Clone)]
struct XllInfo {
    version: u32,
    header_size: usize,
    frame_size: usize,
    nchsets: u32,
    nsegs: u32,
    seg_samples: u32,
    seg_size_nbits: u32,
    band_crc: u32,
    scalable_lsbs: bool,
    ch_mask_nbits: u32,
    fixed_lsb: u32,
    common_crc_ok: bool,
    chset_headers: u32,
    chset_header_bad: u32,
    chset_bytes: Vec<usize>,
}

fn parse_xll(d: &[u8]) -> Result<XllInfo, String> {
    if d.len() < 12 || be32(d) != SYNC_XLL {
        return Err("no XLL sync word".into());
    }
    let mut b = Bits::new(&d[4..]);
    let mut x = XllInfo { version: b.get(4) + 1, ..Default::default() };
    x.header_size = b.get(8) as usize + 1;
    if x.header_size > d.len() {
        return Err("XLL header runs past the data".into());
    }
    x.common_crc_ok = crc::dts(&d[4..x.header_size]) == 0;
    let n = b.get(5) as usize + 1;
    x.frame_size = b.get(n) as usize + 1;
    x.nchsets = b.get(4) + 1;
    let seg_log2 = b.get(4);
    x.nsegs = 1 << seg_log2;
    let ss = b.get(4);
    x.seg_samples = if ss == 0 { 0 } else { 1 << ss };
    x.seg_size_nbits = b.get(5) + 1;
    x.band_crc = b.get(2);
    x.scalable_lsbs = b.flag();
    x.ch_mask_nbits = b.get(5) + 1;
    if x.scalable_lsbs {
        x.fixed_lsb = b.get(4);
    }
    if x.seg_samples == 0 {
        return Err("no samples per XLL segment".into());
    }
    if x.nsegs > 1024 || x.seg_samples > 512 {
        return Err("XLL segment structure out of range".into());
    }
    // channel set headers follow, each ending in its own checksum
    let mut pos = x.header_size;
    for _ in 0..x.nchsets {
        if pos + 3 > d.len() {
            break;
        }
        let size = (((d[pos] as usize) << 2) | (d[pos + 1] as usize >> 6)) + 1;
        x.chset_headers += 1;
        if pos + size > d.len() {
            // the rest of the frame (smoothing buffer) is in the next substream: nothing more to check here
            x.chset_bytes.push(size);
            break;
        }
        if crc::dts(&d[pos..pos + size]) != 0 {
            x.chset_header_bad += 1;
        }
        x.chset_bytes.push(size);
        pos += size;
    }
    Ok(x)
}

/// The sync word is there but the common header (its size is in the first bytes) reaches beyond the data given.
fn parse_xll_runs_past(d: &[u8]) -> bool {
    d.len() >= 6 && (((d[4] as usize) & 0x0F) << 4 | (d[5] as usize) >> 4) + 1 > d.len()
}

fn xll_rows(x: &XllInfo) -> Vec<(String, String)> {
    vec![
        ("version".to_string(), x.version.to_string()),
        ("common header".to_string(), format!("{} bytes, checksum {}", x.header_size, if x.common_crc_ok { "ok" } else { "BAD" })),
        ("frame size".to_string(), format!("{} bytes", x.frame_size)),
        ("channel sets".to_string(), x.nchsets.to_string()),
        ("channel set headers".to_string(), format!("{} bytes in all, {} of {} checksums ok", x.chset_bytes.iter().sum::<usize>(), x.chset_headers - x.chset_header_bad, x.chset_headers)),
        ("segments per frame".to_string(), x.nsegs.to_string()),
        ("samples per segment (per band)".to_string(), x.seg_samples.to_string()),
        ("samples per frame (per band)".to_string(), (x.nsegs * x.seg_samples).to_string()),
        ("segment size field".to_string(), format!("{} bits", x.seg_size_nbits)),
        ("band checksums (band_crc_present)".to_string(), ["none", "MSB part", "MSB and LSB parts", "every band"][x.band_crc as usize].to_string()),
        ("scalable LSBs".to_string(), if x.scalable_lsbs { format!("yes, fixed LSB width {}", x.fixed_lsb) } else { "no".to_string() }),
        ("channel mask field".to_string(), format!("{} bits", x.ch_mask_nbits)),
    ]
}

/// Which DTS:X marker, if any, sits on a 32-bit boundary of this lossless frame (counted from its first byte). The
/// lossless audio ends first and the marker follows it, so it is looked for from the first quarter of the frame on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum XMark {
    X,
    Imax,
}

fn find_x_marker(frame: &[u8]) -> Option<XMark> {
    let from = (frame.len() / 4) & !3;
    let mut i = from;
    while i + 4 <= frame.len() {
        let w = be32(&frame[i..]);
        if w == SYNC_XLL_X {
            return Some(XMark::X);
        }
        if w >> 1 == SYNC_XLL_X_IMAX >> 1 {
            return Some(XMark::Imax);
        }
        i += 4;
    }
    None
}

// ------------------------------------------------------------------------------------------------
// walking a run of frames

#[derive(Default)]
struct Walk {
    aus: u64,
    cores: u64,
    exss: u64,
    core_hdr: Option<Core>,
    core_changes: u64,
    core_bad: u64,
    core_hcrc_bad: u64,
    ext_missing: u64,
    ext_checked: u64,
    exss_first: Option<Exss>,
    exss_bad: u64,
    exss_hdr_crc_bad: u64,
    comp_missing: u64,
    comp_checked: u64,
    xll_first: Option<XllInfo>,
    xll_bad: u64,
    xll_checked: u64,
    x_frames: u64,
    imax_frames: u64,
    xll_chset_bad: u64,
    junk: u64,
    junk_events: u64,
    pad: u64,
    core_samples: u64,
    frame_bytes: Vec<u32>,
    frame_samples: Vec<u32>,
    frame_starts: Vec<u64>,
    hd_rate: u32,
    trailing: u64,
    saw_core: bool,
    saw_xll: bool,
    saw_lbr: bool,
    ext_kinds: std::collections::BTreeSet<&'static str>,
    secondary: bool,
}

fn walk(w: &mut Window, from: u64, to: u64, c: &mut Collector, record: bool) -> Walk {
    let mut s = Walk::default();
    let mut pos = from;
    let mut prev_was_core = false;
    let mut unit_bytes = 0u32;
    let mut unit_samples = 0u32;
    let mut have_unit = false;
    let mut exss_frame_samples: Option<u32> = None;
    let flush = |s: &mut Walk, have: &mut bool, bytes: &mut u32, samples: &mut u32| {
        if *have {
            s.frame_bytes.push(*bytes);
            s.frame_samples.push(*samples);
            *have = false;
        }
    };
    while pos + 4 <= to {
        let head = w.get(pos, 64).to_vec();
        if head.len() < 4 {
            break;
        }
        let sync = be32(&head);
        if sync == CORE_SYNC {
            let h = match parse_core(&head) {
                Ok(h) => h,
                Err(e) => {
                    s.core_bad += 1;
                    c.finding(Status::Fail, Some(pos), format!("core frame header: {e}"));
                    pos += 4;
                    continue;
                }
            };
            if pos + h.fsize as u64 > to {
                s.trailing = to - pos;
                c.finding(Status::Fail, Some(pos), format!("core frame is cut short ({} of {} bytes)", to - pos, h.fsize));
                break;
            }
            flush(&mut s, &mut have_unit, &mut unit_bytes, &mut unit_samples);
            s.aus += 1;
            s.cores += 1;
            s.saw_core = true;
            have_unit = true;
            unit_bytes = h.fsize as u32;
            unit_samples = h.nblks * 32 * if h.ext_present && (h.ext_id == 2 || h.ext_id == 3) { 1 } else { 1 };
            if record {
                s.frame_starts.push(pos);
            }
            s.core_samples += h.nblks as u64 * 32;
            if h.crc_present && !h.hcrc_ok {
                s.core_hcrc_bad += 1;
            }
            // extension inside the core
            if h.ext_present {
                let frame = w.get(pos, h.fsize).to_vec();
                let sync = match h.ext_id {
                    0 | 3 => Some((SYNC_XCH, "XCH")),
                    2 => Some((SYNC_X96, "X96")),
                    6 => Some((SYNC_XXCH, "XXCH")),
                    _ => None,
                };
                if let Some((word, name)) = sync {
                    s.ext_checked += 1;
                    let needle = word.to_be_bytes();
                    let found = frame.windows(4).skip(16).any(|win| win == needle);
                    let found = found || (h.ext_id == 3 && frame.windows(4).any(|win| win == SYNC_X96.to_be_bytes()));
                    if !found {
                        s.ext_missing += 1;
                        if s.ext_missing <= 3 {
                            c.finding(Status::Fail, Some(pos), format!("the core header announces {name} but its sync word is not in the frame"));
                        }
                    } else {
                        s.ext_kinds.insert(name);
                    }
                }
            }
            match &s.core_hdr {
                None => s.core_hdr = Some(h.clone()),
                Some(f) => {
                    if f.rate != h.rate || f.amode != h.amode || f.lff != h.lff || f.nblks != h.nblks || f.ext_id != h.ext_id || f.ext_present != h.ext_present || f.pcmr != h.pcmr {
                        s.core_changes += 1;
                        if s.core_changes <= 5 {
                            c.finding(Status::Warn, Some(pos), "core parameters change here (rate, channels, frame length or extension)");
                        }
                    }
                }
            }
            pos += h.fsize as u64;
            prev_was_core = true;
            continue;
        }
        if sync == EXSS_SYNC {
            let hdr = w.get(pos, 8192.min((to - pos) as usize)).to_vec();
            let e = match parse_exss(&hdr) {
                Ok(e) => e,
                Err(msg) => {
                    s.exss_bad += 1;
                    c.finding(Status::Fail, Some(pos), format!("extension substream header: {msg}"));
                    pos += 4;
                    continue;
                }
            };
            if !e.hdr_crc_ok {
                s.exss_hdr_crc_bad += 1;
                c.finding(Status::Fail, Some(pos), "extension substream header checksum mismatch");
            }
            if pos + e.size as u64 > to {
                s.trailing = to - pos;
                c.finding(Status::Fail, Some(pos), format!("extension substream is cut short ({} of {} bytes)", to - pos, e.size));
                break;
            }
            // a substream after a core belongs to the same access unit; otherwise it starts one
            if !prev_was_core {
                flush(&mut s, &mut have_unit, &mut unit_bytes, &mut unit_samples);
                s.aus += 1;
                have_unit = true;
                unit_bytes = 0;
                unit_samples = 0;
                if record {
                    s.frame_starts.push(pos);
                }
            }
            s.exss += 1;
            unit_bytes += e.size as u32;
            if e.index == 2 {
                s.secondary = true;
            }
            // components
            let body = w.get(pos, e.size).to_vec();
            let check = |s: &mut Walk, c: &mut Collector, present: bool, off: usize, sync: u32, name: &str, extra: usize| {
                if !present {
                    return;
                }
                s.comp_checked += 1;
                let at = off + extra;
                if body.len() >= at + 4 && be32(&body[at..]) == sync {
                    return;
                }
                s.comp_missing += 1;
                if s.comp_missing <= 3 {
                    c.finding(Status::Fail, Some(pos + at as u64), format!("the {name} component does not start with its sync word"));
                }
            };
            check(&mut s, c, e.ext_mask & 0x10 != 0, e.core_offset, SYNC_EXSS_CORE, "core", 0);
            check(&mut s, c, e.ext_mask & 0x20 != 0, e.xbr_offset, SYNC_XBR, "XBR", 0);
            check(&mut s, c, e.ext_mask & 0x40 != 0, e.xxch_offset, SYNC_XXCH, "XXCH", 0);
            check(&mut s, c, e.ext_mask & 0x80 != 0, e.x96_offset, SYNC_X96, "X96", 0);
            check(&mut s, c, e.ext_mask & 0x100 != 0, e.lbr_offset, SYNC_LBR, "LBR", 0);
            if e.ext_mask & 0x40 != 0 {
                s.ext_kinds.insert("XXCH");
            }
            if e.ext_mask & 0x20 != 0 {
                s.ext_kinds.insert("XBR");
            }
            if e.ext_mask & 0x80 != 0 {
                s.ext_kinds.insert("X96");
            }
            if e.ext_mask & 0x100 != 0 {
                s.saw_lbr = true;
            }
            if e.ext_mask & 0x200 != 0 {
                s.saw_xll = true;
                // with the peak-rate smoothing buffer a lossless frame is spread over several substreams: only a
                // component that says it holds the start of a frame (sync present) has a header to check, and the
                // frame may run past the end of the component
                if !e.xll_sync_present {
                    if s.exss_first.is_none() {
                        s.exss_first = Some(e.clone());
                    }
                    if !prev_was_core {
                        unit_samples = exss_frame_samples.unwrap_or(0);
                    }
                    pos += e.size as u64;
                    prev_was_core = false;
                    continue;
                }
                let xoff = e.xll_offset + e.xll_sync_offset;
                s.xll_checked += 1;
                match body.get(xoff..).map(parse_xll) {
                    Some(Ok(x)) => {
                        if !x.common_crc_ok {
                            s.xll_bad += 1;
                            c.finding(Status::Fail, Some(pos + xoff as u64), "XLL common header checksum mismatch");
                        }
                        if x.chset_header_bad > 0 {
                            s.xll_chset_bad += 1;
                            c.finding(Status::Fail, Some(pos + xoff as u64), format!("{} XLL channel set header checksum(s) mismatch", x.chset_header_bad));
                        }
                        if e.xll_pbr == 0 && x.frame_size > e.xll_size {
                            s.xll_bad += 1;
                            c.finding(Status::Fail, Some(pos + xoff as u64), format!("XLL frame ({} bytes) is larger than its component ({} bytes)", x.frame_size, e.xll_size));
                        }
                        if let Some(frame) = body.get(xoff..xoff + x.frame_size.min(body.len() - xoff)) {
                            match find_x_marker(frame) {
                                Some(XMark::X) => s.x_frames += 1,
                                Some(XMark::Imax) => s.imax_frames += 1,
                                None => {}
                            }
                        }
                        // lossless-only frames set the access unit length
                        if exss_frame_samples.is_none() {
                            exss_frame_samples = Some(x.nsegs * x.seg_samples);
                        }
                        if s.xll_first.is_none() {
                            s.xll_first = Some(x);
                        }
                    }
                    Some(Err(_)) if body.len().saturating_sub(xoff) >= 4 && be32(&body[xoff..]) == SYNC_XLL && (body.len() - xoff < 16 || parse_xll_runs_past(&body[xoff..])) => {
                        // the frame starts here with its sync word, but its header continues in the next substream
                    }
                    Some(Err(msg)) => {
                        s.xll_bad += 1;
                        let avail = body.len().saturating_sub(xoff);
                        let first = body.get(xoff..xoff + 4.min(avail)).map(|b| b.iter().map(|x| format!("{x:02X}")).collect::<String>()).unwrap_or_default();
                        c.finding(Status::Fail, Some(pos + xoff as u64), format!("XLL frame: {msg} ({avail} bytes left in the substream, first word {first}, sync offset {}, component {} bytes at {})", e.xll_sync_offset, e.xll_size, e.xll_offset));
                    }
                    None => {
                        s.xll_bad += 1;
                        c.finding(Status::Fail, Some(pos + xoff as u64), "the XLL component lies outside the substream");
                    }
                }
            }
            if s.exss_first.is_none() {
                if e.static_fields {
                    s.hd_rate = e.max_rate;
                }
                s.exss_first = Some(e.clone());
            }
            if !prev_was_core {
                unit_samples = exss_frame_samples.unwrap_or(0);
            }
            pos += e.size as u64;
            prev_was_core = false;
            // keep access-unit grouping: the next substream may still belong to this unit only after a core
            continue;
        }
        // padding (zero bytes up to a word boundary)
        if head[0] == 0 {
            let mut n = 0u64;
            while pos + n < to {
                let chunk = w.get(pos + n, 4096);
                if chunk.is_empty() {
                    break;
                }
                let z = chunk.iter().take_while(|b| **b == 0).count();
                n += z as u64;
                if z < chunk.len() {
                    break;
                }
            }
            if n > 0 {
                s.pad += n;
                unit_bytes += n as u32;
                pos += n;
                continue;
            }
        }
        // junk: find the next sync word
        s.junk_events += 1;
        let mut q = pos + 1;
        let mut found = None;
        'find: while q < to && q < pos + (1 << 20) {
            let chunk = w.get(q, 1 << 16).to_vec();
            if chunk.len() < 4 {
                break;
            }
            for i in 0..chunk.len() - 3 {
                let wd = be32(&chunk[i..]);
                if wd == CORE_SYNC || wd == EXSS_SYNC {
                    found = Some(q + i as u64);
                    break 'find;
                }
            }
            q += chunk.len() as u64 - 3;
        }
        let end = found.unwrap_or(to.min(pos + (1 << 20)));
        s.junk += end - pos;
        c.finding(Status::Fail, Some(pos), format!("{} bytes that are not a DTS frame", end - pos));
        pos = end;
        prev_was_core = false;
    }
    flush(&mut s, &mut have_unit, &mut unit_bytes, &mut unit_samples);
    s
}

// ------------------------------------------------------------------------------------------------
// the container

struct Chunk {
    name: String,
    off: u64,
    size: u64,
}

fn be24(d: &[u8]) -> u32 {
    ((d[0] as u32) << 16) | ((d[1] as u32) << 8) | d[2] as u32
}

pub(crate) fn verify(w: &mut Window, c: &mut Collector, container: bool) -> Probe {
    let len = w.len;
    let mut p = Probe { dts: true, ..Default::default() };
    let (mut start, mut end) = (0u64, len);
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut aupr: Option<(u32, u32, u32, u64, u32, u32)> = None; // rate, frames, samples per frame, total samples, mask, delay
    let mut navi: Option<(u32, u32, Vec<u64>)> = None;
    let mut blackout: Option<(u64, u64)> = None;
    let mut fps = 0u8;
    let mut hd_flags = 0u8;
    let mut chunk_problems = 0u32;
    let mut core_md: Option<(u32, u32, u32, u32)> = None;
    let mut ext_md: Option<Vec<u8>> = None;
    let mut timecode: Option<Vec<u8>> = None;
    let mut build = String::new();
    if container {
        let mut pos = 0u64;
        let mut stream_found = false;
        while pos + 16 <= len {
            let h = w.get(pos, 16).to_vec();
            let name = String::from_utf8_lossy(&h[..8]).to_string();
            let size = u64::from_be_bytes(h[8..16].try_into().unwrap());
            if !name.bytes().all(|b| (0x20..0x7F).contains(&b)) {
                chunk_problems += 1;
                c.finding(Status::Fail, Some(pos), "a container chunk has an unreadable name");
                break;
            }
            let body_off = pos + 16;
            if body_off + size > len {
                chunk_problems += 1;
                c.finding(Status::Fail, Some(pos), format!("chunk {name} claims {size} bytes but only {} remain", len - body_off));
                if name == "STRMDATA" {
                    start = body_off;
                    end = len;
                    stream_found = true;
                }
                chunks.push(Chunk { name, off: body_off, size: len - body_off });
                break;
            }
            let body = if size <= 1 << 20 && name != "STRMDATA" { w.get(body_off, size as usize).to_vec() } else { Vec::new() };
            match name.trim_end() {
                "DTSHDHDR" if body.len() >= 12 => {
                    fps = body[9];
                    hd_flags = body[11];
                }
                "CORESSMD" if body.len() >= 11 => core_md = Some((be24(&body), u16::from_be_bytes([body[3], body[4]]) as u32, u16::from_be_bytes([body[5], body[6]]) as u32, be24(&body[8..]))),
                "EXTSS_MD" => ext_md = Some(body.clone()),
                "AUPR-HDR" if body.len() >= 21 => {
                    let total = ((u32::from_be_bytes(body[12..16].try_into().unwrap()) as u64) << 8) | body[16] as u64;
                    aupr = Some((be24(&body[3..]), u32::from_be_bytes(body[6..10].try_into().unwrap()), u16::from_be_bytes([body[10], body[11]]) as u32, total, u16::from_be_bytes([body[17], body[18]]) as u32, u16::from_be_bytes([body[19], body[20]]) as u32));
                }
                "STRMDATA" => {
                    start = body_off;
                    end = body_off + size;
                    stream_found = true;
                }
                "NAVI-TBL" if body.len() >= 7 => {
                    let n = u32::from_be_bytes(body[..4].try_into().unwrap());
                    let width = body[6] as usize;
                    let mut offs = Vec::new();
                    if (1..=4).contains(&width) {
                        for i in 0..n as usize {
                            let a = 7 + i * width;
                            if a + width > body.len() {
                                break;
                            }
                            offs.push(body[a..a + width].iter().fold(0u64, |v, b| (v << 8) | *b as u64));
                        }
                    }
                    navi = Some((n, width as u32, offs));
                }
                "TIMECODE" => timecode = Some(body.clone()),
                "BUILDVER" => build = String::from_utf8_lossy(&body).replace(['\0', '\r'], "").replace('\n', "; ").trim().to_string(),
                "BLACKOUT" => blackout = Some((body_off, size)),
                _ => {}
            }
            chunks.push(Chunk { name, off: body_off, size });
            pos = body_off + size;
            if pos == len {
                break;
            }
        }
        c.check("Container chunks", if chunk_problems == 0 && stream_found { Status::Pass } else { Status::Fail }, if chunk_problems == 0 && stream_found { format!("{} chunks, sizes consistent with the file", chunks.len()) } else if !stream_found { "there is no STRMDATA chunk".to_string() } else { "a chunk is damaged or runs past the end of the file".to_string() });
        let need = ["DTSHDHDR", "AUPR-HDR", "STRMDATA", "TIMECODE"];
        let missing: Vec<&str> = need.iter().copied().filter(|n| !chunks.iter().any(|c| c.name.trim_end() == *n)).collect();
        c.check("Required chunks", if missing.is_empty() { Status::Pass } else { Status::Fail }, if missing.is_empty() { "DTSHDHDR, AUPR-HDR, STRMDATA and TIMECODE are present".to_string() } else { format!("missing: {}", missing.join(", ")) });
        if !stream_found {
            return p;
        }
    }

    let s = walk(w, start, end, c, container);
    // ---- report the walk
    let core = s.core_hdr.clone();
    // The rate of the decoded audio: the core's, doubled by an X96 extension, or what the extension substream /
    // container states when that is higher (a 96 kHz lossless stream over a 48 kHz core).
    let core_rate = core.as_ref().map(|k| k.rate).unwrap_or(0);
    let x96 = core.as_ref().map_or(false, |k| k.ext_present && (k.ext_id == 2 || k.ext_id == 3));
    let mut rate = if x96 { core_rate * 2 } else { core_rate };
    rate = rate.max(s.exss_first.as_ref().map_or(0, |e| e.max_rate));
    if s.saw_core && aupr.map_or(false, |a| a.0 > rate && core_rate > 0 && a.0 % core_rate == 0) {
        rate = aupr.unwrap().0;
    }
    if !s.saw_core {
        rate = aupr.map(|a| a.0).or(s.exss_first.as_ref().map(|e| e.max_rate)).unwrap_or(0);
    }
    p.sample_rate = rate;
    p.has_core = s.saw_core;
    p.lossless = s.saw_xll;
    let ratio = if s.saw_core && core_rate > 0 { (rate / core_rate).max(1) } else { 1 };
    let unit_samples: Vec<u32> = if !s.saw_core && aupr.map_or(false, |a| a.2 > 0) {
        s.frame_samples.iter().map(|_| aupr.unwrap().2).collect()
    } else if s.frame_samples.iter().all(|x| *x > 0) {
        s.frame_samples.iter().map(|x| x * ratio).collect()
    } else if let Some(a) = aupr {
        s.frame_samples.iter().map(|x| if *x == 0 { a.2 } else { *x * ratio }).collect()
    } else {
        s.frame_samples.clone()
    };
    let total_samples: u64 = unit_samples.iter().map(|x| *x as u64).sum();
    p.frame_bytes = s.frame_bytes.clone();
    p.frame_samples = unit_samples.clone();
    p.duration_secs = if rate > 0 { total_samples as f64 / rate as f64 } else { 0.0 };
    let nch_core = core.as_ref().map(|k| AMODE_CH[k.amode as usize] + u32::from(k.lff != 0)).unwrap_or(0);
    let nch_exss = s.exss_first.as_ref().map(|e| e.nchannels).unwrap_or(0);
    let _ = speaker_count;
    p.channels = nch_core.max(nch_exss);

    let mut parts: Vec<String> = Vec::new();
    if s.saw_core {
        parts.push("core".to_string());
    }
    for k in &s.ext_kinds {
        parts.push((*k).to_string());
    }
    if s.saw_xll {
        parts.push("XLL (lossless)".to_string());
    }
    if s.saw_lbr {
        parts.push("LBR (Express)".to_string());
    }
    let name = if s.saw_xll { "DTS-HD Master Audio" } else if s.saw_lbr { "DTS Express" } else if s.exss > 0 && s.ext_kinds.contains("XBR") || s.ext_kinds.contains("XXCH") && s.exss > 0 { "DTS-HD High Resolution Audio" } else if s.ext_kinds.contains("X96") { "DTS 96/24" } else if s.ext_kinds.contains("XCH") { "DTS-ES" } else { "DTS Digital Surround" };
    c.kv("Format", format!("{name} ({})", parts.join(" + ")));
    c.kv("Sample rate", format!("{rate} Hz"));
    if s.saw_xll {
        // a chance match in audio data is possible in a long file, so the marker must be there in most frames
        let (hits, kind) = if s.imax_frames > s.x_frames { (s.imax_frames, "IMAX Enhanced") } else { (s.x_frames, "DTS:X") };
        if hits * 2 >= s.xll_checked && hits > 0 {
            c.kv("DTS:X", format!("yes: {kind} object data follows the lossless audio (marker in {hits} of {} lossless frames)", s.xll_checked));
            c.check("DTS:X signalling", Status::Info, format!("{kind} marker found in {hits} of {} lossless frames; the object data is not decoded by FFmpeg, so the audio check covers the channel bed", s.xll_checked));
        } else {
            c.kv("DTS:X", "no");
            if hits > 0 {
                c.check("DTS:X signalling", Status::Info, format!("a DTS:X-like marker appeared in {hits} of {} lossless frames, too few to call it DTS:X (most likely a chance match in audio data)", s.xll_checked));
            }
        }
    }
    if let Some(k) = &core {
        c.kv("Core channels", format!("{} {}", AMODE_NAME[k.amode as usize], if k.lff != 0 { "+ LFE" } else { "" }));
    }
    if let Some(e) = &s.exss_first {
        if e.static_fields {
            c.kv("Extension channels", format!("{} ({})", e.nchannels, speakers(e.spk_mask)));
            c.kv("Source resolution", format!("{} bits", e.pcm_bits));
        }
    } else if let Some(k) = &core {
        c.kv("Source resolution", format!("{} bits", PCM_BITS[k.pcmr]));
    }
    c.kv("Access units", s.aus.to_string());
    c.kv("Duration", format!("{:.3} s", p.duration_secs));
    if p.duration_secs > 0.0 {
        c.kv("Average bit rate", format!("{:.1} kbps", (end - start) as f64 * 8.0 / p.duration_secs / 1000.0));
    }

    let ok = |n: u64| if n == 0 { Status::Pass } else { Status::Fail };
    c.check("Frame structure", ok(s.junk_events), if s.junk_events == 0 { format!("{} access units, each substream starts where the previous one ends{}", s.aus, if s.pad > 0 { format!(" ({} padding bytes)", s.pad) } else { String::new() }) } else { format!("{} stretch(es) of data that is not a frame ({} bytes)", s.junk_events, s.junk) });
    if s.cores > 0 {
        c.check("Core headers", ok(s.core_bad), if s.core_bad == 0 { format!("{} core frames hold legal values", s.cores) } else { format!("{} invalid core header(s)", s.core_bad) });
        if s.core_hdr.as_ref().map_or(false, |k| k.crc_present) {
            c.check("Core header checksum", if s.core_hcrc_bad == 0 { Status::Pass } else { Status::Warn }, if s.core_hcrc_bad == 0 { "matches".to_string() } else { format!("{} frame(s) do not match the range assumed here (not necessarily an error)", s.core_hcrc_bad) });
        }
        if s.ext_checked > 0 {
            c.check("Core extension sync words", ok(s.ext_missing), if s.ext_missing == 0 { format!("{} extension headers found where announced", s.ext_checked) } else { format!("{} of {} frames lack the announced extension", s.ext_missing, s.ext_checked) });
        }
        c.check("Stable core parameters", if s.core_changes == 0 { Status::Pass } else { Status::Warn }, if s.core_changes == 0 { "rate, channels and frame length never change".to_string() } else { format!("parameters change {} time(s)", s.core_changes) });
    }
    if s.exss > 0 {
        c.check("Extension substream headers", if s.exss_bad == 0 && s.exss_hdr_crc_bad == 0 { Status::Pass } else { Status::Fail }, if s.exss_bad == 0 && s.exss_hdr_crc_bad == 0 { format!("{} headers parsed, checksums ok", s.exss) } else { format!("{} unreadable, {} with a bad checksum", s.exss_bad, s.exss_hdr_crc_bad) });
        if s.comp_checked > 0 {
            c.check("Extension component sync words", ok(s.comp_missing), if s.comp_missing == 0 { format!("{} components start with their sync word", s.comp_checked) } else { format!("{} of {} components lack their sync word", s.comp_missing, s.comp_checked) });
        }
        if s.xll_checked > 0 {
            c.check("Lossless (XLL) frame headers", if s.xll_bad == 0 && s.xll_chset_bad == 0 { Status::Pass } else { Status::Fail }, if s.xll_bad == 0 && s.xll_chset_bad == 0 { format!("{} frames: common and channel set header checksums ok", s.xll_checked) } else { format!("{} frame(s) with a bad common header, {} with bad channel set headers", s.xll_bad, s.xll_chset_bad) });
        }
    }
    if s.saw_core && !s.saw_xll {
        c.check("Audio data integrity", Status::Info, "the DTS core format has no checksum over its audio data, so damage inside the audio can only be found by decoding (deep verification)");
    }
    if s.trailing > 0 {
        c.check("Complete file", Status::Fail, format!("the stream ends with {} bytes of an unfinished frame", s.trailing));
    } else {
        c.check("Complete file", Status::Pass, "the stream ends on a frame boundary");
    }

    // ---- container cross-checks
    if container {
        if let Some((rate_a, frames_a, spf, total, _mask, delay)) = aupr {
            c.check("Frame count (AUPR-HDR)", if frames_a as u64 == s.aus { Status::Pass } else { Status::Fail }, format!("header says {frames_a}, the stream holds {}", s.aus));
            if rate != 0 {
                c.check("Sample rate (AUPR-HDR)", if rate_a == rate { Status::Pass } else { Status::Fail }, format!("header says {rate_a} Hz, the stream says {rate} Hz"));
            }
            let expect = frames_a as u64 * spf as u64;
            // the stream carries the codec delay and a flush frame beyond the programme
            let slack = delay as u64 + 2 * spf as u64;
            c.check("Programme length (AUPR-HDR)", if total <= expect && total + slack >= expect { Status::Pass } else { Status::Warn }, format!("{total} samples stated; {frames_a} frames of {spf} samples hold {expect} (codec delay {delay})"));
            if p.duration_secs > 0.0 && spf > 0 {
                let actual = unit_samples.iter().filter(|x| **x == spf).count();
                if actual as u64 != s.aus {
                    c.check("Frame length (AUPR-HDR)", Status::Warn, format!("header says {spf} samples per frame; {} of {} frames have that length", actual, s.aus));
                }
            }
            c.kv("Codec delay (container)", format!("{delay} samples"));
            c.kv("Programme length (container)", format!("{total} samples"));
        }
        if let Some((n, _w, offs)) = &navi {
            let matches = offs.len() as u64 == s.aus && offs.iter().zip(&s.frame_starts).all(|(a, b)| *a == b - start);
            c.check("Frame offset table (NAVI-TBL)", if matches && *n as u64 == s.aus { Status::Pass } else { Status::Fail }, if matches { format!("{n} offsets match the frames found") } else { format!("table lists {n} frames; the stream has {}, offsets differ", s.aus) });
        } else if hd_flags & 0x04 != 0 || hd_flags & 0x01 != 0 {
            c.check("Frame offset table (NAVI-TBL)", Status::Warn, "the stream is marked variable rate but has no offset table");
        }
        if let Some(tc) = &timecode {
            let clock = if tc.len() >= 4 { u32::from_be_bytes(tc[..4].try_into().unwrap()) } else { 0 };
            let okc = tc.len() >= 8 && clock == 48_000 && (1..=7).contains(&tc[4]);
            c.check("Timecode chunk", if okc { Status::Pass } else { Status::Warn }, format!("clock {clock} Hz, frame rate code {}", tc.get(4).copied().unwrap_or(0)));
        }
        if let Some((off, size)) = blackout {
            let hole = {
                // the silent frame must itself be a valid frame
                let mut tmp = Collector { report: Default::default() };
                let b = walk(w, off, off + size, &mut tmp, false);
                (b.aus, b.junk_events + b.core_bad + b.exss_bad + b.exss_hdr_crc_bad + b.comp_missing + b.xll_bad)
            };
            c.check("Silent frame (BLACKOUT)", if hole.0 >= 1 && hole.1 == 0 { Status::Pass } else { Status::Fail }, if hole.0 >= 1 && hole.1 == 0 { format!("{size} bytes, a valid frame (lets Add silence work)") } else { "the stored silent frame does not parse".to_string() });
        } else {
            c.check("Silent frame (BLACKOUT)", Status::Info, "none; the DTS tools cannot add silence to this file");
        }
        if c.report.deep {
            let mut rows: Vec<(String, String)> = Vec::new();
            for ch in &chunks {
                rows.push((ch.name.trim_end().to_string(), format!("{} bytes at offset {}", ch.size, ch.off)));
            }
            c.section("Container chunks", rows);
            let mut rows = vec![("frame rate code".to_string(), fps.to_string()), ("stream flags".to_string(), format!("0x{hd_flags:02X}"))];
            if let Some((r, kb, m, pl)) = core_md {
                rows.push(("CORESSMD".to_string(), format!("{r} Hz, {kb} kbps, speaker mask 0x{m:X}, frame payload {pl} bytes")));
            }
            if let Some(e) = &ext_md {
                rows.push(("EXTSS_MD".to_string(), e.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")));
            }
            if let Some(tc) = &timecode {
                rows.push(("TIMECODE".to_string(), tc.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")));
            }
            if !build.is_empty() {
                rows.push(("BUILDVER".to_string(), build.clone()));
            }
            c.section("Container metadata", rows);
        }
    }
    if c.report.deep {
        if let Some(k) = &s.core_hdr {
            c.section("Core substream header (first frame)", core_rows(k));
        }
        if let Some(e) = &s.exss_first {
            c.section("Extension substream header (first frame)", exss_rows(e));
        }
        if let Some(x) = &s.xll_first {
            c.section("Lossless (XLL) frame header (first frame)", xll_rows(x));
        }
    }
    p
}


#[cfg(test)]
mod tests {
    use super::*;

    fn frame_with(word: Option<u32>, at: usize, len: usize) -> Vec<u8> {
        let mut f: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
        if let Some(w) = word {
            f[at..at + 4].copy_from_slice(&w.to_be_bytes());
        }
        f
    }

    #[test]
    fn the_dtsx_marker_is_found_on_a_dword_boundary_after_the_audio() {
        assert_eq!(find_x_marker(&frame_with(Some(SYNC_XLL_X), 4000, 4096)), Some(XMark::X));
        assert_eq!(find_x_marker(&frame_with(Some(SYNC_XLL_X_IMAX), 3000, 4096)), Some(XMark::Imax));
        // the low bit of the IMAX word is free, as in the decoder's own test
        assert_eq!(find_x_marker(&frame_with(Some(SYNC_XLL_X_IMAX | 1), 3000, 4096)), Some(XMark::Imax));
        // not on a dword boundary, or in the first quarter (the lossless headers): ignored
        assert_eq!(find_x_marker(&frame_with(Some(SYNC_XLL_X), 4001, 4096)), None);
        assert_eq!(find_x_marker(&frame_with(Some(SYNC_XLL_X), 400, 4096)), None);
        assert_eq!(find_x_marker(&frame_with(None, 0, 4096)), None);
    }
}
