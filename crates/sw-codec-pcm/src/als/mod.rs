//! MPEG-4 ALS (audio lossless coding, ISO/IEC 14496-3 subpart 11): 16 or 24 bit, any number of channels.
//!
//! A raw `.als` file is a configuration record (`ALSSpecificConfig`, 34 bytes here) followed by the frames. Every
//! frame holds `frame_length` samples of every channel; each channel is one block ([`block`]): linear prediction
//! with reflection coefficients ([`lpc`]) and Rice-coded residuals. Neighbouring channels are paired and may code
//! the difference of the pair instead of one of them (joint stereo), whichever is smaller.
//!
//! The same record can sit inside an MP4 file (one frame per MP4 sample), which is what FFmpeg and most players
//! open: the writer makes an MP4 when the output name ends in `.mp4` or `.m4a`, and a raw file otherwise.
//!
//! Left out for now: block switching, long-term prediction, the BGMC entropy coder, RLS-LMS and floating point.

mod block;
mod lpc;

use crate::bitwriter::BitWriter;
use crate::crc::SampleCrc;
use crate::mp4_audio::{self, atom};
use crate::{to_integers, BlockWriter, Params, SampleFormat};
use block::Settings;
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

/// Highest predictor order used.
const MAX_ORDER: usize = 64;

/// Bytes of the configuration record as written here: 22 fixed, 8 for header and trailer sizes, 4 for the CRC.
const CONFIG_LEN: usize = 34;

/// MPEG-4 sampling frequency indices, for the audio specific config in MP4 files.
const RATES: [u32; 13] = [
    96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
];

/// Frame length in samples for a sample rate: about 85 ms, tried against 2048 and found slightly better.
fn frame_length(rate: u32) -> usize {
    match rate {
        0..=48_000 => 4096,
        48_001..=96_000 => 8192,
        _ => 16_384,
    }
}

/// Which Rice table codes the coefficients best for the rate.
fn coefficient_table(rate: u32) -> usize {
    match rate {
        0..=48_000 => 0,
        48_001..=96_000 => 1,
        _ => 2,
    }
}

enum Output {
    Raw,
    Mp4(mp4_audio::Start),
}

pub struct AlsWriter {
    out: BufWriter<File>,
    output: Output,
    params: Params,
    bits: u32,
    cfg: Settings,
    frame_len: usize,
    pending: Vec<Vec<i32>>,
    /// The last `MAX_ORDER` samples of each channel (what the predictor looks back on).
    history: Vec<Vec<i32>>,
    frame_sizes: Vec<u32>,
    crc: SampleCrc,
}

