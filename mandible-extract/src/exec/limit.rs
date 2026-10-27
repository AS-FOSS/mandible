//! The wall-clock cap for an `extract_node` probe (spec §6 rule 4).
//!
//! Tiers ask [`extract_limit`] instead of holding a constant, so one scope
//! can change the cap for the probes it runs on its thread.

use std::cell::Cell;
use std::time::Duration;

/// The cap on an `extract_node` probe when no scope says otherwise.
pub const EXTRACT_LIMIT: Duration = Duration::from_secs(30);

/// No limit: too long to reach, short enough not to overflow an `Instant`.
const UNLIMITED: Duration = Duration::from_secs(60 * 60 * 24 * 365);

thread_local! {
    static SCOPED: Cell<Option<Option<Duration>>> = const { Cell::new(None) };
}

/// The cap an `extract_node` probe on this thread runs under now.
pub fn extract_limit() -> Duration {
    match SCOPED.with(Cell::get) {
        Some(Some(limit)) => limit,
        Some(None) => UNLIMITED,
        None => EXTRACT_LIMIT,
    }
}

/// Run `f` with `limit` as this thread's `extract_node` cap, `None` for no
/// cap. The previous cap is restored afterwards, also when `f` panics.
pub fn with_extract_limit<T>(limit: Option<Duration>, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<Option<Duration>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            SCOPED.with(|s| s.set(self.0));
        }
    }
    let _restore = Restore(SCOPED.with(|s| s.replace(Some(limit))));
    f()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_cap_is_thirty_seconds() {
        assert_eq!(extract_limit(), Duration::from_secs(30));
    }

    #[test]
    fn a_scope_changes_the_cap_and_restores_it() {
        let short = Duration::from_secs(10);
        with_extract_limit(Some(short), || {
            assert_eq!(extract_limit(), short);
            with_extract_limit(None, || {
                assert!(extract_limit() > Duration::from_secs(86_400))
            });
            assert_eq!(extract_limit(), short);
        });
        assert_eq!(extract_limit(), EXTRACT_LIMIT);
    }

    #[test]
    fn a_scope_does_not_reach_another_thread() {
        with_extract_limit(None, || {
            let other = std::thread::spawn(extract_limit).join().unwrap();
            assert_eq!(other, EXTRACT_LIMIT);
        });
    }
}
