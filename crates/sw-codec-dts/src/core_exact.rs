//! Bit-exact reconstruction of the DTS core in integer arithmetic.
//!
//! The lossless extension codes the difference between the source and the *decoded core*, so the encoder
//! must reproduce the decoder's integer arithmetic exactly: sample dequantization, the DCT based 32-band
//! (or 64-band, for 96 kHz output) synthesis filter bank with its rounding, and the LFE interpolator.
//! The arithmetic follows the DTS reference decoder's fixed-point behaviour (as implemented by widely used
//! decoders); the tests check the result against a decoder.

use crate::tables_fixed::{FIR32_NPR_FIXED, FIR32_PR_FIXED, FIR64_FIXED, LFE_FIR64_FIXED};

#[inline]
fn norm(a: i64, bits: u32) -> i32 {
    if bits > 0 { ((a + (1i64 << (bits - 1))) >> bits) as i32 } else { a as i32 }
}

#[inline]
fn mul(a: i32, b: i32, bits: u32) -> i32 {
    norm(a as i64 * b as i64, bits)
}

#[inline]
pub fn clip23(a: i32) -> i32 {
    a.clamp(-(1 << 23), (1 << 23) - 1)
}

/* ---------------------------------------------------------------------------------------------- */
/* DCT                                                                                              */
/* ---------------------------------------------------------------------------------------------- */

fn sum_a(input: &[i32], output: &mut [i32], len: usize) {
    for i in 0..len {
        output[i] = input[2 * i].wrapping_add(input[2 * i + 1]);
    }
}

fn sum_b(input: &[i32], output: &mut [i32], len: usize) {
    output[0] = input[0];
    for i in 1..len {
        output[i] = input[2 * i].wrapping_add(input[2 * i - 1]);
    }
}

fn sum_c(input: &[i32], output: &mut [i32], len: usize) {
    for i in 0..len {
        output[i] = input[2 * i];
    }
}

fn sum_d(input: &[i32], output: &mut [i32], len: usize) {
    output[0] = input[1];
    for i in 1..len {
        output[i] = input[2 * i - 1].wrapping_add(input[2 * i + 1]);
    }
}

const COS_A: [[i32; 8]; 8] = [
    [8348215, 8027397, 7398092, 6484482, 5321677, 3954362, 2435084, 822227],
    [8027397, 5321677, 822227, -3954362, -7398092, -8348215, -6484482, -2435084],
    [7398092, 822227, -6484482, -8027397, -2435084, 5321677, 8348215, 3954362],
    [6484482, -3954362, -8027397, 822227, 8348215, 2435084, -7398092, -5321677],
    [5321677, -7398092, -2435084, 8348215, -822227, -8027397, 3954362, 6484482],
    [3954362, -8348215, 5321677, 2435084, -8027397, 6484482, 822227, -7398092],
    [2435084, -6484482, 8348215, -7398092, 3954362, 822227, -5321677, 8027397],
    [822227, -2435084, 3954362, -5321677, 6484482, -7398092, 8027397, -8348215],
];

const COS_B: [[i32; 7]; 8] = [
    [8227423, 7750063, 6974873, 5931642, 4660461, 3210181, 1636536],
    [6974873, 3210181, -1636536, -5931642, -8227423, -7750063, -4660461],
    [4660461, -3210181, -8227423, -5931642, 1636536, 7750063, 6974873],
    [1636536, -7750063, -4660461, 5931642, 6974873, -3210181, -8227423],
    [-1636536, -7750063, 4660461, 5931642, -6974873, -3210181, 8227423],
    [-4660461, -3210181, 8227423, -5931642, -1636536, 7750063, -6974873],
    [-6974873, 3210181, 1636536, -5931642, 8227423, -7750063, 4660461],
    [-8227423, 7750063, -6974873, 5931642, -4660461, 3210181, -1636536],
];

fn dct_a(input: &[i32], output: &mut [i32]) {
    for i in 0..8 {
        let mut res = 0i64;
        for j in 0..8 {
            res += COS_A[i][j] as i64 * input[j] as i64;
        }
        output[i] = norm(res, 23);
    }
}

fn dct_b(input: &[i32], output: &mut [i32]) {
    for i in 0..8 {
        let mut res = (input[0] as i64) << 23;
        for j in 0..7 {
            res += COS_B[i][j] as i64 * input[1 + j] as i64;
        }
        output[i] = norm(res, 23);
    }
}

