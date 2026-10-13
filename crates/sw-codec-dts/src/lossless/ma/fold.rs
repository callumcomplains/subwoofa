//! Embedded downmixes of a stream with several channel sets, and applying them to the samples.

use crate::lossless::xll::DmixRow;

/// One term of an embedded downmix: `row += column x coef` (15-bit fixed point), the column taken from the
/// original channel.
#[derive(Debug, Clone)]
pub struct Fold {
    pub row: usize,
    pub col: usize,
    pub coef: i32,
    /// The term belongs to the downmix to 5.1 (the core carries it too), not the one to stereo.
    pub to_5_1: bool,
}

/// Embedded downmixes of a stream with several channel sets. The channels of a set are folded into the channels
/// of the sets below it, so a decoder that reads only the lower sets (or the core) still gets a complete
/// programme: 7.x folds into 5.1, and 5.1 can fold into a stereo set of its own.
#[derive(Debug, Clone)]
pub struct Hier {
    /// Coded channels of every set, set 0 first.
    pub sets: Vec<std::ops::Range<usize>>,
    /// Header rows of every set from set 1 up: one per channel of the sets below.
    pub rows: Vec<Vec<DmixRow>>,
    pub folds: Vec<Fold>,
    /// Gain the core carries relative to the transmitted samples of each coded channel (15-bit fixed point) and
    /// its 16-bit inverse; unity where no set scales the channel.
    pub scale: Vec<i32>,
    pub inv: Vec<i32>,
    /// Set 0 is a stereo downmix.
    pub stereo: bool,
    /// Channels beyond 5.x are folded into the 5.x ones.
    pub extras: bool,
}

#[inline]
pub(crate) fn mul15(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64 + (1 << 14)) >> 15) as i32
}

impl Hier {
    /// Turn the original channels (in coded order) into the transmitted ones: every row gets the weighted columns
    /// added, exactly inverted by the decoder. With `core_only` just the downmix to 5.1 is applied (what the
    /// core carries).
    pub fn apply(&self, pcm: &mut [Vec<i32>], core_only: bool) {
        // the decoder undoes the sets from the bottom up, so the stereo set is built from the 5.1 channels as
        // they are transmitted (with the extra channels already folded in), and those from the originals
        for to_5_1 in [true, false] {
            if !to_5_1 && core_only {
                break;
            }
            let mut snap: Vec<Option<Vec<i32>>> = vec![None; pcm.len()];
            for f in self.folds.iter().filter(|f| f.to_5_1 == to_5_1) {
                if snap[f.col].is_none() {
                    snap[f.col] = Some(pcm[f.col].clone());
                }
            }
            for f in self.folds.iter().filter(|f| f.to_5_1 == to_5_1) {
                let src = snap[f.col].as_ref().expect("snapshot");
                for (d, s) in pcm[f.row].iter_mut().zip(src) {
                    *d += mul15(*s, f.coef);
                }
            }
        }
    }

    /// Gain the core sees relative to the transmitted samples of coded channel `x`.
    pub fn core_gain(&self, x: usize) -> f64 {
        self.scale[x] as f64 / 32768.0
    }
}
