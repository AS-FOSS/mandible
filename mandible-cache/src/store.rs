//! Reading and writing cache entries at `$XDG_CACHE_HOME/mantui/` (spec
//! §11 "Format").

use crate::entry::CacheEntry;
use crate::key::CacheKey;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors from cache I/O. Per spec §11, corrupt or unreadable *entries* are
/// handled by deleting and re-extracting (never surfaced as an error to the
/// caller) — `CacheError` is for failures at the store level itself, e.g.
/// the cache directory being unwritable.
#[derive(Debug, Error)]
pub enum CacheError {
    /// Could not determine or create the cache directory.
    #[error("could not resolve or create the cache directory: {0}")]
    Directory(#[source] std::io::Error),
    /// An I/O error writing a cache entry.
    #[error("cache I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The cache entry could not be serialized.
    #[error("failed to serialize cache entry: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// A directory of gzip-compressed, one-file-per-tool JSON cache entries.
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// Open (creating if needed) the standard XDG cache directory for
    /// mantui.
    pub fn open_default() -> Result<Store, CacheError> {
        let proj_dirs = directories::ProjectDirs::from("", "", "mantui").ok_or_else(|| {
            CacheError::Directory(std::io::Error::other(
                "could not determine a home directory to resolve the cache path",
            ))
        })?;
        Store::open(proj_dirs.cache_dir())
    }

    /// Open (creating if needed) an arbitrary cache directory. Used by
    /// `--doctor`/tests to point at a scratch directory, and by
    /// [`Store::open_default`] for the real XDG path.
    pub fn open(dir: impl AsRef<Path>) -> Result<Store, CacheError> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir).map_err(CacheError::Directory)?;
        Ok(Store { dir })
    }

    /// The directory this store reads/writes.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn entry_path(&self, tool: &str) -> PathBuf {
        self.dir
            .join(format!("{}.json.gz", CacheKey::cache_file_stem(tool)))
    }

    /// Load a cache entry for `tool`, returning `None` if there is no
    /// entry, the entry is corrupt (in which case it is deleted), or the
    /// stored key doesn't match `expected_key` (a stale entry — left in
    /// place; a subsequent [`Store::store`] will overwrite it).
    pub fn load(&self, tool: &str, expected_key: &CacheKey) -> Option<CacheEntry> {
        let path = self.entry_path(tool);
        let bytes = std::fs::read(&path).ok()?;

        let entry = match decode_entry(&bytes) {
            Ok(entry) => entry,
            Err(_) => {
                // Corrupt cache entry: delete and treat as a miss, never
                // propagate as an error (spec §11 "Invalidation").
                let _ = std::fs::remove_file(&path);
                return None;
            }
        };

        if &entry.key != expected_key {
            return None;
        }
        Some(entry)
    }

    /// Write (or overwrite) `entry`'s cache file.
    pub fn store(&self, entry: &CacheEntry) -> Result<(), CacheError> {
        let path = self.entry_path(&entry.tool);
        let bytes = encode_entry(entry)?;
        // Write-then-rename for a reasonably atomic update: a reader never
        // observes a half-written file.
        let tmp_path = path.with_extension("json.gz.tmp");
        std::fs::write(&tmp_path, &bytes)?;
        std::fs::rename(&tmp_path, &path)?;
        Ok(())
    }

    /// Delete a tool's cache entry, if present. Used by `mantui --refresh`
    /// and the `r` key (spec §11 "Invalidation"). Not an error if the entry
    /// didn't exist.
    pub fn invalidate(&self, tool: &str) -> Result<(), CacheError> {
        let path = self.entry_path(tool);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

fn decode_entry(bytes: &[u8]) -> Result<CacheEntry, CacheError> {
    let mut decoder = GzDecoder::new(bytes);
    let mut json = String::new();
    decoder.read_to_string(&mut json)?;
    let entry = serde_json::from_str(&json)?;
    Ok(entry)
}

fn encode_entry(entry: &CacheEntry) -> Result<Vec<u8>, CacheError> {
    let json = serde_json::to_string(entry)?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(json.as_bytes())?;
    Ok(encoder.finish()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::CacheEntry;
    use mantui_core::{CommandNode, Provenance, Source};

    fn sample_key() -> CacheKey {
        CacheKey::build(
            "definitely-not-a-real-tool-xyz",
            None,
            &["known-specs"],
            None,
        )
    }

    fn sample_entry(key: CacheKey, root: Option<CommandNode>) -> CacheEntry {
        CacheEntry {
            key,
            tool: "definitely-not-a-real-tool-xyz".to_string(),
            root,
            tier_statuses: vec![],
            catalog: None,
            cached_at_unix_secs: 0,
        }
    }

    #[test]
    fn round_trips_a_positive_entry() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let key = sample_key();
        let node = CommandNode::new("xyz", Provenance::single(Source::HelpText));
        let entry = sample_entry(key.clone(), Some(node));
        store.store(&entry).unwrap();
        let loaded = store.load("definitely-not-a-real-tool-xyz", &key).unwrap();
        assert_eq!(loaded.root.unwrap().name, "xyz");
    }

    #[test]
    fn round_trips_a_negative_entry() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let key = sample_key();
        let entry = sample_entry(key.clone(), None);
        store.store(&entry).unwrap();
        let loaded = store.load("definitely-not-a-real-tool-xyz", &key).unwrap();
        assert!(loaded.root.is_none());
    }

    #[test]
    fn key_mismatch_is_a_miss_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let key = sample_key();
        let entry = sample_entry(key.clone(), None);
        store.store(&entry).unwrap();

        let mut different_key = key.clone();
        different_key.schema_version += 1;
        assert!(store
            .load("definitely-not-a-real-tool-xyz", &different_key)
            .is_none());
    }

    #[test]
    fn corrupt_entry_is_deleted_and_treated_as_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let key = sample_key();
        let path = store.entry_path("definitely-not-a-real-tool-xyz");
        std::fs::write(&path, b"not a valid gzip stream at all").unwrap();
        assert!(store.load("definitely-not-a-real-tool-xyz", &key).is_none());
        assert!(!path.exists(), "corrupt entry should have been deleted");
    }

    #[test]
    fn invalidate_removes_entry() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let key = sample_key();
        let entry = sample_entry(key.clone(), None);
        store.store(&entry).unwrap();
        store.invalidate("definitely-not-a-real-tool-xyz").unwrap();
        assert!(store.load("definitely-not-a-real-tool-xyz", &key).is_none());
    }

    #[test]
    fn invalidate_missing_entry_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        assert!(store.invalidate("never-cached").is_ok());
    }
}