impl AlsWriter {
    pub fn create(path: &Path, params: Params) -> std::io::Result<AlsWriter> {
        let bits = match params.format {
            SampleFormat::Int16 => 16,
            SampleFormat::Int24 => 24,
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "MPEG-4 ALS here holds 16 or 24 bit integer samples",
                ))
            }
        };
        if params.total_frames > u32::MAX as u64 - 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "MPEG-4 ALS files hold at most 2^32 samples per channel",
            ));
        }
        let mut out = BufWriter::new(File::create(path)?);
        let is_mp4 = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("mp4") || e.eq_ignore_ascii_case("m4a"));
        let output = if is_mp4 {
            Output::Mp4(mp4_audio::write_start(&mut out, b"mp42")?)
        } else {
            out.write_all(&[0u8; CONFIG_LEN])?; // the record is filled in at the end
            Output::Raw
        };
        let channels = params.channels();
        Ok(AlsWriter {
            out,
            output,
            bits,
            cfg: Settings {
                max_order: MAX_ORDER,
                bits,
                table: coefficient_table(params.rate),
            },
            frame_len: frame_length(params.rate),
            pending: vec![Vec::new(); channels],
            history: vec![vec![0; MAX_ORDER]; channels],
            frame_sizes: Vec::new(),
            crc: SampleCrc::new(bits),
            params,
        })
    }

    /// Codes one frame (every channel's block) and writes it.
    fn write_frame(&mut self, samples: &[Vec<i32>]) -> std::io::Result<()> {
        let extended: Vec<Vec<i32>> = samples
            .iter()
            .zip(&self.history)
            .map(|(s, h)| [h.as_slice(), s].concat())
            .collect();
        let mut frame = Vec::new();
        let mut c = 0;
        while c < extended.len() {
            if c + 1 < extended.len() {
                let (first, second) = pair_blocks(&extended[c], &extended[c + 1], self.cfg);
                frame.extend(first);
                frame.extend(second);
                c += 2;
            } else {
                frame.extend(block::encode(&extended[c], MAX_ORDER, false, self.cfg));
                c += 1;
            }
        }
        self.frame_sizes.push(frame.len() as u32);
        self.out.write_all(&frame)?;
        self.crc.update(samples);
        for (history, ext) in self.history.iter_mut().zip(&extended) {
            *history = ext[ext.len() - MAX_ORDER..].to_vec();
        }
        Ok(())
    }

    /// The MP4 start record, if the output is an MP4 file.
    fn output_kind(&self) -> Option<mp4_audio::Start> {
        match self.output {
            Output::Mp4(start) => Some(start),
            Output::Raw => None,
        }
    }

    /// The `ALSSpecificConfig` record, now that the CRC is known.
    fn config(&self) -> Vec<u8> {
        let p = &self.params;
        let mut w = BitWriter::default();
        w.put(u32::from_be_bytes(*b"ALS\0") as u64, 32);
        w.put(p.rate as u64, 32);
        w.put(p.total_frames, 32);
        w.put(p.channels() as u64 - 1, 16);
        w.put(0, 3); // file type: none
        w.put((self.bits / 8 - 1) as u64, 3); // resolution
        w.put(0, 1); // not floating point
        w.put(0, 1); // little endian samples
        w.put(self.frame_len as u64 - 1, 16);
        w.put(0, 8); // random access distance: none
        w.put(0, 2); // random access info: none
        w.put(1, 1); // the order is chosen per block
        w.put(self.cfg.table as u64, 2);
        w.put(0, 1); // no long-term prediction
        w.put(MAX_ORDER as u64, 10);
        w.put(0, 2); // no block switching
        w.put(0, 1); // no BGMC
        w.put(1, 1); // sub-blocks for the Rice parameters
        w.put((p.channels() > 1) as u64, 1); // joint stereo
        w.put(0, 1); // no multi-channel coding
        w.put(0, 1); // no channel configuration
        w.put(0, 1); // no channel sorting
        w.put(1, 1); // CRC present
        w.put(0, 1); // no RLS-LMS
        w.put(0, 5);
        w.put(0, 1); // no auxiliary data
        w.put(u32::MAX as u64, 32); // no stored header
        w.put(u32::MAX as u64, 32); // no stored trailer
        w.put(self.crc.value() as u64, 32);
        w.bytes
    }

    /// The `esds` box that carries the audio specific config and the record for MP4 files.
    fn esds(&self, bytes: u64) -> Vec<u8> {
        let mut asc = BitWriter::default();
        asc.put(31, 5); // object type: escape
        asc.put(36 - 32, 6); // ... to 36, ALS
        match RATES.iter().position(|&r| r == self.params.rate) {
            Some(i) => asc.put(i as u64, 4),
            None => {
                asc.put(15, 4);
                asc.put(self.params.rate as u64, 24);
            }
        }
        let channels = self.params.channels();
        asc.put(if channels <= 2 { channels as u64 } else { 0 }, 4); // the record has the real count
        asc.put(0, 5);
        let mut info = asc.bytes;
        info.extend(self.config());

        let seconds = self.params.total_frames as f64 / self.params.rate as f64;
        let average = if seconds > 0.0 {
            (bytes as f64 * 8.0 / seconds) as u32
        } else {
            0
        };
        let mut decoder = vec![0x40, 0x15, 0, 0, 0];
        decoder.extend_from_slice(&0u32.to_be_bytes());
        decoder.extend_from_slice(&average.to_be_bytes());
        decoder.extend(descriptor(0x05, &info));
        let mut es = vec![0, 0, 0]; // ES id, flags
        es.extend(descriptor(0x04, &decoder));
        es.extend(descriptor(0x06, &[0x02]));
        let mut body = 0u32.to_be_bytes().to_vec();
        body.extend(descriptor(0x03, &es));
        atom(b"esds", &body)
    }
}

