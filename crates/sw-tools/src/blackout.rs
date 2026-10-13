//! The silent frame used to pad a stream.

use crate::Stream;

impl Stream {
    /// The silent frame stored in the file (the BLACKOUT chunk), if there is one.
    pub fn blackout(&self) -> Option<Vec<u8>> {
        if let Some(c) = self.trailer.iter().find(|c| &c.name == b"BLACKOUT") {
            return Some(c.body.clone());
        }
        // StreamTools keeps it in front of the stream data
        let mut pos = 0usize;
        let h = &self.header;
        while pos + 16 <= h.len() {
            let size = u64::from_be_bytes(h[pos + 8..pos + 16].try_into().unwrap()) as usize;
            if &h[pos..pos + 8] == b"BLACKOUT" && pos + 16 + size <= h.len() {
                return Some(h[pos + 16..pos + 16 + size].to_vec());
            }
            pos += 16 + size;
        }
        None
    }
}
