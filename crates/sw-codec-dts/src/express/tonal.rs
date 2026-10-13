//! DTS Express (LBR) tonal components: the encoder side.
//!
//! A tonal component is a sinusoid the decoder synthesises directly in the MDCT coefficient domain (before
//! the inverse transform) from a few parameters: its frequency, an amplitude and a phase per channel. Tones
//! come in five groups with different time/frequency resolution (group `g` has `2^g` subframes per frame of
//! `32 >> g` steps; the frequency is sent in steps of `2^-(5-g)` of a coefficient line). A tone set of one
//! subframe fades in over the subframe and out over the next, 22 steps later than the residual of the same
//! frame, so what the encoder subtracts from the coefficients is exactly what the decoder adds back.
//!
//! This module holds the decoder's tone model (`render`), the search that picks tones for one subframe from
//! the coefficients, and the bit syntax of the tonal chunks.

use crate::express::bits::{BitsLe, Vlc};
use crate::express::filterbank::HOP;
use crate::express::tables::*;

/// Amplitude indices are 1..AMP_MAX-1 (index 0 and above mean "no tone").
pub const AMP_MAX: usize = 56;

/// Steps between the residual of a frame and the first step of its tones.
pub const TONE_DELAY: usize = 22;

#[derive(Clone, Debug, PartialEq)]
pub struct Tone {
    /// Frequency position in units of `2^-(5-g)` coefficient lines (at least 1).
    pub fp: u32,
    /// Amplitude index per channel (0: the tone is absent from that channel).
    pub amp: Vec<u8>,
    /// Phase code (0..7) per channel.
    pub phs: Vec<u8>,
}

/// One group's geometry: steps per subframe and the fade weights over the two subframes a set lives in.
pub struct Group {
    pub g: u32,
    pub steps: usize,
    pub weights: Vec<f64>,
}

impl Group {
    pub fn new(g: u32) -> Group {
        let steps = 32usize >> g;
        let mut weights = vec![0f64; 2 * steps];
        let idx = |pos: usize| ((pos + 1) << g) - 1;
        for r in 0..steps {
            weights[r] = SYNTH_ENV[idx(r)] as f64;
        }
        for pos in 0..steps {
            let k = 30isize - idx(pos) as isize;
            weights[steps + pos] = if k < 0 { 0.0 } else { SYNTH_ENV[k as usize] as f64 };
        }
        Group { g, steps, weights }
    }

    /// Largest frequency position the group's difference codes can reach.
    pub fn coverage(&self) -> u32 {
        let size = tnl_table(self.g).len();
        let s = size - 1;
        (FST_AMP[s] as u32 + (1u32 << (s >> 2))) - 1
    }
}

fn tnl_table(g: u32) -> &'static [(u8, u8)] {
    match g {
        0 => &HUFF_TNL_GROUP_0,
        1 => &HUFF_TNL_GROUP_1,
        2 => &HUFF_TNL_GROUP_2,
        3 => &HUFF_TNL_GROUP_3,
        _ => &HUFF_TNL_GROUP_4,
    }
}

fn cos_tab(i: i32) -> f64 {
    (std::f64::consts::PI * (i & 255) as f64 / 128.0).cos()
}

/// Frequency-dependent constants of a tone: line, correlation row, phase advance per step, and the phase
/// offset that makes the 3-bit phase refer to the end of the fade-in subframe.
fn params(g: u32, fp: u32) -> (usize, usize, i32, i32) {
    let l = 1i32 << (5 - g);
    let x = (fp >> (5 - g)) as usize;
    let fd = ((fp & ((1 << (5 - g)) - 1)) << g) as usize;
    let ph_rot = 256 - (x as i32 & 1) * 128 - fd as i32 * 4;
    let shift = PH0_SHIFT[(x & 3) * 2 + (fp as usize & 1)] as i32 - ph_rot * (l - 1);
    (x, fd, ph_rot, shift)
}

/// Add one step of a tone (`c`, `s` the cosine and sine terms) to the coefficients, folding the lines below
/// zero back as the decoder does.
fn add_lines(v: &mut [f64; HOP], x: usize, fd: usize, c: f64, s: f64) {
    let cf = &CORR_CF[fd];
    let coef = [-s, c, s, -c, -s, c, s, -c, -s, c, s];
    for k in 0..11 {
        if x == 0 && k < 5 {
            continue;
        }
        let idx = x as isize + k as isize - 5;
        let line = if idx < 0 { (-1 - idx) as usize } else { idx as usize };
        if line < HOP {
            v[line] += cf[k] as f64 * coef[k];
        }
    }
}

