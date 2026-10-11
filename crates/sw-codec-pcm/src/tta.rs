//! TTA (True Audio, version 1), 16 or 24 bit. A TTA file is a 22-byte header, a table of frame sizes, then the
//! frames. Frames hold `256 * rate / 245` samples (about 1.04 s). Inside a frame every channel is coded by
//! * a channel decorrelation (each channel but the last stores its difference to the next; the last is adjusted by half
//!   of that difference),
//! * a first-order predictor,
//! * an adaptive 8-tap sign-LMS filter,
//! * adaptive two-stage Rice coding,
//!
//! written least significant bit first, and every frame ends with its CRC-32.

use crate::{to_integers, BlockWriter, Params, SampleFormat};
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

/// `1 << k`, saturating at the top of the table the format uses (index 32 and above give 2^31).
fn shift_1(k: u32) -> u32 {
    1u32 << k.min(31)
}

/// `16 << k` as the Rice adaptation compares sums (the table is the same one shifted by four places).
fn shift_16(k: u32) -> u32 {
    shift_1(k + 4)
}

/// Bits written from the least significant end of each byte.
#[derive(Default)]
struct LeBitWriter {
    bytes: Vec<u8>,
    pending: u64,
    pending_bits: u32,
}

impl LeBitWriter {
    fn put(&mut self, value: u32, count: u32) {
        debug_assert!(count <= 32);
        if count == 0 {
            return;
        }
        self.pending |= ((value as u64) & ((1u64 << count) - 1)) << self.pending_bits;
        self.pending_bits += count;
        while self.pending_bits >= 8 {
            self.bytes.push(self.pending as u8);
            self.pending >>= 8;
            self.pending_bits -= 8;
        }
    }

    /// `count` one bits.
    fn put_ones(&mut self, mut count: u32) {
        while count > 0 {
            let step = count.min(31);
            self.put((1u32 << step) - 1, step);
            count -= step;
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.pending_bits > 0 {
            self.bytes.push(self.pending as u8);
        }
        self.bytes
    }
}

/// CRC-32 (IEEE, reflected), as the frames and the header use.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// The adaptive filter of one channel.
struct Filter {
    shift: u32,
    round: i32,
    error: i32,
    qm: [i32; 8],
    dx: [i32; 8],
    dl: [i32; 8],
}

impl Filter {
    fn new(shift: u32) -> Filter {
        Filter { shift, round: 1 << (shift - 1), error: 0, qm: [0; 8], dx: [0; 8], dl: [0; 8] }
    }

    /// Filters one sample and updates the weights. Returns what is left of the sample.
    fn process(&mut self, input: i32) -> i32 {
        // weights follow the sign of the previous error
        if self.error < 0 {
            for i in 0..8 {
                self.qm[i] = self.qm[i].wrapping_sub(self.dx[i]);
            }
        } else if self.error > 0 {
            for i in 0..8 {
                self.qm[i] = self.qm[i].wrapping_add(self.dx[i]);
            }
        }
        let mut sum = self.round;
        for i in 0..8 {
            sum = sum.wrapping_add(self.dl[i].wrapping_mul(self.qm[i]));
        }
        // shift the history
        for i in 0..4 {
            self.dx[i] = self.dx[i + 1];
            self.dl[i] = self.dl[i + 1];
        }
        self.dx[4] = (self.dl[4] >> 30) | 1;
        self.dx[5] = ((self.dl[5] >> 30) | 2) & !1;
        self.dx[6] = ((self.dl[6] >> 30) | 2) & !1;
        self.dx[7] = ((self.dl[7] >> 30) | 4) & !3;
        self.dl[4] = self.dl[5].wrapping_neg();
        self.dl[5] = self.dl[6].wrapping_neg();
        self.dl[6] = input.wrapping_sub(self.dl[7]);
        self.dl[7] = input;
        self.dl[5] = self.dl[5].wrapping_add(self.dl[6]);
        self.dl[4] = self.dl[4].wrapping_add(self.dl[5]);
        let out = input.wrapping_sub(sum >> self.shift);
        self.error = out;
        out
    }
}

/// Adaptive Rice parameters of one channel.
struct Rice {
    k0: u32,
    k1: u32,
    sum0: u32,
    sum1: u32,
}

impl Rice {
    fn new() -> Rice {
        Rice { k0: 10, k1: 10, sum0: shift_16(10), sum1: shift_16(10) }
    }
}

struct Channel {
    predictor: i32,
    filter: Filter,
    rice: Rice,
}

/// One frame (`channels[c][i]`) as bytes, CRC included.
fn encode_frame(channels: &[Vec<i32>], bytes_per_sample: u32) -> Vec<u8> {
    let nch = channels.len();
    let n = channels[0].len();
    let filter_shift = [10u32, 9, 10, 12][bytes_per_sample as usize - 1];
    let mut state: Vec<Channel> = (0..nch).map(|_| Channel { predictor: 0, filter: Filter::new(filter_shift), rice: Rice::new() }).collect();
    let mut w = LeBitWriter::default();
    for i in 0..n {
        let mut res = 0i32;
        for c in 0..nch {
            let mut value = channels[c][i];
            if nch > 1 {
                if c < nch - 1 {
                    // difference to the next channel
                    value = channels[c + 1][i].wrapping_sub(value);
                    res = value;
                } else {
                    value = value.wrapping_sub(res / 2);
                }
            }
            let ch = &mut state[c];
            let before_prediction = value;
            let predictor_shift = if bytes_per_sample == 1 { 4 } else { 5 };
            let predicted = ((((ch.predictor as i64) << predictor_shift) - ch.predictor as i64) >> predictor_shift) as i32;
            value = value.wrapping_sub(predicted);
            ch.predictor = before_prediction;
            let value = ch.filter.process(value);
            let mut coded: u32 = if value > 0 { ((value as u32) << 1) - 1 } else { (value.wrapping_neg() as u32) << 1 };

            let rice = &mut ch.rice;
            let mut k = rice.k0;
            rice.sum0 = rice.sum0.wrapping_add(coded).wrapping_sub(rice.sum0 >> 4);
            if rice.k0 > 0 && rice.sum0 < shift_16(rice.k0) {
                rice.k0 -= 1;
            } else if rice.sum0 > shift_16(rice.k0 + 1) {
                rice.k0 += 1;
            }
            if coded >= shift_1(k) {
                // second stage: what is above the first-stage range
                coded -= shift_1(k);
                k = rice.k1;
                rice.sum1 = rice.sum1.wrapping_add(coded).wrapping_sub(rice.sum1 >> 4);
                if rice.k1 > 0 && rice.sum1 < shift_16(rice.k1) {
                    rice.k1 -= 1;
                } else if rice.sum1 > shift_16(rice.k1 + 1) {
                    rice.k1 += 1;
                }
                w.put_ones(1 + (coded >> k));
            }
            w.put(0, 1);
            if k > 0 {
                w.put(coded & (shift_1(k) - 1), k);
            }
        }
    }
    let mut bytes = w.finish();
    let crc = crc32(&bytes);
    bytes.extend_from_slice(&crc.to_le_bytes());
    bytes
}

pub struct TtaWriter {
    out: BufWriter<File>,
    params: Params,
    bits: u32,
    frame_samples: usize,
    pending: Vec<Vec<i32>>,
    frame_sizes: Vec<u32>,
    expected_frames: usize,
    /// Bytes reserved at the start for the header and the seek table.
    header_len: u64,
}

impl TtaWriter {
    pub fn create(path: &Path, params: Params) -> std::io::Result<TtaWriter> {
        let bits = match params.format {
            SampleFormat::Int16 => 16,
            SampleFormat::Int24 => 24,
            _ => return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "TTA holds 16 or 24 bit integer samples")),
        };
        if params.total_frames > u32::MAX as u64 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "TTA files hold at most 2^32 samples per channel"));
        }
        let frame_samples = (256u64 * params.rate as u64 / 245) as usize;
        let expected_frames = (params.total_frames as usize).div_ceil(frame_samples);
        let header_len = 22 + 4 * expected_frames as u64 + 4;
        let mut out = BufWriter::new(File::create(path)?);
        out.write_all(&vec![0u8; header_len as usize])?; // filled in at the end
        Ok(TtaWriter { out, bits, frame_samples, pending: vec![Vec::new(); params.channels()], frame_sizes: Vec::new(), expected_frames, header_len, params })
    }

    fn write_frame(&mut self, block: &[Vec<i32>]) -> std::io::Result<()> {
        let bytes = encode_frame(block, self.bits / 8);
        self.frame_sizes.push(bytes.len() as u32);
        self.out.write_all(&bytes)
    }
}

