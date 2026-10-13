//! The decoder's fixed-point arithmetic, which the encoder mirrors exactly.

#[inline]
pub(crate) fn norm16(v: i64) -> i64 {
    (v + (1 << 15)) >> 16
}

#[inline]
pub(crate) fn mul16(a: i32, b: i32) -> i32 {
    norm16(a as i64 * b as i64) as i32
}

#[inline]
pub(crate) fn clip23(v: i64) -> i32 {
    v.clamp(-(1 << 23), (1 << 23) - 1) as i32
}