/// What the decoder adds for a tone of one channel with amplitude index `amp` and phase code `phs`, over the
/// `2 * steps` steps of its set.
pub fn render(grp: &Group, fp: u32, amp: u8, phs: u8, out: &mut [[f64; HOP]]) {
    let (x, fd, ph_rot, shift) = params(grp.g, fp);
    let a = QUANT_AMP[amp as usize] as f64;
    let phi0 = 128 - 32 * phs as i32 + shift;
    for r in 0..2 * grp.steps {
        let w = grp.weights[r] * a;
        if w == 0.0 {
            continue;
        }
        let ph = phi0 + r as i32 * ph_rot;
        add_lines(&mut out[r], x, fd, w * cos_tab(ph), w * cos_tab(ph + 64));
    }
}

/// Basis patterns of a tone at unit amplitude for the phase angle 0 (`u`) and a quarter turn later (`v`):
/// any phase is `cos * u + sin * v`. Written into scratch arrays that are zero outside the footprint; returns
/// the phase offset and the footprint's lines `lo..hi` (clear them with `clear_basis` afterwards).
fn basis_into(grp: &Group, fp: u32, u: &mut [[f64; HOP]], v: &mut [[f64; HOP]]) -> (i32, usize, usize) {
    let (x, fd, ph_rot, shift) = params(grp.g, fp);
    let d = 2.0 * std::f64::consts::PI * ph_rot as f64 / 256.0;
    for r in 0..2 * grp.steps {
        let w = grp.weights[r];
        if w == 0.0 {
            continue;
        }
        let (cr, sr) = ((d * r as f64).cos(), (d * r as f64).sin());
        add_lines(&mut u[r], x, fd, w * cr, -w * sr);
        add_lines(&mut v[r], x, fd, -w * sr, -w * cr);
    }
    (shift, x.saturating_sub(5), (x + 6).min(HOP))
}

fn clear_basis(u: &mut [[f64; HOP]], v: &mut [[f64; HOP]], lo: usize, hi: usize) {
    for (ru, rv) in u.iter_mut().zip(v.iter_mut()) {
        for k in lo..hi {
            ru[k] = 0.0;
            rv[k] = 0.0;
        }
    }
}

fn dot(a: &[[f64; HOP]], b: &[[f64; HOP]], lo: usize, hi: usize) -> f64 {
    let mut acc = 0.0;
    for (ra, rb) in a.iter().zip(b) {
        for k in lo..hi {
            acc += ra[k] * rb[k];
        }
    }
    acc
}

/// Settings of the tone search.
#[derive(Clone, Copy)]
pub struct Search {
    /// Coefficient lines that may carry tones (`4 * subbands - 6`).
    pub max_line: usize,
    /// A tone is kept when the energy it removes exceeds this many times the energy floor of its footprint.
    pub kappa: f64,
    pub max_tones: usize,
}

/// Energy of each coefficient line over all channels, weighted by what a tone would see.
fn line_energy(grp: &Group, targets: &[Vec<[f64; HOP]>], line_hi: usize) -> Vec<f64> {
    let mut spec = vec![0f64; HOP];
    for t in targets {
        for (r, row) in t.iter().enumerate() {
            let w2 = grp.weights[r] * grp.weights[r];
            for k in 0..line_hi {
                spec[k] += w2 * row[k] * row[k];
            }
        }
    }
    spec
}

/// Median of the squares of `values`, which is the local noise floor.
fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    values[values.len() / 2]
}

/// The strongest spectral peaks (at most five) that stand clearly above the median of their neighbourhood, with
/// their ratio to it, best first.
fn find_peaks(spec: &[f64], line_hi: usize) -> Vec<(usize, f64)> {
    let floor_at = |k: usize| -> f64 {
        let lo = k.saturating_sub(12);
        let hi = (k + 13).min(line_hi);
        median(spec[lo..hi].to_vec())
    };
    let mut peaks: Vec<(usize, f64)> = (1..line_hi.saturating_sub(1))
        .filter(|&k| spec[k] >= spec[k - 1] && spec[k] > spec[k + 1] && spec[k] > 0.0)
        .map(|k| (k, spec[k] / (floor_at(k) + 1e-9)))
        .filter(|&(_, r)| r > 6.0)
        .collect();
    peaks.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    peaks.truncate(5);
    peaks
}

/// The inner products of the two basis patterns with each other.
fn basis_gram(bu: &[[f64; HOP]], bv: &[[f64; HOP]], lo: usize, hi: usize) -> (f64, f64, f64) {
    (dot(bu, bu, lo, hi), dot(bu, bv, lo, hi), dot(bv, bv, lo, hi))
}