impl BlockWriter for TtaWriter {
    fn write_block(&mut self, planar: &[Vec<f32>], frames: usize) -> std::io::Result<()> {
        let converted = to_integers(planar, &self.params.order, frames, self.bits);
        for (pending, new) in self.pending.iter_mut().zip(converted) {
            pending.extend(new);
        }
        while self.pending[0].len() >= self.frame_samples {
            let block: Vec<Vec<i32>> = self.pending.iter_mut().map(|p| p.drain(..self.frame_samples).collect()).collect();
            self.write_frame(&block)?;
        }
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> std::io::Result<u64> {
        if !self.pending[0].is_empty() {
            let block: Vec<Vec<i32>> = self.pending.iter_mut().map(std::mem::take).collect();
            self.write_frame(&block)?;
        }
        debug_assert_eq!(self.frame_sizes.len(), self.expected_frames);
        self.out.flush()?;
        // header: "TTA1", format (1 = PCM), channels, bits, rate, samples, CRC-32 of those
        let mut header = Vec::with_capacity(22);
        header.extend_from_slice(b"TTA1");
        header.extend_from_slice(&1u16.to_le_bytes());
        header.extend_from_slice(&(self.params.channels() as u16).to_le_bytes());
        header.extend_from_slice(&(self.bits as u16).to_le_bytes());
        header.extend_from_slice(&self.params.rate.to_le_bytes());
        header.extend_from_slice(&(self.params.total_frames as u32).to_le_bytes());
        let crc = crc32(&header);
        header.extend_from_slice(&crc.to_le_bytes());
        // seek table: the size of every frame, then its CRC-32
        let mut table = Vec::with_capacity(4 * self.frame_sizes.len() + 4);
        for s in &self.frame_sizes {
            table.extend_from_slice(&s.to_le_bytes());
        }
        let crc = crc32(&table);
        table.extend_from_slice(&crc.to_le_bytes());
        let total: u64 = self.header_len + self.frame_sizes.iter().map(|&s| s as u64).sum::<u64>();
        let file = self.out.get_mut();
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&header)?;
        file.write_all(&table)?;
        file.flush()?;
        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_of_the_check_string() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn bits_are_written_from_the_low_end() {
        let mut w = LeBitWriter::default();
        w.put(0b1, 1);
        w.put(0b10, 2); // bits 1..2 hold 0, 1
        w.put_ones(3);
        // bit 0 = 1, bits 1-2 = 0b10 (low bit first: 0, then 1), bits 3-5 = 1
        assert_eq!(w.finish(), vec![0b0011_1101]);
    }
}
