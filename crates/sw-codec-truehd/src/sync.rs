//! The major sync: the 28-byte header at the start of every restart interval. It tells a decoder the stream's
//! sample rate, channel arrangement and substream structure, so playback can begin at any restart.
//!
//! Both flavours start with the same 24-bit sync word; the fourth byte says TrueHD (`0xBA`) or MLP (`0xBB`). The
//! last two bytes are a checksum of the 26 before them.

use crate::bits::{major_sync_checksum, BitWriter};
use crate::layout::MLP_CHANNEL_INFO;
use crate::tables::GROUP_CHANNELS;
use crate::{Encoder, Mlp};

const SYNC_WORD: u32 = 0xF8_726F;
const TRUEHD_ID: u32 = 0xBA;
const MLP_ID: u32 = 0xBB;
/// Fixed value that follows the channel fields of both flavours.
const SIGNATURE: u32 = 0xB752;
/// Bytes of a major sync, checksum included.
const LEN: usize = 28;

/// The peak data rate field: 9.6 Mbit/s, in 1/16 bit per sample period.
fn peak_data_rate(sample_rate: u32) -> u32 {
    (((9_600_000u32 << 4) - 8) / sample_rate) & 0x7FFF
}

/// Pads the bits written so far to the length of a major sync and adds the checksum.
fn seal(w: BitWriter) -> [u8; LEN] {
    let mut bytes = w.snapshot();
    bytes.resize(LEN, 0);
    let checksum = major_sync_checksum(&bytes[..LEN - 2]);
    bytes[LEN - 2..].copy_from_slice(&checksum.to_le_bytes());
    bytes.try_into().unwrap()
}

impl Encoder {
    /// The major sync for this stream: MLP's or TrueHD's.
    pub(crate) fn write_major_sync(&self) -> [u8; LEN] {
        match self.cfg.mlp {
            Some(mlp) => self.mlp_sync(mlp),
            None => self.truehd_sync(),
        }
    }

    /// Channel map of the first six coded channels (the "6-channel presentation").
    fn chanmap6(&self) -> u16 {
        let mut map = 0u16;
        let mut count = 0;
        for group in 0..13 {
            if self.cfg.chanmap >> group & 1 == 1 {
                if count + GROUP_CHANNELS[group] > 6 {
                    break;
                }
                map |= 1 << group;
                count += GROUP_CHANNELS[group];
            }
        }
        map
    }

    fn truehd_sync(&self) -> [u8; LEN] {
        let mut w = BitWriter::new();
        w.put(SYNC_WORD, 24);
        w.put(TRUEHD_ID, 8);
        w.put(self.rate_code, 4);
        w.put(0, 1); // multichannel type of the 6-channel presentation
        w.put(0, 1); // multichannel type of the 8-channel presentation
        w.put(0, 2);
        // channel modifiers of the 2, 6 and 8-channel presentations: 0 ordinary, 1 stereo, 3 mono
        let (modifier2, modifier6, modifier8) = match self.cfg.channels {
            1 => (3, 3, 3),
            2 => (1, 1, 1),
            _ => (0, 0, 0),
        };
        w.put(modifier2, 2);
        w.put(modifier6, 2);
        let arrangement6 = if self.cfg.channels <= 6 {
            self.cfg.chanmap
        } else {
            self.chanmap6()
        };
        w.put(arrangement6 as u32 & 0x1F, 5);
        w.put(modifier8, 2);
        w.put(self.cfg.chanmap as u32, 13);
        w.put(SIGNATURE, 16);
        w.put(0, 16); // flags
        w.put(0, 16);
        w.put(1, 1); // variable bit rate
        w.put(peak_data_rate(self.cfg.sample_rate), 15);
        w.put(self.subs.len() as u32, 4);
        w.put(0, 2);
        w.put(0, 2); // extended substream info
        w.put(if self.cfg.channels <= 2 { 0x14 } else { 0x3C }, 8); // substream info
        w.put(0, 6);
        w.put(0, 4); // control flags
        w.put(0, 7); // dynamic range control start-up gain
                     // the channel-meaning fields (dialogue level, mix level, ...) are not used: all zero
        for bits in [6, 6, 5, 6, 5, 5, 6, 6, 1, 1] {
            w.put(0, bits);
        }
        seal(w)
    }

    fn mlp_sync(&self, mlp: Mlp) -> [u8; LEN] {
        let mut w = BitWriter::new();
        w.put(SYNC_WORD, 24);
        w.put(MLP_ID, 8);
        w.put(
            match mlp.bits {
                16 => 0,
                20 => 1,
                _ => 2,
            },
            4,
        ); // word length of the first group of channels
        w.put(0xF, 4); // second group: not present
        w.put(self.rate_code, 4);
        w.put(0xF, 4); // second group's rate: not present
        w.put(0, 4);
        w.put(0, 4); // multichannel type
        w.put(0, 3);
        w.put(mlp.arrangement as u32, 5);
        w.put(SIGNATURE, 16);
        w.put(0x4000, 16); // flags: DVD-Audio
        w.put(0, 16);
        w.put(1, 1); // variable bit rate
        w.put(peak_data_rate(self.cfg.sample_rate), 15);
        w.put(1, 4); // one substream
        w.put(0, 2);
        w.put(0, 2); // extended substream info
        let high_rate = self.rate_code & 3 == 2;
        let substream_info = 0x04 | u32::from(self.cfg.channels <= 2) | if high_rate { 0x02 } else { 0 };
        w.put(substream_info, 8);
        let rate_field = match self.cfg.sample_rate {
            44_100 => 0x09,
            88_200 => 0x0D,
            176_400 => 0x11,
            48_000 => 0x0A,
            96_000 => 0x0E,
            _ => 0x12,
        };
        w.put(rate_field, 5);
        w.put(mlp.bits, 5);
        let (occupancy, summary) = MLP_CHANNEL_INFO[mlp.arrangement as usize];
        w.put(occupancy as u32, 6);
        w.put(0, 3);
        w.put(0, 10); // speaker layout
        w.put(0, 3); // copy protection
        w.put(0x8080, 16);
        w.put(0, 7);
        w.put(0, 4); // source format
        w.put(summary as u32, 5);
        seal(w)
    }
}
