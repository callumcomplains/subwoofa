//! Digital cinema sound: an AS-DCP / SMPTE ST 429-3 MXF track file of PCM audio (or its older MXF Interop form).
//!
//! The file is 24-bit little-endian PCM, 48 or 96 kHz, up to 16 channels, cut into *edit units* of one picture
//! frame each (2000 samples per channel at 48 kHz and 24 fps). Its layout, which this writer follows from what the
//! reference toolkit (asdcplib) writes:
//! * a header partition with the metadata (packages, tracks, the wave audio descriptor), padded to 16384 bytes,
//! * for SMPTE files a body partition, then one KLV triplet per edit unit,
//! * a footer partition holding a constant-bit-rate index table, and a random index pack.
//!
//! The MXF building blocks are in [`mxf`]; the header's tag table is in [`primer`].

mod mxf;
mod primer;

use crate::{to_integers, BlockWriter, Params, SampleFormat};
use mxf::{batch, klv, label, rational, umid, utf16, uuid, Kind, Label, Partition, Set};
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

/// Size of the header partition's metadata area: the first essence byte is at this offset (plus a pack for SMPTE).
const HEADER_SIZE: usize = 16384;

/// Bytes a KLV triplet adds to every frame (16-byte key, 4-byte length).
const KLV_OVERHEAD: u32 = 20;

/// The SID the footer's index table has, and the one the essence has.
const INDEX_SID: u32 = 129;
const BODY_SID: u32 = 1;

/// Key of a frame-wrapped wave essence element.
const ESSENCE_KEY: &str = "060e2b34010201010d01030116010101";

/// Options the file format has beyond the audio.
#[derive(Clone, Copy, Debug)]
pub struct DcpOptions {
    /// Frame rate of the picture the sound goes with, as a fraction (24/1, 25/1, 24000/1001 ...).
    pub frame_rate: (u32, u32),
    /// SMPTE labels (the current standard) instead of MXF Interop labels.
    pub smpte: bool,
}

impl Default for DcpOptions {
    fn default() -> DcpOptions {
        DcpOptions {
            frame_rate: (24, 1),
            smpte: true,
        }
    }
}

impl DcpOptions {
    /// Reads a setting like `24`, `23.976`, `25-interop`: the frame rate, then optionally `-interop`.
    pub fn parse(setting: &str) -> Result<DcpOptions, String> {
        let (rate, standard) = setting.split_once('-').unwrap_or((setting, "smpte"));
        let frame_rate = match rate {
            "" | "24" => (24, 1),
            "23.976" | "23.98" => (24_000, 1001),
            "25" => (25, 1),
            "30" => (30, 1),
            "48" => (48, 1),
            "50" => (50, 1),
            "60" => (60, 1),
            other => {
                return Err(format!(
                    "'{other}' is not a DCP frame rate (24, 23.976, 25, 30, 48, 50 or 60)"
                ))
            }
        };
        match standard {
            "smpte" => Ok(DcpOptions {
                frame_rate,
                smpte: true,
            }),
            "interop" => Ok(DcpOptions {
                frame_rate,
                smpte: false,
            }),
            other => Err(format!("'{other}' is not a DCP standard (smpte or interop)")),
        }
    }
}

/// The frame rates a sound track file may have.
pub const FRAME_RATES: [(u32, u32); 7] = [(24, 1), (25, 1), (30, 1), (48, 1), (50, 1), (60, 1), (24_000, 1001)];

pub struct DcpWriter {
    out: BufWriter<File>,
    params: Params,
    options: DcpOptions,
    channel_label: Option<Label>,
    ids: Ids,
    frame_bytes: usize,
    frames_total: u64,
    pending: Vec<u8>,
    frames_written: u64,
}

/// Every object in the header has an identifier of its own.
struct Ids {
    identification: [u8; 16],
    generation: [u8; 16],
    content: [u8; 16],
    container_data: [u8; 16],
    material: [u8; 16],
    file: [u8; 16],
    material_umid: Vec<u8>,
    file_umid: Vec<u8>,
    descriptor: [u8; 16],
    preface: [u8; 16],
}

impl Ids {
    fn new() -> Ids {
        Ids {
            identification: uuid(),
            generation: uuid(),
            content: uuid(),
            container_data: uuid(),
            material: uuid(),
            file: uuid(),
            material_umid: umid(),
            file_umid: umid(),
            descriptor: uuid(),
            preface: uuid(),
        }
    }
}

