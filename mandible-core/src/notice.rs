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

/// The notice on a loaded command page that holds nothing past its
/// summary: the command printed no help mandible could read, and `t`
/// shows what it did print.
pub const NO_FURTHER_HELP_NOTICE: &str =
    "No further help was read for this command. Press t to see what it printed.";

/// The notice on a command page whose help probe was stopped at the
/// wall-clock cap, so the page has nothing to show. The seconds are
/// `mandible_extract::exec::EXTRACT_LIMIT`; a test in `mandible-tui` holds
/// the two together.
pub const TIMED_OUT_NOTICE: &str =
    "Reading this command's help took longer than 30 seconds, so it was stopped. Press r to try again with more time.";

/// The line a command shows while its help is being read for the first
/// time, in the parsed view and in the verbatim view.
pub const READING_NOTICE: &str = "Reading this command's help…";

/// The notice a same-as-ancestor node's detail pane shows in place of its
/// usage/children/flags (spec [M-19], docs/design.md §16's ruling): this
/// command's own `--help` fingerprinted as byte-identical to an ancestor's,
/// so there is nothing further to parse here that isn't already shown one
/// level up — and `t` still fetches this node's own live text on demand.
/// One place, so a test can assert it without a string literal duplicated
/// in the pane and in the test.
pub const SAME_AS_ANCESTOR_NOTICE: &str =
    "This command prints the same help as its parent. Press t to see it.";
