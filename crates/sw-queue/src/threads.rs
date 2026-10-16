//! Running work on a chosen number of threads.

/// Runs `work` with the encoders limited to `threads` threads; 0 means every core (the shared default pool).
///
/// The encoders spread their work with rayon, so the limit is a rayon pool made for this one call, and `work` runs
/// on one of its threads (hence `Send`). The call waits for the pool to finish.
pub fn with_threads<R: Send>(threads: u32, work: impl FnOnce() -> R + Send) -> R {
    if threads == 0 {
        return work();
    }
    let Ok(pool) = rayon::ThreadPoolBuilder::new().num_threads(threads as usize).build() else {
        return work();
    };
    pool.install(work)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limit_is_what_the_work_sees() {
        assert_eq!(with_threads(1, rayon::current_num_threads), 1);
        assert_eq!(with_threads(3, rayon::current_num_threads), 3);
    }

    #[test]
    fn borrowed_state_can_be_changed_inside() {
        let mut calls = Vec::new();
        let result = with_threads(2, || {
            calls.push(1);
            "done"
        });
        assert_eq!((result, calls), ("done", vec![1]));
    }
}
