//! Stereo decorrelation: the ways a channel pair can be coded.

/// How a stereo pair is stored, with the two numbers the decoder needs.
#[derive(Clone, Copy)]
pub(crate) struct Stereo {
    pub(crate) shift: u32,
    pub(crate) left_weight: u32,
}

pub(crate) const LEFT_RIGHT: Stereo = Stereo {
    shift: 0,
    left_weight: 0,
};

/// The four ways of storing a pair: left/right, left/side, right/side, mid/side. Returns the changed channels.
pub(crate) fn stereo_variants(left: &[i32], right: &[i32]) -> Vec<(Stereo, Vec<i32>, Vec<i32>)> {
    let n = left.len();
    let mut out = vec![(LEFT_RIGHT, left.to_vec(), right.to_vec())];
    // left and side
    out.push((
        Stereo {
            shift: 0,
            left_weight: 1,
        },
        left.to_vec(),
        (0..n).map(|i| left[i].wrapping_sub(right[i])).collect(),
    ));
    // right and side
    let mut l2 = Vec::with_capacity(n);
    let mut r2 = Vec::with_capacity(n);
    for i in 0..n {
        let side = left[i].wrapping_sub(right[i]);
        r2.push(side);
        l2.push(right[i].wrapping_add(side >> 31));
    }
    out.push((
        Stereo {
            shift: 31,
            left_weight: 1,
        },
        l2,
        r2,
    ));
    // mid and side
    let mut l3 = Vec::with_capacity(n);
    let mut r3 = Vec::with_capacity(n);
    for i in 0..n {
        l3.push(left[i].wrapping_add(right[i]) >> 1);
        r3.push(left[i].wrapping_sub(right[i]));
    }
    out.push((
        Stereo {
            shift: 1,
            left_weight: 1,
        },
        l3,
        r3,
    ));
    out
}
