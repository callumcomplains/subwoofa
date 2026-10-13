//! Reading helpers: a bit reader over a header, and a sliding window over a file.

use crate::Result;
use std::io::{Read, Seek, SeekFrom};

pub(crate) struct Bits<'a> {
    pub(crate) d: &'a [u8],
    pub(crate) p: usize,
}

impl<'a> Bits<'a> {
    pub(crate) fn new(d: &'a [u8]) -> Self {
        Bits { d, p: 0 }
    }
    pub(crate) fn get(&mut self, n: usize) -> u32 {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = self.d.get(self.p >> 3).copied().unwrap_or(0);
            v = (v << 1) | ((byte >> (7 - (self.p & 7))) & 1) as u32;
            self.p += 1;
        }
        v
    }
}

/// Sliding window over a file: indexing reads forward in big blocks instead of seeking per frame.
pub(crate) struct Window {
    pub(crate) base: u64,
    pub(crate) start: u64,
    pub(crate) data: Vec<u8>,
    pub(crate) total: u64,
}

impl Window {
    pub(crate) fn new(base: u64, total: u64) -> Window {
        Window {
            base,
            start: 0,
            data: Vec::new(),
            total,
        }
    }

    /// Up to `n` bytes at stream position `pos`.
    pub(crate) fn get<R: Read + Seek>(&mut self, r: &mut R, pos: u64, n: usize) -> Result<&[u8]> {
        let end = (pos + n as u64).min(self.total);
        if pos < self.start || end > self.start + self.data.len() as u64 {
            r.seek(SeekFrom::Start(self.base + pos))?;
            let want = ((self.total - pos) as usize).min(8 << 20);
            self.data.resize(want, 0);
            let mut got = 0;
            while got < want {
                let k = r.read(&mut self.data[got..])?;
                if k == 0 {
                    break;
                }
                got += k;
            }
            self.data.truncate(got);
            self.start = pos;
        }
        let off = (pos - self.start) as usize;
        Ok(&self.data[off..(off + n).min(self.data.len())])
    }
}
