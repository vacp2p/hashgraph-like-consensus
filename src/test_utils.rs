//! Helpers shared by the crate's inline test modules.

/// Current time in seconds since Unix epoch — the caller-supplied `now`
/// the library expects.
pub(crate) fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