/// The energy a tone at `fp` would remove from the targets, or `None` when its two basis patterns are dependent.
fn tone_gain(
    grp: &Group,
    targets: &[Vec<[f64; HOP]>],
    fp: u32,
    bu: &mut [[f64; HOP]],
    bv: &mut [[f64; HOP]],
) -> Option<f64> {
    let (_, lo_l, hi_l) = basis_into(grp, fp, bu, bv);
    let (uu, uv, vv) = basis_gram(bu, bv, lo_l, hi_l);
    let det = uu * vv - uv * uv;
    let mut gain = None;
    if det.abs() > 1e-12 {
        let mut total = 0.0;
        for t in targets {
            let (tu, tv) = (dot(t, bu, lo_l, hi_l), dot(t, bv, lo_l, hi_l));
            let a = (tu * vv - tv * uv) / det;
            let b = (tv * uu - tu * uv) / det;
            total += a * tu + b * tv;
        }
        gain = Some(total);
    }
    clear_basis(bu, bv, lo_l, hi_l);
    gain
}

/// The frequency (in the group's fine steps) near one of the `peaks` where a tone removes the most energy. The
/// candidates are tried side by side, each with its own scratch patterns, and the first of equals wins.
fn best_frequency(
    grp: &Group,
    targets: &[Vec<[f64; HOP]>],
    peaks: &[(usize, f64)],
    tones: &[Tone],
    cfg: &Search,
) -> Option<u32> {
    use rayon::prelude::*;
    let f = 1u32 << (5 - grp.g);
    let cover = grp.coverage();
    let mut candidates: Vec<u32> = Vec::new();
    for &(k0, _) in peaks {
        let lo = ((k0 as u32).saturating_sub(1) * f).max(1);
        let hi = (((k0 as u32) + 1) * f).min(cover - 1);
        for fp in lo..=hi {
            if (fp >> (5 - grp.g)) as usize > cfg.max_line {
                continue;
            }
            // a second tone within a line of one already chosen is the same partial refitted
            if tones
                .iter()
                .any(|t| (t.fp as i64 - fp as i64).unsigned_abs() < f as u64)
            {
                continue;
            }
            candidates.push(fp);
        }
    }
    let rows = 2 * grp.steps;
    let gains: Vec<Option<f64>> = candidates
        .par_iter()
        .map_init(
            || (vec![[0f64; HOP]; rows], vec![[0f64; HOP]; rows]),
            |(bu, bv), &fp| tone_gain(grp, targets, fp, bu, bv),
        )
        .collect();
    let mut best: Option<(f64, u32)> = None;
    for (&fp, gain) in candidates.iter().zip(gains) {
        if let Some(gain) = gain {
            if best.is_none_or(|(g, _)| gain > g) {
                best = Some((gain, fp));
            }
        }
    }
    best.map(|(_, fp)| fp)
}

/// Correlation of a phase angle with the target, and the energy of the tone at unit amplitude.
fn phase_terms(theta: f64, (tu, tv): (f64, f64), (uu, uv, vv): (f64, f64, f64)) -> (f64, f64) {
    let (c, s) = (theta.cos(), theta.sin());
    (c * tu + s * tv, c * c * uu + 2.0 * c * s * uv + s * s * vv)
}

/// Angle of a phase code, with the rotation the group's frequency adds.
fn phase_angle(code: u8, shift: i32) -> f64 {
    std::f64::consts::PI * (128 - 32 * code as i32 + shift) as f64 / 128.0
}

/// Nearest amplitude index (on the log scale) to `a`.
fn nearest_amp(a: f64) -> usize {
    let mut idx = 1usize;
    let mut dbest = f64::MAX;
    for i in 1..AMP_MAX {
        let d = (QUANT_AMP[i] as f64 / a).ln().abs();
        if d < dbest {
            dbest = d;
            idx = i;
        }
    }
    idx
}