const MOD_A: [i32; 16] = [4199362, 4240198, 4323885, 4454708, 4639772, 4890013, 5221943, 5660703, -6245623, -7040975, -8158494, -9809974, -12450076, -17261920, -28585092, -85479984];

fn mod_a(input: &[i32], output: &mut [i32]) {
    for i in 0..8 {
        output[i] = mul(MOD_A[i], input[i].wrapping_add(input[8 + i]), 23);
    }
    let mut k = 7i32;
    for i in 8..16 {
        output[i] = mul(MOD_A[i], input[k as usize].wrapping_sub(input[8 + k as usize]), 23);
        k -= 1;
    }
}

const MOD_B: [i32; 8] = [4214598, 4383036, 4755871, 5425934, 6611520, 8897610, 14448934, 42791536];

fn mod_b(input: &mut [i32], output: &mut [i32]) {
    for i in 0..8 {
        input[8 + i] = mul(MOD_B[i], input[8 + i], 23);
    }
    for i in 0..8 {
        output[i] = input[i].wrapping_add(input[8 + i]);
    }
    let mut k = 7i32;
    for i in 8..16 {
        output[i] = input[k as usize].wrapping_sub(input[8 + k as usize]);
        k -= 1;
    }
}

const MOD_C: [i32; 32] = [
    1048892, 1051425, 1056522, 1064244, 1074689, 1087987, 1104313, 1123884, 1146975, 1173922, 1205139, 1241133, 1282529, 1330095, 1384791, 1447815, -1520688, -1605358, -1704360, -1821051, -1959964, -2127368, -2332183, -2587535, -2913561, -3342802, -3931480, -4785806, -6133390,
    -8566050, -14253820, -42727120,
];

fn mod_c(input: &[i32], output: &mut [i32]) {
    for i in 0..16 {
        output[i] = mul(MOD_C[i], input[i].wrapping_add(input[16 + i]), 23);
    }
    let mut k = 15i32;
    for i in 16..32 {
        output[i] = mul(MOD_C[i], input[k as usize].wrapping_sub(input[16 + k as usize]), 23);
        k -= 1;
    }
}

fn clp_v(input: &mut [i32]) {
    for v in input.iter_mut() {
        *v = clip23(*v);
    }
}

/// 32-point inverse MDCT "half" transform of the reference synthesis.
pub fn imdct_half_32(output: &mut [i32], input: &[i32]) {
    let mut buf_a = [0i32; 32];
    let mut buf_b = [0i32; 32];
    let mag: i32 = input[..32].iter().map(|v| v.abs()).sum();
    let shift = if mag > 0x40_0000 { 2 } else { 0 };
    let round = if shift > 0 { 1 << (shift - 1) } else { 0 };
    for i in 0..32 {
        buf_a[i] = (input[i] + round) >> shift;
    }
    let (b0, b1) = buf_b.split_at_mut(16);
    sum_a(&buf_a, b0, 16);
    sum_b(&buf_a, b1, 16);
    clp_v(&mut buf_b);

    {
        let (a0, rest) = buf_a.split_at_mut(8);
        let (a1, rest) = rest.split_at_mut(8);
        let (a2, a3) = rest.split_at_mut(8);
        sum_a(&buf_b[0..], a0, 8);
        sum_b(&buf_b[0..], a1, 8);
        sum_c(&buf_b[16..], a2, 8);
        sum_d(&buf_b[16..], a3, 8);
    }
    clp_v(&mut buf_a);

    {
        let (c0, rest) = buf_b.split_at_mut(8);
        let (c1, rest) = rest.split_at_mut(8);
        let (c2, c3) = rest.split_at_mut(8);
        dct_a(&buf_a[0..], c0);
        dct_b(&buf_a[8..], c1);
        dct_b(&buf_a[16..], c2);
        dct_b(&buf_a[24..], c3);
    }
    clp_v(&mut buf_b);

    {
        let (x, y) = buf_a.split_at_mut(16);
        mod_a(&buf_b[0..], x);
        let mut tmp = [0i32; 16];
        tmp.copy_from_slice(&buf_b[16..32]);
        mod_b(&mut tmp, y);
    }
    clp_v(&mut buf_a);

    mod_c(&buf_a, &mut buf_b);
    for v in buf_b.iter_mut() {
        *v = clip23(v.wrapping_mul(1 << shift));
    }
    let mut k = 31usize;
    for i in 0..16 {
        output[i] = clip23(buf_b[i].wrapping_sub(buf_b[k]));
        output[16 + i] = clip23(buf_b[i].wrapping_add(buf_b[k]));
        k -= 1;
    }
}