/// The speaker label SMPTE ST 429-2 gives a channel arrangement, if the arrangement has one.
fn channel_label(channels: usize, mask: u32) -> Label {
    // WAV masks: bits for L R C LFE then the back/side/centre pairs
    const BACK_PAIR: u32 = 0x30;
    const SIDE_PAIR: u32 = 0x600;
    const CENTRE_BACK: u32 = 0x100;
    const LEFT_RIGHT_OF_CENTRE: u32 = 0x00C0;
    const FRONT_AND_LFE: u32 = 0xF;
    let base = mask & FRONT_AND_LFE == FRONT_AND_LFE;
    let code = match channels {
        6 if base => "01",
        7 if base && mask & CENTRE_BACK != 0 => "02",
        8 if base && mask & (SIDE_PAIR | BACK_PAIR) == (SIDE_PAIR | BACK_PAIR) => "05",
        8 if base && mask & LEFT_RIGHT_OF_CENTRE == LEFT_RIGHT_OF_CENTRE => "03",
        _ => "04", // any other arrangement: "wild track format"
    };
    label(&format!("060e2b340401010b040202100301{code}00"))
}

impl DcpWriter {
    pub fn create(path: &Path, params: Params, options: DcpOptions) -> std::io::Result<DcpWriter> {
        let bad = |what: String| std::io::Error::new(std::io::ErrorKind::InvalidInput, what);
        if params.format != SampleFormat::Int24 {
            return Err(bad("DCP sound is 24-bit PCM".into()));
        }
        if params.rate != 48_000 && params.rate != 96_000 {
            return Err(bad(format!("DCP sound is 48 or 96 kHz, not {} Hz", params.rate)));
        }
        if params.channels() > 16 {
            return Err(bad("DCP sound holds at most 16 channels".into()));
        }
        if !FRAME_RATES.contains(&options.frame_rate) {
            return Err(bad(format!(
                "{}/{} is not a DCP frame rate (24, 25, 30, 48, 50, 60 or 24000/1001)",
                options.frame_rate.0, options.frame_rate.1
            )));
        }
        let (num, den) = options.frame_rate;
        let per_frame = params.rate as u64 * den as u64;
        if !per_frame.is_multiple_of(num as u64) {
            return Err(bad(format!(
                "{} Hz does not divide into whole samples per {}/{} fps frame",
                params.rate, num, den
            )));
        }
        let samples_per_frame = (per_frame / num as u64) as usize;
        let frame_bytes = samples_per_frame * params.channels() * 3;
        let frames_total = params.total_frames.div_ceil(samples_per_frame as u64);
        let channel_label = options.smpte.then(|| channel_label(params.channels(), params.mask));

        let mut out = BufWriter::new(File::create(path)?);
        out.write_all(&vec![0u8; HEADER_SIZE])?; // the header is written last, when the footer's position is known
        let mut writer = DcpWriter {
            out,
            params,
            options,
            channel_label,
            ids: Ids::new(),
            frame_bytes,
            frames_total,
            pending: Vec::new(),
            frames_written: 0,
        };
        if options.smpte {
            let body = writer.partition(Kind::Body, HEADER_SIZE as u64, 0, 0, 0, 0, 0, BODY_SID);
            writer.out.write_all(&body)?;
        }
        Ok(writer)
    }

