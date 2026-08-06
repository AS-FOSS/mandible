//! The on-disk shape of one tool's cached extraction result (spec §11
//! "Contents").

use crate::key::CacheKey;
use mantui_core::CommandNode;
use serde::{Deserialize, Serialize};

/// One tier's recorded outcome for a cached extraction, stored alongside
/// the tree so `--doctor` and the `?` overlay can explain a cached result
/// without re-running extraction (spec §5.3, §11).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredTierStatus {
    /// The tier's name, as returned by
    /// [`mantui_extract::ExtractionTier::name`](../mantui_extract/trait.ExtractionTier.html#tymethod.name).
    pub tier: String,
    /// Whether the tier detected the tool as one it could handle.
    pub detected: bool,
    /// `Some(message)` if the tier detected but failed to extract.
    pub error: Option<String>,
}

/// A stamp of which vendored catalog snapshot contributed to a cached
/// entry, so the UI can show "cached 3d ago · from carapace commit
/// 7bb0290" style staleness information (spec §7 "Staleness", §11
/// "Staleness in the UI").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogStamp {
    /// The catalog provider, e.g. `"carapace-spec"`.
    pub provider: String,
    /// The upstream commit the snapshot was generated at.
    pub commit: String,
    /// An RFC 3339 timestamp of when the snapshot was generated.
    pub generated: String,
}

/// One tool's complete cache entry: the (possibly partial, possibly
/// entirely absent) tree, plus enough bookkeeping to explain and safely
/// invalidate it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    /// The key this entry was stored under. Compared against a freshly
    /// built key on load; a mismatch means the entry is stale (spec §11
    /// "Invalidation").
    pub key: CacheKey,
    /// The tool name this entry is for.
    pub tool: String,
    /// The merged tree, or `None` — a **negative** result, meaning no tier
    /// could extract anything for this tool. Caching this matters as much
    /// as caching positive results: otherwise every launch re-probes tiers
    /// that don't apply, which is most of them (spec §11 "Contents").
    pub root: Option<CommandNode>,
    /// Per-tier detect/extract outcomes, for `--doctor` and the `?`
    /// overlay.
    pub tier_statuses: Vec<StoredTierStatus>,
    /// The vendored catalog snapshot's stamp, if catalog data contributed
    /// to this entry.
    pub catalog: Option<CatalogStamp>,
    /// When this entry was written, as Unix seconds — used to render
    /// `cached 3d ago` in the UI footer.
    pub cached_at_unix_secs: i64,
}