const MOD64_A: [i32; 32] = [
    4195568, 4205700, 4226086, 4256977, 4298755, 4351949, 4417251, 4495537, 4587901, 4695690, 4820557, 4964534, 5130115, 5320382, 5539164, 5791261, -6082752, -6421430, -6817439, -7284203, -7839855, -8509474, -9328732, -10350140, -11654242, -13371208, -15725922, -19143224, -24533560,
    -34264200, -57015280, -170908480,
];

fn mod64_a(input: &[i32], output: &mut [i32]) {
    for i in 0..16 {
        output[i] = mul(MOD64_A[i], input[i].wrapping_add(input[16 + i]), 23);
    }
    let mut k = 15i32;
    for i in 16..32 {
        output[i] = mul(MOD64_A[i], input[k as usize].wrapping_sub(input[16 + k as usize]), 23);
        k -= 1;
    }
}

const MOD64_B: [i32; 16] = [4199362, 4240198, 4323885, 4454708, 4639772, 4890013, 5221943, 5660703, 6245623, 7040975, 8158494, 9809974, 12450076, 17261920, 28585092, 85479984];

fn mod64_b(input: &mut [i32], output: &mut [i32]) {
    for i in 0..16 {
        input[16 + i] = mul(MOD64_B[i], input[16 + i], 23);
    }
    for i in 0..16 {
        output[i] = input[i].wrapping_add(input[16 + i]);
    }
    let mut k = 15i32;
    for i in 16..32 {
        output[i] = input[k as usize].wrapping_sub(input[16 + k as usize]);
        k -= 1;
    }
}

const MOD64_C: [i32; 64] = [
    741511, 741958, 742853, 744199, 746001, 748262, 750992, 754197, 757888, 762077, 766777, 772003, 777772, 784105, 791021, 798546, 806707, 815532, 825054, 835311, 846342, 858193, 870912, 884554, 899181, 914860, 931667, 949686, 969011, 989747, 1012012, 1035941, -1061684, -1089412,
    -1119320, -1151629, -1186595, -1224511, -1265719, -1310613, -1359657, -1413400, -1472490, -1537703, -1609974, -1690442, -1780506, -1881904, -1996824, -2128058, -2279225, -2455101, -2662128, -2909200, -3208956, -3579983, -4050785, -4667404, -5509372, -6726913, -8641940, -12091426,
    -20144284, -60420720,
];

fn mod64_c(input: &[i32], output: &mut [i32]) {
    for i in 0..32 {
        output[i] = mul(MOD64_C[i], input[i].wrapping_add(input[32 + i]), 23);
    }
    let mut k = 31i32;
    for i in 32..64 {
        output[i] = mul(MOD64_C[i], input[k as usize].wrapping_sub(input[32 + k as usize]), 23);
        k -= 1;
    }
}

