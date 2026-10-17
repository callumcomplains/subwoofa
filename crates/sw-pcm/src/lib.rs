//! Native PCM input: WAV / Broadcast WAV / RF64 and AIFF / AIFF-C, read as `f32`.
//!
//! The reader is streaming (never loads a whole file), exposes format info including the
//! Broadcast-WAV time reference, and `StemSet` maps per-channel mono/stereo files onto a
//! channel layout with the same pairing rules professional encoders use.

mod channels;
mod reader;
mod stems;

pub use channels::{multichannel_inputs, slot_map, suggest_layout, ChannelOrder};
pub use reader::{Container, PcmInfo, PcmReader};
pub use stems::StemSet;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}: {1}")]
    Io(String, #[source] std::io::Error),
    #[error("{0}: not a WAV or AIFF file")]
    NotPcm(String),
    #[error("{0}: unsupported format ({1})")]
    Unsupported(String, String),
    #[error("{0}: file is truncated or corrupt")]
    Corrupt(String),
    #[error("{0}")]
    Mismatch(String),
}

/* ------------------------------------------------------------------------------------ */

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::map_channel_mask;
    use crate::reader::extended_to_f64;
    use std::collections::HashMap;
    use std::fs::File;
    use std::io::Write;
    use std::path::Path;
    use std::path::PathBuf;
    use sw_core::Channel;
    use sw_core::ChannelLayout;

    #[test]
    fn mask_mapping_for_common_layouts() {
        let five_one = ChannelLayout::find("5.1").unwrap();
        // 5.1 with back speakers (0x3F) and with side speakers (0x60F) both map in file order
        assert_eq!(map_channel_mask(five_one, 0x3F), Some(vec![0, 1, 2, 3, 4, 5]));
        assert_eq!(map_channel_mask(five_one, 0x60F), Some(vec![0, 1, 2, 3, 4, 5]));
        // 7.1 with side + rear speakers: file order is FL FR FC LFE BL BR SL SR
        let seven = ChannelLayout::find("7.1-side-rear").unwrap(); // L R C LFE Lss Rss Lsr Rsr
        assert_eq!(map_channel_mask(seven, 0x63F), Some(vec![0, 1, 2, 3, 6, 7, 4, 5]));
        // a mask with a speaker the layout lacks is rejected
        assert_eq!(map_channel_mask(five_one, 0x63F), None);
    }

    #[test]
    fn eight_channel_extensible_wav_is_split_by_mask() {
        let seven = ChannelLayout::find("7.1-side-rear").unwrap();
        let dir = std::env::temp_dir().join(format!("sw_pcm_mc_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("eight.wav");
        // one frame, eight channels with distinct values 1..=8, mask 0x63F, 16-bit
        let mut f = File::create(&p).unwrap();
        let data: Vec<u8> = (1..=8i16).flat_map(|v| (v * 256).to_le_bytes()).collect();
        f.write_all(b"RIFF").unwrap();
        f.write_all(&((4 + 8 + 40 + 8 + data.len()) as u32).to_le_bytes())
            .unwrap();
        f.write_all(b"WAVEfmt ").unwrap();
        f.write_all(&40u32.to_le_bytes()).unwrap();
        f.write_all(&0xFFFEu16.to_le_bytes()).unwrap();
        f.write_all(&8u16.to_le_bytes()).unwrap();
        f.write_all(&48_000u32.to_le_bytes()).unwrap();
        f.write_all(&(48_000u32 * 16).to_le_bytes()).unwrap();
        f.write_all(&16u16.to_le_bytes()).unwrap();
        f.write_all(&16u16.to_le_bytes()).unwrap();
        f.write_all(&22u16.to_le_bytes()).unwrap();
        f.write_all(&16u16.to_le_bytes()).unwrap();
        f.write_all(&0x63Fu32.to_le_bytes()).unwrap();
        f.write_all(&[0x01, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71])
            .unwrap(); // PCM sub-format GUID
        f.write_all(b"data").unwrap();
        f.write_all(&(data.len() as u32).to_le_bytes()).unwrap();
        f.write_all(&data).unwrap();
        drop(f);

        let r = PcmReader::open(&p).unwrap();
        assert_eq!(r.info().channels, 8);
        assert_eq!(r.info().channel_mask, Some(0x63F));
        let inputs = multichannel_inputs(seven, &p).expect("one file for all channels");
        let read = |order| {
            let mut s = StemSet::open_ordered(seven, &inputs, order).unwrap();
            let mut planar = Vec::new();
            assert_eq!(s.read_planar(&mut planar, 4).unwrap(), 1);
            planar
                .iter()
                .map(|c| (c[0] * 32768.0).round() as i32 / 256)
                .collect::<Vec<i32>>()
        };
        // Windows convention: file order FL FR FC LFE BL BR SL SR  ->  layout L R C LFE Lss Rss Lsr Rsr
        assert_eq!(read(ChannelOrder::Mask), vec![1, 2, 3, 4, 7, 8, 5, 6]);
        // the standard 7.1 mask is read in file (film) order by default
        assert_eq!(read(ChannelOrder::Auto), vec![1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(read(ChannelOrder::File), vec![1, 2, 3, 4, 5, 6, 7, 8]);
        let slots = slot_map(seven, &inputs, ChannelOrder::Mask).unwrap();
        assert_eq!(
            slots.iter().map(|(_, c)| *c).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 6, 7, 4, 5]
        );
    }

    fn write_wav(path: &Path, channels: u16, rate: u32, bits: u16, samples: &[i32]) {
        let bytes = bits as usize / 8;
        let data_len = samples.len() * bytes;
        let mut f = File::create(path).unwrap();
        f.write_all(b"RIFF").unwrap();
        f.write_all(&((36 + data_len) as u32).to_le_bytes()).unwrap();
        f.write_all(b"WAVEfmt ").unwrap();
        f.write_all(&16u32.to_le_bytes()).unwrap();
        f.write_all(&1u16.to_le_bytes()).unwrap();
        f.write_all(&channels.to_le_bytes()).unwrap();
        f.write_all(&rate.to_le_bytes()).unwrap();
        f.write_all(&(rate * channels as u32 * bytes as u32).to_le_bytes())
            .unwrap();
        f.write_all(&(channels * bytes as u16).to_le_bytes()).unwrap();
        f.write_all(&bits.to_le_bytes()).unwrap();
        f.write_all(b"data").unwrap();
        f.write_all(&(data_len as u32).to_le_bytes()).unwrap();
        for s in samples {
            f.write_all(&s.to_le_bytes()[..bytes]).unwrap();
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sw_pcm_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join(name)
    }

    #[test]
    fn reads_24_bit_wav() {
        let p = tmp("a.wav");
        write_wav(&p, 1, 48_000, 24, &[0, 4_194_304, -4_194_304, 8_388_607]);
        let mut r = PcmReader::open(&p).unwrap();
        assert_eq!(
            (r.info().channels, r.info().sample_rate, r.info().bits, r.info().frames),
            (1, 48_000, 24, 4)
        );
        let mut v = Vec::new();
        assert_eq!(r.read_f32(&mut v, 100).unwrap(), 4);
        assert_eq!(&v[..3], &[0.0, 0.5, -0.5]);
        assert!((v[3] - 1.0).abs() < 1e-6);
        assert_eq!(r.read_f32(&mut v, 10).unwrap(), 0);
    }

    #[test]
    fn stereo_file_pairs_into_adjacent_slots() {
        let layout = ChannelLayout::find("2.0").unwrap();
        let p = tmp("st.wav");
        write_wav(&p, 2, 48_000, 16, &[1000, -1000, 2000, -2000]);
        let mut inputs = HashMap::new();
        inputs.insert(Channel::L, p.clone());
        inputs.insert(Channel::R, p);
        let mut s = StemSet::open(layout, &inputs).unwrap();
        let mut planar = Vec::new();
        assert_eq!(s.read_planar(&mut planar, 10).unwrap(), 2);
        assert!(planar[0][0] > 0.0 && planar[1][0] < 0.0);
        assert!((planar[0][1] - 2000.0 / 32768.0).abs() < 1e-6);
    }

    #[test]
    fn short_files_are_zero_padded() {
        let layout = ChannelLayout::find("2.0").unwrap();
        let (a, b) = (tmp("long.wav"), tmp("short.wav"));
        write_wav(&a, 1, 48_000, 16, &[100, 200, 300, 400]);
        write_wav(&b, 1, 48_000, 16, &[500]);
        let inputs = HashMap::from([(Channel::L, a), (Channel::R, b)]);
        let mut s = StemSet::open(layout, &inputs).unwrap();
        assert_eq!(s.frames, 4);
        let mut planar = Vec::new();
        assert_eq!(s.read_planar(&mut planar, 4).unwrap(), 4);
        assert_eq!(planar[1][1..], [0.0, 0.0, 0.0]);
    }

    #[test]
    fn sample_rate_mismatch_is_rejected() {
        let layout = ChannelLayout::find("2.0").unwrap();
        let (a, b) = (tmp("r48.wav"), tmp("r44.wav"));
        write_wav(&a, 1, 48_000, 16, &[1]);
        write_wav(&b, 1, 44_100, 16, &[1]);
        let inputs = HashMap::from([(Channel::L, a), (Channel::R, b)]);
        assert!(matches!(StemSet::open(layout, &inputs), Err(Error::Mismatch(_))));
    }

    #[test]
    fn aiff_extended_rate() {
        let mut b = [0u8; 10];
        b[0] = 0x40;
        b[1] = 0x0E; // 44100 = 0xAC44 * 2^(14-... )
        b[2] = 0xAC;
        b[3] = 0x44;
        assert_eq!(extended_to_f64(b), 44100.0);
    }

    #[test]
    fn stereo_feeds_pair() {
        let layout = ChannelLayout::find("2.0").unwrap();
        let p = tmp("pair.wav");
        write_wav(&p, 2, 48_000, 16, &[1000, -2000]);
        // a lone stereo file fills a stereo layout: left to L, right to R
        let inputs = multichannel_inputs(layout, &p).expect("a stereo file fills 2.0");
        let mut s = StemSet::open(layout, &inputs).unwrap();
        let mut planar = Vec::new();
        s.read_planar(&mut planar, 1).unwrap();
        assert!(planar[0][0] > 0.0 && planar[1][0] < 0.0, "{planar:?}");
    }

    #[test]
    fn stereo_alone_refused() {
        let mono = ChannelLayout::find("1.0").unwrap();
        let p = tmp("half.wav");
        write_wav(&p, 2, 48_000, 16, &[1000, -2000]);
        let inputs = HashMap::from([(Channel::C, p)]);
        assert!(matches!(StemSet::open(mono, &inputs), Err(Error::Mismatch(_))));
    }

    #[test]
    fn rejects_garbage() {
        let p = tmp("junk.wav");
        std::fs::write(&p, b"not audio at all, just text").unwrap();
        assert!(matches!(PcmReader::open(&p), Err(Error::NotPcm(_))));
    }
}
