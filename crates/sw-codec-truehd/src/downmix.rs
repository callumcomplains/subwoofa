//! The stereo presentation: the downmix coefficients carried in substream 0, how the coded front pair is
//! derived from the source so that a decoder can recover the original channels exactly.

use crate::layout::LayoutMap;
use sw_core::Channel;
use sw_core::ChannelLayout;

/// Stereo downmix carried in substream 0 (coded channels 0 and 1).
///
/// With `dest = D` (2.14 fixed point, `1 << 14` = unity up to 32767 ~ 2.0) coded channel 0 becomes
/// `Lo = (L*2^14 + sum(lo[j] * x[j])) / D`, and channel 1 likewise for `Ro`, summing over the other
/// channels `j >= 2`. `D > 2^14` attenuates the mix by `2^14 / D` for headroom. A full decoder
/// recovers L exactly as `((D*Lo - sum(lo[j] * x[j])) >> 14) + b`, where `b` is one stored bit per
/// sample (the "LSB bypass") that absorbs the rounding of `Lo`. With `D == 2^14` no bit is needed.
/// For eight channels the rear pair (coded 6 and 7) is first folded into the side surrounds
/// (coded 4 and 5) with unit gain by `fold`, which defines the 6-channel presentation.
#[derive(Debug, Clone, Default)]
pub struct Downmix {
    /// Coefficient of each coded channel into Lo (indexes 0 and 1 are ignored).
    pub lo: Vec<i32>,
    pub ro: Vec<i32>,
    /// 2.14 coefficient folding Lrs/Rrs into Ls/Rs (7.1 only).
    pub fold: i32,
    /// Decoder-side gain of the front channels in 2.14 (16384 to 32767).
    pub dest: i32,
}

/// Round-up division by 2^14 used by the downmix (the exact inverse of the decoder's floor shift).
#[inline]
pub(crate) fn ceil14(y: i64) -> i64 {
    (y + 16383) >> 14
}

/// Default ITU-style Lo/Ro downmix for a layout: centre and surrounds at -3 dB, LFE omitted.
/// Returns `None` for layouts where a downmix cannot be expressed (mono, stereo, unusual 7ch/8ch).
pub fn default_downmix(layout: &ChannelLayout, map: &LayoutMap) -> Option<Downmix> {
    let n = map.source_index.len();
    if n <= 2 {
        return None;
    }
    let m = sw_core::downmix::Matrix::lo_ro(layout);
    // the front pair must be exactly L and R in coded slots 0 and 1
    let l = layout.channels[map.source_index[0]];
    let r = layout.channels[map.source_index[1]];
    if !matches!(l, Channel::L | Channel::Lt) || !matches!(r, Channel::R | Channel::Rt) {
        return None;
    }
    // half-level mix for headroom: a stereo-only decoder must be able to hold L + 0.7C + 0.7Ls in 24 bits
    let mut dm = Downmix {
        lo: vec![0; n],
        ro: vec![0; n],
        fold: 0,
        dest: 32767,
    };
    let q = |db: f32| -> i32 {
        if db == sw_core::downmix::OFF {
            0
        } else {
            (10f64.powf(db as f64 / 20.0) * 16384.0).round() as i32
        }
    };
    let gain = |from: Channel, to: Channel| {
        m.in_idx(from)
            .zip(m.out_idx(to))
            .map(|(i, o)| m.cells[o][i].gain_db)
            .unwrap_or(sw_core::downmix::OFF)
    };
    if n > 6 {
        // eight channels: only L R C LFE Ls Rs Lrs Rrs (side + rear surrounds) is supported
        if n != 8 {
            return None;
        }
        let rear_l = layout.channels[map.source_index[6]];
        let rear_r = layout.channels[map.source_index[7]];
        if !matches!(rear_l, Channel::Lsr) || !matches!(rear_r, Channel::Rsr) {
            return None;
        }
        dm.fold = q(-3.0103);
    }
    for j in 2..n.min(6) {
        let ch = layout.channels[map.source_index[j]];
        dm.lo[j] = q(gain(ch, Channel::L));
        dm.ro[j] = q(gain(ch, Channel::R));
    }
    Some(dm)
}

/// Quantizer step for the coded stereo channel when the source front channel has `m` unused low
/// bits: the largest step whose grid still pins the reconstructed channel to its own grid.
pub(crate) fn coarse_quant(dest: i32, m: u8) -> u8 {
    let mut q = m;
    while q > 0 && (dest as i64) << q > 1i64 << (14 + m as u32) {
        q -= 1;
    }
    q
}

/// Coded-domain channels and LSB-bypass bits for the downmix layout (see [`Downmix`]).
/// `mq` is the number of unused low bits of the front channels (coarse downmix when at least one).
pub(crate) fn apply_downmix(
    chans: &[Vec<i32>],
    n: usize,
    dm: &Downmix,
    nch: usize,
    mq: [u8; 2],
) -> (Vec<Vec<i32>>, [Vec<u8>; 2]) {
    let mut v: Vec<Vec<i32>> = chans.iter().map(|c| c[..n].to_vec()).collect();
    if nch == 8 {
        for i in 0..n {
            v[4][i] = (chans[4][i] as i64 + ceil14(dm.fold as i64 * chans[6][i] as i64)) as i32;
            v[5][i] = (chans[5][i] as i64 + ceil14(dm.fold as i64 * chans[7][i] as i64)) as i32;
        }
    }
    let last = nch.min(6);
    let d = dm.dest as i64;
    let unity = dm.dest == 1 << 14;
    let mut bits = [vec![0u8; n], vec![0u8; n]];
    for i in 0..n {
        let (mut y0, mut y1) = (0i64, 0i64);
        for j in 2..last {
            y0 += dm.lo[j] as i64 * v[j][i] as i64;
            y1 += dm.ro[j] as i64 * v[j][i] as i64;
        }
        for (k, (orig, y)) in [(chans[0][i] as i64, y0), (chans[1][i] as i64, y1)]
            .into_iter()
            .enumerate()
        {
            let coded = if mq[k] >= 1 {
                // smallest multiple of 2^q with D*z - y >= orig * 2^14: the decoder's masked output is then `orig`
                let q = coarse_quant(dm.dest, mq[k]);
                let step = d << q;
                let target = (orig << 14) + y;
                (target.div_euclid(step) + i64::from(target.rem_euclid(step) != 0)) << q
            } else if unity {
                orig + ceil14(y)
            } else {
                // smallest s with D*s - y >= (orig - 1) * 2^14, so that (D*s - y) >> 14 is orig or orig - 1
                let target = ((orig - 1) << 14) + y;
                let s = target.div_euclid(d) + i64::from(target.rem_euclid(d) != 0);
                let l_hat = (d * s - y) >> 14;
                debug_assert!(orig - l_hat == 0 || orig - l_hat == 1, "bypass bit out of range");
                bits[k][i] = (orig - l_hat) as u8;
                s
            };
            v[k][i] = coded as i32;
        }
    }
    (v, bits)
}