/// 64-point variant used when the core is synthesised at twice its sampling rate.
pub fn imdct_half_64(output: &mut [i32], input: &[i32]) {
    let mut buf_a = [0i32; 64];
    let mut buf_b = [0i32; 64];
    let mag: i32 = input[..64].iter().map(|v| v.abs()).sum();
    let shift = if mag > 0x40_0000 { 2 } else { 0 };
    let round = if shift > 0 { 1 << (shift - 1) } else { 0 };
    for i in 0..64 {
        buf_a[i] = (input[i] + round) >> shift;
    }
    {
        let (b0, b1) = buf_b.split_at_mut(32);
        sum_a(&buf_a, b0, 32);
        sum_b(&buf_a, b1, 32);
    }
    clp_v(&mut buf_b);

    {
        let (a0, rest) = buf_a.split_at_mut(16);
        let (a1, rest) = rest.split_at_mut(16);
        let (a2, a3) = rest.split_at_mut(16);
        sum_a(&buf_b[0..], a0, 16);
        sum_b(&buf_b[0..], a1, 16);
        sum_c(&buf_b[32..], a2, 16);
        sum_d(&buf_b[32..], a3, 16);
    }
    clp_v(&mut buf_a);

    {
        let mut tmp = [0i32; 64];
        sum_a(&buf_a[0..], &mut tmp[0..], 8);
        sum_b(&buf_a[0..], &mut tmp[8..], 8);
        sum_c(&buf_a[16..], &mut tmp[16..], 8);
        sum_d(&buf_a[16..], &mut tmp[24..], 8);
        sum_c(&buf_a[32..], &mut tmp[32..], 8);
        sum_d(&buf_a[32..], &mut tmp[40..], 8);
        sum_c(&buf_a[48..], &mut tmp[48..], 8);
        sum_d(&buf_a[48..], &mut tmp[56..], 8);
        buf_b = tmp;
    }
    clp_v(&mut buf_b);

    {
        let mut tmp = [0i32; 64];
        dct_a(&buf_b[0..], &mut tmp[0..]);
        for g in 1..8 {
            dct_b(&buf_b[8 * g..], &mut tmp[8 * g..]);
        }
        buf_a = tmp;
    }
    clp_v(&mut buf_a);

    {
        let mut tmp = [0i32; 64];
        mod_a(&buf_a[0..], &mut tmp[0..]);
        for g in 1..4 {
            let mut inp = [0i32; 16];
            inp.copy_from_slice(&buf_a[16 * g..16 * g + 16]);
            mod_b(&mut inp, &mut tmp[16 * g..]);
        }
        buf_b = tmp;
    }
    clp_v(&mut buf_b);

    {
        let mut tmp = [0i32; 64];
        mod64_a(&buf_b[0..], &mut tmp[0..]);
        let mut inp = [0i32; 32];
        inp.copy_from_slice(&buf_b[32..64]);
        mod64_b(&mut inp, &mut tmp[32..]);
        buf_a = tmp;
    }
    clp_v(&mut buf_a);

    mod64_c(&buf_a, &mut buf_b);
    for v in buf_b.iter_mut() {
        *v = clip23(v.wrapping_mul(1 << shift));
    }
    let mut k = 63usize;
    for i in 0..32 {
        output[i] = clip23(buf_b[i].wrapping_sub(buf_b[k]));
        output[32 + i] = clip23(buf_b[i].wrapping_add(buf_b[k]));
        k -= 1;
    }
}

/* ---------------------------------------------------------------------------------------------- */
/* synthesis filter banks                                                                           */
/* ---------------------------------------------------------------------------------------------- */

/// Polyphase synthesis state for one channel.
pub struct FixedSynth {
    perfect: bool,
    x96: bool,
    hist1: Vec<i32>,
    offset: usize,
    hist2: Vec<i32>,
}

impl FixedSynth {
    /// `x96`: synthesise 64 bands at twice the core rate (upper 32 bands zero).
    pub fn new(perfect: bool, x96: bool) -> FixedSynth {
        let n = if x96 { 64 } else { 32 };
        FixedSynth { perfect, x96, hist1: vec![0; n * 16], offset: 0, hist2: vec![0; n] }
    }

    /// One block: 32 subband samples in, 32 (or 64) PCM samples out.
    pub fn step(&mut self, subband: &[i32; 32], out: &mut [i32]) {
        if self.x96 {
            self.step64(subband, out);
        } else {
            self.step32(subband, out);
        }
    }

    fn step32(&mut self, input: &[i32; 32], out: &mut [i32]) {
        let window: &[i32; 512] = if self.perfect { &FIR32_PR_FIXED } else { &FIR32_NPR_FIXED };
        let off = self.offset;
        let mut synth = [0i32; 32];
        imdct_half_32(&mut synth, input);
        self.hist1[off..off + 32].copy_from_slice(&synth);
        let buf = &self.hist1;
        // index into the circular history relative to the write position
        let at = |idx: usize| -> i64 { buf[(off + idx) & 511] as i64 };
        for i in 0..16 {
            let mut a = (self.hist2[i] as i64) << 21;
            let mut b = (self.hist2[i + 16] as i64) << 21;
            let mut c = 0i64;
            let mut d = 0i64;
            let mut j = 0;
            while j < 512 {
                a += window[i + j] as i64 * at(i + j);
                b += window[i + j + 16] as i64 * at(15 - i + j);
                c += window[i + j + 32] as i64 * at(16 + i + j);
                d += window[i + j + 48] as i64 * at(31 - i + j);
                j += 64;
            }
            out[i] = clip23(norm(a, 21));
            out[i + 16] = clip23(norm(b, 21));
            self.hist2[i] = norm(c, 21);
            self.hist2[i + 16] = norm(d, 21);
        }
        self.offset = (self.offset.wrapping_sub(32)) & 511;
    }

