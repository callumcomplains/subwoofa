//! Ogg page writing (RFC 3533), enough for FLAC-in-Ogg: whole packets are laced into pages, packets longer than a
//! page continue on the next one, and every page carries its CRC.

use std::io::Write;

/// Pages are closed once their body reaches about this many bytes.
const TARGET_PAGE_BYTES: usize = 8192;
/// "No packet finishes on this page."
const NO_GRANULE: u64 = u64::MAX;

const FLAG_CONTINUED: u8 = 0x01;
const FLAG_FIRST: u8 = 0x02;
const FLAG_LAST: u8 = 0x04;

/// CRC-32 of Ogg pages: polynomial 0x04C11DB7, most significant bit first, starting at zero, no final inversion.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0u32;
    for &byte in data {
        crc ^= (byte as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// One finished page.
pub fn build_page(serial: u32, sequence: u32, flags: u8, granule: u64, lacing: &[u8], body: &[u8]) -> Vec<u8> {
    let mut page = Vec::with_capacity(27 + lacing.len() + body.len());
    page.extend_from_slice(b"OggS");
    page.push(0); // stream structure version
    page.push(flags);
    page.extend_from_slice(&granule.to_le_bytes());
    page.extend_from_slice(&serial.to_le_bytes());
    page.extend_from_slice(&sequence.to_le_bytes());
    page.extend_from_slice(&[0, 0, 0, 0]); // checksum, filled in below
    page.push(lacing.len() as u8);
    page.extend_from_slice(lacing);
    page.extend_from_slice(body);
    let crc = crc32(&page);
    page[22..26].copy_from_slice(&crc.to_le_bytes());
    page
}

/// Writes packets as pages to `out`.
pub struct PacketWriter<W: Write> {
    out: W,
    serial: u32,
    sequence: u32,
    lacing: Vec<u8>,
    body: Vec<u8>,
    /// Granule position of the last packet that finishes in the page being built.
    granule: Option<u64>,
    /// The page being built starts in the middle of a packet.
    continued: bool,
    first_page: bool,
    pub bytes: u64,
}

impl<W: Write> PacketWriter<W> {
    pub fn new(out: W, serial: u32) -> PacketWriter<W> {
        PacketWriter {
            out,
            serial,
            sequence: 0,
            lacing: Vec::new(),
            body: Vec::new(),
            granule: None,
            continued: false,
            first_page: true,
            bytes: 0,
        }
    }

    /// Adds a packet whose last sample is number `granule`.
    pub fn packet(&mut self, data: &[u8], granule: u64) -> std::io::Result<()> {
        // a full page is closed when the next packet arrives, so the last page stays open for the end flag
        if self.body.len() >= TARGET_PAGE_BYTES {
            self.flush(false)?;
        }
        let mut offset = 0usize;
        loop {
            let remaining = data.len() - offset;
            let segments_needed = remaining / 255 + 1; // the last segment is shorter than 255 (maybe empty)
            let room = 255 - self.lacing.len();
            if segments_needed <= room {
                self.lacing.extend(std::iter::repeat(255u8).take(remaining / 255));
                self.lacing.push((remaining % 255) as u8);
                self.body.extend_from_slice(&data[offset..]);
                self.granule = Some(granule);
                return Ok(());
            }
            // the packet does not fit: fill this page with 255-byte segments and carry on in the next one
            self.lacing.extend(std::iter::repeat(255u8).take(room));
            self.body.extend_from_slice(&data[offset..offset + room * 255]);
            offset += room * 255;
            self.flush(true)?;
        }
    }

    /// Closes the page being built (nothing to do if it is empty). `unfinished`: its last packet goes on.
    pub fn flush(&mut self, unfinished: bool) -> std::io::Result<()> {
        if self.lacing.is_empty() {
            return Ok(());
        }
        let mut flags = 0;
        if self.continued {
            flags |= FLAG_CONTINUED;
        }
        if self.first_page {
            flags |= FLAG_FIRST;
        }
        let granule = if unfinished && self.granule.is_none() {
            NO_GRANULE
        } else {
            self.granule.unwrap_or(NO_GRANULE)
        };
        self.write_page(flags, granule)?;
        self.continued = unfinished;
        Ok(())
    }

    /// Closes the stream: the last page is marked as the end.
    pub fn finish(mut self) -> std::io::Result<W> {
        if self.lacing.is_empty() {
            // no data page left to mark: the end flag needs a page of its own
            let granule = self.granule.unwrap_or(0);
            let page = build_page(
                self.serial,
                self.sequence,
                FLAG_LAST | if self.continued { FLAG_CONTINUED } else { 0 },
                granule,
                &[],
                &[],
            );
            self.bytes += page.len() as u64;
            self.out.write_all(&page)?;
        } else {
            let mut flags = FLAG_LAST;
            if self.continued {
                flags |= FLAG_CONTINUED;
            }
            if self.first_page {
                flags |= FLAG_FIRST;
            }
            let granule = self.granule.unwrap_or(NO_GRANULE);
            self.write_page(flags, granule)?;
        }
        self.out.flush()?;
        Ok(self.out)
    }

    fn write_page(&mut self, flags: u8, granule: u64) -> std::io::Result<()> {
        let page = build_page(self.serial, self.sequence, flags, granule, &self.lacing, &self.body);
        self.out.write_all(&page)?;
        self.bytes += page.len() as u64;
        self.sequence += 1;
        self.first_page = false;
        self.lacing.clear();
        self.body.clear();
        self.granule = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_of_a_known_page_header() {
        // an empty first page: checks the checksum the way a reader would, by recomputing it with the field zeroed
        let page = build_page(1, 0, FLAG_FIRST, 0, &[1], &[0xAB]);
        let stored = u32::from_le_bytes(page[22..26].try_into().unwrap());
        let mut zeroed = page.clone();
        zeroed[22..26].fill(0);
        assert_eq!(stored, crc32(&zeroed));
        assert_eq!(&page[..4], b"OggS");
    }

    #[test]
    fn long_packets_continue_on_the_next_page() {
        let mut w = PacketWriter::new(Vec::new(), 7);
        w.packet(&vec![1u8; 100_000], 10).unwrap();
        let bytes = w.finish().unwrap();
        // 100000 bytes need 393 segments: two pages, the second flagged as a continuation
        let pages: Vec<usize> = bytes
            .windows(4)
            .enumerate()
            .filter(|(_, w)| *w == b"OggS")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(pages.len(), 2);
        assert_eq!(bytes[pages[1] + 5] & FLAG_CONTINUED, FLAG_CONTINUED);
        assert_eq!(
            u64::from_le_bytes(bytes[pages[0] + 6..pages[0] + 14].try_into().unwrap()),
            NO_GRANULE
        );
        assert_eq!(
            u64::from_le_bytes(bytes[pages[1] + 6..pages[1] + 14].try_into().unwrap()),
            10
        );
    }
}