/// An MPEG-4 descriptor: tag, length, contents (every length here is below 128).
fn descriptor(tag: u8, body: &[u8]) -> Vec<u8> {
    debug_assert!(body.len() < 128);
    let mut out = vec![tag, body.len() as u8];
    out.extend_from_slice(body);
    out
}

/// The two blocks of a channel pair, with joint stereo used where it makes them smaller.
fn pair_blocks(left: &[i32], right: &[i32], cfg: Settings) -> (Vec<u8>, Vec<u8>) {
    let difference: Vec<i32> = left.iter().zip(right).map(|(&l, &r)| r.wrapping_sub(l)).collect();
    let l = block::encode(left, MAX_ORDER, false, cfg);
    let r = block::encode(right, MAX_ORDER, false, cfg);
    let d = block::encode(&difference, MAX_ORDER, true, cfg);
    let plain = l.len() + r.len();
    let with_difference_first = d.len() + r.len();
    let with_difference_second = l.len() + d.len();
    if plain <= with_difference_first && plain <= with_difference_second {
        (l, r)
    } else if with_difference_first <= with_difference_second {
        (d, r)
    } else {
        (l, d)
    }
}

impl BlockWriter for AlsWriter {
    fn write_block(&mut self, planar: &[Vec<f32>], frames: usize) -> std::io::Result<()> {
        let converted = to_integers(planar, &self.params.order, frames, self.bits);
        for (pending, new) in self.pending.iter_mut().zip(converted) {
            pending.extend(new);
        }
        while self.pending[0].len() >= self.frame_len {
            let frame: Vec<Vec<i32>> = self
                .pending
                .iter_mut()
                .map(|p| p.drain(..self.frame_len).collect())
                .collect();
            self.write_frame(&frame)?;
        }
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> std::io::Result<u64> {
        if !self.pending[0].is_empty() {
            let frame: Vec<Vec<i32>> = self.pending.iter_mut().map(std::mem::take).collect();
            self.write_frame(&frame)?;
        }
        let data_bytes: u64 = self.frame_sizes.iter().map(|&s| s as u64).sum();
        let output = self.output_kind();
        match output {
            Some(start) => {
                let track = mp4_audio::Track {
                    rate: self.params.rate,
                    channels: self.params.channels() as u16,
                    bits: self.bits as u16,
                    frame_samples: self.frame_len as u32,
                    total_samples: self.params.total_frames,
                    frame_sizes: &self.frame_sizes,
                    entry_name: b"mp4a",
                    entry_boxes: self.esds(data_bytes),
                };
                mp4_audio::finish(&mut self.out, &start, &track)
            }
            None => {
                let config = self.config();
                debug_assert_eq!(config.len(), CONFIG_LEN);
                self.out.flush()?;
                self.out.seek(SeekFrom::Start(0))?;
                self.out.write_all(&config)?;
                self.out.flush()?;
                Ok(CONFIG_LEN as u64 + data_bytes)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_has_its_fixed_size() {
        let params = Params {
            rate: 48_000,
            format: SampleFormat::Int16,
            total_frames: 1000,
            order: vec![0, 1],
            mask: 3,
            labels: vec![],
            time_reference: None,
        };
        let dir = std::env::temp_dir().join(format!("sw_als_cfg_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let writer = AlsWriter::create(&dir.join("x.als"), params).unwrap();
        assert_eq!(writer.config().len(), CONFIG_LEN);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn descriptors_carry_their_length() {
        assert_eq!(descriptor(5, &[1, 2, 3]), [5, 3, 1, 2, 3]);
    }

    #[test]
    fn frames_are_shorter_at_lower_rates() {
        assert!(frame_length(44_100) < frame_length(96_000));
        assert!(frame_length(96_000) < frame_length(192_000));
    }
}
