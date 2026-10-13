//! Writing a substream: the decoder state the encoder tracks, the restart header, the parameter blocks and the
//! blocks of residuals of one access unit.

use crate::bits::checksum8;
use crate::bits::parity;
use crate::bits::restart_checksum;
use crate::bits::BitWriter;
use crate::coding::{shifted, Coding};
use crate::filter::{Fir, NO_FILTER};
use crate::predict::ChannelAnalysis;
use crate::tables::HUFFMAN;
use crate::MAX_CHANNELS;
use crate::{Config, Downmix};

/// One primitive matrix: rewrites channel `out` from a weighted sum of all channels (2.14).
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct MatrixSpec {
    pub(crate) out: usize,
    pub(crate) coeff: Vec<i32>,
    pub(crate) lsb_bypass: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct ChParams {
    pub(crate) fir: Fir,
    pub(crate) iir: Fir,
    pub(crate) codebook: u8,
    pub(crate) lsbs: u8,
}

pub(crate) const DEFAULT_CH: ChParams = ChParams {
    fir: NO_FILTER,
    iir: NO_FILTER,
    codebook: 0,
    lsbs: 24,
};

/// What the decoder currently believes (mirrors the decoder's state machine).
#[derive(Clone, Debug)]
pub(crate) struct DecState {
    pub(crate) blocksize: usize,
    pub(crate) quant: [u8; MAX_CHANNELS],
    pub(crate) ch: [ChParams; MAX_CHANNELS],
    pub(crate) matrices: Vec<MatrixSpec>,
}

impl DecState {
    pub(crate) fn restart() -> DecState {
        DecState {
            blocksize: 8,
            quant: [0; MAX_CHANNELS],
            ch: [DEFAULT_CH; MAX_CHANNELS],
            matrices: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Substream {
    pub(crate) min: usize,
    pub(crate) max: usize,
    /// Primitive matrices this substream carries.
    pub(crate) matrices: Vec<MatrixSpec>,
}

/// How the coded channels divide into substreams, and the matrices each carries. One substream for mono, stereo and
/// MLP; otherwise channels 0-1 come first so that a stereo decoder can stop there, then the rest (7.1: 2-5, then
/// 6-7). Substreams that carry a downmix hold the matrices that rebuild the front pair from it.
pub(crate) fn substream_layout(cfg: &Config) -> Vec<Substream> {
    let n = cfg.channels;
    let front = |dm: &Downmix, max: usize| -> Vec<MatrixSpec> {
        [(0usize, &dm.lo), (1usize, &dm.ro)]
            .into_iter()
            .map(|(dest, q)| {
                let mut c = vec![0i32; max + 1];
                c[dest] = dm.dest;
                for j in 2..=max.min(5) {
                    c[j] = -q[j];
                }
                MatrixSpec {
                    out: dest,
                    coeff: c,
                    lsb_bypass: false,
                }
            })
            .collect()
    };
    let dm = cfg.downmix.as_ref();
    let none = || Vec::new();

    if cfg.mlp.is_some() || n <= 2 {
        vec![Substream {
            min: 0,
            max: n - 1,
            matrices: none(),
        }]
    } else if n <= 6 {
        vec![
            Substream {
                min: 0,
                max: 1,
                matrices: none(),
            },
            Substream {
                min: 2,
                max: n - 1,
                matrices: dm.map(|d| front(d, n - 1)).unwrap_or_default(),
            },
        ]
    } else {
        let mut full = dm.map(|d| front(d, 7)).unwrap_or_default();
        if let Some(d) = dm {
            // undo the rear fold after the front matrices have read the folded surrounds
            for (dest, rear) in [(4usize, 6usize), (5, 7)] {
                let mut c = vec![0i32; 8];
                c[dest] = 1 << 14;
                c[rear] = -d.fold;
                full.push(MatrixSpec {
                    out: dest,
                    coeff: c,
                    lsb_bypass: false,
                });
            }
        }
        vec![
            Substream {
                min: 0,
                max: 1,
                matrices: none(),
            },
            Substream {
                min: 2,
                max: 5,
                matrices: dm.map(|d| front(d, 5)).unwrap_or_default(),
            },
            Substream {
                min: 6,
                max: n - 1,
                matrices: full,
            },
        ]
    }
}

pub(crate) struct BlockPlan {
    pub(crate) start: usize,
    pub(crate) len: usize,
    /// desired decoder state after the block's parameter update
    pub(crate) want: DecState,
    pub(crate) codings: [Coding; MAX_CHANNELS],
}

pub(crate) fn plan_blocks(
    sub: &Substream,
    mats: &[MatrixSpec],
    out_q: &[u8],
    an: &[ChannelAnalysis],
    au_index: usize,
    au: usize,
    restart: bool,
) -> Vec<BlockPlan> {
    let ranges: Vec<(usize, usize)> = if restart {
        vec![(0, 8), (8, au - 8)]
    } else {
        vec![(au_index * au, au)]
    };
    let mut plans = Vec::new();
    for (bi, (start, len)) in ranges.into_iter().enumerate() {
        let mut want = DecState::restart();
        want.blocksize = len;
        want.matrices = mats.to_vec();
        want.quant[..sub.min].copy_from_slice(&out_q[..sub.min]);
        let mut codings = [Coding {
            codebook: 0,
            lsb_bits: 0,
            bits: 0,
        }; MAX_CHANNELS];
        for ch in sub.min..=sub.max {
            let a = &an[ch];
            want.quant[ch] = a.quant;
            // the priming block is unfiltered; later blocks use the interval's predictor
            let (fir, iir) = if restart && bi == 0 {
                (Fir::default(), Fir::default())
            } else {
                (a.fir, a.iir)
            };
            let c = a.codings[au_index][bi];
            codings[ch] = c;
            want.ch[ch] = ChParams {
                fir,
                iir,
                codebook: c.codebook,
                lsbs: (c.lsb_bits + a.quant as u32) as u8,
            };
        }
        plans.push(BlockPlan {
            start,
            len,
            want,
            codings,
        });
    }
    plans
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn write_substream_au(
    sub: &Substream,
    mats: &[MatrixSpec],
    out_q: &[u8],
    an: &[ChannelAnalysis],
    st: &mut DecState,
    au_index: usize,
    au: usize,
    restart: bool,
    end_of_stream_shorten: Option<usize>,
    mlp: bool,
    output_timing: u16,
    lossless_check: u8,
    max_output_bits: u32,
    bypass: &[Vec<u8>; 2],
) -> Vec<u8> {
    let mut w = BitWriter::new();
    let plans = plan_blocks(sub, mats, out_q, an, au_index, au, restart);
    let nblocks = plans.len();
    if restart {
        *st = DecState::restart();
    }
    for (bi, plan) in plans.iter().enumerate() {
        let first_with_restart = restart && bi == 0;
        let diff = params_differ(sub, st, &plan.want);
        if first_with_restart || diff {
            w.put(1, 1); // parameter block present
            if first_with_restart {
                w.put(1, 1); // restart header present
                write_restart_header(
                    &mut w,
                    sub,
                    output_timing,
                    lossless_check,
                    max_output_bits,
                    plans.iter().map(|p| &p.want).fold(0u32, |m, s| {
                        (sub.min..=sub.max).map(|c| s.ch[c].lsbs as u32).fold(m, u32::max)
                    }),
                );
            } else {
                w.put(0, 1);
            }
            write_params(&mut w, sub, st, &plan.want);
            *st = plan.want.clone();
        } else {
            w.put(0, 1);
        }
        write_block_data(&mut w, sub, an, plan, bypass);
        w.put(u32::from(bi == nblocks - 1), 1); // last block of this access unit
    }
    w.align(16);
    if let Some(shorten) = end_of_stream_shorten.filter(|s| *s > 0) {
        w.put(0xD234, 16);
        w.put(
            if mlp {
                0xD234
            } else {
                (shorten as u32 & 0x1FFF) | 0xE000
            },
            16,
        );
    }
    let mut bytes = w.into_bytes();
    let p = parity(&bytes) ^ 0xA9;
    let c = checksum8(&bytes);
    bytes.push(p);
    bytes.push(c);
    bytes
}

pub(crate) fn params_differ(sub: &Substream, cur: &DecState, want: &DecState) -> bool {
    cur.blocksize != want.blocksize
        || cur.quant != want.quant
        || cur.matrices != want.matrices
        || (sub.min..=sub.max).any(|c| cur.ch[c] != want.ch[c])
}

pub(crate) fn write_restart_header(
    w: &mut BitWriter,
    sub: &Substream,
    output_timing: u16,
    lossless_check: u8,
    max_output_bits: u32,
    max_huff_lsbs: u32,
) {
    let start = w.bit_len();
    // matrix channels above 5 need the "noise type" variant of the restart sync word
    w.put(if sub.max > 5 { 0x31EB } else { 0x31EA }, 14);
    w.put(output_timing as u32, 16);
    w.put(sub.min as u32, 4);
    w.put(sub.max as u32, 4);
    w.put(sub.max as u32, 4); // max matrix channel: no matrices, so the last coded channel
    w.put(0, 4); // noise shift
    w.put(0, 23); // noise generator seed
    w.put(0, 4); // max shift
    w.put(max_huff_lsbs.min(31), 5);
    w.put(max_output_bits, 5);
    w.put(max_output_bits, 5);
    w.put(0, 1); // no per-block length check
    w.put(lossless_check as u32, 8);
    w.put(0, 16);
    for ch in 0..=sub.max {
        w.put(ch as u32, 6); // output channel assignment: identity
    }
    let bits = w.bit_len() - start;
    // the checksum covers the header bits but not the two flag bits before it
    let buf = {
        let mut b = w.snapshot();
        b.push(0);
        b
    };
    let flag_bits_offset = start - 2;
    debug_assert!(flag_bits_offset == 0, "restart header must start the substream");
    let ck = restart_checksum(&buf, bits);
    w.put(ck as u32, 8);
}

/// Matrix parameters: each primitive matrix rewrites one channel from a weighted sum of all channels.
pub(crate) fn write_matrices(w: &mut BitWriter, sub: &Substream, mats: &[MatrixSpec]) {
    // substreams ending at channel 5 or below use the sync word with two noise channels, which add
    // two (always zero) coefficients per matrix; the 0x31EB variant has a noise shift instead
    let noise_channels = sub.max <= 5;
    w.put(mats.len() as u32, 4);
    for m in mats {
        w.put(m.out as u32, 4);
        w.put(14, 4); // fractional bits
        w.put(u32::from(m.lsb_bypass), 1);
        let count = sub.max + 1 + if noise_channels { 2 } else { 0 };
        for ch in 0..count {
            let c = m.coeff.get(ch).copied().unwrap_or(0);
            if c != 0 {
                w.put(1, 1);
                w.put_signed(c, 16);
            } else {
                w.put(0, 1);
            }
        }
        if !noise_channels {
            w.put(0, 4); // matrix noise shift
        }
    }
}

pub(crate) fn write_filter(w: &mut BitWriter, f: &Fir) {
    w.put(f.order as u32, 4);
    if f.order > 0 {
        w.put(f.shift as u32, 4);
        w.put(f.coeff_bits as u32, 5);
        w.put(f.coeff_shift as u32, 3);
        for i in 0..f.order as usize {
            w.put_signed(f.coeff[i] >> f.coeff_shift, f.coeff_bits as u32);
        }
        w.put(0, 1); // no state data
    }
}

/// Write the decoding parameter block that moves the decoder from `cur` to `want`.
pub(crate) fn write_params(w: &mut BitWriter, sub: &Substream, cur: &DecState, want: &DecState) {
    w.put(0, 1); // parameter presence flags unchanged
                 // blocksize
    if cur.blocksize != want.blocksize {
        w.put(1, 1);
        w.put(want.blocksize as u32, 9);
    } else {
        w.put(0, 1);
    }
    if cur.matrices != want.matrices {
        w.put(1, 1);
        write_matrices(w, sub, &want.matrices);
    } else {
        w.put(0, 1);
    }
    w.put(0, 1); // output shift unchanged
                 // quantizer steps cover channels 0..=max
    if cur.quant != want.quant {
        w.put(1, 1);
        for ch in 0..=sub.max {
            w.put(want.quant[ch] as u32, 4);
        }
    } else {
        w.put(0, 1);
    }
    for ch in sub.min..=sub.max {
        if cur.ch[ch] != want.ch[ch] {
            w.put(1, 1);
            if cur.ch[ch].fir != want.ch[ch].fir {
                w.put(1, 1);
                write_filter(w, &want.ch[ch].fir);
            } else {
                w.put(0, 1);
            }
            if cur.ch[ch].iir != want.ch[ch].iir {
                w.put(1, 1);
                write_filter(w, &want.ch[ch].iir);
            } else {
                w.put(0, 1);
            }
            w.put(0, 1); // no huffman offset change
            w.put(want.ch[ch].codebook as u32, 2);
            w.put(want.ch[ch].lsbs as u32, 5);
        } else {
            w.put(0, 1);
        }
    }
}

pub(crate) fn write_block_data(
    w: &mut BitWriter,
    sub: &Substream,
    an: &[ChannelAnalysis],
    plan: &BlockPlan,
    bypass: &[Vec<u8>; 2],
) {
    for i in 0..plan.len {
        // one bypassed LSB per flagged matrix, ahead of the sample's channel data
        for m in plan.want.matrices.iter() {
            if m.lsb_bypass {
                w.put(bypass[m.out][plan.start + i] as u32, 1);
            }
        }
        for ch in sub.min..=sub.max {
            let a = &an[ch];
            let v = a.residual[plan.start + i] >> a.quant;
            let c = plan.codings[ch];
            let y = shifted(v, c.codebook, c.lsb_bits);
            debug_assert!(y >= 0, "negative symbol input");
            if c.codebook > 0 {
                let sym = (y >> c.lsb_bits) as usize;
                let (code, len) = HUFFMAN[c.codebook as usize - 1][sym];
                w.put(code as u32, len as u32);
            }
            if c.lsb_bits > 0 {
                w.put((y & ((1i64 << c.lsb_bits) - 1)) as u32, c.lsb_bits);
            }
        }
    }
}
