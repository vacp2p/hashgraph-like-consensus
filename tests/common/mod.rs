//! Helpers shared by the integration-test crates.

/// Current time in seconds since Unix epoch — the caller-supplied `now`
/// the library expects.
pub fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
