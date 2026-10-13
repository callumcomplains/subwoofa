//! Indexing a raw DTS / DTS-HD stream: core frames, extension substreams and the access units they form.

use crate::reader::{Bits, Window};
use crate::{Error, Frame, Result};
use std::io::{Read, Seek};

pub(crate) const CORE_SYNC: u32 = 0x7FFE_8001;

pub(crate) const EXSS_SYNC: u32 = 0x6458_2025;

pub(crate) const SFREQ: [u32; 16] = [
    0, 8000, 16000, 32000, 0, 0, 11025, 22050, 44100, 0, 0, 12000, 24000, 48000, 0, 0,
];

pub(crate) const FS_INDEX: [u32; 16] = [
    8000, 16000, 32000, 64000, 128000, 22050, 44100, 88200, 176400, 352800, 12000, 24000, 48000, 96000, 192000, 384000,
];

pub(crate) const AMODE_CHANNELS: [u32; 16] = [1, 2, 2, 2, 2, 3, 3, 4, 4, 5, 6, 6, 6, 7, 8, 8];

pub(crate) fn be32(d: &[u8]) -> u32 {
    u32::from_be_bytes([d[0], d[1], d[2], d[3]])
}

pub(crate) struct CoreInfo {
    pub(crate) size: usize,
    pub(crate) samples: u32,
    pub(crate) rate: u32,
    pub(crate) channels: u32,
    pub(crate) ext_x96: bool,
    pub(crate) xch: bool,
}

pub(crate) fn parse_core(d: &[u8]) -> Option<CoreInfo> {
    if d.len() < 16 || be32(d) != CORE_SYNC {
        return None;
    }
    let mut b = Bits::new(&d[4..]);
    b.get(1);
    b.get(5);
    b.get(1);
    let nblks = b.get(7);
    let fsize = b.get(14);
    let amode = b.get(6) as usize;
    let sfreq = b.get(4) as usize;
    b.get(5);
    b.get(5); // mix, dynf, timef, auxf, hdcd
    let ext_id = b.get(3);
    let ext = b.get(1);
    b.get(1);
    let lff = b.get(2);
    if nblks < 4 || fsize < 94 {
        return None;
    }
    let channels = AMODE_CHANNELS.get(amode).copied().unwrap_or(0) + u32::from(lff != 0);
    Some(CoreInfo {
        size: fsize as usize + 1,
        samples: (nblks + 1) * 32,
        rate: SFREQ[sfreq & 15],
        channels,
        ext_x96: ext == 1 && ext_id == 2,
        xch: ext == 1 && ext_id == 0,
    })
}

pub(crate) struct ExssInfo {
    pub(crate) size: usize,
    /// (frame duration in reference clock ticks, reference clock rate)
    pub(crate) ticks: Option<(u32, u32)>,
    pub(crate) rate: u32,
    pub(crate) channels: u32,
}

pub(crate) fn parse_exss(d: &[u8], carry: Option<(u32, u32)>) -> Option<ExssInfo> {
    if d.len() < 16 || be32(d) != EXSS_SYNC {
        return None;
    }
    let mut b = Bits::new(&d[4..]);
    b.get(8);
    let idx = b.get(2);
    let wide = b.get(1) as usize;
    let _hsize = b.get(8 + 4 * wide);
    let fsize = b.get(16 + 4 * wide) as usize + 1;
    let nb4f = 16 + 4 * wide;
    let static_fields = b.get(1);
    let mut ticks = carry;
    let (mut rate, mut channels) = (0, 0);
    if static_fields == 1 {
        let clk = b.get(2) as usize;
        let dur = 512 * (b.get(3) + 1);
        ticks = Some((dur, [32000, 44100, 48000, 48000][clk]));
        if b.get(1) == 1 {
            b.get(36);
        }
        let npres = b.get(3) as usize + 1;
        let nass = b.get(3) as usize + 1;
        let mut masks = Vec::new();
        for _ in 0..npres {
            masks.push(b.get(idx as usize + 1));
        }
        for m in &masks {
            for ss in 0..=idx as usize {
                if m >> ss & 1 == 1 {
                    b.get(8);
                }
            }
        }
        if b.get(1) == 1 {
            b.get(2);
            let nb = (b.get(2) as usize + 1) << 2;
            let n = b.get(2) as usize + 1;
            for _ in 0..n {
                b.get(nb);
            }
        }
        for _ in 0..nass {
            b.get(nb4f);
        }
        // first asset descriptor
        b.get(9);
        b.get(3);
        if b.get(1) == 1 {
            b.get(4);
        }
        if b.get(1) == 1 {
            b.get(24);
        }
        if b.get(1) == 1 {
            let n = b.get(10) as usize + 1;
            b.p += n * 8;
        }
        b.get(5);
        rate = FS_INDEX[b.get(4) as usize & 15];
        channels = b.get(8) + 1;
    }
    Some(ExssInfo {
        size: fsize,
        ticks,
        rate,
        channels,
    })
}

