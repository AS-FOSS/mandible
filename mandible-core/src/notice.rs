//! The mandible-authored prose that travels *inside* a raw-help document.
//!
//! `mandible-extract`'s `not_attested_fallback` prepends a refusal notice
//! (and, when the safe root-help fallback succeeds, a label) to the
//! tool-authored `--help` bytes, and `mandible-tui`'s raw-help renderer
//! must tell those two apart: mandible's own prose wraps at the pane
//! width, the tool's preformatted lines never do. Both crates therefore
//! need the exact same spellings — a drifted copy on either side would
//! silently turn wrapping back into clipping (PR #38's bug), with no
//! compile error and no failing test on the side that changed.
//!
//! These constants are that single spelling. Producers format around
//! them; recognizers match against them.

/// Everything before the quoted subcommand name in the unverified-
/// subcommand refusal: `mandible could not verify "<name>" as a real
/// subcommand name: …`. A raw-help body whose first line starts with this
/// is mandible-authored prose, not tool output.
pub const UNVERIFIED_SUBCOMMAND_NOTICE_PREFIX: &str = "mandible could not verify \"";

/// The label between the refusal notice and the tool's own root `--help`
/// bytes when the safe fallback probe succeeded. Always emitted framed by
/// blank lines; also mandible-authored prose.
pub const ROOT_HELP_FALLBACK_LABEL: &str =
    "Showing the tool's own root --help instead, labelled below:";
