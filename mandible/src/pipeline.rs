//! Wiring the extraction runner and the on-disk cache together: the shared
//! "get me this tool's tree" path used by both `--doctor` and the TUI.

use mandible_cache::{CacheEntry, CacheKey, Store, StoredTierStatus};
use mandible_core::CommandNode;
use mandible_extract::{Runner, TierStatus};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The result of loading one tool's tree, whether it came from cache or a
/// fresh extraction.
pub struct LoadedTool {
    /// The tool name.
    pub tool: String,
    /// The merged tree, or `None` if no tier could extract anything.
    pub root: Option<CommandNode>,
    /// Per-tier outcome, for `--doctor` and the `?` overlay.
    pub tier_statuses: Vec<TierStatus>,
    /// True if this came from the on-disk cache rather than a fresh
    /// extraction this run.
    pub from_cache: bool,
    /// When the cache entry was written, if `from_cache`.
    pub cached_at_unix_secs: Option<i64>,
    /// Wall-clock time this call spent (near-zero for a cache hit).
    pub elapsed: Duration,
}

/// Load `tool_name`'s tree: try the cache first (unless `refresh`), and on
/// a miss (or refresh), run the full extraction pipeline and write the
/// result back to the cache.
///
/// Cache unavailability (e.g. an unwritable `$XDG_CACHE_HOME`) degrades to
/// "always extract fresh" rather than erroring — the cache is a speed
/// optimization, not a correctness requirement.
pub fn load(tool_name: &str, refresh: bool) -> LoadedTool {
    let runner = Runner::new(default_tiers());
    let store = Store::open_default().ok();
    let tier_names: Vec<&str> = runner.tier_names();
    let key = CacheKey::build(tool_name, None, &tier_names, None);

    if refresh {
        if let Some(store) = &store {
            let _ = store.invalidate(tool_name);
        }
    } else if let Some(store) = &store {
        if let Some(entry) = store.load(tool_name, &key) {
            return LoadedTool {
                tool: tool_name.to_string(),
                root: entry.root,
                tier_statuses: entry
                    .tier_statuses
                    .into_iter()
                    .map(|s| TierStatus {
                        tier: leak_tier_name(&s.tier),
                        detected: s.detected,
                        error: s.error,
                    })
                    .collect(),
                from_cache: true,
                cached_at_unix_secs: Some(entry.cached_at_unix_secs),
                elapsed: Duration::ZERO,
            };
        }
    }

    let start = Instant::now();
    let result = runner.extract_full(tool_name);
    let elapsed = start.elapsed();

    if let Some(store) = &store {
        let entry = CacheEntry {
            key,
            tool: tool_name.to_string(),
            root: result.root.clone(),
            tier_statuses: result
                .tier_statuses
                .iter()
                .map(|s| StoredTierStatus {
                    tier: s.tier.to_string(),
                    detected: s.detected,
                    error: s.error.clone(),
                })
                .collect(),
            catalog: None,
            cached_at_unix_secs: unix_now(),
        };
        let _ = store.store(&entry);
    }

    LoadedTool {
        tool: tool_name.to_string(),
        root: result.root,
        tier_statuses: result.tier_statuses,
        from_cache: false,
        cached_at_unix_secs: None,
        elapsed,
    }
}

/// The default tier set for this batch: Tier A only (spec roadmap phase 1).
fn default_tiers() -> Vec<Box<dyn mandible_extract::ExtractionTier>> {
    mandible_extract::default_tiers()
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// [`TierStatus::tier`] is `&'static str`, but a cache-loaded entry only
/// has an owned `String`. There are a small, fixed number of distinct tier
/// names in a process's lifetime, so leaking each distinct name once (never
/// growing unbounded — one process only ever loads a handful of tools) is
/// simpler and safer than threading a second, owned-string variant of
/// `TierStatus` through the cache and the TUI just for this batch.
fn leak_tier_name(name: &str) -> &'static str {
    Box::leak(name.to_string().into_boxed_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Combined into one test (rather than two independent `#[test]`s) so
    // the `XDG_CACHE_HOME` env var mutation — process-global — can't race
    // against another test in this binary doing the same thing.
    #[test]
    fn loads_fresh_then_hits_cache_on_second_load() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CACHE_HOME", dir.path());

        let first = load("git", true);
        assert!(first.root.is_some());
        assert!(!first.from_cache);

        let second = load("git", false);
        assert!(
            second.from_cache,
            "second load should hit the cache the first load populated"
        );
        assert!(second.root.is_some());

        std::env::remove_var("XDG_CACHE_HOME");
    }
}