/// Index a raw DTS / DTS-HD elementary stream (positions relative to `base`).
pub(crate) fn index_dts<R: Read + Seek>(r: &mut R, base: u64, len: u64) -> Result<(Vec<Frame>, u32, u32, u32, String)> {
    let mut frames: Vec<Frame> = Vec::new();
    let mut pos = 0u64;
    let (mut rate, mut chans, mut tick_rate) = (0u32, 0u32, 0u32);
    let mut carry: Option<(u32, u32)> = None;
    let (mut has_core, mut has_exss, mut x96, mut xch) = (false, false, false, false);
    let mut win = Window::new(base, len);
    let mut cur: Option<Frame> = None;
    while pos + 16 <= len {
        let buf = win.get(r, pos, 256)?.to_vec();
        let n = buf.len();
        if n < 16 {
            break;
        }
        let w = be32(&buf);
        if w == CORE_SYNC {
            if let Some(c) = parse_core(&buf[..n]) {
                if let Some(f) = cur.take() {
                    frames.push(f);
                }
                has_core = true;
                x96 |= c.ext_x96;
                xch |= c.xch;
                rate = c.rate * if c.ext_x96 { 2 } else { 1 };
                chans = chans.max(c.channels + u32::from(c.xch));
                tick_rate = c.rate;
                cur = Some(Frame {
                    off: pos,
                    len: c.size as u32,
                    ticks: c.samples,
                    restart: true,
                });
                pos += c.size as u64;
                continue;
            }
        } else if w == EXSS_SYNC {
            if let Some(e) = parse_exss(&buf[..n], carry) {
                carry = e.ticks;
                has_exss = true;
                if e.rate > 0 {
                    rate = e.rate;
                    chans = chans.max(e.channels);
                }
                match cur.as_mut() {
                    Some(f) if has_core && f.off + f.len as u64 == pos => f.len += e.size as u32,
                    _ => {
                        if let Some(f) = cur.take() {
                            frames.push(f);
                        }
                        let (dur, clk) = e.ticks.unwrap_or((512, 48000));
                        tick_rate = clk;
                        cur = Some(Frame {
                            off: pos,
                            len: e.size as u32,
                            ticks: dur,
                            restart: true,
                        });
                    }
                }
                pos += e.size as u64;
                continue;
            }
        }
        // not at a frame start: look for the next sync word
        pos += 4;
    }
    if let Some(f) = cur.take() {
        frames.push(f);
    }
    if frames.is_empty() {
        return Err(Error::Format("no DTS frames found".into()));
    }
    if tick_rate == 0 {
        tick_rate = rate.max(1);
    }
    let mut parts = vec![if has_exss {
        "DTS-HD".to_string()
    } else {
        "DTS".to_string()
    }];
    if has_core && has_exss {
        parts.push("core + extension substream".into());
    } else if has_core {
        parts.push("core only".into());
    } else {
        parts.push("extension substream only (lossless)".into());
    }
    if x96 {
        parts.push("X96".into());
    }
    if xch {
        parts.push("XCH".into());
    }
    Ok((frames, tick_rate, rate, chans, parts.join(", ")))
}
