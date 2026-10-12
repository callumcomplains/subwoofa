//! The MP4 wrapper shared by the audio formats that live in MP4 files (Apple Lossless, MPEG-4 ALS).
//!
//! A file is `ftyp`, one `mdat` holding the frames back to back (its size is patched when the length is known),
//! and a `moov` box that describes the one audio track: how long it is, how big every frame is, where they are.
//! Each format only supplies the boxes inside its sample entry (the codec's own configuration).

use std::io::{Seek, SeekFrom, Write};

/// An MP4 box: size, four-character name, contents.
pub(crate) fn atom(name: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(&((8 + body.len()) as u32).to_be_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(body);
    out
}

/// Where the audio data starts, and where the size field of the `mdat` box is (to patch later).
#[derive(Clone, Copy)]
pub(crate) struct Start {
    pub data_start: u64,
    pub mdat_size_at: u64,
}

/// Writes `ftyp` and the beginning of a 64-bit-sized `mdat`.
pub(crate) fn write_start<W: Write>(out: &mut W, major_brand: &[u8; 4]) -> std::io::Result<Start> {
    let mut ftyp = Vec::new();
    ftyp.extend_from_slice(major_brand);
    ftyp.extend_from_slice(&0u32.to_be_bytes());
    for brand in [major_brand, b"mp42", b"isom"] {
        ftyp.extend_from_slice(brand);
    }
    let ftyp = atom(b"ftyp", &ftyp);
    out.write_all(&ftyp)?;
    out.write_all(&1u32.to_be_bytes())?;
    out.write_all(b"mdat")?;
    out.write_all(&0u64.to_be_bytes())?;
    Ok(Start { data_start: ftyp.len() as u64 + 16, mdat_size_at: ftyp.len() as u64 + 8 })
}

/// Writes the `moov` box and patches the `mdat` size. Returns the size of the whole file.
pub(crate) fn finish<W: Write + Seek>(out: &mut W, start: &Start, track: &Track) -> std::io::Result<u64> {
    let data_bytes: u64 = track.frame_sizes.iter().map(|&s| s as u64).sum();
    let data_end = start.data_start + data_bytes;
    let moov = moov(track, start.data_start, data_end);
    out.write_all(&moov)?;
    out.seek(SeekFrom::Start(start.mdat_size_at))?;
    out.write_all(&(16 + data_bytes).to_be_bytes())?;
    out.flush()?;
    Ok(data_end + moov.len() as u64)
}

/// What the `moov` box says about the track.
pub(crate) struct Track<'a> {
    pub rate: u32,
    pub channels: u16,
    pub bits: u16,
    /// Samples per channel in a full frame (the last one may be shorter).
    pub frame_samples: u32,
    pub total_samples: u64,
    pub frame_sizes: &'a [u32],
    /// Name of the sample entry (`alac`, `mp4a`) and the boxes inside it.
    pub entry_name: &'a [u8; 4],
    pub entry_boxes: Vec<u8>,
}

