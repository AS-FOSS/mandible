//! Cache key construction (spec §11 "Key").
//!
//! Not a content hash of the binary — hashing a 50 MB `docker` binary costs
//! more than the parse it protects. Instead: file identity (path, size,
//! mtime, inode) plus the tool's own reported version when cheaply
//! available, plus mantui's own schema/binary/feature versions so a mantui
//! upgrade or a feature-flag change invalidates old entries.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Bumped whenever the on-disk cache entry shape changes incompatibly.
pub const SCHEMA_VERSION: u32 = 1;

/// Identifies exactly which extraction result a cache entry represents.
/// Two `CacheKey`s that differ in any field must be treated as unrelated
/// entries (spec §11 "Invalidation": "Key mismatch" invalidates).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheKey {
    /// The resolved, canonicalized path to the tool binary, if it was
    /// found on `PATH`. `None` for a tool mantui could not locate (the
    /// cache entry in that case only really carries negative/tier-status
    /// information).
    pub realpath: Option<PathBuf>,
    /// The binary's file size in bytes, at the time of resolution.
    pub size: Option<u64>,
    /// The binary's modification time, in nanoseconds since the Unix
    /// epoch, at the time of resolution.
    pub mtime_ns: Option<i128>,
    /// The binary's inode number (Unix) — catches the case of a file
    /// replaced in place with the same size and a coarse-grained mtime.
    pub inode: Option<u64>,
    /// The tool's own `--version`-style output, when available from an
    /// inert probe already being invoked for another tier. Catches a
    /// package manager swapping a binary without changing its size.
    pub tool_version: Option<String>,
    /// [`SCHEMA_VERSION`] at the time this key was built.
    pub schema_version: u32,
    /// mantui's own crate version (`CARGO_PKG_VERSION`).
    pub mantui_version: String,
    /// Sorted, deduplicated list of enabled extraction feature flags, so
    /// enabling e.g. `manpage` invalidates entries built without it.
    pub enabled_features: Vec<String>,
}

impl CacheKey {
    /// Build a key for `tool_name`, resolved against `PATH`. `tool_version`
    /// is optional and supplied by the caller (the runner), since obtaining
    /// it may itself require an inert subprocess call the cache crate must
    /// not make itself (spec §6: only `mantui-extract/src/exec/` spawns
    /// processes).
    pub fn build(
        tool_name: &str,
        tool_version: Option<String>,
        enabled_features: &[&str],
    ) -> CacheKey {
        let resolved = resolve_on_path(tool_name);
        let (realpath, size, mtime_ns, inode) = match &resolved {
            Some(path) => {
                let meta = std::fs::metadata(path).ok();
                let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
                let size = meta.as_ref().map(|m| m.len());
                let mtime_ns = meta.as_ref().and_then(file_mtime_ns);
                let inode = meta.as_ref().and_then(file_inode);
                (Some(canon), size, mtime_ns, inode)
            }
            None => (None, None, None, None),
        };
        let mut enabled_features: Vec<String> =
            enabled_features.iter().map(|s| s.to_string()).collect();
        enabled_features.sort();
        enabled_features.dedup();
        CacheKey {
            realpath,
            size,
            mtime_ns,
            inode,
            tool_version,
            schema_version: SCHEMA_VERSION,
            mantui_version: env!("CARGO_PKG_VERSION").to_string(),
            enabled_features,
        }
    }

    /// A filesystem-safe file stem to store this tool's cache entry under.
    /// Not part of the key's identity check (that's a full struct
    /// comparison after loading) — just a lookup name.
    pub fn cache_file_stem(tool_name: &str) -> String {
        tool_name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }
}

#[cfg(unix)]
fn file_mtime_ns(meta: &std::fs::Metadata) -> Option<i128> {
    use std::os::unix::fs::MetadataExt;
    Some(meta.mtime() as i128 * 1_000_000_000 + meta.mtime_nsec() as i128)
}

#[cfg(not(unix))]
fn file_mtime_ns(meta: &std::fs::Metadata) -> Option<i128> {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i128)
}

#[cfg(unix)]
fn file_inode(meta: &std::fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(meta.ino())
}

#[cfg(not(unix))]
fn file_inode(_meta: &std::fs::Metadata) -> Option<u64> {
    None
}

fn resolve_on_path(name: &str) -> Option<PathBuf> {
    if name.contains(std::path::MAIN_SEPARATOR) {
        let p = PathBuf::from(name);
        return p.is_file().then_some(p);
    }
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_file_stem_sanitizes_path_like_names() {
        assert_eq!(CacheKey::cache_file_stem("git"), "git");
        assert_eq!(CacheKey::cache_file_stem("./weird/tool"), "__weird_tool");
    }

    #[test]
    fn build_for_unknown_tool_has_no_file_identity() {
        let key = CacheKey::build("definitely-not-a-real-tool-xyz", None, &["known-specs"]);
        assert!(key.realpath.is_none());
        assert!(key.size.is_none());
    }

    #[test]
    fn features_are_sorted_and_deduped() {
        let key = CacheKey::build("definitely-not-a-real-tool-xyz", None, &["b", "a", "a"]);
        assert_eq!(key.enabled_features, vec!["a".to_string(), "b".to_string()]);
    }
}
