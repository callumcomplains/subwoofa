//! The core substream's frame header (ETSI TS 102 114 clause 5.3): parsing it, checking its values and listing it.

use crate::bits::Bits;
use crate::crc;
use crate::dts::consts::{AMODE_NAME, BIT_RATES, EXT_AUDIO, PCM_BITS, SAMPLE_RATES};

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Core {
    pub(crate) normal: bool,
    pub(crate) short: u32,
    pub(crate) crc_present: bool,
    pub(crate) nblks: u32,
    pub(crate) fsize: usize,
    pub(crate) amode: u32,
    pub(crate) rate: u32,
    pub(crate) rate_code: u32,
    pub(crate) br_code: usize,
    pub(crate) dynf: bool,
    pub(crate) timef: bool,
    pub(crate) auxf: bool,
    pub(crate) hdcd: bool,
    pub(crate) ext_id: u32,
    pub(crate) ext_present: bool,
    pub(crate) aspf: bool,
    pub(crate) lff: u32,
    pub(crate) hflag: bool,
    pub(crate) filts: bool,
    pub(crate) vernum: u32,
    pub(crate) chist: u32,
    pub(crate) pcmr: usize,
    pub(crate) sumf: bool,
    pub(crate) sums: bool,
    pub(crate) dialnorm: u32,
    pub(crate) hcrc_ok: bool,
}

pub(crate) fn parse_core(h: &[u8]) -> Result<Core, String> {
    if h.len() < 16 {
        return Err("header cut short".into());
    }
    let mut b = Bits::new(&h[4..]);
    let mut c = Core {
        normal: b.flag(),
        short: b.get(5),
        crc_present: b.flag(),
        nblks: b.get(7) + 1,
        fsize: b.get(14) as usize + 1,
        amode: b.get(6),
        rate_code: 0,
        ..Default::default()
    };
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

pub(crate) fn core_rows(c: &Core) -> Vec<(String, String)> {
    let mut v = vec![
        (
            "frame type (FTYPE)".to_string(),
            if c.normal {
                "normal".to_string()
            } else {
                format!("termination, {} deficit samples", c.short)
            },
        ),
        ("header checksum present (CPF)".to_string(), c.crc_present.to_string()),
        (
            "sample blocks (NBLKS)".to_string(),
            format!("{} ({} samples)", c.nblks, c.nblks * 32),
        ),
        ("frame size (FSIZE)".to_string(), format!("{} bytes", c.fsize)),
        (
            "channel arrangement (AMODE)".to_string(),
            format!("{} = {}", c.amode, AMODE_NAME[c.amode as usize]),
        ),
        (
            "sampling frequency (SFREQ)".to_string(),
            format!("{} = {} Hz", c.rate_code, c.rate),
        ),
        (
            "transmission bit rate (RATE)".to_string(),
            match BIT_RATES[c.br_code] {
                0 => ["open", "variable", "lossless"]
                    .get(c.br_code.wrapping_sub(29))
                    .copied()
                    .unwrap_or("?")
                    .to_string(),
                k => format!("{} = {k} kbps", c.br_code),
            },
        ),
        ("dynamic range coefficients (DYNF)".to_string(), c.dynf.to_string()),
        ("time stamp (TIMEF)".to_string(), c.timef.to_string()),
        ("auxiliary data (AUXF)".to_string(), c.auxf.to_string()),
        ("HDCD (HDCD)".to_string(), c.hdcd.to_string()),
        (
            "extension (EXT_AUDIO_ID)".to_string(),
            if c.ext_present {
                EXT_AUDIO[c.ext_id as usize].to_string()
            } else {
                "none".to_string()
            },
        ),
        (
            "LFE (LFF)".to_string(),
            ["none", "128x interpolation", "64x interpolation", "invalid"][c.lff as usize].to_string(),
        ),
        (
            "multirate filter bank (FILTS)".to_string(),
            if c.filts {
                "perfect reconstruction".to_string()
            } else {
                "non-perfect".to_string()
            },
        ),
        ("encoder version (VERNUM)".to_string(), c.vernum.to_string()),
        (
            "source PCM resolution (PCMR)".to_string(),
            format!(
                "{} bits{}",
                PCM_BITS[c.pcmr],
                if c.pcmr & 1 == 1 { " (ES)" } else { "" }
            ),
        ),
        ("front sum/difference (SUMF)".to_string(), c.sumf.to_string()),
        ("surround sum/difference (SUMS)".to_string(), c.sums.to_string()),
        (
            "dialogue normalization (DIALNORM)".to_string(),
            format!("{}", c.dialnorm),
        ),
    ];
    if c.crc_present {
        v.push(("header checksum matches".to_string(), c.hcrc_ok.to_string()));
    }
    v
}