    fn operational_pattern(&self) -> Label {
        label(if self.options.smpte {
            "060e2b34040101020d01020110000000"
        } else {
            "060e2b34040101010d01020110000000"
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn partition(
        &self,
        kind: Kind,
        this: u64,
        previous: u64,
        footer: u64,
        header_bytes: u64,
        index_bytes: u64,
        index_sid: u32,
        body_sid: u32,
    ) -> Vec<u8> {
        Partition {
            kind,
            this,
            previous,
            footer,
            header_bytes,
            index_bytes,
            index_sid,
            body_sid,
            operational_pattern: self.operational_pattern(),
            containers: vec![
                label("060e2b34040101010d01030102060100"),
                label("060e2b34040101030d010301027f0100"),
            ],
        }
        .bytes()
    }

    /// The header partition: pack, primer, all the metadata sets, padded with filler to `HEADER_SIZE`.
    fn header(&self, footer: u64) -> std::io::Result<Vec<u8>> {
        let metadata = self.metadata();
        let entries: Vec<(u16, Label)> = primer::ENTRIES
            .iter()
            .filter(|(tag, _)| *tag != primer::CHANNEL_ASSIGNMENT || self.channel_label.is_some())
            .map(|&(tag, ul)| (tag, label(ul)))
            .collect();
        let primer = mxf::primer(&entries);
        let body_sid = if self.options.smpte { 0 } else { BODY_SID };
        let pack_size = self.partition(Kind::Header, 0, 0, footer, 0, 0, 0, body_sid).len();
        let used = primer.len() + metadata.len();
        let fill = mxf::filler(pack_size + used, HEADER_SIZE).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the DCP header does not fit in 16 KiB",
            )
        })?;
        let header_bytes = (primer.len() + metadata.len() + fill.len()) as u64;
        let mut out = self.partition(Kind::Header, 0, 0, footer, header_bytes, 0, 0, body_sid);
        out.extend(primer);
        out.extend(metadata);
        out.extend(fill);
        Ok(out)
    }

    /// Every header metadata set, in the order the reference toolkit writes them.
    fn metadata(&self) -> Vec<u8> {
        let ids = &self.ids;
        let frames = self.frames_total;
        let (num, den) = self.options.frame_rate;
        let rounded_rate = num.div_ceil(den) as u16;
        let now = mxf::timestamp();
        let (tc_dd, sound_dd) = ("060e2b34040101010103020101000000", "060e2b34040101010103020202000000");
        let sound_track_number = 0x1601_0101u32;

        let ids_tc_track = [uuid(), uuid()];
        let ids_tc_seq = [uuid(), uuid()];
        let ids_tc = [uuid(), uuid()];
        let ids_sound_track = [uuid(), uuid()];
        let ids_sound_seq = [uuid(), uuid()];
        let ids_clip = [uuid(), uuid()];
        let id_batch = |items: &[[u8; 16]]| batch(&items.iter().map(|i| i.to_vec()).collect::<Vec<_>>());

        let mut out = Vec::new();

        let mut preface = Set::new("060e2b34025301010d01010101012f00");
        preface
            .prop(0x3c0a, &ids.preface)
            .prop(0x3b02, &now)
            .u16(0x3b05, 0x0102)
            .u32(0x3b07, 1);
        preface
            .prop(0x3b08, &ids.file)
            .prop(0x3b06, &id_batch(&[ids.identification]))
            .prop(0x3b03, &ids.content);
        preface.prop(0x3b09, &self.operational_pattern());
        preface.prop(
            0x3b0a,
            &batch(&[
                label("060e2b34040101010d01030102060100").to_vec(),
                label("060e2b34040101030d010301027f0100").to_vec(),
            ]),
        );
        preface.prop(0x3b0b, &batch(&[]));
        out.extend(preface.finish());

        let mut ident = Set::new("060e2b34025301010d01010101013000");
        ident
            .prop(0x3c0a, &ids.identification)
            .prop(0x3c09, &ids.generation)
            .prop(0x3c01, &utf16("Subwoofa"))
            .prop(0x3c02, &utf16("Subwoofa"));
        ident
            .prop(0x3c03, &[0u8; 10])
            .prop(0x3c04, &utf16(sw_core::version::NUMBER))
            .prop(0x3c05, &label("5375626f6f6661201f9f4e7a8c3d4b01"));
        ident
            .prop(0x3c06, &now)
            .prop(0x3c07, &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0])
            .prop(0x3c08, &utf16("Windows"));
        out.extend(ident.finish());

        let mut content = Set::new("060e2b34025301010d01010101011800");
        content
            .prop(0x3c0a, &ids.content)
            .prop(0x1901, &id_batch(&[ids.file, ids.material]))
            .prop(0x1902, &id_batch(&[ids.container_data]));
        out.extend(content.finish());

        let mut data = Set::new("060e2b34025301010d01010101012300");
        data.prop(0x3c0a, &ids.container_data)
            .prop(0x2701, &ids.file_umid)
            .u32(0x3f06, INDEX_SID)
            .u32(0x3f07, BODY_SID);
        out.extend(data.finish());

