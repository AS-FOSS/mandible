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
//! These constants and functions are that single spelling. Producers build
//! the text with them; recognizers match with them.

/// Between the quoted name and the tool's name in the unverified-subcommand
/// refusal: `"<name>" was not run: mandible only runs names that <tool>'s
/// help lists under a heading.`
const UNVERIFIED_MIDDLE: &str = "\" was not run: mandible only runs names that ";

/// What follows the tool's name in the unverified-subcommand refusal.
const UNVERIFIED_END: &str = "'s help lists under a heading.";

/// The refusal for a name mandible will not run: `name` is the attempted
/// words joined by spaces, `tool` the tool's name.
pub fn unverified_subcommand_notice(name: &str, tool: &str) -> String {
    format!("\"{name}{UNVERIFIED_MIDDLE}{tool}{UNVERIFIED_END}")
}

/// Whether `line` is the refusal [`unverified_subcommand_notice`] builds.
/// A raw-help body whose first line is one is mandible-authored prose, not
/// tool output.
pub fn is_unverified_subcommand_notice(line: &str) -> bool {
    line.starts_with('"') && line.contains(UNVERIFIED_MIDDLE) && line.ends_with(UNVERIFIED_END)
}

/// The line between the refusal and the tool's own root help bytes when
/// the safe fallback probe succeeded: `Showing <argv> instead:`. Always
/// emitted framed by blank lines; also mandible-authored prose.
pub fn root_help_fallback_label(argv: &str) -> String {
    format!("Showing {argv} instead:")
}

/// Whether `line` is the label [`root_help_fallback_label`] builds.
pub fn is_root_help_fallback_label(line: &str) -> bool {
    line.starts_with("Showing ") && line.ends_with(" instead:")
}

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

/// What the commands pane says when a search finished and nothing matched.
/// The query is quoted after [`NO_MATCHES_NOTICE_PREFIX`].
pub const NO_MATCHES_NOTICE_PREFIX: &str = "No matches for \"";

/// The whole no-match line for `query`: `No matches for "<query>"`.
pub fn no_matches_notice(query: &str) -> String {
    format!("{NO_MATCHES_NOTICE_PREFIX}{query}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_refusal_and_its_label_are_recognised_by_the_functions_that_build_them() {
        let notice = unverified_subcommand_notice("stash drop", "git");
        assert_eq!(
            notice,
            "\"stash drop\" was not run: mandible only runs names that git's help lists under a heading."
        );
        assert!(is_unverified_subcommand_notice(&notice));
        assert!(!is_unverified_subcommand_notice("usage: git [options]"));
        let label = root_help_fallback_label("git --help");
        assert_eq!(label, "Showing git --help instead:");
        assert!(is_root_help_fallback_label(&label));
        assert!(!is_root_help_fallback_label("Showing the options"));
    }
}
