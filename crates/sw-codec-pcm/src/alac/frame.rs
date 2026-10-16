//! One frame: the elements and their headers.

use crate::alac::entropy::entropy_code;
use crate::alac::layout::{elements_for, ELEMENT_END, ELEMENT_PAIR};
use crate::alac::predict::{best_prediction, Predicted};
use crate::alac::search::Search;
use crate::alac::stereo::{stereo_variants, Stereo, LEFT_RIGHT};
use crate::alac::{FRAME_SAMPLES, PREDICTOR_SHIFT, RICE_MODIFIER};
use crate::bitwriter::BitWriter;
use crate::roughness;

/// Writes one element (a single channel or a pair) of a frame.
pub(crate) fn write_element(
    w: &mut BitWriter,
    kind: u32,
    instance: u32,
    channels: &[&[i32]],
    bits: u32,
    verbatim: bool,
    search: Search,
) {
    let n = channels[0].len();
    let extra_bits = if verbatim { 0 } else { bits.saturating_sub(16) };
    w.put(kind, 3);
    w.put(instance, 4);
    w.put(0, 12); // unused
    let has_size = n < FRAME_SAMPLES;
    w.put(u32::from(has_size), 1);
    w.put(extra_bits >> 3, 2);
    w.put(u32::from(verbatim), 1);
    if has_size {
        w.put(n as u32, 32);
    }
    if verbatim {
        for i in 0..n {
            for ch in channels {
                w.put_signed(ch[i], bits);
            }
        }
        return;
    }
    let width = bits - extra_bits + channels.len() as u32 - 1; // the side channel needs one more bit
                                                               // the low bytes are kept aside; the rest is what gets predicted
    let mask = (1i32 << extra_bits) - 1;
    let low: Vec<Vec<i32>> = channels.iter().map(|c| c.iter().map(|v| v & mask).collect()).collect();
    let high: Vec<Vec<i32>> = channels
        .iter()
        .map(|c| c.iter().map(|v| v >> extra_bits).collect())
        .collect();
    let (stereo, coded): (Stereo, Vec<Vec<i32>>) = if channels.len() == 2 {
        // try every way of storing the pair with a quick predictor (or judge them by how rough they are), keep
        // the smallest
        let mut best: Option<(usize, Stereo, Vec<Vec<i32>>)> = None;
        for (s, l, r) in stereo_variants(&high[0], &high[1]) {
            let cost = if search.all_stereo {
                best_prediction(&l, width, &[8])
                    .bits
                    .saturating_add(best_prediction(&r, width, &[8]).bits)
            } else {
                roughness(&l) + roughness(&r)
            };
            if best.as_ref().is_none_or(|b| cost < b.0) {
                best = Some((cost, s, vec![l, r]));
            }
        }
        let (_, s, c) = best.unwrap();
        (s, c)
    } else {
        (LEFT_RIGHT, high)
    };
    w.put(stereo.shift, 8);
    w.put(stereo.left_weight, 8);
    let predictions: Vec<Predicted> = coded.iter().map(|c| best_prediction(c, width, search.orders)).collect();
    for p in &predictions {
        w.put(0, 4); // prediction type
        w.put(PREDICTOR_SHIFT, 4);
        w.put(RICE_MODIFIER, 3);
        w.put(p.coefficients.len() as u32, 5);
        // the decoder reads them oldest tap last: the stream starts with the newest
        for &c in &p.coefficients {
            w.put_signed(c, 16);
        }
    }
    if extra_bits > 0 {
        for i in 0..n {
            for l in &low {
                w.put(l[i] as u32, extra_bits);
            }
        }
    }
    for p in &predictions {
        entropy_code(w, &p.residual, width);
    }
}

/// A whole frame (all elements) as bytes.
pub(crate) fn encode_frame(channels: &[Vec<i32>], bits: u32, search: Search) -> Vec<u8> {
    let n = channels[0].len();
    let max_bytes = {
        let header_bits = 23 + 32 * usize::from(n < FRAME_SAMPLES);
        (header_bits + bits as usize * channels.len() * n + 3).div_ceil(8)
    };
    for verbatim in [false, true] {
        let mut w = BitWriter::default();
        let (mut singles, mut pairs, mut ch) = (0u32, 0u32, 0usize);
        for &kind in elements_for(channels.len()) {
            if kind == ELEMENT_PAIR {
                write_element(
                    &mut w,
                    kind,
                    pairs,
                    &[&channels[ch], &channels[ch + 1]],
                    bits,
                    verbatim,
                    search,
                );
                pairs += 1;
                ch += 2;
            } else {
                write_element(&mut w, kind, singles, &[&channels[ch]], bits, verbatim, search);
                singles += 1;
                ch += 1;
            }
        }
        w.put(ELEMENT_END, 3);
        w.align(8);
        if verbatim || w.bytes().len() <= max_bytes {
            return w.into_bytes();
        }
    }
    unreachable!("the verbatim frame is always accepted")
}
