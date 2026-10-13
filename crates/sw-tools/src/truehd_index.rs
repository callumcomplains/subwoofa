//! Indexing a TrueHD / MLP stream: access units and the restart points where a cut may begin.

use crate::reader::{Bits, Window};
use crate::{Error, Format, Frame, Result};
use std::io::{Read, Seek};

pub(crate) type TrueHdIndex = (Vec<Frame>, u32, u32, u32, String, Format);

pub(crate) fn index_truehd<R: Read + Seek>(r: &mut R, base: u64, len: u64) -> Result<TrueHdIndex> {
    let mut frames = Vec::new();
    let mut pos = 0u64;
    let (mut rate, mut chans, mut au_ticks) = (0u32, 0u32, 40u32);
    let mut fmt = Format::TrueHd;
    let mut win = Window::new(base, len);
    while pos + 4 <= len {
        let buf = win.get(r, pos, 40)?.to_vec();
        let n = buf.len();
        if n < 4 {
            break;
        }
        let size = ((((buf[0] & 0x0F) as usize) << 8) | buf[1] as usize) * 2;
        if size < 4 || pos + size as u64 > len {
            break;
        }
        let mut restart = false;
        if n >= 28 && buf[4] == 0xF8 && buf[5] == 0x72 && buf[6] == 0x6F && (buf[7] == 0xBA || buf[7] == 0xBB) {
            restart = true;
            let mut b = Bits::new(&buf[8..]);
            if buf[7] == 0xBA {
                let code = b.get(4);
                b.get(1);
                b.get(1);
                b.get(2);
                b.get(2);
                b.get(2);
                b.get(5);
                b.get(2);
                let map = b.get(13);
                const GROUP: [u32; 13] = [2, 1, 1, 2, 2, 2, 2, 1, 1, 2, 2, 1, 1];
                chans = (0..13).filter(|g| map >> g & 1 == 1).map(|g| GROUP[g]).sum();
                rate = (if code & 8 != 0 { 44100 } else { 48000 }) << (code & 7);
                au_ticks = 40 << (code & 7);
            } else {
                fmt = Format::Mlp;
                b.get(8);
                let code = b.get(4);
                b.get(4);
                b.get(4);
                b.get(4);
                b.get(3);
                let arr = b.get(5);
                const OCC: [u32; 13] = [1, 2, 3, 4, 3, 4, 5, 3, 4, 5, 4, 5, 6];
                chans = OCC.get(arr as usize).copied().unwrap_or(0);
                rate = (if code & 8 != 0 { 44100 } else { 48000 }) << (code & 7);
                au_ticks = 40 << (code & 7);
            }
        }
        frames.push(Frame {
            off: pos,
            len: size as u32,
            ticks: au_ticks,
            restart,
        });
        pos += size as u64;
    }
    if frames.is_empty() {
        return Err(Error::Format("no access units found".into()));
    }
    let desc = if fmt == Format::Mlp {
        "MLP (DVD-Audio)"
    } else {
        "Dolby TrueHD"
    }
    .to_string();
    Ok((frames, rate, rate, chans, desc, fmt))
}