        // the material package: what a player plays, pointing into the file package
        let mut material = Set::new("060e2b34025301010d01010101013600");
        material
            .prop(0x3c0a, &ids.material)
            .prop(0x4401, &ids.material_umid)
            .prop(0x4402, &utf16("Material Package"));
        material
            .prop(0x4405, &now)
            .prop(0x4404, &now)
            .prop(0x4403, &id_batch(&[ids_tc_track[0], ids_sound_track[0]]));
        out.extend(material.finish());
        out.extend(self.timecode_track(
            &ids_tc_track[0],
            &ids_tc_seq[0],
            &ids_tc[0],
            tc_dd,
            frames,
            rounded_rate,
        ));
        out.extend(self.sound_track(
            &ids_sound_track[0],
            &ids_sound_seq[0],
            &ids_clip[0],
            sound_dd,
            0,
            frames,
            Some(ids.file_umid.clone()),
        ));

        // the file package: describes the essence in this file
        let mut file = Set::new("060e2b34025301010d01010101013700");
        file.prop(0x3c0a, &ids.file)
            .prop(0x4401, &ids.file_umid)
            .prop(0x4402, &utf16("File Package: SMPTE 382M frame wrapping of wave audio"));
        file.prop(0x4405, &now)
            .prop(0x4404, &now)
            .prop(0x4403, &id_batch(&[ids_tc_track[1], ids_sound_track[1]]))
            .prop(0x4701, &ids.descriptor);
        out.extend(file.finish());
        out.extend(self.timecode_track(
            &ids_tc_track[1],
            &ids_tc_seq[1],
            &ids_tc[1],
            tc_dd,
            frames,
            rounded_rate,
        ));
        out.extend(self.sound_track(
            &ids_sound_track[1],
            &ids_sound_seq[1],
            &ids_clip[1],
            sound_dd,
            sound_track_number,
            frames,
            None,
        ));

        out.extend(self.descriptor());
        out
    }

    fn track(&self, id: &[u8; 16], track_id: u32, number: u32, name: &str, sequence: &[u8; 16]) -> Vec<u8> {
        let (num, den) = self.options.frame_rate;
        let mut t = Set::new("060e2b34025301010d01010101013b00");
        t.prop(0x3c0a, id)
            .u32(0x4801, track_id)
            .u32(0x4804, number)
            .prop(0x4802, &utf16(name))
            .prop(0x4803, sequence);
        t.prop(0x4b01, &rational(num, den)).u64(0x4b02, 0);
        t.finish()
    }

    fn timecode_track(
        &self,
        track: &[u8; 16],
        sequence: &[u8; 16],
        component: &[u8; 16],
        data_def: &str,
        frames: u64,
        rounded_rate: u16,
    ) -> Vec<u8> {
        let mut out = self.track(track, 1, 0, "Timecode Track", sequence);
        let mut seq = Set::new("060e2b34025301010d01010101010f00");
        seq.prop(0x3c0a, sequence)
            .prop(0x0201, &label(data_def))
            .u64(0x0202, frames)
            .prop(0x1001, &batch(&[component.to_vec()]));
        out.extend(seq.finish());
        let mut tc = Set::new("060e2b34025301010d01010101011400");
        tc.prop(0x3c0a, component)
            .prop(0x0201, &label(data_def))
            .u64(0x0202, frames)
            .u16(0x1502, rounded_rate)
            .u64(0x1501, 0)
            .u8(0x1503, 0);
        out.extend(tc.finish());
        out
    }

    /// A sound track: the track, its sequence and a source clip (pointing at `source`, or at nothing for the file package).
    fn sound_track(
        &self,
        track: &[u8; 16],
        sequence: &[u8; 16],
        clip: &[u8; 16],
        data_def: &str,
        number: u32,
        frames: u64,
        source: Option<Vec<u8>>,
    ) -> Vec<u8> {
        let mut out = self.track(track, 2, number, "Sound Track", sequence);
        let mut seq = Set::new("060e2b34025301010d01010101010f00");
        seq.prop(0x3c0a, sequence)
            .prop(0x0201, &label(data_def))
            .u64(0x0202, frames)
            .prop(0x1001, &batch(&[clip.to_vec()]));
        out.extend(seq.finish());
        let mut c = Set::new("060e2b34025301010d01010101011100");
        c.prop(0x3c0a, clip)
            .prop(0x0201, &label(data_def))
            .u64(0x0202, frames)
            .u64(0x1201, 0);
        match source {
            Some(umid) => c.prop(0x1101, &umid).u32(0x1102, 2),
            None => c.prop(0x1101, &[0u8; 32]).u32(0x1102, 0),
        };
        out.extend(c.finish());
        out
    }

    fn descriptor(&self) -> Vec<u8> {
        let (num, den) = self.options.frame_rate;
        let channels = self.params.channels() as u32;
        let block_align = (channels * 3) as u16;
        let mut d = Set::new("060e2b34025301010d01010101014800");
        d.prop(0x3c0a, &self.ids.descriptor)
            .u32(0x3006, 2)
            .prop(0x3001, &rational(num, den))
            .u64(0x3002, self.frames_total);
        d.prop(0x3004, &label("060e2b34040101010d01030102060100"))
            .prop(0x3d03, &rational(self.params.rate, 1))
            .u8(0x3d02, 0);
        d.u32(0x3d07, channels)
            .u32(0x3d01, 24)
            .u16(0x3d0a, block_align)
            .u32(0x3d09, self.params.rate * block_align as u32);
        if let Some(assignment) = &self.channel_label {
            d.prop(primer::CHANNEL_ASSIGNMENT, assignment);
        }
        d.finish()
    }

    /// One edit unit of audio: the pending interleaved samples (padded with silence if short).
    fn write_frame(&mut self) -> std::io::Result<()> {
        let take = self.pending.len().min(self.frame_bytes);
        let mut frame: Vec<u8> = self.pending.drain(..take).collect();
        frame.resize(self.frame_bytes, 0);
        self.out.write_all(&klv(&label(ESSENCE_KEY), &frame))?;
        self.frames_written += 1;
        Ok(())
    }
}

