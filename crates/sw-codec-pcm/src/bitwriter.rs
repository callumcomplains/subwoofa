//! The shared most-significant-bit-first writer (`sw_core::bits`), and the Rice code FLAC writes with it.

pub(crate) use sw_core::bits::BitWriter;

/// Rice code: the quotient in unary (that many zeros, then a one) and the remainder in `k` bits.
pub(crate) fn put_rice(w: &mut BitWriter, zigzag: u64, k: u32) {
    let mut zeros = zigzag >> k;
    while zeros >= 32 {
        w.put(0, 32);
        zeros -= 32;
    }
    w.put(0, zeros as u32);
    w.put(1, 1);
    w.put((zigzag & ((1u64 << k) - 1)) as u32, k);
}