/// For a tone at `fp`: each channel's best phase code and amplitude index, and the energy the tone removes in all.
fn fit_tone(
    grp: &Group,
    targets: &[Vec<[f64; HOP]>],
    fp: u32,
    basis: (&mut [[f64; HOP]], &mut [[f64; HOP]]),
) -> (Tone, f64) {
    let (bu, bv) = basis;
    let (shift, lo_l, hi_l) = basis_into(grp, fp, bu, bv);
    let gram = basis_gram(bu, bv, lo_l, hi_l);
    let nch = targets.len();
    let mut tone = Tone {
        fp,
        amp: vec![0; nch],
        phs: vec![0; nch],
    };
    let mut total_gain = 0.0;
    for (ch, t) in targets.iter().enumerate() {
        let target = (dot(t, bu, lo_l, hi_l), dot(t, bv, lo_l, hi_l));
        let mut best_phase: Option<(f64, u8, f64)> = None; // (gain, phase code, amplitude)
        for p in 0..8u8 {
            let (tp, pp) = phase_terms(phase_angle(p, shift), target, gram);
            if pp <= 0.0 || tp <= 0.0 {
                continue;
            }
            if best_phase.is_none_or(|(g, _, _)| tp * tp / pp > g) {
                best_phase = Some((tp * tp / pp, p, tp / pp));
            }
        }
        let Some((_, p, a)) = best_phase else { continue };
        let idx = nearest_amp(a);
        let aq = QUANT_AMP[idx] as f64;
        let (tp, pp) = phase_terms(phase_angle(p, shift), target, gram);
        let g = 2.0 * aq * tp - aq * aq * pp;
        if g > 0.0 {
            tone.amp[ch] = idx as u8;
            tone.phs[ch] = p;
            total_gain += g;
        }
    }
    clear_basis(bu, bv, lo_l, hi_l);
    (tone, total_gain)
}

/// The energy floor of a tone's footprint: median line energy of the neighbourhood times the support size.
fn footprint_floor(grp: &Group, targets: &[Vec<[f64; HOP]>], fp: u32, line_hi: usize) -> f64 {
    let wsum2: f64 = grp.weights.iter().map(|w| w * w).sum();
    let k0 = (fp >> (5 - grp.g)) as usize;
    let mut floor = 0.0;
    for t in targets {
        let mut e: Vec<f64> = Vec::new();
        for row in t.iter() {
            for k in k0.saturating_sub(12)..(k0 + 13).min(line_hi) {
                e.push(row[k] * row[k]);
            }
        }
        floor += median(e) * 11.0 * wsum2;
    }
    floor
}

/// Subtracts from the targets what the decoder will add for the tone.
fn subtract_tone(grp: &Group, targets: &mut [Vec<[f64; HOP]>], tone: &Tone) {
    for (ch, t) in targets.iter_mut().enumerate() {
        if tone.amp[ch] == 0 {
            continue;
        }
        let mut rendered = vec![[0f64; HOP]; 2 * grp.steps];
        render(grp, tone.fp, tone.amp[ch], tone.phs[ch], &mut rendered);
        for (row, add) in t.iter_mut().zip(&rendered) {
            for k in 0..HOP {
                row[k] -= add[k];
            }
        }
    }
}

/// Pick tones for one subframe set. `targets[ch]` holds the coefficients of the `2 * steps` steps of the set
/// (updated: the chosen tones are subtracted).
pub fn pick(grp: &Group, targets: &mut [Vec<[f64; HOP]>], cfg: &Search, bits_left: &mut i64) -> Vec<Tone> {
    let nch = targets.len();
    let mut tones: Vec<Tone> = Vec::new();
    let line_hi = (cfg.max_line + 1).min(HOP);
    let mut bu = vec![[0f64; HOP]; 2 * grp.steps];
    let mut bv = vec![[0f64; HOP]; 2 * grp.steps];
    for _ in 0..cfg.max_tones {
        if *bits_left < 40 {
            break;
        }
        let peaks = find_peaks(&line_energy(grp, targets, line_hi), line_hi);
        if peaks.is_empty() {
            break;
        }
        let Some(fp) = best_frequency(grp, targets, &peaks, &tones, cfg) else {
            break;
        };
        let (tone, total_gain) = fit_tone(grp, targets, fp, (&mut bu, &mut bv));
        let floor = footprint_floor(grp, targets, fp, line_hi);
        if total_gain < cfg.kappa * floor || tone.amp.iter().all(|&a| a == 0) {
            break;
        }
        subtract_tone(grp, targets, &tone);
        *bits_left -= 20 + 4 * tone.amp.iter().filter(|&&a| a > 0).count() as i64 + nch as i64;
        tones.push(tone);
    }
    tones.sort_by_key(|t| t.fp);
    tones.dedup_by_key(|t| t.fp);
    tones
}

/// The six tonal scale factor references for a frame's tones (`groups[g][j]` the tones of subframe `j`).
pub fn scale_factors(groups: &[Vec<Vec<Tone>>], limited_range: i32) -> [u8; 6] {
    let mut min_amp = [i32::MAX; 6];
    for (g, subs) in groups.iter().enumerate() {
        for tones in subs {
            for t in tones {
                if let Some(main) = main_channel(t) {
                    let sb = (t.fp >> (7 - g as u32)) as usize;
                    let sg = FREQ_TO_SB[sb.min(31)] as usize;
                    min_amp[sg] = min_amp[sg].min(t.amp[main] as i32);
                }
            }
        }
    }
    let mut out = [0u8; 6];
    for i in 0..6 {
        if min_amp[i] != i32::MAX {
            out[i] = (min_amp[i] - limited_range + 2).clamp(0, 63) as u8;
        }
    }
    out
}

