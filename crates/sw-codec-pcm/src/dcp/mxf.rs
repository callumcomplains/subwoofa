//! The parts of an MXF file (SMPTE ST 377-1) that an AS-DCP sound track file is made of.
//!
//! MXF is a sequence of KLV triplets: a 16-byte key (a SMPTE universal label), a BER length, the value. This file
//! builds the triplets this writer needs: header metadata *sets* (lists of tag/length/value properties), partition
//! packs, the primer pack that names the property tags, the index table segment and the random index pack.

use std::time::{SystemTime, UNIX_EPOCH};

pub(super) type Label = [u8; 16];

/// A label written as the 32 hex digits the specifications print it with.
pub(super) fn label(hex: &str) -> Label {
    let mut out = [0u8; 16];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("labels are hex");
    }
    out
}

/// A KLV triplet with a four-byte BER length (`0x83` and three length bytes), as AS-DCP writes them.
pub(super) fn klv(key: &Label, value: &[u8]) -> Vec<u8> {
    let len = value.len() as u32;
    let mut out = Vec::with_capacity(20 + value.len());
    out.extend_from_slice(key);
    out.extend_from_slice(&[0x83, (len >> 16) as u8, (len >> 8) as u8, len as u8]);
    out.extend_from_slice(value);
    out
}

/// A local set: properties, each a two-byte tag, a two-byte length and the value.
pub(super) struct Set {
    key: Label,
    body: Vec<u8>,
}

impl Set {
    pub fn new(key: &str) -> Set {
        Set {
            key: label(key),
            body: Vec::new(),
        }
    }

    pub fn prop(&mut self, tag: u16, value: &[u8]) -> &mut Set {
        self.body.extend_from_slice(&tag.to_be_bytes());
        self.body.extend_from_slice(&(value.len() as u16).to_be_bytes());
        self.body.extend_from_slice(value);
        self
    }

    pub fn u8(&mut self, tag: u16, v: u8) -> &mut Set {
        self.prop(tag, &[v])
    }

    pub fn u16(&mut self, tag: u16, v: u16) -> &mut Set {
        self.prop(tag, &v.to_be_bytes())
    }

    pub fn u32(&mut self, tag: u16, v: u32) -> &mut Set {
        self.prop(tag, &v.to_be_bytes())
    }

    pub fn u64(&mut self, tag: u16, v: u64) -> &mut Set {
        self.prop(tag, &v.to_be_bytes())
    }

    pub fn finish(&self) -> Vec<u8> {
        klv(&self.key, &self.body)
    }
}

/// A batch: how many items, how long each is, then the items.
pub(super) fn batch(items: &[Vec<u8>]) -> Vec<u8> {
    let size = items.first().map_or(16, |i| i.len());
    let mut out = Vec::new();
    out.extend_from_slice(&(items.len() as u32).to_be_bytes());
    out.extend_from_slice(&(size as u32).to_be_bytes());
    for item in items {
        out.extend_from_slice(item);
    }
    out
}

/// A rational number (an edit rate, a sampling rate).
pub(super) fn rational(numerator: u32, denominator: u32) -> Vec<u8> {
    let mut out = numerator.to_be_bytes().to_vec();
    out.extend_from_slice(&denominator.to_be_bytes());
    out
}

/// A string as UTF-16, big endian, as MXF stores text.
pub(super) fn utf16(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(|c| c.to_be_bytes()).collect()
}

/// Random bytes: a SplitMix64 generator seeded from the clock, the process and a counter.
fn random_bytes<const N: usize>() -> [u8; N] {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let mut state = nanos
        ^ (std::process::id() as u64).rotate_left(32)
        ^ COUNTER.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
    let mut out = [0u8; N];
    for chunk in out.chunks_mut(8) {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        chunk.copy_from_slice(&z.to_le_bytes()[..chunk.len()]);
    }
    out
}

/// A random (version 4) UUID.
pub(super) fn uuid() -> [u8; 16] {
    let mut id = random_bytes::<16>();
    id[6] = (id[6] & 0x0F) | 0x40;
    id[8] = (id[8] & 0x3F) | 0x80;
    id
}

/// A SMPTE UMID (basic, 32 bytes) for a package: the fixed prefix and 16 random bytes of material number.
pub(super) fn umid() -> Vec<u8> {
    let mut out = label("060a2b340101010501010f2013000000").to_vec();
    out.extend_from_slice(&uuid());
    out
}

/// The current time as an MXF timestamp: year (2 bytes), month, day, hour, minute, second, quarter-milliseconds.
pub(super) fn timestamp() -> [u8; 8] {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rest) = (secs / 86_400, secs % 86_400);
    let (year, month, day) = civil_from_days(days as i64);
    let year = (year as u16).to_be_bytes();
    [
        year[0],
        year[1],
        month as u8,
        day as u8,
        (rest / 3600) as u8,
        (rest % 3600 / 60) as u8,
        (rest % 60) as u8,
        0,
    ]
}

/// Calendar date from days since 1970-01-01 (proleptic Gregorian; the usual civil-from-days algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Which partition a pack describes (the seventh byte of its key).
#[derive(Clone, Copy)]
pub(super) enum Kind {
    Header = 2,
    Body = 3,
    Footer = 4,
}