fn moov(t: &Track, data_start: u64, data_end: u64) -> Vec<u8> {
    let (rate, total) = (t.rate, t.total_samples);
    let frames = t.frame_sizes.len() as u32;

    let mut entry = Vec::new();
    entry.extend_from_slice(&[0u8; 6]);
    entry.extend_from_slice(&1u16.to_be_bytes()); // data reference
    entry.extend_from_slice(&[0u8; 8]); // version, revision, vendor
    entry.extend_from_slice(&t.channels.to_be_bytes());
    entry.extend_from_slice(&t.bits.to_be_bytes());
    entry.extend_from_slice(&[0u8; 4]); // compression id, packet size
    entry.extend_from_slice(&((if rate <= 0xFFFF { rate } else { 0 }) << 16).to_be_bytes());
    entry.extend_from_slice(&t.entry_boxes);
    let mut stsd = Vec::new();
    stsd.extend_from_slice(&0u32.to_be_bytes());
    stsd.extend_from_slice(&1u32.to_be_bytes());
    stsd.extend_from_slice(&atom(t.entry_name, &entry));

    let stts = time_to_sample(frames, t.frame_samples, total);
    // all frames in one chunk
    let mut stsc = Vec::new();
    stsc.extend_from_slice(&0u32.to_be_bytes());
    stsc.extend_from_slice(&1u32.to_be_bytes());
    stsc.extend_from_slice(&1u32.to_be_bytes());
    stsc.extend_from_slice(&frames.to_be_bytes());
    stsc.extend_from_slice(&1u32.to_be_bytes());
    let mut stsz = Vec::new();
    stsz.extend_from_slice(&0u32.to_be_bytes());
    stsz.extend_from_slice(&0u32.to_be_bytes()); // sizes follow
    stsz.extend_from_slice(&frames.to_be_bytes());
    for s in t.frame_sizes {
        stsz.extend_from_slice(&s.to_be_bytes());
    }
    let mut chunk_offsets = Vec::new();
    chunk_offsets.extend_from_slice(&0u32.to_be_bytes());
    chunk_offsets.extend_from_slice(&1u32.to_be_bytes());
    let offsets = if data_end > u32::MAX as u64 {
        chunk_offsets.extend_from_slice(&data_start.to_be_bytes());
        atom(b"co64", &chunk_offsets)
    } else {
        chunk_offsets.extend_from_slice(&(data_start as u32).to_be_bytes());
        atom(b"stco", &chunk_offsets)
    };
    let mut stbl = Vec::new();
    stbl.extend_from_slice(&atom(b"stsd", &stsd));
    stbl.extend_from_slice(&atom(b"stts", &stts));
    stbl.extend_from_slice(&atom(b"stsc", &stsc));
    stbl.extend_from_slice(&atom(b"stsz", &stsz));
    stbl.extend_from_slice(&offsets);

    let mut dref = Vec::new();
    dref.extend_from_slice(&0u32.to_be_bytes());
    dref.extend_from_slice(&1u32.to_be_bytes());
    dref.extend_from_slice(&atom(b"url ", &1u32.to_be_bytes())); // media data in this file
    let dinf = atom(b"dinf", &atom(b"dref", &dref));
    let smhd = atom(b"smhd", &[0u8; 8]);
    let mut minf = Vec::new();
    minf.extend_from_slice(&smhd);
    minf.extend_from_slice(&dinf);
    minf.extend_from_slice(&atom(b"stbl", &stbl));

    let mut mdhd = Vec::new();
    mdhd.extend_from_slice(&[1, 0, 0, 0]); // version 1: 64-bit times
    mdhd.extend_from_slice(&0u64.to_be_bytes());
    mdhd.extend_from_slice(&0u64.to_be_bytes());
    mdhd.extend_from_slice(&rate.to_be_bytes());
    mdhd.extend_from_slice(&total.to_be_bytes());
    mdhd.extend_from_slice(&0x55C4u16.to_be_bytes()); // language: undetermined
    mdhd.extend_from_slice(&0u16.to_be_bytes());
    let mut hdlr = Vec::new();
    hdlr.extend_from_slice(&0u32.to_be_bytes());
    hdlr.extend_from_slice(&0u32.to_be_bytes());
    hdlr.extend_from_slice(b"soun");
    hdlr.extend_from_slice(&[0u8; 12]);
    hdlr.extend_from_slice(b"SoundHandler\0");
    let mut mdia = Vec::new();
    mdia.extend_from_slice(&atom(b"mdhd", &mdhd));
    mdia.extend_from_slice(&atom(b"hdlr", &hdlr));
    mdia.extend_from_slice(&atom(b"minf", &minf));

    let matrix: [u32; 9] = [0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000];
    let mut tkhd = Vec::new();
    tkhd.extend_from_slice(&[1, 0, 0, 3]); // version 1; enabled and in the movie
    tkhd.extend_from_slice(&0u64.to_be_bytes());
    tkhd.extend_from_slice(&0u64.to_be_bytes());
    tkhd.extend_from_slice(&1u32.to_be_bytes()); // track id
    tkhd.extend_from_slice(&0u32.to_be_bytes());
    tkhd.extend_from_slice(&total.to_be_bytes()); // duration in movie time units (= samples)
    tkhd.extend_from_slice(&[0u8; 8]);
    tkhd.extend_from_slice(&0u16.to_be_bytes()); // layer
    tkhd.extend_from_slice(&0u16.to_be_bytes()); // alternate group
    tkhd.extend_from_slice(&0x0100u16.to_be_bytes()); // volume
    tkhd.extend_from_slice(&0u16.to_be_bytes());
    for m in matrix {
        tkhd.extend_from_slice(&m.to_be_bytes());
    }
    tkhd.extend_from_slice(&[0u8; 8]); // width, height
    let mut trak = Vec::new();
    trak.extend_from_slice(&atom(b"tkhd", &tkhd));
    trak.extend_from_slice(&atom(b"mdia", &mdia));

    let mut mvhd = Vec::new();
    mvhd.extend_from_slice(&[1, 0, 0, 0]);
    mvhd.extend_from_slice(&0u64.to_be_bytes());
    mvhd.extend_from_slice(&0u64.to_be_bytes());
    mvhd.extend_from_slice(&rate.to_be_bytes()); // movie time scale = sample rate
    mvhd.extend_from_slice(&total.to_be_bytes());
    mvhd.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // rate 1.0
    mvhd.extend_from_slice(&0x0100u16.to_be_bytes()); // volume
    mvhd.extend_from_slice(&[0u8; 10]);
    for m in matrix {
        mvhd.extend_from_slice(&m.to_be_bytes());
    }
    mvhd.extend_from_slice(&[0u8; 24]);
    mvhd.extend_from_slice(&2u32.to_be_bytes()); // next track id
    let mut moov = Vec::new();
    moov.extend_from_slice(&atom(b"mvhd", &mvhd));
    moov.extend_from_slice(&atom(b"trak", &trak));
    atom(b"moov", &moov)
}

/// `stts`: every frame is `frame_samples` long but the last, which holds what is left.
fn time_to_sample(frames: u32, frame_samples: u32, total: u64) -> Vec<u8> {
    let mut stts = Vec::new();
    stts.extend_from_slice(&0u32.to_be_bytes());
    let last = (total - (frames as u64 - 1) * frame_samples as u64) as u32;
    if last == frame_samples {
        stts.extend_from_slice(&1u32.to_be_bytes());
        stts.extend_from_slice(&frames.to_be_bytes());
        stts.extend_from_slice(&frame_samples.to_be_bytes());
    } else {
        stts.extend_from_slice(&2u32.to_be_bytes());
        stts.extend_from_slice(&(frames - 1).to_be_bytes());
        stts.extend_from_slice(&frame_samples.to_be_bytes());
        stts.extend_from_slice(&1u32.to_be_bytes());
        stts.extend_from_slice(&last.to_be_bytes());
    }
    stts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_carry_their_size() {
        let b = atom(b"free", &[1, 2, 3]);
        assert_eq!(b, [0, 0, 0, 11, b'f', b'r', b'e', b'e', 1, 2, 3]);
    }

    #[test]
    fn last_frame_may_be_short() {
        let full = time_to_sample(3, 100, 300);
        let short = time_to_sample(3, 100, 250);
        assert!(short.len() > full.len());
    }
}