fn main_channel(t: &Tone) -> Option<usize> {
    t.amp
        .iter()
        .enumerate()
        .filter(|(_, &a)| a > 0)
        .max_by_key(|(_, &a)| a)
        .map(|(i, _)| i)
}

/// Bits of one group's chunk body: every subframe's tones in frequency order, closed by a zero difference.
pub fn write_group(g: u32, subs: &[Vec<Tone>], nch: usize, scf: &[u8; 6], limited_range: i32) -> BitsLe {
    let tab = Vlc::new(tnl_table(g));
    let amp_vlc = Vlc::new(&HUFF_TNL_SCF);
    let damp_vlc = Vlc::new(&HUFF_DAMP);
    let dph_vlc = Vlc::new(&HUFF_DPH);
    let ch_bits = if nch <= 1 {
        0
    } else {
        32 - (nch as u32 - 1).leading_zeros()
    };
    let mut w = BitsLe::new();
    let put_diff = |w: &mut BitsLe, d: u32| {
        let size = tnl_table(g).len();
        let mut s = 0;
        for i in 0..size {
            if FST_AMP[i] as u32 <= d {
                s = i;
            }
        }
        tab.put(w, s as u32);
        let extra_bits = (s >> 2) as u32;
        if extra_bits > 0 {
            w.put(d - FST_AMP[s] as u32, extra_bits);
        }
    };
    for tones in subs {
        let mut prev = 0u32;
        for t in tones {
            let main = main_channel(t).expect("a tone with an amplitude");
            put_diff(&mut w, t.fp - prev + 1);
            prev = t.fp;
            w.put(main as u32, ch_bits);
            let sb = (t.fp >> (7 - g)) as usize;
            let sg = FREQ_TO_SB[sb.min(31)] as usize;
            let v = t.amp[main] as i32 - scf[sg] as i32 - limited_range + 2;
            amp_vlc.put(&mut w, v.max(0) as u32);
            w.put(t.phs[main] as u32, 3);
            for ch in 0..nch {
                if ch == main {
                    continue;
                }
                if t.amp[ch] > 0 {
                    w.put(1, 1);
                    damp_vlc.put(&mut w, (t.amp[main] - t.amp[ch]) as u32);
                    dph_vlc.put(&mut w, ((t.phs[main] as i32 - t.phs[ch] as i32).rem_euclid(8)) as u32);
                } else {
                    w.put(0, 1);
                }
            }
        }
        put_diff(&mut w, 0);
    }
    w
}

/// The body of the SCF chunk: six 6-bit tonal scale factors.
pub fn write_scf(scf: &[u8; 6]) -> BitsLe {
    let mut w = BitsLe::new();
    for v in scf {
        w.put(*v as u32, 6);
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tone model: a pure sinusoid fitted and subtracted leaves a small remainder.
    #[test]
    fn a_fitted_tone_is_mostly_removed() {
        let grp = Group::new(0);
        let n = 2 * grp.steps;
        // a target made by the model itself with amplitude and phase on the quantiser grid
        let mut target = vec![vec![[0f64; HOP]; n]];
        render(&grp, 514, 36, 3, &mut target[0]);
        let e0: f64 = target[0].iter().flatten().map(|v| v * v).sum();
        let cfg = Search {
            max_line: 120,
            kappa: 1.0,
            max_tones: 4,
        };
        let mut bits = 1000;
        let tones = pick(&grp, &mut target, &cfg, &mut bits);
        let e1: f64 = target[0].iter().flatten().map(|v| v * v).sum();
        assert!(!tones.is_empty());
        assert!(e1 < e0 * 1e-3, "left {} of the energy", e1 / e0);
    }

    #[test]
    fn group_syntax_ends_every_subframe() {
        let tones = vec![
            Tone {
                fp: 40,
                amp: vec![24],
                phs: vec![3],
            },
            Tone {
                fp: 55,
                amp: vec![23],
                phs: vec![7],
            },
        ];
        let mut subs: Vec<Vec<Tone>> = vec![Vec::new(); 8];
        subs[2] = tones;
        let bytes = write_group(3, &subs, 1, &[0, 0, 23, 25, 0, 0], 0).into_bytes();
        assert!(!bytes.is_empty());
    }
}