/// A partition pack (closed and complete).
pub(super) struct Partition {
    pub kind: Kind,
    pub this: u64,
    pub previous: u64,
    pub footer: u64,
    pub header_bytes: u64,
    pub index_bytes: u64,
    pub index_sid: u32,
    pub body_sid: u32,
    pub operational_pattern: Label,
    pub containers: Vec<Label>,
}

impl Partition {
    pub fn bytes(&self) -> Vec<u8> {
        let mut key = label("060e2b34020501010d01020101000400");
        key[13] = self.kind as u8;
        let mut v = Vec::new();
        v.extend_from_slice(&1u16.to_be_bytes()); // major version
        v.extend_from_slice(&2u16.to_be_bytes()); // minor version
        v.extend_from_slice(&1u32.to_be_bytes()); // KAG size
        for field in [
            self.this,
            self.previous,
            self.footer,
            self.header_bytes,
            self.index_bytes,
        ] {
            v.extend_from_slice(&field.to_be_bytes());
        }
        v.extend_from_slice(&self.index_sid.to_be_bytes());
        v.extend_from_slice(&0u64.to_be_bytes()); // body offset
        v.extend_from_slice(&self.body_sid.to_be_bytes());
        v.extend_from_slice(&self.operational_pattern);
        v.extend(batch(&self.containers.iter().map(|c| c.to_vec()).collect::<Vec<_>>()));
        klv(&key, &v)
    }
}

/// The primer pack: the tag to label table.
pub(super) fn primer(entries: &[(u16, Label)]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    v.extend_from_slice(&18u32.to_be_bytes());
    for (tag, ul) in entries {
        v.extend_from_slice(&tag.to_be_bytes());
        v.extend_from_slice(ul);
    }
    klv(&label("060e2b34020501010d01020101050100"), &v)
}

/// Filler that makes `used` bytes into `total` (it takes at least a key and a length).
pub(super) fn filler(used: usize, total: usize) -> Option<Vec<u8>> {
    let room = total.checked_sub(used + 20)?;
    Some(klv(&label("060e2b34010101020301021001000000"), &vec![0u8; room]))
}

/// A constant-bit-rate index table segment: no entries, one fixed size for every edit unit.
pub(super) fn index_segment(
    edit_rate: (u32, u32),
    duration: u64,
    edit_unit_bytes: u32,
    index_sid: u32,
    body_sid: u32,
) -> Vec<u8> {
    let mut s = Set::new("060e2b34025301010d01020101100100");
    s.prop(0x3c0a, &uuid());
    s.prop(0x3f0b, &rational(edit_rate.0, edit_rate.1));
    s.u64(0x3f0c, 0); // start position
    s.u64(0x3f0d, duration);
    s.u32(0x3f05, edit_unit_bytes);
    s.u32(0x3f06, index_sid);
    s.u32(0x3f07, body_sid);
    s.u8(0x3f08, 0); // slice count
    s.u8(0x3f0e, 0); // position table count
    s.prop(0x3f0a, &[0, 0, 0, 0, 0, 0, 0, 0x0b]); // an empty entry array of 11-byte entries
    s.finish()
}

/// The random index pack: where each partition starts (body SID, offset), then the pack's own length.
pub(super) fn random_index(entries: &[(u32, u64)]) -> Vec<u8> {
    let mut v = Vec::new();
    for (sid, offset) in entries {
        v.extend_from_slice(&sid.to_be_bytes());
        v.extend_from_slice(&offset.to_be_bytes());
    }
    let total = 20 + v.len() as u32 + 4;
    v.extend_from_slice(&total.to_be_bytes());
    klv(&label("060e2b34020501010d01020101110100"), &v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_come_out_right() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_000), (2024, 10, 4));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn uuids_are_version_four_and_differ() {
        let (a, b) = (uuid(), uuid());
        assert_ne!(a, b);
        assert_eq!(a[6] >> 4, 4);
        assert_eq!(a[8] >> 6, 2);
    }

    #[test]
    fn klv_has_a_four_byte_length() {
        let k = klv(&[0; 16], &[1, 2, 3]);
        assert_eq!(&k[16..20], &[0x83, 0, 0, 3]);
        assert_eq!(k.len(), 23);
    }

    #[test]
    fn set_properties_are_tag_length_value() {
        let mut s = Set::new("060e2b34025301010d01010101012f00");
        s.u16(0x3b05, 0x0102);
        let bytes = s.finish();
        assert_eq!(&bytes[20..], &[0x3b, 0x05, 0x00, 0x02, 0x01, 0x02]);
    }

    #[test]
    fn partition_pack_is_120_bytes_with_two_containers() {
        let p = Partition {
            kind: Kind::Header,
            this: 0,
            previous: 0,
            footer: 0,
            header_bytes: 0,
            index_bytes: 0,
            index_sid: 0,
            body_sid: 0,
            operational_pattern: label("060e2b34040101020d01020110000000"),
            containers: vec![
                label("060e2b34040101010d01030102060100"),
                label("060e2b34040101030d010301027f0100"),
            ],
        };
        assert_eq!(p.bytes().len(), 140);
    }
}
