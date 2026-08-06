//! `mantui-cache`: the on-disk extraction cache (spec §11).
//!
//! Extraction is too slow to redo on every launch (spec §5.1), so results
//! are cached at `$XDG_CACHE_HOME/mantui/`, one gzip-compressed JSON file
//! per tool, keyed by file identity rather than a content hash (hashing a
//! 50 MB `docker` binary costs more than the parse it protects).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod entry;
mod key;
mod store;

pub use entry::{CacheEntry, CatalogStamp, StoredTierStatus};
pub use key::{CacheKey, SCHEMA_VERSION};
pub use store::{CacheError, Store};
