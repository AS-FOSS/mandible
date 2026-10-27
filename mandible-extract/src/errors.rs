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
    /// The tool's help probe was stopped at the wall-clock cap, so the page
    /// has no text to read (spec §6 rule 4). Never answered from another
    /// probe's text, because which probe finished in time would then differ
    /// from one launch to the next.
    #[error("the help probe took too long and was stopped")]
    TimedOut,
    /// A catch-all for tier-specific failures not covered above.
    #[error("{0}")]
    Other(String),
}