impl BlockWriter for DcpWriter {
    fn write_block(&mut self, planar: &[Vec<f32>], frames: usize) -> std::io::Result<()> {
        let samples = to_integers(planar, &self.params.order, frames, 24);
        for n in 0..frames {
            for channel in &samples {
                self.pending.extend_from_slice(&channel[n].to_le_bytes()[..3]);
            }
        }
        while self.pending.len() >= self.frame_bytes {
            self.write_frame()?;
        }
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> std::io::Result<u64> {
        while self.frames_written < self.frames_total {
            self.write_frame()?;
        }
        self.out.flush()?;
        let footer = self.out.stream_position()?;
        let index = mxf::index_segment(
            self.options.frame_rate,
            if self.options.smpte { 0 } else { self.frames_total },
            self.frame_bytes as u32 + KLV_OVERHEAD,
            INDEX_SID,
            BODY_SID,
        );
        let previous = if self.options.smpte { HEADER_SIZE as u64 } else { 0 };
        let pack = self.partition(
            Kind::Footer,
            footer,
            previous,
            footer,
            0,
            index.len() as u64,
            INDEX_SID,
            0,
        );
        let rip = if self.options.smpte {
            mxf::random_index(&[(0, 0), (BODY_SID, HEADER_SIZE as u64), (0, footer)])
        } else {
            mxf::random_index(&[(BODY_SID, 0), (0, footer)])
        };
        for part in [&pack, &index, &rip] {
            self.out.write_all(part)?;
        }
        let header = self.header(footer)?;
        self.out.seek(SeekFrom::Start(0))?;
        self.out.write_all(&header)?;
        self.out.flush()?;
        Ok(footer + (pack.len() + index.len() + rip.len()) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_follow_the_channel_arrangement() {
        assert_eq!(channel_label(6, 0x3F)[14], 0x01);
        assert_eq!(channel_label(8, 0x63F)[14], 0x05);
        assert_eq!(channel_label(6, 0x07)[14], 0x04);
        assert_eq!(channel_label(16, 0)[14], 0x04);
    }

    #[test]
    fn settings_are_read() {
        assert_eq!(DcpOptions::parse("").unwrap().frame_rate, (24, 1));
        let o = DcpOptions::parse("23.976-interop").unwrap();
        assert_eq!((o.frame_rate, o.smpte), ((24_000, 1001), false));
        assert!(DcpOptions::parse("29.97").is_err());
        assert!(DcpOptions::parse("24-oldschool").is_err());
    }

    #[test]
    fn a_frame_holds_one_picture_frame_of_audio() {
        assert_eq!(48_000 / 24, 2000);
        assert!(FRAME_RATES
            .iter()
            .all(|&(n, d)| (48_000u64 * d as u64).is_multiple_of(n as u64)));
    }
}
