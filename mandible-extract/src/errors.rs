//! Error types shared across extraction tiers.

use thiserror::Error;

/// An error from a single tier's [`crate::ExtractionTier::extract_node`]
/// call. Recorded per-node, per-tier; does not invalidate the tier
/// globally. Spec §5.3.
#[derive(Debug, Error)]
pub enum ExtractError {
    /// The tool could not be located on `PATH`.
    #[error("tool not found on PATH")]
    ToolNotFound,
    /// The requested path did not resolve within the extracted tree.
    #[error("path did not resolve within the extracted tree")]
    PathNotFound,
    /// The tier's source data (a catalog entry, help text, a completion
    /// script) failed to parse.
    #[error("failed to parse source data: {0}")]
    ParseFailed(String),
    /// Running the tool under the §6 execution policy failed.
    #[error("execution failed: {0}")]
    Exec(#[from] crate::exec::ExecError),
    /// A catch-all for tier-specific failures not covered above.
    #[error("{0}")]
    Other(String),
}
