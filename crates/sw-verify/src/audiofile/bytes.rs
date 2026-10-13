//! Reading numbers out of header bytes, the size check every format shares, and the CRC-32 several use.

use crate::{Collector, Status};

pub(crate) fn le16(d: &[u8]) -> u16 {
    u16::from_le_bytes([d[0], d[1]])
}

pub(crate) fn le32(d: &[u8]) -> u32 {
    u32::from_le_bytes([d[0], d[1], d[2], d[3]])
}

pub(crate) fn le64(d: &[u8]) -> u64 {
    u64::from_le_bytes(d[..8].try_into().unwrap())
}

pub(crate) fn be16(d: &[u8]) -> u16 {
    u16::from_be_bytes([d[0], d[1]])
}

pub(crate) fn be32(d: &[u8]) -> u32 {
    u32::from_be_bytes([d[0], d[1], d[2], d[3]])
}

pub(crate) fn be64(d: &[u8]) -> u64 {
    u64::from_be_bytes(d[..8].try_into().unwrap())
}

/// Whether the given size claims fit the file; reports the outcome as one check.
pub(crate) fn size_check(c: &mut Collector, name: &str, claimed: u64, actual: u64) {
    if claimed == actual {
        c.check(name, Status::Pass, format!("{actual} bytes, as the header says"));
    } else if claimed < actual {
        c.check(
            name,
            Status::Warn,
            format!("the header accounts for {claimed} bytes but the file has {actual} (trailing data)"),
        );
    } else {
        c.check(
            name,
            Status::Fail,
            format!("the header says {claimed} bytes but the file has only {actual} (truncated)"),
        );
    }
}

pub(crate) fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}