    fn step64(&mut self, input32: &[i32; 32], out: &mut [i32]) {
        let window: &[i32; 1024] = &FIR64_FIXED;
        let off = self.offset;
        let mut input = [0i32; 64];
        input[..32].copy_from_slice(input32);
        let mut synth = [0i32; 64];
        imdct_half_64(&mut synth, &input);
        self.hist1[off..off + 64].copy_from_slice(&synth);
        let buf = &self.hist1;
        let at = |idx: usize| -> i64 { buf[(off + idx) & 1023] as i64 };
        for i in 0..32 {
            let mut a = (self.hist2[i] as i64) << 20;
            let mut b = (self.hist2[i + 32] as i64) << 20;
            let mut c = 0i64;
            let mut d = 0i64;
            let mut j = 0;
            while j < 1024 {
                a += window[i + j] as i64 * at(i + j);
                b += window[i + j + 32] as i64 * at(31 - i + j);
                c += window[i + j + 64] as i64 * at(32 + i + j);
                d += window[i + j + 96] as i64 * at(63 - i + j);
                j += 128;
            }
            out[i] = clip23(norm(a, 20));
            out[i + 32] = clip23(norm(b, 20));
            self.hist2[i] = norm(c, 20);
            self.hist2[i + 32] = norm(d, 20);
        }
        self.offset = (self.offset.wrapping_sub(64)) & 1023;
    }
}

/// Dequantize `input` (quantization indexes) with step size and scale factor, as the decoder does.
pub fn dequantize(input: &[i32], step_size: i32, scale: i32, out: &mut [i32]) {
    let mut step_scale = step_size as i64 * scale as i64;
    let mut shift = 0u32;
    if step_scale > (1 << 23) {
        shift = (63 - ((step_scale >> 23) as u64).leading_zeros()) + 1;
        step_scale >>= shift;
    }
    for (o, v) in out.iter_mut().zip(input) {
        *o = clip23(norm(*v as i64 * step_scale, 22 - shift));
    }
}

/* ---------------------------------------------------------------------------------------------- */
/* LFE                                                                                              */
/* ---------------------------------------------------------------------------------------------- */

pub struct FixedLfe {
    history: [i32; 8],
    x96_hist: i32,
}

impl FixedLfe {
    pub fn new() -> FixedLfe {
        FixedLfe { history: [0; 8], x96_hist: 0 }
    }

    /// Interpolate `samples` (decimated by 64) to `64 * samples.len()` PCM samples; with `x96` the
    /// result is doubled again by the image-attenuating two-point interpolator.
    pub fn interpolate(&mut self, samples: &[i32], x96: bool) -> Vec<i32> {
        let n = samples.len();
        let mut buf = Vec::with_capacity(8 + n);
        buf.extend_from_slice(&self.history);
        buf.extend_from_slice(samples);
        let mut pcm = vec![0i32; 64 * n];
        for i in 0..n {
            for j in 0..32 {
                let mut a = 0i64;
                let mut b = 0i64;
                for k in 0..8 {
                    let s = buf[8 + i - k] as i64;
                    a += LFE_FIR64_FIXED[j * 8 + k] as i64 * s;
                    b += LFE_FIR64_FIXED[255 - j * 8 - k] as i64 * s;
                }
                pcm[64 * i + j] = clip23(norm(a, 23));
                pcm[64 * i + 32 + j] = clip23(norm(b, 23));
            }
        }
        for k in 0..8 {
            self.history[k] = buf[n + k];
        }
        if !x96 {
            return pcm;
        }
        let mut out = Vec::with_capacity(pcm.len() * 2);
        let mut prev = self.x96_hist;
        for s in &pcm {
            let a = 2097471i64 * *s as i64 + 6291137i64 * prev as i64;
            let b = 6291137i64 * *s as i64 + 2097471i64 * prev as i64;
            prev = *s;
            out.push(clip23(norm(a, 23)));
            out.push(clip23(norm(b, 23)));
        }
        self.x96_hist = prev;
        out
    }
}

impl Default for FixedLfe {
    fn default() -> Self {
        Self::new()
    }
}

/// Dequantize LFE samples (8-bit signed) with the 7-bit scale index.
pub fn dequantize_lfe(q: &[i32], scale_idx: usize) -> Vec<i32> {
    let scale = mul(4697620, crate::tables::SCALE_7[scale_idx] as i32, 23);
    q.iter().map(|v| clip23((v.wrapping_mul(scale)) >> 4)).collect()
}
