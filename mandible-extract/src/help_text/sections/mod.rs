//! Layout-driven parsing of `--help` output: the `Usage:` block, and
//! indentation-delimited sections (`Options:`, `Flags:`, git's headingless
//! command groups, tar's "Main operation mode", ...).
//!
//! This is deliberately *not* keyed on specific heading text — headings
//! vary too much across tools (`Options:` vs `Flags:` vs a full sentence
//! with no trailing colon, as git uses) for that to generalize. Instead a
//! block is recognized purely by layout: a column-0 line followed
//! (possibly after blank lines) by more-indented lines, running until the
//! next column-0 line. Within a block, whether entries are flags or
//! subcommands is decided by whether entry lines start with `-` — content
//! shape, not the heading's wording, which is what keeps this general
//! rather than a per-tool special case (spec §1).
//!
//! **Never invent subcommands (spec §7 Tier B, [M-10]).** An earlier
//! version of this parser reported `tar` as having 39 subcommands — none
//! real. They were wrapped description continuation lines (*"treat them as
//! errors"*) and `--format=`'s enum values (`gnu`, `oldgnu`, `pax`, ...)
//! misread as bare-word command entries. Four binding rules now gate every
//! bare-word block before it is allowed to become subcommands:
//!
//! 1. The block's heading must be *recognized* — it mentions "command(s)"
//!    or "subcommand(s)" as a whole word, or the block is part of a
//!    contiguous run that started under such a heading (git's group
//!    headings, e.g. `"start a working area (see also: ...)"`, never say
//!    "command" themselves, but git's leading blurb *does*: `"These are
//!    common Git commands used in various situations:"`). Layout alone —
//!    "here is a heading, and indented content follows" — is never
//!    sufficient evidence a block is subcommands rather than, say, an enum
//!    value list.
//! 2. A line at the description column with nothing at the name column is
//!    a **continuation** of the previous row, never a new row.
//! 3. A candidate name must look like one: `^[a-z][a-z0-9_.-]*$`. Entries
//!    failing this (e.g. *"treat them as errors"*) are dropped, not
//!    emitted — fabricated structure is strictly worse than missing
//!    structure, because a user cannot tell it is wrong.
//! 4. An unrecognized bare-word block nested under a flag (its heading
//!    names that flag, e.g. `"Valid arguments for the --quoting-style
//!    option are:"`, or it immediately follows the flag with no other
//!    heading between) becomes that flag's [`mandible_core::Entity::choices`],
//!    not subcommands. If no owning flag can be identified either, the
//!    block is dropped rather than guessed at.

use super::grammar::{
    bracket_flag_row_content, is_bare_flag_spelling, is_bare_flag_token,
    looks_like_bracket_flag_row, looks_like_flag_start, looks_like_paren_alternation_open,
    looks_like_stanza_head_flag, paren_alternation_member_content, paren_depth_delta,
    parse_bundled_shorts, parse_flag_alternation, parse_flag_spec, split_alternatives, FlagSpec,
};
use super::profile::{heading_matches_markers, FrameworkProfile};
use mandible_core::{
    is_command_name_shaped, strip_escapes, CommandNode, Entity, Provenance, Source, Spelling, Text,
    ValueKind,
};

mod emit;
mod entry;
mod flag_rows;
mod heading;
mod layout;
mod preamble;
mod scan;
mod spelling;
#[cfg(test)]
mod test_support;
mod usage;

pub use emit::*;
pub use entry::*;
use flag_rows::*;
pub use heading::*;
pub use layout::*;
use preamble::*;
use scan::*;
use spelling::*;
#[cfg(test)]
use test_support::*;
pub use usage::*;

/// Hard cap on distinct entries (subcommands, flags, or choices) accepted
/// from a single probe's output. Real `--help` output never remotely
/// approaches this — `git` has ~170 top-level subcommands, `tar` 171
/// flags. It exists as a general defense against a degenerate input: the
/// coverage harness (spec §13.1) found a tool (`instmodsh`, a Perl REPL
/// that ignores `--help` entirely and free-runs printing its own 3-line
/// banner until the wall-clock cap killed it) whose 8 MiB of captured,
/// almost entirely repeated output parsed into 58,663 "subcommands" —
/// mostly duplicate names. (The same investigation also found and fixed a
/// real quadratic-time bug in this file's leading-prose scan, which was
/// the larger share of that tool's multi-minute parse time; this cap is a
/// second, independent line of defense — against the downstream bucket-
/// merge cost of tens of thousands of same-named nodes, and against a
/// hypothetical input that's huge but *not* quadratic-scan-triggering.)
/// Capping (and deduplicating) at the point of recovery, rather than
/// trying to bound cost after the fact, is what keeps one pathological
/// tool from making the whole pipeline slow.
const MAX_RECOVERED_ENTRIES: usize = 4096;

/// Everything recovered from one `--help` invocation's output.
#[derive(Debug, Default)]
pub struct ParsedHelp {
    /// Leading prose before the `Usage:` line or the first section,
    /// if any (e.g. tar's "GNU 'tar' saves many files together...").
    pub description: Option<String>,
    /// The `Usage:` line(s), verbatim (joined).
    pub usage: Vec<String>,
    /// Positional placeholders pulled out of the usage line
    /// (`<value>`/`FILE`-shaped tokens not preceded by `-`).
    pub positionals: Vec<Entity>,
    /// Flags recovered from dash-led blocks.
    pub flags: Vec<Entity>,
    /// Subcommand stubs recovered from bare-word blocks under a
    /// recognized command heading (not yet extracted themselves —
    /// `children_filled: false`).
    pub subcommands: Vec<CommandNode>,
    /// Fraction of recognized entry lines the grammar fully understood,
    /// in `[0.0, 1.0]`.
    pub confidence: f32,
    /// True if at least one block yielded a name failing the candidate
    /// regex, or was dropped for lack of an owning heading/flag. Surfaced
    /// so `extract_node` can mark the node's provenance as a guess.
    pub saw_unattributable_content: bool,
    /// Names already accepted into `subcommands`, tracked alongside it so
    /// [`ParsedHelp::try_push_subcommand`] can reject duplicates in O(1)
    /// instead of an O(n) scan of `subcommands` per candidate (which would
    /// itself become O(n^2) on the exact degenerate input this whole cap
    /// exists for).
    subcommand_names_seen: std::collections::HashSet<String>,
}

impl ParsedHelp {
    /// Accept `node` into `subcommands` unless it's a duplicate name (an
    /// entry recovered twice, e.g. because a heading's block repeats
    /// verbatim in genuinely broken output) or the recovery cap has
    /// already been hit. Returns whether it was accepted.
    fn try_push_subcommand(&mut self, node: CommandNode) -> bool {
        if self.subcommands.len() >= MAX_RECOVERED_ENTRIES {
            return false;
        }
        if !self.subcommand_names_seen.insert(node.name.clone()) {
            return false;
        }
        self.subcommands.push(node);
        true
    }
}

/// Section headings that introduce worked examples or prose, not
/// structure — a general (not tool-specific) exclusion, since "Examples:"
/// sections showing up as fake subcommands is a real failure mode (e.g.
/// tar's `Examples:` block contains lines starting with the bare word
/// `tar`, which would otherwise look exactly like a subcommand entry).
fn is_ignorable_heading(heading: &str) -> bool {
    // Note: deliberately *not* matching "see also" — git's own command
    // group headings legitimately contain that phrase as a parenthetical
    // aside (`"start a working area (see also: git help tutorial)"`), and
    // an early version of this filter dropped every such group entirely.
    let lower = heading.to_lowercase();
    lower.starts_with("example") || lower.contains("report bugs")
}

/// True when `heading` positively names a section whose rows describe CLI
/// flags.  This vocabulary is used only to leave an otherwise-contained
/// examples/reporting region at the *same* indentation; ordinary section
/// parsing remains shape-driven and does not require these words.
///
/// Same-indent text inside a worked example is inherently ambiguous.  A
/// label such as `Input:` can govern `--flag`-shaped sample data just as an
/// `Options:` heading governs real flags.  Requiring both this explicit CLI
/// vocabulary and a real flag-block shape below the heading is the positive
/// evidence that lets the containment boundary reopen without treating the
/// first generic `X:` label as structural documentation.
fn names_flag_section(heading: &str) -> bool {
    if !is_section_heading_line(heading) {
        return false;
    }
    let lower = heading.to_lowercase();
    if lower.contains("example") {
        return false;
    }
    lower.split(|c: char| !c.is_alphanumeric()).any(|word| {
        matches!(
            word,
            "option"
                | "options"
                | "flag"
                | "flags"
                | "switch"
                | "switches"
                | "modifier"
                | "modifiers"
                | "mode"
                | "modes"
                | "operation"
                | "operations"
                | "argument"
                | "arguments"
        )
    })
}

/// Whether the line at `heading_idx` is strong enough to end a hidden
/// ignorable region without a dedent.  The heading wording alone is not
/// enough: its following nonblank content must be more indented and must
/// independently satisfy the existing bounded flag-block recognizer.
fn starts_attested_flag_section(lines: &[&str], heading_idx: usize) -> bool {
    const MIN_ATTESTED_SECTION_FLAGS: usize = 2;

    let heading = lines[heading_idx];
    if !names_flag_section(heading.trim()) {
        return false;
    }
    let heading_indent = leading_whitespace(heading);
    let mut content_idx = heading_idx + 1;
    while content_idx < lines.len() && lines[content_idx].trim().is_empty() {
        content_idx += 1;
    }
    if content_idx >= lines.len() || leading_whitespace(lines[content_idx]) <= heading_indent {
        return false;
    }
    let Some(flags_start) = flags_block_start(lines, content_idx) else {
        return false;
    };
    // One flag-shaped sample row is still cheap to produce inside a worked
    // example.  A run of at least two independently parsed rows, combined
    // with the explicit heading vocabulary above, is the minimum evidence
    // that may reopen a same-indent section.  A physical dedent remains the
    // lossless exit for genuine one-row sections.
    let (_, entries, _) = scan_flags_block(lines, flags_start, false);
    entries.len() >= MIN_ATTESTED_SECTION_FLAGS
}

/// True if `heading` mentions "command"/"commands"/"subcommand"/
/// "subcommands" as a whole word (case-insensitive) — spec §7 Tier B rule
/// 1's literal recognized-heading test. Matches `"Commands:"`,
/// `"Available Commands:"`, `"SUBCOMMANDS"`, and prose like `"These are
/// common Git commands used in various situations:"`.
fn mentions_commands_word(s: &str) -> bool {
    s.split(|c: char| !c.is_alphanumeric())
        .map(|w| w.to_lowercase())
        .any(|w| {
            matches!(
                w.as_str(),
                "command" | "commands" | "subcommand" | "subcommands"
            )
        })
}

// Rule 3's name-shape test (`^[a-z][a-z0-9_.-]*$`) lives in
// `mandible_core::is_command_name_shaped` (imported above) — it's also half
// of the coverage harness's structure-sanity check (spec §13.1), so there
// is exactly one definition of "looks like a name, not a fabricated
// fragment" rather than two that could drift apart.

/// Parse raw `--help` text (already selected as stdout-or-stderr by the
/// caller) into structured pieces, with no framework knowledge — the
/// generic layout engine alone (spec §7 Tier B step 2, "unidentified").
/// Equivalent to `parse_with_profile(raw, None, None)`. `#[cfg(test)]`: the
/// one production caller (`help_text::build_node`) always has a definite
/// answer to "was a framework identified?" (and always knows the tool's own
/// name) and calls [`parse_with_profile`] directly; this zero-argument
/// spelling exists only because most of this module's own
/// (pre-batch-6-part-4) test suite below calls it, and its behavior must
/// stay exactly what it always was.
#[cfg(test)]
pub fn parse(raw: &str) -> ParsedHelp {
    parse_with_profile(raw, None, None)
}

/// [`parse`], but naming the tool whose `--help` this is — see
/// [`parse_with_profile`]'s `tool_name` parameter. `#[cfg(test)]` for the
/// same reason as [`parse`]: the production caller always passes a real
/// name directly to `parse_with_profile`.
#[cfg(test)]
fn parse_named(raw: &str, tool_name: &str) -> ParsedHelp {
    parse_with_profile(raw, None, Some(tool_name))
}

/// Same engine as [`parse`], but consulting `profile`'s framework-specific
/// heading vocabulary and subcommand-concept knowledge when present (spec
/// §7 Tier B step 1, "framework identified"). `None` reproduces [`parse`]'s
/// generic behavior exactly — this is what keeps the two degradation
/// levels (spec §7 Tier B: identified vs. unidentified) sharing one engine
/// instead of forking into two.
///
/// `tool_name` is the probed tool's own root name (`ResolvedTool::name`,
/// e.g. `"git"` for both `git --help` and `git rebase --help`) when known.
/// It feeds the usage-block scanner's "starts a new entry" test alongside
/// the `usage:`/`or:` markers — see that block's own comment for why
/// indentation alone cannot carry this weight. `None` is always safe: it
/// only makes the name-based half of that test inert, never wrong (the
/// marker- and content-shape-based halves are unaffected).
pub fn parse_with_profile(
    raw: &str,
    profile: Option<&FrameworkProfile>,
    tool_name: Option<&str>,
) -> ParsedHelp {
    // Strip terminal escape sequences over the *whole* document, once,
    // before any layout analysis runs — never only per-field at
    // `Text::sanitize` emission time, which is too late: headings,
    // indentation and column gaps are all measured on this raw string,
    // and an escape sequence still embedded in it corrupts every one of
    // those measurements, not just what a user eventually reads.
    // `systemd-creds --help` is the specimen this was measured against:
    // it writes `[0mCommands:` (a colorizing library's reset code
    // glued directly onto its own heading), and left in place that
    // defeats `mentions_commands_word` — the escape and the heading's
    // first two characters fuse into one alphanumeric run, `0mCommands`,
    // which matches no recognized heading word, so the tool's real
    // `Commands:` block (six real subcommands: `list`, `cat`, `setup`,
    // `encrypt`, `decrypt`, `has-tpm2`) was never recognized as
    // introducing a command list. Reuses `mandible_core::strip_escapes`
    // rather than a second copy of the same state machine — see that
    // function's own doc comment.
    let raw = strip_escapes(raw);
    // A heading that shares its physical line with the first row of its
    // own table is rewritten into the two lines it means before the
    // engine below ever sees it — see `split_shared_heading_rows`. Doing
    // it here, once, rather than inside the section loop is what keeps
    // the recovered row subject to every *block-level* decision
    // (`block_is_multi_column`, `block_has_aligned_spelling_column`)
    // alongside the rows beneath it; a row bolted on afterwards would
    // have been parsed under decisions taken without it.
    //
    // Structurally non-recursive: the rewrite is applied at most once,
    // and `parse_body` never calls back into this function.
    match split_shared_heading_rows(&raw) {
        Some((rewritten, bnf_row_lines)) => {
            parse_body(&rewritten, profile, tool_name, &bnf_row_lines)
        }
        None => parse_body(&raw, profile, tool_name, &std::collections::HashSet::new()),
    }
}

/// [`parse_with_profile`]'s engine, over text whose shared heading rows
/// have already been split out. `bnf_row_lines` is that split's own
/// record of which *row* lines (by index into `raw`'s own lines) came from
/// a `:=` BNF production rather than an ordinary column-gap heading — see
/// [`split_shared_heading_rows`]'s doc comment for why this is the only
/// place that fact can still be read from, and why it is keyed on the row
/// rather than the heading beside it.
fn parse_body(
    raw: &str,
    profile: Option<&FrameworkProfile>,
    tool_name: Option<&str>,
    bnf_row_lines: &std::collections::HashSet<usize>,
) -> ParsedHelp {
    let lines: Vec<&str> = raw.lines().collect();
    let mut result = ParsedHelp::default();

    // Some tools answer `--help` with their *man page* rather than a help
    // summary — `git bisect --help` renders GIT-BISECT(1) in full. That is
    // a different document format with different conventions, and feeding
    // it to this grammar produces nonsense: git bisect acquired the
    // subcommands "follows", "testing.", "command" and "skipped." from
    // sentences in the DESCRIPTION prose. Man pages are Tier D's job
    // (spec §7 Tier D, not yet implemented), so until that exists the
    // honest outcome is no structure at all, which the caller renders
    // verbatim (spec §7 Tier B step 3) — the author's own manual, shown
    // as written, instead of invented commands.
    if looks_like_man_page(&lines) {
        return result;
    }

    let mut i = 0;
    // Physical usage lines (one string per source line, pre-join), kept
    // alive past the block below so the deferred `extract_usage_flags`
    // call further down can read the same per-line shape `extract_positionals`
    // does — see that block's own comment for why.
    let mut usage_lines: Vec<String> = Vec::new();
    // 1. Usage block: one or more *logical* entries — each a `usage:` /
    // `or:` / own-name line plus whatever continues it — collected from
    // the physical lines starting at the first (case-insensitive)
    // "usage:" line.
    //
    // `usage_lines` stays one string per *physical* source line: it feeds
    // `extract_positionals` and (later, at the `extract_usage_flags` call
    // below) the [M-15] synopsis flag grammar, exactly as before this
    // change — neither is touched by the grouping introduced here, so
    // their output (18 root flags and positionals `command`/`args` for
    // `git`) is unaffected by it.
    //
    // `usage_entries` is the display/verbatim form (`result.usage`), one
    // string per logical invocation, and this is where the join happens.
    //
    // A line **starts a new entry**, regardless of indentation, when it is
    // itself a `usage:`/`Usage:` line (some tools repeat the label per
    // form), starts with the GNU coreutils `or:` marker (case-insensitive:
    // `du`'s `  or:  du [OPTION]... --files0-from=F`), or begins with the
    // tool's own name at a word boundary (`tool_name`, when known — a tool
    // that lists alternative forms *without* any marker by literally
    // repeating itself, `prog foo` / `prog bar`, must still read as two
    // entries). Anything else is a **continuation** of the entry above it —
    // *unless* it ends the block entirely; see below.
    //
    // Indentation alone decided "continuation vs. block end" before this
    // comment was rewritten, and it is not sufficient: `git`'s wrapped
    // synopsis continuations sit *more* indented than `usage:` (column 0
    // vs. 11), but `lsof`'s sit at *exactly the same* indent as its own
    // `usage:` marker (both column 1) —
    //
    // ```text
    //  usage: [-?abhKlnNoOPRtUvVX] [+|-c c] ...
    //  [-F [f]] [-g [s]] [-i [i]] ...
    // ```
    //
    // — so the old `leading_whitespace(l) <= base_indent` test read every
    // one of lsof's continuation lines as the block already having ended,
    // silently dropping them (and the six flags documented only in them,
    // none elsewhere in lsof's own two-column options table) before they
    // ever reached `usage_lines`/`extract_usage_flags`. Simply loosening
    // the indentation test doesn't work either: `du`'s block ends with an
    // ordinary prose sentence ("Summarize device usage of the set of
    // FILEs...") at that *same* column-0-or-less position, immediately
    // after the `or:` line, with no blank separator — a line that is not a
    // marker, does not start with `du`, and must still end the block.
    // Indentation genuinely cannot tell these two same-indent cases apart;
    // only content shape can:
    //
    // - **More indented than the block's base indent**: always a
    //   continuation (git's hanging-indent wrap). Indentation *is*
    //   sufficient signal here, so no further test is applied.
    // - **At or below the base indent, and not a marker/own-name line**:
    //   a continuation only if it still *reads like more usage grammar* —
    //   opens with one of the docopt-style group delimiters spec §7 names
    //   (`[`, `<`, `{` — "`[OPTIONS]`, `<required>`, `{a|b|c}`"), as every
    //   one of lsof's continuation fragments does. Anything else (a
    //   sentence of prose, a two-column flag row) ends the block: `du`'s
    //   trailing sentence starts with a capital word, not a delimiter, and
    //   a continuation line that itself reads as a flag *row* (checked
    //   first, below) ends the block the same way it always has.
    //
    // Joined fragments are separated by a single space. This is not
    // re-flowing (spec §7: usage is "kept verbatim, not re-flowed") — each
    // fragment's own text is untouched, byte for byte; only the join
    // character between fragments is chosen, and a single space is what
    // the wrap itself removed by breaking the line there.
    // The usage block's entry point recognizes two labelled shapes and
    // one unlabelled one, tried in this order:
    //
    // 1. An ordinary `usage:`/`Usage:` line, anywhere in the document —
    //    unchanged from before this comment existed.
    // 2. The C `fprintf(stderr, "%s: Usage: ...", argv[0])` idiom: the
    //    tool's own name, a literal `": "`, then `usage:` — `nfsidmap`'s
    //    `nfsidmap: Usage: nfsidmap [-vh] ...`. `starts_with_usage_prefix`
    //    tests the line's *start*, so this shape is otherwise invisible to
    //    it; see `starts_with_name_prefixed_usage`'s own doc comment for
    //    why the match stays this tight (no scanning for `usage:` anywhere
    //    inside a line).
    // 3. Only when *neither* of the above appears anywhere in the
    //    document — a tool with a real `Usage:` line is completely
    //    unaffected by this arm — an **unlabelled synopsis**: a line that
    //    opens with the tool's own name and reads as usage grammar rather
    //    than prose (`looks_like_unlabeled_synopsis_line`'s own doc
    //    comment has the two-part evidence test and why a name match
    //    alone is not enough). Bounded to the lines before the document's
    //    real body starts (its first flag row or section heading) so this
    //    can only ever find a synopsis sitting where one actually belongs,
    //    never something that merely happens to open with the tool's name
    //    deep in the document.
    let labelled_usage_start = lines.iter().position(|l| {
        let t = l.trim_start();
        starts_with_usage_prefix(t)
            || tool_name.is_some_and(|name| starts_with_name_prefixed_usage(t, name))
    });
    let unlabelled_synopsis_start = if labelled_usage_start.is_none() {
        tool_name.and_then(|name| {
            let body_start = lines
                .iter()
                .position(|l| {
                    let t = l.trim_start();
                    !t.is_empty() && (looks_like_flag_start(t) || is_section_heading_line(t))
                })
                .unwrap_or(lines.len());
            lines[..body_start].iter().enumerate().position(|(idx, l)| {
                let t = l.trim_start();
                looks_like_unlabeled_synopsis_line(t, name)
                    // LVM's own emitter (`vgck`, `vgextend`, `vgrename`)
                    // writes a *bare* invocation line — `vgck` alone, or
                    // `vgextend VG PV ...` with no bracket notation at
                    // all on the head line itself — and puts every bit of
                    // docopt notation on the rows that continue it:
                    //
                    // ```text
                    //   vgck
                    //   \t[    --reportformat basic|json ]
                    //   \t[ COMMON_OPTIONS ]
                    // ```
                    //
                    // `looks_like_unlabeled_synopsis_line` alone can never
                    // find this: its whole test is notation evidence *on
                    // this line*, and this line has none. So a bare
                    // own-name line is accepted too, but only when the
                    // very next physical line is unambiguous flag-row
                    // evidence ([`looks_like_bracket_flag_row`]) — a
                    // narrow, structural signal (never "is this LVM") that
                    // a name-only line's continuation really is usage
                    // grammar and not, say, a one-word section title.
                    || looks_like_bare_synopsis_head(&lines, idx, name)
            })
        })
    } else {
        None
    };
    let usage_start = labelled_usage_start.or(unlabelled_synopsis_start);
    if let Some(start) = usage_start {
        i = start;
        let base_indent = leading_whitespace(lines[i]);
        usage_lines.push(lines[i].trim().to_string());
        let mut usage_entries = vec![lines[i].trim().to_string()];
        // Parallel to `usage_lines`: which `usage_entries` index each
        // physical line ended up folded into — a wrapped entry (`sg_
        // sanitize`'s five-line synopsis) spans several physical lines but
        // is one entry, and [`primary_synopsis_lines`]'s refinement in
        // `extract_positionals` needs every one of them, not just the
        // first, to recognize the whole thing as the primary line.
        let mut line_entry_index = vec![0usize];
        // Running depth of an open parenthesized alternation group (LVM's
        // "for options listed in parentheses, any one is required"
        // convention — `vgchange`'s own first stanza), tracked only for an
        // unlabelled synopsis (`labelled_usage_start.is_none()`, matching
        // every other LVM-shape guard in this loop): a member row routinely
        // opens with `-` itself, which the "a continuation line that reads
        // as a flag entry ends the block" check just below would otherwise
        // end the block on. See `grammar::looks_like_paren_alternation_open`
        // and `grammar::paren_depth_delta`'s own doc comments for why depth,
        // not per-line content, is what has to decide this.
        let mut paren_group_depth: i32 = 0;
        // True for exactly the one loop iteration right after
        // `paren_group_depth` returns to zero — consulted only by the blank-
        // line handler just below, which needs to tell "a blank line right
        // after the group's own closing `)`" (still the *same* stanza's own
        // trailing bracket-row flag list, `vgchange`'s own `[ -A|--autobackup
        // y|n ]` and its siblings) apart from an ordinary between-stanza
        // blank line (which requires `looks_like_stanza_continuation_head`
        // evidence instead, below). Reset unconditionally at the bottom of
        // every other line's handling so it cannot survive past the one
        // blank line it exists for.
        let mut just_closed_paren_group = false;
        i += 1;
        while i < lines.len() {
            let l = lines[i];
            if l.trim().is_empty() {
                if paren_group_depth > 0 {
                    // A blank line inside an unclosed group is not a shape
                    // any real specimen produces; refuse to guess at what it
                    // means rather than fabricate a continuation across it.
                    paren_group_depth = 0;
                    just_closed_paren_group = false;
                }
                if just_closed_paren_group {
                    just_closed_paren_group = false;
                    if let Some(next) = lines.get(i + 1) {
                        let t = next.trim_start();
                        if !t.is_empty() && looks_like_bracket_flag_row(t) {
                            // The group's own trailing bracket-row flag list
                            // continues after exactly one blank line —
                            // `vgchange`'s first stanza reads `( ... )` then
                            // a blank line then `[ -A|--autobackup y|n ]`,
                            // still the *same* stanza, not a new one, so
                            // this is deliberately not the
                            // `looks_like_stanza_continuation_head` check
                            // below (that one is for a fresh `<tool> ...`
                            // head line, never a bracket row on its own).
                            i += 1;
                            continue;
                        }
                    }
                }
                // Some tools write their unlabelled synopsis as **one
                // stanza per operation mode / invocation form** — a prose
                // description line, then its own `<tool> <args>` head, then
                // the form's own continuation rows — with a blank line
                // *between* stanzas. LVM's own emitter (`vgck`, `vgchange`,
                // `lvconvert`, the whole `lv*`/`vg*`/`pv*` family) is the
                // specimen this fix was measured against; `adduser` and
                // `pydoc3` hit the identical shape with their own
                // completely unrelated help formatters, which is why the
                // predicates below key on structure, never a tool's name.
                // `vgck`'s own two stanzas:
                //
                // ```text
                //   Read and display information about a VG.
                //   vgck
                //   \t[ --reportformat basic|json ]
                //   \t[ COMMON_OPTIONS ]
                //
                //   Rewrite VG metadata to correct problems.
                //   vgck --updatemetadata VG
                //   \t[ COMMON_OPTIONS ]
                // ```
                //
                // A blank line ended the usage block unconditionally here
                // before this fix, so only the first stanza was ever read —
                // `vgck --updatemetadata` (a *flag*, not a subcommand) was
                // completely absent from the tree, and `lvconvert` alone
                // hides 26 more stanzas the same way.
                //
                // This is deliberately **not** "any blank line continues
                // the block" (that would let it swallow an unrelated
                // trailing paragraph or reopen on a coincidental later
                // own-name mention — the exact fabrication spec §7 [M-10]
                // forbids). It fires only for the unlabelled-synopsis entry
                // point (`labelled_usage_start.is_none()` — a tool with a
                // real `Usage:` line is completely unaffected), and only
                // when the very next non-consumed line is itself unambiguous
                // synopsis-head evidence: `looks_like_unlabeled_synopsis_line`
                // (notation on the line itself) or
                // [`looks_like_stanza_continuation_head`] (a bare
                // own-name line carrying its own flag token, or whose next
                // line is unambiguous flag-row evidence) — see that
                // function's own doc comment for why it is a separate,
                // slightly wider test than the one that opens the block in
                // the first place ([`looks_like_bare_synopsis_head`]).
                // At most one line in between may be skipped, and only when
                // it reads as a full English sentence ([`is_prose_sentence`])
                // rather than more notation — the stanza's own description
                // line, which is consumed here and must land in neither the
                // synopsis nor the tool's description (the first stanza's
                // prose remains the sole description candidate). Anything
                // else — a section heading (`Common options for lvm:`), a
                // flag row, an unrelated paragraph — fails this narrow
                // lookahead and falls through to the ordinary `break`
                // below, ending the block exactly as before this fix.
                if labelled_usage_start.is_none() {
                    if let Some(name) = tool_name {
                        let mut j = i + 1;
                        // Deliberately *not* `looks_like_unlabeled_synopsis_line`
                        // here (unlike the entry point above it): that test
                        // alone would also admit `corepack`'s own headingless
                        // invocation-table rows — `corepack enable
                        // [--install-directory #0] ...` reads as bracket
                        // notation plus a non-prose remainder (the trailing
                        // `...` defeats `is_prose_sentence`) exactly the way
                        // a real synopsis continuation does, and re-fabricating
                        // that row into more usage text would have demoted a
                        // subcommand `scan_headingless_invocation_table`
                        // already recovers correctly into lost structure —
                        // measured fleet-wide: only `corepack` hits this,
                        // losing 1 subcommand, before this predicate was
                        // narrowed to it. `looks_like_stanza_continuation_head`
                        // alone is sufficient for every real stanza this fix
                        // targets, LVM's family included (its own doc
                        // comment above has both clauses), so nothing is
                        // lost by dropping the wider test here.
                        let is_head = |lines: &[&str], j: usize| {
                            j < lines.len() && looks_like_stanza_continuation_head(lines, j, name)
                        };
                        if !is_head(&lines, j) {
                            if let Some(next) = lines.get(j) {
                                let t = next.trim_start();
                                if !t.is_empty() && is_prose_sentence(t) {
                                    j += 1;
                                }
                            }
                        }
                        if is_head(&lines, j) {
                            let trimmed = lines[j].trim().to_string();
                            usage_lines.push(trimmed.clone());
                            usage_entries.push(trimmed);
                            line_entry_index.push(usage_entries.len() - 1);
                            i = j + 1;
                            continue;
                        }
                    }
                }
                break;
            }
            let trimmed_start = l.trim_start();
            if labelled_usage_start.is_none() && paren_group_depth > 0 {
                // Already inside an open parenthesized alternation group:
                // every line up to the matching close is a member of it,
                // regardless of shape. A member row routinely opens with
                // `-` itself (`-p|--maxphysicalvolumes Number,`), which the
                // "a continuation line that reads as a flag entry ends the
                // block" check just below would otherwise misread as a
                // fresh flag-table row ending the usage block one line into
                // the group — depth, not content, is what says this line
                // still belongs to it, so it is folded in unconditionally,
                // bypassing every content-shape check below the same way a
                // backslash continuation does.
                paren_group_depth += paren_depth_delta(trimmed_start);
                if paren_group_depth <= 0 {
                    paren_group_depth = 0;
                    just_closed_paren_group = true;
                } else {
                    just_closed_paren_group = false;
                }
                let trimmed = l.trim().to_string();
                usage_lines.push(trimmed.clone());
                if let Some(last) = usage_entries.last_mut() {
                    last.push(' ');
                    last.push_str(&trimmed);
                }
                line_entry_index.push(usage_entries.len() - 1);
                i += 1;
                continue;
            }
            if labelled_usage_start.is_none()
                && paren_group_depth == 0
                && looks_like_paren_alternation_open(trimmed_start)
            {
                // Opens the group: hand off to the depth-tracking branch
                // above for every later line, but this opening row itself
                // still falls through to the ordinary flow just below,
                // which already appends it correctly (it is more indented
                // than the stanza head and, starting with `(` rather than
                // `-`, trips none of the content checks that would end the
                // block).
                paren_group_depth = paren_depth_delta(trimmed_start);
            }
            just_closed_paren_group = false;
            let is_marker =
                starts_with_usage_prefix(trimmed_start) || starts_with_or_marker(trimmed_start);
            let is_own_name =
                tool_name.is_some_and(|name| starts_with_tool_name(trimmed_start, name));
            let starts_new_entry = is_marker || is_own_name;

            // A line the one above it ended with a backslash is a
            // continuation by the tool's own explicit statement, and no
            // content test may overrule that. `update-xmlcatalog` wraps
            // its synopsis mid-invocation:
            //
            // ```text
            //     update-xmlcatalog <options> --del --root --type <type> \
            //                                                 --id <id>
            // ```
            //
            // The wrapped tail begins with `--id`, so the `curl` guard
            // below ("a continuation that reads as a flag entry ends the
            // block") fired on it and the usage block stopped one line in
            // — taking `--del`, `--root` and `--type` with it, none of
            // which this tool documents anywhere else, and spilling the
            // remaining synopsis lines into the section scanner where
            // they were read as headings. A backslash is unambiguous
            // where a leading dash is not, which is why it is checked
            // first rather than folded into the guard below.
            let continues_previous_line = i > 0 && lines[i - 1].trim_end().ends_with('\\');
            if !starts_new_entry && !continues_previous_line {
                // A continuation line that itself reads as a flag entry
                // ends the usage block, even though it is indented and
                // unseparated by a blank line. A usage continuation is an
                // *alternative invocation form* (`   curl [options...]
                // <url>`); it never begins with a dash. Tools that run
                // their flag list straight into the usage line with no
                // blank separator and no `Options:` heading are common
                // enough that not stopping here silently swallowed every
                // flag they have: `curl --help` indents its 13 flag rows
                // by one space directly under `Usage:`, and all 13 landed
                // in `usage` with zero flags parsed — reported as `ok` at
                // "no flags to describe", which is the same class of
                // confidently-wrong result as [M-10]. This is a layout
                // fact, true of every framework, so it lives in the shared
                // engine rather than in any profile.
                if looks_like_flag_start(trimmed_start) {
                    break;
                }
                // A section heading ends the usage block no matter how far
                // it is indented. Indentation alone says "continuation"
                // here, and for a tool that indents its *whole* body under
                // the synopsis that answer is wrong for every line after
                // the first heading: binutils `ar` opens `Usage: ar ...`,
                // then indents ` commands:` by one space and its eight
                // command rows by two, so the heading, all eight commands
                // and the two modifier sections after them were joined
                // into a single `usage` string and the tree got zero
                // subcommands. A heading is never an alternative
                // invocation form, which is the only thing a usage
                // continuation can be, so its shape — and not its column —
                // is what has to decide.
                if is_section_heading_line(trimmed_start) {
                    break;
                }
                // A line more indented than the base is not *always* a
                // continuation — only when it still reads as usage grammar.
                // `sg_emc_trespass` opens `Usage:  sg_emc_trespass [-d]
                // [-hr] [-s] [-V] DEVICE` at column 0, then follows with two
                // ordinary English sentences indented two spaces under it:
                // "Change ownership of a LUN from another SP to this one."
                // and "EMC CLARiiON CX-/AX-family + FC5300/FC4500/FC4700."
                // Both are more indented than the base, so the old rule —
                // "more indented, no further test" — read them as
                // continuations of the synopsis and joined them onto it
                // verbatim; `extract_positionals` then mined their bare
                // uppercase words `LUN`, `SP` and `EMC` as three fabricated
                // required operands the tool does not have. Reuses
                // [`is_prose_sentence`] rather than a second copy of it —
                // the same predicate the base-indent fallback just below
                // already relies on for `du`'s own trailing-sentence
                // precedent, applied here one indentation tier higher up.
                // `git`'s genuine hanging-indent continuations are usage
                // fragments (bracketed notation, no sentence terminator),
                // never prose, so they are untouched by this check.
                //
                // This drops the one prose line rather than ending the
                // whole block, deliberately unlike every other check in
                // this loop — `mdadm --help` interleaves a one-line
                // description under *each* of its several `mdadm --mode
                // ...` alternative forms (`mdadm --assemble device
                // options...` / `Assemble a previously created array.` /
                // `mdadm --build device options...` / ...), so breaking on
                // the first description would end the block after the
                // *first* form and silently drop every later one — 7 real
                // mode flags (`--assemble`, `--build`, `--grow`,
                // `--incremental`, `--manage`, `--misc`, `--monitor`) on
                // this tool alone. Each later `mdadm ...` line still starts
                // its own new entry ([`starts_with_tool_name`], checked
                // above, independent of this line entirely), so skipping
                // just the description in between costs nothing and the
                // next real form is still found.
                // Guarded to lines *strictly more indented* than the base —
                // `du`'s own trailing sentence sits *at* the base indent
                // (column 0, same as `Usage:` and `or:`) and must keep
                // taking the base-indent fallback just below, unchanged:
                // that fallback's `break` (not a skip) is what lets the
                // sentence fall out of the usage block *and* stay before
                // `i`, so the leading-description scan below still picks
                // it up. Applying this check there too advanced `i` past
                // it instead, which pulled the sentence *into*
                // `in_usage_block`'s range and silently deleted `du`'s (and
                // six other tools': `expand`, `grub-set-default`, `lzless`,
                // `sbverify`, `sha1sum`) description outright — caught by
                // the corpus suite, not by inspection.
                if leading_whitespace(l) > base_indent && is_prose_sentence(trimmed_start) {
                    i += 1;
                    continue;
                }
                // Below the base indent (never above it: `leading_whitespace`
                // is unsigned, so this also covers "equal to"), indentation
                // alone can't distinguish a genuine continuation (lsof) from
                // the block having ended (du) — fall back to content shape.
                if leading_whitespace(l) <= base_indent && !looks_like_usage_fragment(trimmed_start)
                {
                    break;
                }
            }
            let trimmed = l.trim().to_string();
            usage_lines.push(trimmed.clone());
            if starts_new_entry {
                // A form keeps the indentation its author gave it: `ip`
                // lines its second invocation form up under the first, and
                // spec §4.1 has the pane reproduce that alignment rather
                // than flatten every form to the left edge. Only the
                // display form (`usage_entries`) carries it —
                // `usage_lines`, which feeds the positional and
                // synopsis-flag grammars below, stays trimmed, because
                // those read tokens and never columns.
                usage_entries.push(l.trim_end().to_string());
            } else if let Some(last) = usage_entries.last_mut() {
                // The backslash *is* the join: it is the marker the wrap
                // introduced, exactly as the removed newline was, so it
                // goes where the newline went. Dropping it is the same
                // decision as choosing a single space to join with (see
                // this block's own comment on why that is not re-flowing)
                // — without it the displayed synopsis reads
                // `--type <type> \ --id <id>`, a continuation marker
                // stranded in the middle of a line it no longer continues.
                if last.ends_with('\\') {
                    last.pop();
                    let trimmed_tail = last.trim_end().len();
                    last.truncate(trimmed_tail);
                }
                last.push(' ');
                last.push_str(&trimmed);
            }
            line_entry_index.push(usage_entries.len() - 1);
            i += 1;
        }
        // The self-closed-bracket-group refinement is further scoped to a
        // *labelled* block (`Usage:`/`or:` found somewhere in the
        // document) — never an **unlabelled** synopsis
        // (`unlabelled_synopsis_start`, `looks_like_unlabeled_synopsis_line`'s
        // own convention: `dbus-cleanup-sockets [--version] [--help]
        // <socketdir>`, `lvreduce -L|--size [-]Size[m|UNIT] LV`, neither
        // preceded by any `Usage:` text at all). `xtask`'s existence
        // oracle's own synopsis scanner (`existence::synopsis_lines`)
        // recognizes only labelled lines; it has no unlabelled-synopsis
        // support yet, so *any* operand recovered from one — by this fix
        // or any other, in any shape — reports as invented today. Measured
        // on the same full-`PATH` sweep as the primary-line scoping above:
        // 3 tools (`dbus-cleanup-sockets`, `dbus-run-session`, `lvreduce`).
        let primary_lines = if labelled_usage_start.is_some() {
            primary_synopsis_lines(&usage_entries, &line_entry_index, usage_lines.len())
        } else {
            std::collections::HashSet::new()
        };
        result.positionals = extract_positionals(&usage_lines, primary_lines);
        result.usage = usage_entries;
    }

    // 2. Leading prose before the usage block (or before the first
    // section, if there's no usage block) becomes the description.
    //
    // `leading_prose_bound` is O(lines.len()) — it must be computed once
    // here, outside the loop below, not inside the loop condition (which
    // would re-run it on every iteration and turn this whole function
    // quadratic in input size; found via the coverage harness (spec
    // §13.1) parsing a degenerate multi-megabyte input in over two
    // minutes instead of milliseconds).
    let description_bound = i.max(leading_prose_bound(&lines));
    // A column-0 line inside the recovered usage block's own line range
    // (`usage_start..i`) is never description prose, whichever of the
    // three entry shapes above found it — not just an ordinary `usage:`
    // line. Checking the *range* rather than re-testing `starts_with_usage_prefix`
    // here is what keeps this correct for the name-prefixed
    // (`nfsidmap: Usage: ...`) and unlabelled (`wpa_cli [-p<path>] ...`)
    // shapes too: neither line's text starts with `usage:`, so the old
    // per-line text test alone would have let it leak into the
    // description exactly the way it did before this fix — `wpa_cli`'s
    // root description was the tool's own invalid-option banner run
    // straight into its synopsis and its entire `commands:` block, because
    // nothing about that text said "usage" at its own start.
    let in_usage_block = |idx: usize| usage_start.is_some_and(|s| (s..i).contains(&idx));
    // Collected as *paragraphs* (blank-line-separated runs), not one flat
    // list, so a leading version/author/URL banner can be told apart from
    // the tool's real description — see `is_banner_paragraph` below. A
    // paragraph boundary is a genuinely blank line; a skipped indented line
    // (a usage continuation sitting in this same zone, `du`'s `  or: ...`)
    // does not break the paragraph it sits inside, matching this loop's
    // pre-batch behavior of simply ignoring such lines rather than treating
    // them as a break.
    let mut paragraphs: Vec<Vec<&str>> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut j = 0;
    while j < lines.len() && j < description_bound {
        let l = lines[j];
        if l.trim().is_empty() {
            if !current.is_empty() {
                paragraphs.push(std::mem::take(&mut current));
            }
        } else if leading_whitespace(l) == 0 {
            let t = l.trim_start();
            if !starts_with_usage_prefix(t)
                && !in_usage_block(j)
                && !looks_like_bnf_production_line(t)
            {
                current.push(l);
            }
        }
        j += 1;
    }
    if !current.is_empty() {
        paragraphs.push(current);
    }
    // clap's own `--help` template (and every framework that copies its
    // shape) renders `<name> <version>` / author / homepage as one
    // paragraph, a blank line, and *then* the real description
    // (`zoxide --help`: "zoxide 0.9.9" / "Ajeet D'Souza <...>" /
    // "https://github.com/..." / blank / "A smarter cd command for your
    // terminal"). Concatenating every leading column-0 line regardless of
    // that blank line put the email address and URL into the description
    // shown in the detail pane — nothing fabricated, nothing missing, so no
    // gate fires, but the pane shows junk. Only drop the first paragraph,
    // and only when it *looks* like this banner shape (never by checking it
    // against the tool's own name — see `is_banner_paragraph`) — and only
    // when a later paragraph exists to fall back to, so a tool whose entire
    // leading prose happens to open with something version-shaped never
    // loses its only description.
    // The tool's own option-error complaint (`is_option_error_paragraph`,
    // e.g. `ssh-keygen`'s `unknown option -- -`) is checked first and
    // independently of the banner check above it: unlike a banner, it is
    // allowed to drop the *only* leading paragraph (see that function's
    // doc comment for why losing the description there is the honest
    // outcome), so it cannot be folded into the `paragraphs.len() > 1`
    // guard the banner check needs.
    let drop_first_paragraph = match paragraphs.first() {
        Some(first) if is_option_error_paragraph(first) => true,
        Some(first) if paragraphs.len() > 1 && is_banner_paragraph(first) => true,
        _ => false,
    };
    // Handed over with its line structure intact — one `\n` per source
    // line, `\n\n` between paragraphs — rather than pre-flattened with
    // spaces. Deciding which of those breaks is hard-wrapping to be
    // undone and which is structure to be kept is `Text::sanitize`'s job
    // (spec §4.1), and it can only do that job on text that still has the
    // breaks: joining here with a space threw the evidence away first and
    // then asked the sanitizer to reflow the result. `grep --help`'s
    // `Example: grep -i 'hello world' menu.h main.c` is the case — the
    // sanitizer keeps an example row on its own line, but only when it is
    // still given one.
    let kept: Vec<Vec<&str>> = if drop_first_paragraph {
        paragraphs.into_iter().skip(1).collect()
    } else {
        paragraphs
    };
    let description = kept
        .iter()
        .map(|paragraph| paragraph.join("\n"))
        .filter(|paragraph| !paragraph.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if !description.is_empty() {
        result.description = Some(description);
    }

    // A run of command-group headings is recognized either by its own
    // wording (rule 1's literal test) or by being contiguous with an
    // earlier signal that a command list is starting — git's own group
    // headings ("start a working area (see also: ...)") never say
    // "command" themselves, but the blurb introducing them does. Seeding
    // from the leading description only (not the whole document) is what
    // keeps this from also lighting up on tar's `--occurrence` flag
    // description, which happens to say "one of the subcommands" deep in
    // prose describing something else entirely.
    //
    // Deliberately *not* also seeded from `result.usage`: a docopt-style
    // `prog [OPTIONS] COMMAND ...` synopsis is an extremely common
    // convention (cobra, urfave/cli, click, and plain GNU-argp tools all
    // write it), and turning `command_mode` on for the rest of the
    // document on that word alone reaches headings the synopsis says
    // nothing about — measured on a full-`PATH` sweep: `containerd --help`
    // and `ctr --help` (urfave/cli) both write `USAGE:\n   <tool> [global
    // options] command [command options]`, and seeding from that alone
    // turned their unrelated `VERSION:\n   v2.3.3` block into a fabricated
    // subcommand literally named `v2.3.3` — the exact class of defect
    // [M-10] exists to prevent. See this fix's PR description for the
    // `systemd-creds`/`systemd-sysext`/`systemd-confext` regression this
    // predicate would otherwise have repaired (their own `Commands:`
    // heading is ANSI-corrupted — `\x1b[0mCommands:` reads as one alnum
    // run, `0mCommands`, to `mentions_commands_word` — a separate,
    // pre-existing gap left for a follow-up rather than fixed here by
    // widening a sticky chain that a fleet sweep just showed fabricates).
    let mut command_mode = result
        .description
        .as_deref()
        .is_some_and(|d| command_mode_seed(d, profile));
    // True from the moment an `is_ignorable_heading` heading (`EXAMPLES:`,
    // a `Report bugs`-shaped line) is captured until a *structurally
    // strong* block — a real flags block, word grid, or command table, not
    // merely "content that fell through to the generic bare-block
    // fallback" — is recognized under some later, non-ignorable heading.
    // Exists solely to gate [`recover_stanza_head_flag`] below: an
    // `EXAMPLES:` section's own invocation lines (`bpftrace -e
    // 'tracepoint:raw_syscalls:sys_enter { ... }'`, `bpftrace -l
    // '*sleep*'`) are, line for line, indistinguishable from a genuine
    // LVM stanza head — the tool's own name, then a bare flag token, with
    // more-indented content (the example's own one-line description)
    // beneath it. `is_ignorable_heading` alone cannot see this: it is
    // tested per candidate heading, and neither example line's own text
    // starts with "example" or contains "report bugs" — only the
    // `EXAMPLES:` heading that introduces them does, and every following
    // heading candidate is re-examined independently of it (the "rewind"
    // path below carries no memory of what came before). Measured: a full
    // sweep before this guard existed fabricated `bpftrace -e`/`-l` flag
    // rows whose value was a fragment of the example invocation, and it
    // displaced the real, described `-e`/`-l` rows the tool's own
    // `Options:`-shaped block already provides.
    //
    // Deliberately *not* reset by the generic `scan_bare_block` +
    // `emit_choices`/`emit_subcommands` fallback (the path every one of
    // `EXAMPLES:`'s own invocation lines actually takes, since their
    // one-line description is not flag- or command-shaped): resetting
    // there would clear the flag on the *first* example line and reopen
    // the fabrication on the second. Only a positively-recognized
    // structural block — the same set of branches that already gate their
    // own emission on `!is_ignorable_heading(&heading)`, plus a few more
    // whose shape cannot occur inside an examples section at all — clears
    // it.
    let mut in_ignorable_section = false;
    // Some hand-written help indents `Examples:` beneath the prose sentence
    // immediately before it.  The generic relative-indent engine otherwise
    // promotes that prose sentence to a heading and consumes the marker as
    // ordinary content, so `is_ignorable_heading` never gets to establish
    // the section context at all.  While this is `Some(indent)`, the marker
    // was found in exactly that obscured shape and the whole region is
    // fenced before *any* headingless or headed emission path can see its
    // rows.  A physical dedent always closes the region.  At the marker's
    // own indent, only `starts_attested_flag_section` may close it; a plain
    // `Input:`/`Output:` label inside the example may not.
    //
    // This state is deliberately separate from `in_ignorable_section`:
    // direct, correctly-recognized `Examples:` headings retain their
    // established behavior, while the stronger whole-region fence applies
    // only to markers that the prose-parent quirk would otherwise hide.
    let mut obscured_ignorable_indent: Option<usize> = None;

    // 3. Section blocks: scan the rest of the output for a heading line
    // followed by more-indented content — or, if the very first content
    // already looks like a flag entry, a headingless flags block (sed's
    // `--help` has no "Options:" line at all; its entries start on line
    // one). "Heading" is a relative notion, not "column 0": tar indents
    // its own headings by one space (` Main operation mode:`) while its
    // entries sit at two, so a block is recognized whenever some line is
    // followed (after any blank lines) by content indented *more than
    // that line*.
    let mut total_entries = 0usize;
    let mut clean_entries = 0usize;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        if let Some(marker_indent) = obscured_ignorable_indent {
            let indent = leading_whitespace(line);
            if indent < marker_indent
                || (indent == marker_indent && starts_attested_flag_section(&lines, i))
            {
                obscured_ignorable_indent = None;
                in_ignorable_section = false;
            } else {
                i += 1;
                continue;
            }
        }
        // Headingless flags block: the current line already looks like a
        // flag entry, so there is no heading to consume — scan it in
        // place.
        if looks_like_flag_start(line.trim_start()) {
            // No recognized heading governs this block, but the row
            // itself may still be the one `split_shared_heading_rows`
            // recovered from a `:=` production whose own heading the
            // engine never revisited as a heading (see that function's
            // doc comment on why it is keyed on the row for exactly this
            // reason) — `dcb` and `vdpa` both reach their `OPTIONS`
            // row this way, since their single-line `where OBJECT := ...`
            // production is misread as an ordinary heading whose
            // "content" is the next, unrelated line.
            let heading_is_bnf = bnf_row_lines.contains(&i);
            let (end, entries, packed) = scan_flags_block(&lines, i, heading_is_bnf);
            i = end;
            if packed {
                let seen = entries.len();
                emit_packed_flags(None, entries, &mut result);
                total_entries += seen;
                clean_entries += seen;
            } else {
                let (seen, clean) = emit_flags(None, entries, &mut result);
                total_entries += seen;
                clean_entries += clean;
            }
            command_mode = false;
            continue;
        }

        // Headingless invocation table (spec §7 Tier B): a run of rows the
        // tool prints of its own invocation forms, with **no governing
        // heading at all** — `btrfs --help`'s command table sits directly
        // under a blank line once its flags block ends, never introduced
        // by a "Commands:"-shaped line. Every other command-recovery path
        // in this loop requires a recognized heading (rule 1); this one
        // instead requires every row to start with the tool's own name at
        // a word boundary, which is what supplies the positive evidence a
        // heading would otherwise supply. Tried only when the current line
        // isn't already consumed as a flag row (above) or reached as a
        // heading's own indented content (below, where this line would
        // instead be read as a heading and its rows — wrongly — as that
        // heading's bare-word block); see `scan_headingless_invocation_table`'s
        // own doc comment for the admission rules and why this call site
        // structurally can never land inside an `Examples:`/`Report bugs:`
        // region (`is_ignorable_heading`) or a block a real heading already
        // governs.
        if let Some(tool_name) = tool_name {
            if starts_with_tool_name(line.trim_start(), tool_name) {
                if let Some((end, nodes, seen, clean)) =
                    scan_headingless_invocation_table(&lines, i, tool_name, raw)
                {
                    i = end;
                    total_entries += seen;
                    clean_entries += clean;
                    for node in nodes {
                        result.try_push_subcommand(node);
                    }
                    command_mode = false;
                    continue;
                }
            }
        }

        let heading_indent = leading_whitespace(line);
        let heading = line.trim().to_string();
        let heading_idx = i;
        if is_prose_sentence(&heading) {
            let mut marker_idx = heading_idx + 1;
            while marker_idx < lines.len() && lines[marker_idx].trim().is_empty() {
                marker_idx += 1;
            }
            if marker_idx < lines.len()
                && leading_whitespace(lines[marker_idx]) > heading_indent
                && is_ignorable_heading(lines[marker_idx].trim())
            {
                obscured_ignorable_indent = Some(leading_whitespace(lines[marker_idx]));
                in_ignorable_section = true;
                command_mode = false;
                i = marker_idx + 1;
                continue;
            }
        }
        if is_ignorable_heading(&heading) {
            in_ignorable_section = true;
        }
        // A hard-wrapped prose sentence, whose second physical line the
        // indentation-alone heading rule would otherwise hand to the flags
        // scanner. Fenced whole — see `wrapped_prose_region_end`.
        if let Some(end) = wrapped_prose_region_end(&lines, heading_idx) {
            i = end;
            continue;
        }
        i += 1;
        while i < lines.len() && lines[i].trim().is_empty() {
            i += 1;
        }
        if i >= lines.len() || leading_whitespace(lines[i]) <= heading_indent {
            // Nothing more-indented follows. Some tools (openssl's
            // `--help`, and BSD-style listings generally) present a
            // command list as a same-indent word grid instead: a heading
            // line immediately followed by lines of several bare
            // identifier-shaped tokens each, no descriptions at all. This
            // is still a general, non-tool-specific shape — recognized by
            // content, not by which tool happens to do it.
            //
            // Starting a grid requires >=3 name-shaped tokens on the
            // trigger line — not just the >=2 used for continuation rows
            // — specifically so a genuine two-word heading immediately
            // above the grid (openssl's "Standard commands") is never
            // itself mistaken for the first grid row and swallowed as
            // data; it gets rewound and re-examined as its own heading
            // one line later, which is what lets it end up as `group`.
            if i < lines.len()
                && leading_whitespace(lines[i]) == heading_indent
                && looks_like_word_grid_start(lines[i])
            {
                let grid_start = i;
                while i < lines.len() {
                    if lines[i].trim().is_empty() {
                        break;
                    }
                    if leading_whitespace(lines[i]) != heading_indent
                        || !looks_like_word_grid_line(lines[i])
                    {
                        break;
                    }
                    i += 1;
                }
                if !is_ignorable_heading(&heading) {
                    in_ignorable_section = false;
                    let recognized = is_recognized_command_heading(&heading, profile);
                    if recognized {
                        command_mode = true;
                    }
                    let (seen, clean) = process_word_grid(
                        &heading,
                        &lines[grid_start..i],
                        recognized || command_mode,
                        &mut result,
                    );
                    total_entries += seen;
                    clean_entries += clean;
                }
                continue;
            }
            // A command table that sits at the *same* indent as its own
            // heading, rather than beneath it. `dnf` 4 prints its whole
            // command list this way, flush at column 0:
            //
            // ```text
            // List of Main Commands:
            //
            // alias                     List or create command aliases
            // autoremove                remove all unneeded packages
            // ```
            //
            // The engine's "content is indented more than its heading"
            // rule cannot see that at all, so `mandible dnf` reported one
            // node and no subcommands — silently missing structure, which
            // §7 treats as no better than inventing it.
            //
            // Guarded much harder than the indented case, because this is
            // the shape [M-10] came in through: `apt-get --help`'s prose
            // paragraph became the subcommands *"and"*, *"information"*,
            // *"about"*, *"them"*. Three things must all hold — the
            // heading must be a *recognized* command heading (never merely
            // a line ending in a colon), every row must be column-aligned
            // in the [M-10] sense (a name-shaped token, then a 2+ space
            // gap, then description text), and there must be at least two
            // such rows. Prose is single-spaced, so it fails the second
            // test on its first line.
            // The heading must not itself look like one of the rows. At a
            // shared indent there is no structural difference between a
            // heading and a table row, so without this every row is a
            // candidate heading for the rows beneath it — and
            // `mentions_commands_word` splits on non-alphanumerics, so a
            // row whose *name* merely contains "command" qualifies.
            //
            // Measured: `mysqlslap --help` ends with a flush-left table of
            // config variables and their defaults, and the row
            // `init-command    (No default value)` was taken as a heading,
            // fabricating 28 subcommands out of MySQL settings — [M-10]
            // exactly, reached by a new route. A real heading is a single
            // field (`List of Main Commands:`); a row is two columns
            // separated by a 2+ space gap.
            let heading_is_itself_a_row = find_description_gap(lines[heading_idx]).is_some();

            if i < lines.len()
                && leading_whitespace(lines[i]) == heading_indent
                && !heading_is_itself_a_row
                && !is_ignorable_heading(&heading)
                && is_recognized_command_heading(&heading, profile)
            {
                if let Some((end, entries)) =
                    scan_same_indent_entry_table(&lines, i, heading_indent)
                {
                    i = end;
                    command_mode = true;
                    in_ignorable_section = false;
                    let (seen, clean) = emit_subcommands(&heading, entries, &mut result);
                    total_entries += seen;
                    clean_entries += clean;
                    continue;
                }
            }

            // Not actually a heading — but if it reads like an
            // introduction to a command list ("These are common Git
            // commands used in various situations:", itself flanked by
            // blank lines rather than indented content, so it never forms
            // a block of its own), remember that: the group headings that
            // immediately follow (git's "start a working area (see also:
            // ...)" and friends) say nothing about "commands" themselves.
            if command_mode_seed(&heading, profile) {
                command_mode = true;
            }
            // Rewind to just past the original line and continue scanning
            // it as its own candidate.
            i = heading_idx + 1;
            continue;
        }

        // Reaching here means genuinely more-indented content follows this
        // heading (the branch above always `continue`s otherwise) — LVM's
        // own stanza shape, a head line naming a mode-selecting flag
        // followed by that mode's `[...]`/`(...)` rows. Recovering the
        // flag is independent of whatever the content turns out to be
        // (a recognized flags block below, a word grid, or — `vgchange
        // --systemid String VG`'s `[ COMMON_OPTIONS ]` — a single
        // placeholder row `flags_block_start` never recognizes as a flags
        // block at all): the head line names its flag either way, so this
        // runs once per heading rather than being folded into any one of
        // the branches that follow.
        //
        // Gated on `!in_ignorable_section`: `bpftrace --help`'s own
        // `EXAMPLES:` block writes each example as "the tool's own name,
        // then a bare flag token, then a one-line description" —
        // `bpftrace -e 'tracepoint:raw_syscalls:sys_enter { ... }'` — the
        // identical shape to a real stanza head, and without this guard it
        // fabricated `-e`/`-l` rows whose value was a fragment of the
        // example invocation, displacing the real, described rows from
        // `bpftrace`'s own `Options:` block. See `in_ignorable_section`'s
        // own doc comment for why this has to be section context rather
        // than a per-line check.
        if !in_ignorable_section {
            if let Some(flag) = recover_stanza_head_flag(&heading, tool_name) {
                if result.flags.len() < MAX_RECOVERED_ENTRIES {
                    result.flags.push(flag);
                }
            }
        }

        // A headed command table whose first row sits on the heading's own
        // physical line (`apt-ftparchive`'s
        // `Commands: packages binarypath [overridefile [pathprefix]]`),
        // with the remaining rows column-aligned beneath it. This row's
        // text never becomes "content indented below the heading" in the
        // engine's own sense — it trails the heading on the very same
        // line — so without this it vanishes whole into the `heading`
        // string above and never reaches any scanner as data: today
        // `mandible apt-ftparchive` reports zero subcommands and a group
        // literally named `"Commands: packages binarypath [overridefile
        // [pathprefix]]"`. See `split_heading_inline_row` and
        // `scan_bare_command_table`'s doc comments for the admission
        // rules, and `emit_headed_command_table`'s for why these nodes
        // are `invocation_attested` rather than `heading_attested`.
        //
        // Gated on `is_recognized_command_heading(label, ...)` for *this*
        // heading directly, never a `command_mode` chain — see the other
        // `scan_bare_command_table` call site below (the ` = `-separator
        // one) for the fabrication a sticky chain enables: it can reach an
        // unrelated wrapped-prose block where ordinary English words pass
        // every other guard this recognizer has.
        if let Some((label, inline_row)) = split_heading_inline_row(line.trim()) {
            if !is_ignorable_heading(&heading)
                && is_recognized_command_heading(label, profile)
                && !profile.is_some_and(|p| {
                    heading_matches_markers(&label.to_lowercase(), p.non_command_heading_markers)
                })
            {
                if let Some(first_name) = leading_command_name(inline_row) {
                    if let Some((end, mut entries)) = scan_bare_command_table(&lines, i) {
                        entries.insert(0, (first_name, None));
                        i = end;
                        command_mode = true;
                        in_ignorable_section = false;
                        let raw_tokens = command_table_token_index(raw);
                        let (seen, clean) =
                            emit_headed_command_table(entries, &raw_tokens, &mut result);
                        total_entries += seen;
                        clean_entries += clean;
                        continue;
                    }
                }
            }
        }

        // Peek the first content lines to decide flags vs. bare-word. Not
        // just the *first*: some tools document a positional at the top of
        // their options table, and keying the whole decision off row one
        // threw the rest of the block away. See `flags_block_start`.
        if let Some(flags_start) = flags_block_start(&lines, i) {
            // `flags_start` — never `heading_idx` — is the evidence: see
            // `split_shared_heading_rows`'s doc comment for why the BNF
            // fact is keyed on the row rather than the heading beside it.
            let heading_is_bnf = bnf_row_lines.contains(&flags_start);
            let (end, entries, packed) = scan_flags_block(&lines, flags_start, heading_is_bnf);
            i = end;
            if is_ignorable_heading(&heading) {
                command_mode = false;
                continue;
            }
            // A real, non-ignorable flags block — structurally strong
            // evidence we are not (or no longer) inside an examples-shaped
            // section. See `in_ignorable_section`'s own doc comment.
            in_ignorable_section = false;
            if packed {
                let seen = entries.len();
                emit_packed_flags(meaningful_flag_group(heading), entries, &mut result);
                total_entries += seen;
                clean_entries += seen;
            } else {
                let (seen, clean) =
                    emit_flags(meaningful_flag_group(heading), entries, &mut result);
                total_entries += seen;
                clean_entries += clean;
            }
            command_mode = false;
            continue;
        }

        // Argparse's subparser blocks (spec §7 Tier B, batch 6 part 4) are
        // structurally distinct from every other framework's bare-word
        // block — see `scan_argparse_subparsers`'s doc comment — so they
        // get first refusal here, gated on the profile explicitly opting
        // in. A miss (no `{...}` pseudo-entry evidence found) falls
        // straight through to the ordinary bare-block handling below, same
        // as any other tool.
        //
        // Deliberately *not* also gated on the heading reading `"positional
        // arguments"`. It was, and that made `add_subparsers(title=...)` —
        // the ordinary way an argparse tool styles this heading — collapse
        // the entire command tree to nothing: the scan never ran, and the
        // general engine then read the `{a,b,c}` pseudo-entry as the single
        // entry with the real subcommands as its continuation lines. A
        // twelve-level fixture and `smokecli` both rendered one node.
        //
        // The structural evidence the scan already demands is what makes
        // dropping the text check safe, and it is strictly stronger than
        // the heading was: a `{...}` pseudo-entry *with deeper lines
        // beneath it*. A plain positional carrying `choices=[...]` renders
        // the same `{...}` metavar but has nothing beneath it, so it still
        // returns `None` and is still never promoted to subcommands —
        // which is the [M-10] fabrication this guard exists to prevent, and
        // is asserted directly by
        // `argparse_profile_does_not_fabricate_subcommands_from_plain_positionals`.
        if profile.is_some_and(|p| p.argparse_subparser_quirk) {
            if let Some((end, entries)) = scan_argparse_subparsers(&lines, i, heading_indent) {
                i = end;
                command_mode = false;
                in_ignorable_section = false;
                let (seen, clean) = emit_subcommands(&heading, entries, &mut result);
                total_entries += seen;
                clean_entries += clean;
                continue;
            }
        }

        // A framework-declared *positional-operand* heading (see
        // `FrameworkProfile::positional_heading_markers`). Sits directly
        // below the subparser scan on purpose: for argparse the two read
        // the *same* heading, and the subparser scan gets first refusal
        // because a `{...}` pseudo-entry with real entries beneath it is
        // strictly stronger evidence than the heading text alone. Only once
        // that scan has declined does the block mean what its heading says
        // — a list of the tool's plain positional operands.
        //
        // Like a non-command heading below, this also breaks the sticky
        // `command_mode` chain: a block the framework itself labels
        // "positional arguments" is positive evidence that whatever
        // command list was being followed has ended.
        if profile.is_some_and(|p| {
            heading_matches_markers(&heading.to_lowercase(), p.positional_heading_markers)
        }) {
            let (end, entries) = scan_bare_block(&lines, i, heading_indent, false);
            i = end;
            command_mode = false;
            in_ignorable_section = false;
            let (block_seen, block_clean) =
                emit_declared_positionals(entries, &usage_lines, &mut result);
            total_entries += block_seen;
            clean_entries += block_clean;
            continue;
        }

        // A framework-declared *non*-command heading (spec §7 Tier B,
        // batch 6 part 4 — see `FrameworkProfile::non_command_heading_markers`'s
        // doc comment) both refuses this block and breaks the engine's
        // sticky same-indent chain, so nothing after it inherits a
        // `command_mode` this heading positively contradicts.
        let is_declared_non_command = profile.is_some_and(|p| {
            heading_matches_markers(&heading.to_lowercase(), p.non_command_heading_markers)
        });
        let recognized = is_recognized_command_heading(&heading, profile);
        // Issue #3: ` - ` (space-dash-space) is accepted as an entry
        // separator alongside the usual 2+-space column gap, but *only*
        // when this block is already headed for `emit_subcommands` —
        // i.e. its heading is recognized, or it's continuing a chain one
        // started (`command_mode`), and no framework has declared this
        // heading a non-command one. Scoping the decision to before the
        // scan (rather than loosening `find_description_gap` itself,
        // which every bare block — including `emit_choices`'s enum-value
        // lists — shares) is what keeps a bare ` - ` in ordinary prose
        // from ever manufacturing commands again: that's the exact
        // failure apt-get's own description paragraph produced once
        // already (spec [M-10]), just via the column-gap rule instead of
        // this one.
        let allow_dash_separator = (recognized || command_mode) && !is_declared_non_command;

        // A headed command table whose rows use ` = ` as a separator
        // instead of a column gap, or (some rows) no separator at all
        // (`wpa_cli`'s `commands:` block: `status [verbose] = get current
        // WPA/EAPOL/EAP status`, `wps_cancel Cancels the pending WPS
        // operation`).
        //
        // Gated on `recognized` **alone** — deliberately narrower than
        // `allow_dash_separator` just above, which also accepts a
        // `command_mode` chain. That extra breadth is exactly what turned
        // this into a fabrication path during development: `fail2ban-
        // client`'s `Command:` block nests dozens of column-aligned rows
        // whose own descriptions wrap across several more-indented lines
        // (`"reload [--restart]...     reloads the configuration
        // without\n     restarting of the server, the\n     option
        // '--restart' activates\n..."`). The engine's own "not actually a
        // heading, rewind" path (this file's `i = heading_idx + 1;
        // continue;`) re-examines each such row as a fresh pseudo-heading
        // once `command_mode` is already stuck on from the real `Command:`
        // heading many rows earlier, and when a wrapped continuation block
        // ends up reachable on its own (disconnected from the row it
        // actually describes), every one of `scan_bare_command_table`'s
        // safeguards — no column gap, no dash, a name-shaped leading token,
        // more than one token on the line — is satisfied by ordinary
        // English (`"restarting of the server"`, `"option '--restart'
        // activates"`), because a lowercase word is indistinguishable from
        // a command name by shape alone. Measured: `of`, `the`, `for`,
        // `back`, `adds`, `sets`, `calls`, `otherwise` and a dozen more
        // English words became `invocation_attested` "subcommands" this
        // way. `recognized` — true only when *this exact heading's own
        // text* mentions "command(s)" — is false for every one of those
        // pseudo-headings (`"reload [--restart]...     reloads the
        // configuration without"` names no command), so requiring it
        // directly (never inherited through the sticky chain) closes this
        // off while still admitting both real fixtures: `wpa_cli`'s
        // `commands:` and `apt-ftparchive`'s `Commands:` are each
        // literally recognized at the exact point this is tried, with no
        // `command_mode` inheritance needed. `!is_declared_non_command`
        // guards the same framework override `allow_dash_separator` does,
        // for the same reason. See `scan_bare_command_table`'s own doc
        // comment for the column-gap/dash bail-out that separately keeps
        // this from ever overriding an already-working column- or
        // dash-separated table.
        if recognized && !is_declared_non_command {
            if let Some((end, entries)) = scan_bare_command_table(&lines, i) {
                i = end;
                command_mode = true;
                in_ignorable_section = false;
                let raw_tokens = command_table_token_index(raw);
                let (seen, clean) = emit_headed_command_table(entries, &raw_tokens, &mut result);
                total_entries += seen;
                clean_entries += clean;
                continue;
            }
        }

        // Busybox's applet list (spec issue #1) is a single flat,
        // comma-separated run under one heading — structurally distinct
        // from every other framework's per-line bare-word block, so it
        // gets first refusal here exactly like argparse's subparser scan
        // above (see `FrameworkProfile::comma_separated_command_list`'s
        // doc comment). Gated on the profile flag (busybox only) *and*
        // this heading already being recognized or continuing a
        // `command_mode` chain, so it can never fire for an unrelated
        // tool's ordinary bare-word block.
        if profile.is_some_and(|p| p.comma_separated_command_list)
            && (recognized || command_mode)
            && !is_declared_non_command
        {
            let (end, entries) = scan_comma_separated_commands(&lines, i);
            i = end;
            command_mode = true;
            in_ignorable_section = false;
            let (seen, clean) = emit_subcommands(&heading, entries, &mut result);
            total_entries += seen;
            clean_entries += clean;
            continue;
        }

        let (end, entries) = scan_bare_block(&lines, i, heading_indent, allow_dash_separator);
        i = end;
        if is_ignorable_heading(&heading) {
            continue;
        }

        if is_declared_non_command {
            command_mode = false;
            let (seen, clean) = emit_choices(&heading, entries, &mut result);
            total_entries += seen;
            clean_entries += clean;
            continue;
        }

        if recognized || command_mode {
            command_mode = true;
            // Only `recognized` (this exact heading's own text says
            // "commands") is strong enough to clear the flag here — the
            // `command_mode` sticky-chain half of this condition is not:
            // it can still be true from an *inherited* chain rather than
            // this heading's own evidence, which is exactly the kind of
            // indirect signal `in_ignorable_section` must not trust (see
            // its own doc comment on why the generic bare-block fallback
            // is deliberately excluded from clearing it).
            if recognized {
                in_ignorable_section = false;
            }
            let (seen, clean) = emit_subcommands(&heading, entries, &mut result);
            total_entries += seen;
            clean_entries += clean;
        } else {
            command_mode = false;
            let (seen, clean) = emit_choices(&heading, entries, &mut result);
            total_entries += seen;
            clean_entries += clean;
        }
    }

    // spec [M-15]: mine the usage synopsis for flag spellings too, not just
    // positionals — `git --help` documents fourteen long options and six
    // short ones (`-p | --paginate | -P | --no-pager`, `--git-dir=<path>`,
    // ...) *only* in its `usage:` block, and until now nothing read that
    // block for anything but positionals, so tools whose options live only
    // in their synopsis reported zero flags at status `ok` ([M-15]: 378 of
    // 1,895 `ok` tools fleet-wide). Deferred to here (after the section-
    // block scan above, which is where a `Options:`-style block's
    // *described* flags land in `result.flags`) so a duplicate spelling can
    // be recognized and dropped rather than added a second time.
    //
    // **Deliberately not `mandible_core::merge_entity_lists`.** A first cut
    // used it and a real-`PATH` sweep caught the bug: that function
    // rebuckets *every* flag in the combined list by identity, which is
    // correct for merging several tiers' candidates for the same node (each
    // tier should contribute one canonical entry) but wrong here, because a
    // single `Options:`-style block can legitimately list one spelling
    // twice for two different forms — `du --help`'s bare `--time` and
    // valued `--time=WORD` rows, `ex --help` (vim)'s bare `-r` and
    // `-r (with file name)` rows, each pair with its own real description.
    // Running the whole list through identity-based rebucketing merged
    // those pre-existing, legitimate pairs into one row apiece and dropped
    // a real description every time — measured: `ex` lost 2 descriptions
    // with its flag count unchanged (two collapses cancelled out by two
    // genuinely new usage flags), `du` lost a flag outright. Only a usage-
    // derived flag is allowed to be judged redundant; a block-derived flag
    // is never rebucketed, never dropped, never has a field replaced — so
    // whatever the block scan already produced, however it shaped up,
    // survives byte-for-byte.
    // Reads `usage_lines` (physical, pre-join), not `result.usage` (the
    // logical, joined-for-display entries built above) — deliberately, so
    // the join introduced for rendering cannot change what this recovers.
    // `usage_segments` is line-shaped and self-contained (bracket-matching
    // and tokenizing within one string), so joined input should be
    // equivalent in practice, but there is no reason to make the [M-15]
    // flag grammar depend on that equivalence when the pre-join lines are
    // still sitting right here.
    if !usage_lines.is_empty() {
        for flag in extract_usage_flags(&usage_lines) {
            if result.flags.len() >= MAX_RECOVERED_ENTRIES {
                break;
            }
            if !flag_spelling_already_present(&flag, &result.flags) {
                result.flags.push(flag);
            }
            // else: this spelling (by short or by long) already names a
            // flag the block scan recovered — described or not, recovered
            // from real structure either way — so the usage-derived,
            // always-undescribed duplicate is simply not added. This is
            // "let the described version win" taken literally: the
            // existing entry is never touched, so it cannot lose a field it
            // already had.
        }
    }

    // Last, over everything both scans produced: the repeated-character
    // flag repair needs the whole node's flag list to answer its own
    // question (see [`repair_repeated_character_flags`]), so it cannot run
    // at the row that produced any one flag.
    // One pass over the document, shared by both repairs below: each of
    // them asks the same glued-and-delimited question once per surviving
    // flag, and asking it of the document directly is `O(candidates x
    // document)`. See [`GluedTokenIndex`].
    let glued_tokens = GluedTokenIndex::new(raw);
    repair_repeated_character_flags(&mut result.flags, &glued_tokens);
    // Then, over the same assembled list and for the same reason (it must
    // be able to read a flag's `Source`, which only exists once the flag is
    // built): the single-dash long-option repair. Ordered after the
    // repeated-character pass deliberately — that pass consumes `-vv` and
    // friends and rewrites them into `long`/`single_dash`, so by the time
    // this one runs the whole repeated-character family is already gone
    // from the `short && !long && Required` fingerprint the two share, and
    // the disjointness the two detectors assert about each other holds in
    // the fixes as well. The explicit condition-6 check below is kept
    // anyway, so the disjointness does not rest on the call order.
    repair_single_dash_long_options(&mut result.flags, &glued_tokens);
    // Third pass of the same kind, and last because it can only fill what
    // the two above have finished naming: descriptions that the document
    // wrote as free prose paragraphs instead of as option-table columns.
    backfill_prose_paragraph_descriptions(&mut result.flags, &lines);

    result.confidence = compute_confidence(total_entries, clean_entries, !result.usage.is_empty());
    result
}

/// Re-read every `-vv`-shaped flag in `flags` as the multi-character
/// single-dash option it is, instead of as its own first character carrying
/// a required value.
///
/// # The defect
///
/// `bpftrace`'s option table writes six rows and this parser produced four
/// flags from them:
///
/// ```text
///     -k             emit a warning when a bpf helper returns an error
///     -kk            check all bpf helper functions
///     -v                      verbose messages
///     -vv                     more verbose messages (max 2)
///     -d                      (dry run) debug info
///     -dd                     (dry run) verbose debug info
/// ```
///
/// [`parse_flag_spec`] has no way to read `-vv` as one name: `try_short`
/// takes the `v` and `try_value` glues the second one on as a required
/// value. So `-k`, `-v` and `-d` land correctly as booleans and `-kk`,
/// `-vv` and `-dd` land as *the same three letters again*, each carrying one
/// copy of its own letter — three real, separately-described switches that
/// are not in the tree under any spelling a user could type. Six of the
/// seed-2 audit's 94 verdicts are this defect (all five `.bt` wrappers
/// around `bpftrace`, plus `ntfsfallocate`, whose help text has the identical
/// `-v`/`-vv` pair).
///
/// # The rule, and why it needs the whole list
///
/// A flag is rewritten when **all** of these hold — the same four conditions
/// `xtask`'s `repeated_char` oracle counts the defect with, deliberately and
/// character for character, because that detector is meant to read zero once
/// this lands and it can only do that if the fix and the measurement agree
/// on what the defect is:
///
/// 1. it has a short spelling, no long name, and a `Required` value;
/// 2. the value is that short character repeated
///    ([`value_repeats_short`]);
/// 3. **another flag in the same node is the bare boolean spelling of the
///    same character** ([`documents_bare_boolean`]);
/// 4. the reconstructed token occurs glued and delimited in the tool's own
///    raw text ([`token_occurs_glued`]).
///
/// **Condition 3 is the whole safety argument, and it is why this is a
/// post-pass rather than a change to [`parse_flag_spec`].** Conditions 1, 2
/// and 4 alone are satisfied by `lessecho`'s real `[-nn]`, which is its
/// genuine "-n followed by a number" flag and a correct parse. Nothing about
/// the *token* separates the two: same length, same shape, same glued
/// spelling. What separates them is the document — `bpftrace` writes a row
/// for `-v` and a row for `-vv` with two different descriptions, while
/// `lessecho` writes `[-nn]` and never mentions a bare `-n` at all. A tool
/// that documents `-v` as taking no value has said, in its own words, that
/// `-vv` cannot be `-v` carrying a value. One fragment cannot see that;
/// the assembled list can.
///
/// The knowing false negative, measured on the fleet and left alone under
/// the no-false-positives rule: a repeated-character flag whose bare form the
/// tool never writes on its own row (`strace`'s `[-DDD]`,
/// `wpa_supplicant`'s `[-BddhKLqqstuvW]`) stays split, because the only
/// evidence that would admit it is the token's shape and `lessecho`'s `-nn`
/// has exactly that shape.
fn repair_repeated_character_flags(flags: &mut [Entity], glued_tokens: &GluedTokenIndex<'_>) {
    let booleans: Vec<char> = flags
        .iter()
        .filter(|f| f.value_kind == ValueKind::None)
        .filter_map(|f| f.short())
        .collect();
    for flag in flags.iter_mut() {
        let Some(short) = flag.short() else { continue };
        if flag.long().is_some() || flag.value_kind != ValueKind::Required {
            continue;
        }
        let Some(value) = flag.value_name.as_deref() else {
            continue;
        };
        if !value_repeats_short(short, value) {
            continue;
        }
        if !booleans.contains(&short) {
            continue;
        }
        let token = format!("-{short}{value}");
        if !glued_tokens.contains(&token) {
            continue;
        }
        // The whole run becomes one single-dash long spelling, replacing
        // the short-plus-glued-value pair the grammar produced: the name
        // is held bare and `Dashes::Single` is what puts one dash in front
        // of it at display time.
        flag.spellings = vec![Spelling::single_dash(&token[1..])];
        flag.value_name = None;
        flag.value_kind = ValueKind::None;
    }
}

/// True when `value` is one or more copies of `short` and nothing else.
///
/// `-vv` stores `"v"`, `-vvv` stores `"vv"`, `strace`'s `[-DDD]` stores
/// `"DD"`. The emptiness guard matters: an empty value is `Required` with
/// nothing in it, which `chars().all(..)` would call vacuously true.
/// Case-sensitive, like every other spelling comparison here — `-v` and `-V`
/// are different flags, so `-vV` is two flags glued, not one repeated.
fn value_repeats_short(short: char, value: &str) -> bool {
    !value.is_empty() && value.chars().all(|c| c == short)
}

/// What "word-shaped" means on either side of a glued token, for both
/// [`token_occurs_glued`] and [`GluedTokenIndex`] — one definition, so the
/// index and the scan cannot drift apart.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '-' || c == '_'
}

/// True when `candidate` occurs in `raw` as an isolated token: nothing
/// word-shaped immediately before or after it.
///
/// The twin of `xtask::existence::spelling_occurs`, and deliberately the
/// same rule: `value_name` alone cannot tell `-vv` from `-v v`, since
/// [`parse_flag_spec`] reads both into the identical fields, and only the
/// raw text says which one the tool wrote. Char-indexed throughout, never a
/// byte-offset `&str` slice — AGENTS.md's rule against slicing captured tool
/// output at a raw byte offset.
///
/// **This is the definition, not the hot path.** It scans the whole
/// document once per candidate, which is fine for one question and
/// quadratic for a document's worth of them; [`GluedTokenIndex`] answers
/// the same question from one pass over the document and is what the two
/// repairs call. This form stays because it is the readable statement of
/// the predicate, because the index falls back to it for the one candidate
/// shape the index cannot key (see [`GluedTokenIndex::contains`]), and
/// because `indexed_form_agrees_with_scanning_form` pins the two together.
fn token_occurs_glued(raw: &str, candidate: &str) -> bool {
    let hay: Vec<char> = raw.chars().collect();
    let needle: Vec<char> = candidate.chars().collect();
    if needle.is_empty() || hay.len() < needle.len() {
        return false;
    }
    (0..=(hay.len() - needle.len())).any(|start| {
        let end = start + needle.len();
        hay[start..end] == needle[..]
            && (start == 0 || !is_word_char(hay[start - 1]))
            && (end == hay.len() || !is_word_char(hay[end]))
    })
}

/// One document's answer to every [`token_occurs_glued`] question the two
/// flag repairs will ask of it, built in one pass over the document.
///
/// # Why it exists
///
/// [`token_occurs_glued`] scans the whole document per candidate, and both
/// repairs ask it once per surviving flag. That was affordable while the
/// conditions in front of it admitted a handful of candidates per node;
/// widening them (v0.4.0's single-dash long-option work) put ~679
/// candidates in front of it for one tool, against `ffplay`'s 752 KB of
/// help text, and `mandible --doctor ffplay` went from ~1.4 s to 3.2 s.
/// The cost is `O(candidates x document)` and the document is the part
/// nobody controls, so the fix is to stop re-reading it.
///
/// # The structure, and why this one
///
/// Every maximal run of word characters ([`is_word_char`]) in the document,
/// keyed by the run's own text. That is the whole index; a lookup is a hash
/// of the candidate's leading run.
///
/// It works because of what the predicate's two boundary conditions say
/// about a match. A candidate always opens on a word character (`-`), so at
/// any position where it matches, the document's run of word characters
/// starting there is *exactly* the candidate's own leading run: the
/// left boundary makes that position a run start, and the candidate's first
/// non-word character — or, if it has none, the right boundary — is where
/// that run ends. So "does this candidate occur glued and delimited" is
/// "is the candidate's leading run a run of this document, and does the
/// document continue with the candidate's remainder". For the common
/// candidate, all word characters (`-help`, `-vv`), the remainder is empty
/// and a run being maximal *is* both boundary conditions holding, so the
/// hash lookup alone is the answer.
///
/// A candidate carrying a non-word character (`-foffload=<targets>`, the
/// glued-value shape [`split_glued_value`] admits) needs the remainder
/// checked against the text after each occurrence of its leading run —
/// hence the offsets, and hence a map rather than a set. That list is as
/// long as that one run's occurrence count, not as long as the document.
struct GluedTokenIndex<'a> {
    /// The document this was built from, for the fallback in
    /// [`GluedTokenIndex::contains`].
    raw: &'a str,
    /// Every maximal run of word characters in `raw`, keyed by the run's
    /// text and valued by the byte offset just past each occurrence of it.
    runs: std::collections::HashMap<&'a str, Vec<usize>>,
}

impl<'a> GluedTokenIndex<'a> {
    /// One pass over `raw`, cutting it at every boundary between a word
    /// character and a non-word one.
    ///
    /// The offsets come from `char_indices`, so every slice taken here is
    /// taken at a character boundary by construction — and is taken through
    /// `get`, which returns `None` rather than panicking, so AGENTS.md's
    /// rule about byte-offset slicing of captured tool output holds by
    /// construction *and* by API even if that reasoning is ever wrong.
    fn new(raw: &'a str) -> Self {
        let mut runs: std::collections::HashMap<&'a str, Vec<usize>> =
            std::collections::HashMap::new();
        let mut open: Option<usize> = None;
        for (offset, ch) in raw.char_indices() {
            if is_word_char(ch) {
                open.get_or_insert(offset);
            } else if let Some(begin) = open.take() {
                if let Some(run) = raw.get(begin..offset) {
                    runs.entry(run).or_default().push(offset);
                }
            }
        }
        // A run that reaches the end of the document closes there.
        if let Some(begin) = open {
            if let Some(run) = raw.get(begin..) {
                runs.entry(run).or_default().push(raw.len());
            }
        }
        Self { raw, runs }
    }

    /// Exactly [`token_occurs_glued`]`(self.raw, candidate)`, without
    /// re-reading the document.
    fn contains(&self, candidate: &str) -> bool {
        let head = candidate
            .find(|c| !is_word_char(c))
            .unwrap_or(candidate.len());
        // A candidate that opens on a non-word character has no leading run
        // to key on. Both callers ask about a `-`-led token so nothing
        // reaches this in practice, but the predicate is defined for every
        // string and the scanning form answers those correctly; keeping the
        // fallback is cheaper than narrowing the type.
        if head == 0 {
            return token_occurs_glued(self.raw, candidate);
        }
        let (run, rest) = candidate.split_at(head);
        let Some(ends) = self.runs.get(run) else {
            return false;
        };
        if rest.is_empty() {
            // The key matched a *maximal* run, so there is a non-word
            // character (or the end of the document) on both sides of it
            // already — which is the whole predicate.
            return true;
        }
        ends.iter().any(|&end| {
            self.raw
                .get(end..)
                .and_then(|after| after.strip_prefix(rest))
                .is_some_and(|tail| !tail.chars().next().is_some_and(is_word_char))
        })
    }
}

/// The fewest characters a swallowed tail must carry before it is read as
/// the rest of a single-dash long option's name.
///
/// Two, and the same two `xtask::single_dash_long::MIN_SWALLOWED_CHARS`
/// counts the defect with — the two numbers are one number, and the
/// duplication is the same deliberate one `MIN_CLUSTER_MEMBERS` carries
/// against `bundling::MIN_BUNDLED_MEMBERS`. At one swallowed character the
/// shape is genuinely ambiguous: `rpcgen`'s `-Ss`, `xxd`'s `-ps`, `sg_map`'s
/// `-st`, `mandoc`'s `-ac`, `which`'s `-as` are all two-character
/// single-dash tokens and roughly half of that population is a correct
/// parse of a real character-argument flag. Deliberate lost recall.
const MIN_SWALLOWED_NAME_CHARS: usize = 2;

/// Re-read every `-help`-shaped option-table row as the single-dash long
/// option it is, instead of as its own first character carrying a required
/// value.
///
/// # The defect
///
/// `qemu-arm64-static`'s option table writes its long options and its
/// genuine value-taking short flags on adjacent rows, separated by nothing
/// but a space:
///
/// ```text
/// -h                                        print this help
/// -help
/// -g port              QEMU_GDB             wait gdb connection to 'port'
/// -cpu model           QEMU_CPU             select CPU (-cpu help for list)
/// -one-insn-per-tb     QEMU_ONE_INSN_PER_TB run with one guest instruction per emulated TB
/// -version             QEMU_VERSION         display version information and exit
/// ```
///
/// [`parse_flag_spec`] has no way to read `-help` as one name: `try_short`
/// takes the `h` and `try_value` glues the rest on as a required value, so
/// the tree gains a second `-h` carrying the value `"elp"` and loses
/// `-help` under any spelling a user could type. Eleven of that one tool's
/// rows go the same way — `-cpu` → `-c` + `"pu"`, `-version` → `-v` +
/// `"ersion"` — while `-g port`, `-L path` and `-R size` on the same rows
/// are entirely correct. A fleet sweep of this machine's `PATH` measured
/// the family at **132 tools and 8,784 flags**, 17.6% of every flag
/// extracted.
///
/// # The rule
///
/// A flag is rewritten when **all** of these hold — the same seven
/// conditions `xtask`'s `single_dash_long` oracle counts the defect with,
/// deliberately and character for character, because that detector is meant
/// to read zero once this lands and it can only do that if the fix and the
/// measurement agree on what the defect *is*:
///
/// 1. it is **option-table-sourced** ([`Source::HelpText`], never
///    [`Source::HelpTextSynopsis`]);
/// 2. it has a short spelling, no long name, and a `Required` value;
/// 3. the swallowed text's **name half** — everything before the first `=`,
///    or the whole tail when there is no `=` ([`split_glued_value`]) — is
///    **option-name-shaped** ([`is_option_name_tail`]);
/// 4. that name half is at least [`MIN_SWALLOWED_NAME_CHARS`] characters;
/// 5. the reconstructed **name token** is **uniformly lowercase**
///    ([`token_is_uniformly_lowercase`]);
/// 6. the tail is not the flag's own character repeated — the
///    [`repair_repeated_character_flags`] family, handed off rather than
///    claimed twice;
/// 7. the reconstructed token — name **and** glued value — occurs glued and
///    delimited in the tool's own raw text ([`token_occurs_glued`]).
///
/// # The glued `=value` half, and why the first version missed it
///
/// `dbiprof` writes one option table and this parser used to repair half of
/// it:
///
/// ```text
///     -number=N        show top N, defaults to 10
///     -sort=S          sort by S, defaults to total
///     -reverse         reverse the sort
///     -match=K=V       for filtering, see docs
/// ```
///
/// `-reverse` came out right and `-number=N` came out as `-n` carrying
/// `"umber=N"`, in the same table, on adjacent rows. The reason is entirely
/// in condition 3: it asked whether the *whole* swallowed run was an option
/// name, and `umber=N` is not one — it is an option name **plus the value
/// spec the tool glued onto it**. `=` is the one character that says where
/// the name stops, so the fix is to read the two halves separately rather
/// than to admit `=` into [`is_option_name_tail`], which would also admit
/// `-E var=value` and every other value spec that carries one.
///
/// **Condition 5 is unchanged in substance and is still the whole safety
/// argument.** It is measured over the name token (`-number`) instead of
/// the full token (`-number=N`) because the value half is now known to be a
/// value half — and a value spec shouts (`-foffload=<targets>`,
/// `-print-file-name=<lib>`) without saying anything about the flag. The
/// population it must stay silent on is unmoved by that: Ghostscript's real
/// `-sDEVICE=png16m` is a genuine glued short whose *name* token is
/// `-sDEVICE`, `cpp`'s `-DMACRO=value` is `-DMACRO`, `-Wl,-rpath=…` is
/// `-Wl,…` (rejected by condition 3 before case is even consulted) — every
/// one of them shouts on the left of the `=`, which is exactly where the
/// convention puts the argument, and every one is still rejected. What the
/// change buys is the mirror-image population, whose name half is a
/// lowercase *word*: `dbiprof`'s `-number`/`-sort`/`-match`/`-exclude`,
/// `gcc`'s `-foffload`, `-print-file-name`, `-print-prog-name`, `-specs`,
/// `-std` and `-save-temps=<arg>`.
///
/// Unlike the spaced-value case below, the value spec here is **kept**: the
/// document wrote it on the same token, so `-foffload` stays a
/// value-taking flag named `<targets>` rather than becoming a boolean.
///
/// **Conditions 1 and 5 are the whole safety argument, and 5 is why this
/// cannot be a change to [`parse_flag_spec`].** Conditions 2, 3, 4, 6 and 7
/// are satisfied character for character by the GCC/Clang glued-value
/// convention — `cargo -Zscript`, `rpcgen -Dname`, `makewhatis -Tutf8`,
/// `perl -Idirectory`, `find -Olevel`, `cc -oOUTFILE`, `gcc -DMACRO` —
/// thousands of **correct** parses fleet-wide, every one of which this must
/// leave alone. What separates them is case, and only case: the convention
/// is an uppercase flag letter with its argument glued on, while a long
/// option is a *word* and words in `--help` output are lowercase.
/// Condition 5 is measured over the whole token rather than the tail alone,
/// deliberately, and the difference is `-oOUTFILE`: its flag letter is
/// lowercase and only the argument shouts, so a tail-only test would admit
/// it and destroy a correct parse. Condition 1 is what keeps the entire
/// bundled-short population out (`rpcbind`'s `[-adhilswfr]` is
/// all-lowercase, unsorted and indistinguishable from a long option on
/// every other condition) — `parse_bundled_shorts` owns that shape from the
/// synopsis, and the identical shape from an option table is this family.
///
/// # Why `_` is a name character
///
/// Condition 3 used to reject `_`, on the theory that it "also appears in
/// glued value placeholders". It does — and so does every letter of the
/// alphabet. `_` is a **word separator inside a name**, the same job `-`
/// does, and none of the conditions above is measured on which separator
/// a name spells its word breaks with: `-DFOO_BAR` is still rejected by
/// condition 5, `-oOUT_FILE` still by condition 5 read over the whole
/// token, `-o out_file` still by condition 7 (it never occurs glued), and
/// `-d item_a[,...]` still by condition 3's own punctuation test.
///
/// Measured on a full-`PATH` sweep of this machine (2,254 tools, aarch64
/// Ubuntu 24.04), admitting `_` moves **17 tools and 604 flag spellings**,
/// and moves nothing else: no tool appeared or disappeared, no tool
/// changed status or tier, and no flag was lost — the field-level
/// `sweep-diff` reports `0 lost across 0 tool(s)`. Every one of the 604
/// recovered names was then checked against its own tool's raw capture,
/// and **all 604 occur as the leading token of a row the tool itself
/// writes** — `clang -fchar8_t`/`-fno-char8_t`, `llvm-install-name-tool
/// -add_rpath`/`-delete_all_rpaths`, `llvm-lipo -verify_arch`,
/// `llvm-otool -chained_fixups`, `ffmpeg -pix_fmts`/`-filter_script`,
/// `dbiprof -case_sensitive`. There were no counter-examples.
///
/// **ffplay and ffprobe are 97% of it**, and they are the case worth
/// stating explicitly because their rows carry a value spec in a
/// *space-separated* column plus a capability column:
///
/// ```text
///   -is_avc            <boolean>    .D.V..X.... is avc (default false)
///   -grab_x            <int>        .D......... Initial x coordinate. (from 0 to INT_MAX)
/// ```
///
/// Neither column is at risk, because neither was ever in `value_name` —
/// the grammar stored the swallowed name half there (`-i` + `"s_avc"`)
/// and both columns went into the *description*, which this repair does
/// not touch. `ffplay`'s tree keeps the same 1,136 flags and the same
/// 1,135 descriptions, byte for byte, before and after; 679 of them stop
/// being fabricated shorts. The rows that were already recovered on the
/// unmodified parser — `-idct`, `-threads`, `-debug`, whose names carry
/// no underscore — have always read exactly this way, so this is the
/// underscore rows joining them rather than a new behaviour.
///
/// # A rejected alternative: "is the candidate short documented?"
///
/// Recorded because it is the obvious next idea and it is **wrong**:
/// allow the long reading only when the tool's help documents no bare row
/// for the candidate short — `dbiprof` documents no `-c`, so `-c` is
/// fabricated there and `-case_sensitive` wins.
///
/// It does not discriminate. Measured over the 604 spellings above it
/// refuses 111 of them, and every single one of the 111 is a documented
/// row token — a 100% false-refusal rate, buying nothing. `ffplay`
/// documents `-f fmt` **and** `-filter_threads`; `-i input_file` **and**
/// `-is_avc`. A tool documenting both a short and a long option that
/// starts with the same letter is the ordinary case, not a suspicious
/// one. Worse, as a general rule it would revert work already shipped:
/// across those same 17 tools it refuses **632 of the 8,260** single-dash
/// long options the parser already recovers, `ffplay -help` among them —
/// the exact `-h` beside `-help` coexistence
/// `xtask::single_dash_long`'s own doc comment opens with. What the idea
/// is reaching for is already supplied, and supplied better, by
/// conditions 5 and 7 together.
///
/// # What this deliberately does not claim
///
/// Named here rather than discovered later, and each one is a place the
/// oracle is silent too — this fix claims **nothing** the detector does
/// not:
///
/// - **Uppercase-led single-dash long options** (`-Wall`, `-Xlint`).
///   Excluded by condition 5, which cannot tell them from `-Zscript`.
/// - **`ip`'s bracketed abbreviations** (`-h[uman-readable]`, `-b[atch]`,
///   `-rc[vbuf]`). The raw text writes brackets, so the grammar records
///   `ValueKind::Optional` — a value spec a human deliberately typed — and
///   condition 2 never admits it.
/// - **Tails carrying layout punctuation.** `sg_emc_trespass` writes
///   `-hr: Set Honor Reservation bit`, so the tail is `"r:"` and condition
///   3 rejects it. No tail-shape rule can claim that without also admitting
///   every value spec that leaks punctuation.
/// - **Tails whose *name* half carries brackets or other value-spec
///   punctuation.** Condition 3 still rejects `[`, `<`, `,`, `.` and `/`
///   in the name half, for the same reason the oracle does — `-d
///   item[,...]` and `-b{blocksize}` are value specs, not names. Only `=`
///   is read structurally, and only as the boundary between the two halves.
/// - **A tail that ends at the `=` with nothing after it** — refused
///   outright by [`split_glued_value`], which has no evidence for either
///   reading of it.
/// - **One-character tails** ([`MIN_SWALLOWED_NAME_CHARS`]).
///
/// The value a rewritten row's *real* spaced argument named (`-cpu model`
/// documents a `model`) is not recovered: by the time the fragment reached
/// here the grammar had already stored `"pu"` and dropped `"model"` on the
/// floor. The flag becomes the boolean `-cpu` rather than `-c` taking
/// `"pu"` — the correct **name** under a missing value spec, which is
/// strictly better than a fabricated name under a fabricated value spec,
/// and is exactly what `repair_repeated_character_flags` does with `-vv`.
fn repair_single_dash_long_options(flags: &mut [Entity], glued_tokens: &GluedTokenIndex<'_>) {
    for flag in flags.iter_mut() {
        // 1. Option-table-sourced, never synopsis.
        if !flag.provenance.sources.contains(&Source::HelpText)
            || flag.provenance.sources.contains(&Source::HelpTextSynopsis)
        {
            continue;
        }
        // 2. A bare short flag carrying a required value.
        let Some(short) = flag.short() else { continue };
        if flag.long().is_some() || flag.value_kind != ValueKind::Required {
            continue;
        }
        let Some(tail) = flag.value_name.as_deref() else {
            continue;
        };
        // 3a. Split the swallowed text at the first `=` — see
        //     [`split_glued_value`]. Without a `=` the name half is the
        //     whole tail and every condition below reads exactly as it did
        //     before this split existed.
        let Some((name_tail, glued_value)) = split_glued_value(tail) else {
            continue;
        };
        // 4. Enough *name* to be a name rather than a character argument.
        if name_tail.chars().count() < MIN_SWALLOWED_NAME_CHARS {
            continue;
        }
        // 3. The name half is option-name-shaped.
        if !is_option_name_tail(name_tail) {
            continue;
        }
        // 6. Not the repeated-character family, which is the other repair's.
        if value_repeats_short(short, tail) {
            continue;
        }
        let name_token = format!("-{short}{name_tail}");
        // 5. Uniformly lowercase — the only thing separating this from the
        //    glued-value convention. See this function's doc comment.
        if !token_is_uniformly_lowercase(&name_token) {
            continue;
        }
        // 7. The whole token — name *and* glued value — occurs, glued and
        //    delimited, in the raw text. Last because it is the only
        //    condition that reads the document at all — one hash lookup
        //    now, against an index built once for the whole document
        //    ([`GluedTokenIndex`]), rather than a scan per candidate.
        if !glued_tokens.contains(&format!("-{short}{tail}")) {
            continue;
        }
        // The run up to the `=` becomes one single-dash long spelling,
        // replacing the short-plus-glued-name pair the grammar produced:
        // the name is held bare and `Dashes::Single` is what puts one dash
        // in front of it at display time.
        flag.spellings = vec![Spelling::single_dash(&name_token[1..])];
        match glued_value {
            // `-foffload=<targets>`: the document wrote the value spec
            // itself, so it survives the repair on the flag it belongs to.
            Some(value) => flag.value_name = Some(value.to_string()),
            // `-cpu model`: the value was dropped on the floor by the
            // grammar long before this ran, so the flag becomes the
            // boolean it is correctly *named* rather than keeping a
            // fabricated one. See this function's doc comment.
            None => {
                flag.value_name = None;
                flag.value_kind = ValueKind::None;
            }
        }
    }
}

/// Split a swallowed tail into the option-name half and the glued value
/// half: `"umber=N"` → `("umber", Some("N"))`, `"elp"` → `("elp", None)`.
///
/// `None` — refuse the row entirely — when the tail ends at the `=` with
/// nothing after it (`"oo="`). A `Required` value whose spec is the empty
/// string is a shape nothing in the fleet was measured on, and inventing
/// either reading of it (boolean, or a value named `""`) would be a claim
/// this repair has no evidence for.
///
/// **Splitting at the *first* `=` is what makes `dbiprof`'s `-match=K=V`
/// come out right**: the name ends at the first one and everything after it
/// is the value spec the tool wrote, `=` included.
///
/// The twin of `xtask::single_dash_long::split_glued_value`, character for
/// character, for the reason [`repair_single_dash_long_options`]'s doc
/// comment gives.
fn split_glued_value(tail: &str) -> Option<(&str, Option<&str>)> {
    match tail.split_once('=') {
        Some((_, "")) => None,
        Some((name, value)) => Some((name, Some(value))),
        None => Some((tail, None)),
    }
}

/// True when `tail` could be the rest of a single-dash long option's name:
/// ASCII alphanumerics, `-` and `_`, with at least one ASCII letter in it.
///
/// The twin of `xtask::single_dash_long::is_option_name_tail`, character
/// for character. The letter requirement is what stops a glued *numeric*
/// argument (`-b4096`, `-j8`) from riding in on a run that is technically
/// alphanumeric. Everything else is rejected because a long option's name
/// does not contain it: `:` (`sg_emc_trespass`'s layout-mangled `-hr:`),
/// `[`/`{`/`<`/`,` (`-d item[,...]`, `-b{blocksize}`), `.` and `/` (paths).
///
/// `_` is admitted on the same footing as `-`, for the reason given in
/// [`repair_single_dash_long_options`]'s "Why `_` is a name character"
/// section: it separates words inside a name, and every condition that
/// makes this repair safe is measured over the token, not over which
/// separator the name happens to spell its word breaks with.
///
/// `=` never reaches here: [`split_glued_value`] has already consumed it as
/// the boundary between the name and its glued value spec, so what this
/// sees is only ever the name half.
fn is_option_name_tail(tail: &str) -> bool {
    tail.chars().any(|c| c.is_ascii_alphabetic())
        && tail
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// True when `token` carries no ASCII uppercase letter at all — the
/// discriminator against the GCC/Clang glued-value convention, whose whole
/// population is an uppercase flag letter with its argument glued on
/// (`-Zscript`, `-Dname`, `-Tutf8`, `-Idirectory`, `-Olevel`, `-DMACRO`,
/// `-oOUTFILE`, `-Wall`).
///
/// The twin of `xtask::single_dash_long::token_is_uniformly_lowercase`,
/// and measured over the *whole* token rather than only the tail for the
/// reason recorded there: `-oOUTFILE`'s flag letter is lowercase and only
/// its argument shouts, so a tail-only test would admit it.
fn token_is_uniformly_lowercase(token: &str) -> bool {
    !token.chars().any(|c| c.is_ascii_uppercase())
}

/// A ratio computed from an option-table sample of exactly **one** row is
/// not a measurement, and treating it as one produced a real bug:
/// `ssh-keygen --help` writes nothing but a 30-line usage synopsis, and the
/// final wrapped continuation line of its last invocation form
/// (`-n namespace -s signature_file [-r krl_file] [-O option]`) opens with a
/// dash, which the usage-block scanner's own curl-shaped-flags guard
/// (above, "a continuation line that itself reads as a flag entry ends the
/// usage block") correctly hands to the generic flags-block scanner — that
/// guard exists precisely so a tool like `curl`, which runs 13 real flags
/// straight into its usage line with no heading, does not lose them. For
/// `curl` the handoff finds a real table. For `ssh-keygen` there is nothing
/// real to find — the line is a wrap artifact, not a table row — so the
/// scanner reads exactly one entry, fails to parse it cleanly (it is not
/// one), and `0 / 1` reads as "the grammar understood *nothing*", a
/// confident zero indistinguishable in the footer from `find` (19 rows, 2
/// clean) or `ip` (11 rows, 7 clean) — tools where the ratio is a real
/// measurement over a real sample.
///
/// **A one-row sample is folded into a dedicated fallback, `0.5`, and
/// deliberately *not* the same zero-row fallback that already exists
/// below.** An earlier version of this fix folded `total_entries <= 1` into
/// the zero-row arm (`had_usage ? 0.5 : 0.15`), which inherits that arm's
/// usage-line penalty for a reason that does not apply here: the penalty
/// exists because finding *no* structure at all *and* no usage line is a
/// stronger signal of a bad parse than finding no structure but at least a
/// usage line. A one-row sample is a different situation — real structure
/// *was* found, there just isn't enough of it to divide by.
///
/// **Measured, not asserted.** A one-off scan of every frozen capture in
/// `audit/queue-captures/` (2,301 tools; untracked, local-only, not part of
/// CI) against `total_entries == 1` splits four ways: 16 tools clean/with a
/// usage line (already `1.0`, unaffected either way), 12 unclean/with a
/// usage line and 11 unclean/without one (both were a confident `0.0`
/// before this fix — `ssh-keygen` is one of these 23 — and land at `0.5`
/// under both the buggy and the corrected rule, so both versions of this
/// fix fix them), and — the case that proves the *first* version of this
/// fix wrong — 7 tools clean/without a usage line (`byobu-disable`,
/// `byobu-enable`, `bzless`, `bzmore`, `debconf-apt-progress`,
/// `validlocale`, `xdg-user-dir`), each a real, cleanly-parsed single flag
/// that a version folding `total_entries <= 1` into the zero-row arm took
/// from a correct `1.0` down to a fabricated `0.15`, stamping `low
/// confidence: 15% parsed` on a document that parsed fine — the same class
/// of dishonesty this fix exists to remove, in the other direction. `0/1`
/// and `1/1` are equally uninformative regardless of whether a usage line
/// happened to be present, so both land at the same `0.5` "found
/// structure, cannot rate it" value the zero-row arm already uses for its
/// *better* case, independent of `had_usage`. Net effect of the corrected
/// rule against the true pre-this-PR baseline, same scan: 0 tools gain a
/// badge, 23 lose one (the tools above whose one row was unclean).
///
/// **A spot-check of five of those 23** (`e4defrag`, `unix_chkpwd`,
/// `iscsi_discovery`, `finalrd`, `rust-gdbgui`), reading each tool's raw
/// captured text directly: none of them is a case where silence hides a
/// real problem. Each one's single recovered "entry" is noise from
/// something that is not option-table structure at all — a setuid
/// helper's refusal message (`unix_chkpwd`: "This binary is not designed
/// for running in this way"), a mount permission error (`finalrd`), an
/// IP-address parser rejecting `--help` as a bad address
/// (`iscsi_discovery`), a `Usage\t:` line whose tab-before-colon spelling
/// this grammar's marker recognizer doesn't match (`e4defrag`), and a
/// wrapper script's prose usage note with no `Usage:` marker at all
/// (`rust-gdbgui`). None of these documents ever had real option-table
/// content for `compute_confidence` to rate; the previous confident `0.0`
/// was exactly as uninformative as the new silent `0.5`, just dressed as a
/// finding instead of admitting it wasn't one. The actual gap here — a
/// handful of grammar/tier-routing cases that misread an error message or
/// an unrecognized usage-marker spelling as one flag — is real but
/// pre-existing and out of this fix's scope (it is not something
/// `compute_confidence` can fix by choosing a different number).
const SINGLE_ROW_SAMPLE_CONFIDENCE: f32 = 0.5;

fn compute_confidence(total_entries: usize, clean_entries: usize, had_usage: bool) -> f32 {
    if total_entries == 1 {
        return SINGLE_ROW_SAMPLE_CONFIDENCE;
    }
    if total_entries == 0 {
        return if had_usage { 0.5 } else { 0.15 };
    }
    (clean_entries as f32 / total_entries as f32).clamp(0.0, 1.0)
}

/// The largest indent at which a line may still be read as flush-left
/// document prose rather than as part of some block's own body.
///
/// Prose paragraphs that document an option are written at the document's
/// own margin; the same sentence *indented under an option row* is that
/// option's continuation text, and already belongs to whichever flag the
/// row named. Keeping the two apart is the whole reason this is a bound
/// and not simply "any line": `java`, `jdeps` and `rg` all write
/// "The --x option …" sentences deep inside another flag's description
/// column, and reading those as standalone paragraphs would attach one
/// flag's prose to a different flag.
const MAX_PROSE_PARAGRAPH_INDENT: usize = 3;

/// Fill in descriptions a document wrote as **prose paragraphs keyed by
/// option name**, rather than as text in the option table's own
/// description column.
///
/// `jdeprscan --help` is the specimen. Its `options:` block is a bare list
/// of spellings with no description column at all, and every option's
/// prose lives further down the document in its own flush-left paragraph:
///
/// ```text
/// options:
///         --for-removal
///   -l    --list
/// …
/// The --for-removal option limits scanning or listing to APIs that are
/// deprecated for removal. Cannot be used with a release value of 6, 7, or 8.
///
/// The --list (-l) option prints out the set of deprecated APIs. No scanning is done,
/// so no directory, jar, or class arguments should be provided.
/// ```
///
/// The table parses correctly — the spellings are all recovered — and then
/// every description is dropped on the floor, because nothing in the
/// grammar ever revisits a flag with text found later in the document
/// (measured: 8 flags, 0.0% with text). This is that revisit, and it is a
/// pass over the assembled flag list for the same reason
/// [`repair_repeated_character_flags`] and
/// [`repair_single_dash_long_options`] are: the question it answers needs
/// the whole node's flags, so it cannot be answered at the row that
/// produced any one of them.
///
/// Shape-keyed, never tool-keyed (spec §1). A paragraph qualifies when:
///
/// 1. Every one of its lines sits at indent ≤ [`MAX_PROSE_PARAGRAPH_INDENT`]
///    and none of them starts with `-`, so an option table's own rows and
///    an option's indented continuation text can never be read as one.
/// 2. Its first line opens `The <spelling> option …` — an article, one
///    option spelling, an optional parenthesised alias list, then the word
///    `option`, `flag` or `switch`. That is a *reference* to an option, the
///    one form in which running prose names one unambiguously.
///
/// Two invariants bound what this can cost, and both matter more than the
/// recall it gives up:
///
/// - **It never creates a flag.** A spelling that names nothing already in
///   `flags` is ignored, so a paragraph mentioning an option the tool did
///   not table (`apt-ftparchive`'s `--source-override`) cannot fabricate
///   one — the invention class spec §7 Tier B forbids.
/// - **It never overwrites a description.** Only a flag whose description
///   is `None` can be filled, so a table that already said something keeps
///   saying it (`apropos`'s `--regex` is described in its own table *and*
///   mentioned in a trailing paragraph; the table wins, untouched).
///
/// Matching is by *any* spelling the reference names, primary or
/// parenthesised, which is what makes it independent of how well the table
/// row parsed: jdeprscan's `-l    --list` row yields a flag with
/// `short: 'l'` and no long name at all, and `The --list (-l) option …`
/// still finds it through the `-l` in the parenthetical.
fn backfill_prose_paragraph_descriptions(flags: &mut [Entity], lines: &[&str]) {
    if flags.is_empty() {
        return;
    }
    let mut i = 0usize;
    while i < lines.len() {
        if lines[i].trim().is_empty() {
            i += 1;
            continue;
        }
        let start = i;
        while i < lines.len() && !lines[i].trim().is_empty() {
            i += 1;
        }
        let paragraph = &lines[start..i];
        if !paragraph.iter().all(|l| {
            leading_whitespace(l) <= MAX_PROSE_PARAGRAPH_INDENT && !l.trim_start().starts_with('-')
        }) {
            continue;
        }
        let Some(spellings) = prose_option_reference(paragraph[0]) else {
            continue;
        };
        let text = paragraph
            .iter()
            .map(|l| l.trim())
            .collect::<Vec<_>>()
            .join(" ");
        let Some(description) = non_empty_text(&text) else {
            continue;
        };
        for flag in flags.iter_mut() {
            if flag.description.is_some() {
                continue;
            }
            if spellings.iter().any(|s| flag_answers_to_spelling(flag, s)) {
                flag.description = Some(description.clone());
                break;
            }
        }
    }
}

/// Every option spelling named by a paragraph-opening option *reference* —
/// `The --list (-l) option …` → `["--list", "-l"]` — or `None` when the
/// line does not open with one.
///
/// Grammar, all of it required and in this order: an optional article
/// (`The`/`A`/`An`), one dash-led spelling, an optional parenthesised list
/// of further dash-led spellings, then the literal word `option`, `flag` or
/// `switch`. The trailing noun is what distinguishes a reference from a
/// sentence that merely happens to start with a flag-shaped token, and the
/// leading article keeps it clear of an option *table* row, which starts
/// with the spelling itself.
fn prose_option_reference(line: &str) -> Option<Vec<String>> {
    let mut words = line.split_whitespace().peekable();
    let first = words.peek()?;
    if matches!(*first, "The" | "A" | "An" | "the" | "a" | "an") {
        words.next();
    }
    let primary = words.next()?;
    if !primary.starts_with('-') || primary.len() < 2 {
        return None;
    }
    let mut spellings = vec![primary.to_string()];
    // An optional parenthesised alias list: `(-? -h)`, `(-l)`.
    if words.peek().is_some_and(|w| w.starts_with('(')) {
        let mut closed = false;
        for word in words.by_ref() {
            let inner = word.trim_start_matches('(').trim_end_matches(')');
            if inner.starts_with('-') && inner.len() >= 2 {
                spellings.push(inner.to_string());
            }
            if word.ends_with(')') {
                closed = true;
                break;
            }
        }
        if !closed {
            return None;
        }
    }
    let noun = words.next()?;
    if !matches!(noun, "option" | "flag" | "switch") {
        return None;
    }
    Some(spellings)
}

/// True if `flag` is the flag `spelling` names — `--list` against its
/// `long`, `-l` against its `short`, and a single-dash long option
/// (`-print-sysroot`) against its long spelling when the entity says
/// that is how the tool spells it.
fn flag_answers_to_spelling(flag: &Entity, spelling: &str) -> bool {
    if let Some(long) = spelling.strip_prefix("--") {
        return !long.is_empty() && flag.long() == Some(long) && !flag.single_dash();
    }
    let Some(rest) = spelling.strip_prefix('-') else {
        return false;
    };
    if rest.is_empty() {
        return false;
    }
    let mut chars = rest.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if flag.short() == Some(c) {
            return true;
        }
    }
    flag.single_dash() && flag.long() == Some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- compute_confidence's one-row-sample fallback -------------------

    /// Pins all four `total_entries == 1` combinations. The regression
    /// this guards against: an earlier version of this fix folded
    /// `total_entries <= 1` into the *zero-row* fallback (`had_usage ? 0.5
    /// : 0.15`), which silently applied that arm's usage-line penalty to a
    /// case it was never designed for — a one-row sample that parsed
    /// *cleanly* and had no usage line dropped from a correct `1.0` to a
    /// fabricated-low `0.15`, measured on 10 real tools (`byobu-disable`,
    /// `bzip2recover`, `debconf`, `uvicorn`, ... — see this function's own
    /// doc comment). A single row is uninformative regardless of clean/dirty
    /// or usage-line presence, so all four combinations below must land at
    /// the same `SINGLE_ROW_SAMPLE_CONFIDENCE`.
    #[test]
    fn a_single_row_sample_is_uninformative_regardless_of_usage_or_cleanliness() {
        assert_eq!(compute_confidence(1, 0, true), SINGLE_ROW_SAMPLE_CONFIDENCE);
        assert_eq!(compute_confidence(1, 1, true), SINGLE_ROW_SAMPLE_CONFIDENCE);
        assert_eq!(
            compute_confidence(1, 0, false),
            SINGLE_ROW_SAMPLE_CONFIDENCE
        );
        assert_eq!(
            compute_confidence(1, 1, false),
            SINGLE_ROW_SAMPLE_CONFIDENCE
        );
    }

    /// The zero-row fallback is untouched by the one-row fix: it still
    /// carries its own usage-line penalty (long-standing, calibrated
    /// behavior, not this fix's to move).
    #[test]
    fn a_zero_row_sample_keeps_its_usage_line_penalty() {
        assert_eq!(compute_confidence(0, 0, true), 0.5);
        assert_eq!(compute_confidence(0, 0, false), 0.15);
    }

    /// A real sample (two or more rows) is untouched: still a plain
    /// ratio, not folded into either fallback. `find`'s real shape (19
    /// rows, 2 clean) must keep dividing.
    #[test]
    fn a_two_or_more_row_sample_still_divides() {
        assert!((compute_confidence(19, 2, true) - (2.0 / 19.0)).abs() < 1e-6);
        assert_eq!(compute_confidence(4, 4, false), 1.0);
        assert_eq!(compute_confidence(4, 0, false), 0.0);
    }

    // --- the repeated-character flag repair -----------------------------

    /// `bpftrace`'s real troubleshooting block, byte-exact from
    /// `corpus/killsnoop.bt/audit-seed2/help.stderr.txt`. Four rows, four
    /// real flags; before the repair the tree had two.
    const BPFTRACE_TROUBLESHOOTING: &str = concat!(
        "TROUBLESHOOTING OPTIONS:\n",
        "    -v                      verbose messages\n",
        "    -vv                     more verbose messages (max 2)\n",
        "    -d                      (dry run) debug info\n",
        "    -dd                     (dry run) verbose debug info\n",
    );

    #[test]
    fn bpftraces_repeated_character_flags_become_single_dash_long_options() {
        let parsed = parse(BPFTRACE_TROUBLESHOOTING);
        for (name, description) in [
            ("vv", "more verbose messages (max 2)"),
            ("dd", "(dry run) verbose debug info"),
        ] {
            let flag = flag_named(&parsed, name);
            assert!(flag.single_dash(), "-{name} is spelled with one dash");
            assert_eq!(flag.spelling(), format!("-{name}"));
            assert_eq!(flag.short(), None);
            assert_eq!(flag.value_name, None);
            assert_eq!(flag.value_kind, ValueKind::None);
            assert_eq!(
                flag.description.as_ref().map(|t| t.as_str()),
                Some(description),
                "the row's own description must survive the repair"
            );
        }
        // ...and the booleans the repair reads as its evidence are still
        // there, untouched. A repair that consumed them would satisfy the
        // must_contain_flags contract and destroy the tool.
        for short in ['v', 'd'] {
            let flag = parsed
                .flags
                .iter()
                .find(|f| f.short() == Some(short))
                .unwrap_or_else(|| panic!("-{short} must survive"));
            assert_eq!(flag.value_kind, ValueKind::None);
        }
    }

    /// The false positive the whole design turns on: `lessecho`'s `[-nn]`
    /// is character-for-character this shape and is a correct parse of a
    /// real flag taking a number. It survives only because `lessecho` never
    /// writes a bare `-n`.
    #[test]
    fn lessechos_real_glued_character_arguments_are_left_alone() {
        let raw = "usage: lessecho [-ox] [-cx] [-pn] [-dn] [-mx] [-nn] [-ex] [-a] file ...\n";
        let parsed = parse(raw);
        assert!(
            parsed.flags.iter().all(|f| f.long().is_none()),
            "no lessecho flag may be rewritten: {:?}",
            parsed
                .flags
                .iter()
                .map(|f| f.spelling())
                .collect::<Vec<_>>()
        );
        // ...and the identical token *is* repaired the moment a document
        // declares the bare spelling a boolean, confirming that condition
        // is what was doing the work rather than some other one failing.
        let parsed = parse("  -n         never overwrite\n  -nn        never ever overwrite\n");
        assert!(flag_named(&parsed, "nn").single_dash());
    }

    /// A spaced value is indistinguishable from a glued one once
    /// [`parse_flag_spec`] has stored it, so the raw text is what decides.
    #[test]
    fn a_spaced_value_is_never_repaired() {
        let parsed = parse("  -v         verbose\n  -v v       take a v\n");
        assert!(
            parsed.flags.iter().all(|f| f.long().is_none()),
            "only a glued token may be repaired: {:?}",
            parsed
                .flags
                .iter()
                .map(|f| f.spelling())
                .collect::<Vec<_>>()
        );
    }

    /// The other two families sharing the `short && !long && value_name`
    /// fingerprint must come through untouched, even when the document
    /// offers the bare boolean the repair looks for.
    #[test]
    fn the_bundle_and_long_option_families_are_not_repaired_as_repeats() {
        let parsed = parse("  -2         two\n  -2CDlNuVv  a cluster\n  -Z         z\n  -Zscript   an unstable flag\n");
        assert!(
            parsed.flags.iter().all(|f| f.long().is_none()),
            "{:?}",
            parsed
                .flags
                .iter()
                .map(|f| f.spelling())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn value_repeats_short_is_case_sensitive_and_rejects_empty() {
        assert!(value_repeats_short('v', "v"));
        assert!(value_repeats_short('v', "vv"));
        assert!(!value_repeats_short('v', "V"));
        assert!(!value_repeats_short('W', "all"));
        assert!(!value_repeats_short('v', ""));
    }

    #[test]
    fn token_occurs_glued_needs_both_boundaries() {
        assert!(token_occurs_glued("    -vv    more verbose\n", "-vv"));
        assert!(!token_occurs_glued("    -vvv   even more\n", "-vv"));
        assert!(!token_occurs_glued("    -v v   spaced\n", "-vv"));
        assert!(!token_occurs_glued("", "-vv"));
    }

    /// The index is an optimization and nothing else, so the thing worth
    /// pinning is not any one answer but the *agreement*: for every case
    /// below, [`GluedTokenIndex::contains`] and [`token_occurs_glued`] must
    /// return the same thing, and that thing must be the documented one.
    ///
    /// The cases are the ones where an index built out of maximal word
    /// runs could plausibly disagree with a scan — a glued neighbour on
    /// either side, a token flush against the start or the end of the
    /// document with no delimiter there at all, a match that is a real
    /// substring but not a delimited one, the same token written more than
    /// once, a candidate carrying a non-word character (the
    /// [`split_glued_value`] shape, which is what makes the index a map of
    /// offsets rather than a set), one whose leading run occurs repeatedly
    /// but with the right remainder behind only one of them, a candidate
    /// that opens on a non-word character (the fallback path), and
    /// multi-byte delimiters, which is where a byte-offset index would
    /// panic or silently miss.
    #[test]
    fn indexed_form_agrees_with_scanning_form() {
        let cases: &[(&str, &str, bool)] = &[
            // glued vs delimited
            ("    -vv    more verbose\n", "-vv", true),
            ("    -vvv   even more\n", "-vv", false),
            ("    -v v   spaced\n", "-vv", false),
            ("  -help_me  ", "-help", false),
            // flush against the start and the end of the document
            ("-help", "-help", true),
            ("-help  print this\n", "-help", true),
            ("see -help", "-help", true),
            ("see -helper", "-help", false),
            // a substring, but not a delimited one
            ("  --help  ", "-help", false),
            ("  x-help  ", "-help", false),
            // the same token more than once
            ("-cpu model\n-cpu model\n", "-cpu", true),
            // a candidate carrying a non-word character
            (
                "  -foffload=<targets>   offload\n",
                "-foffload=<targets>",
                true,
            ),
            ("  -foffload=<targets>x  ", "-foffload=<targets>", false),
            ("  -foffload  ", "-foffload=<targets>", false),
            // leading run repeated, remainder behind only one of them
            ("-a=c and -a=b\n", "-a=b", true),
            ("-a=c and -a=cc\n", "-a=b", false),
            ("-a=bc\n", "-a=b", false),
            // the fallback: a candidate that opens on a non-word character
            ("a=b", "=b", false),
            (" =b ", "=b", true),
            // degenerate
            ("", "-vv", false),
            ("-vv", "", false),
            // multi-byte delimiters on both sides
            ("★-help★", "-help", true),
            ("… -cpu …", "-cpu", true),
        ];
        for &(raw, candidate, expected) in cases {
            let scanned = token_occurs_glued(raw, candidate);
            let indexed = GluedTokenIndex::new(raw).contains(candidate);
            assert_eq!(
                scanned, expected,
                "scanning form disagreed with the documented answer for {candidate:?} in {raw:?}"
            );
            assert_eq!(
                indexed, scanned,
                "indexed form disagreed with the scanning form for {candidate:?} in {raw:?}"
            );
        }
    }

    // --- the single-dash long-option repair -----------------------------

    /// `qemu-arm64-static`'s real option table, byte-exact from
    /// `corpus/qemu-arm64-static/audit-seed2/help.txt` — the long options
    /// and the genuine value-taking short flags on adjacent rows, which is
    /// the whole false-positive problem in six lines.
    const QEMU_TABLE: &str = concat!(
        "-h                                        print this help\n",
        "-help                                     \n",
        "-g port              QEMU_GDB             wait gdb connection to 'port'\n",
        "-cpu model           QEMU_CPU             select CPU (-cpu help for list)\n",
        "-one-insn-per-tb     QEMU_ONE_INSN_PER_TB run with one guest instruction per emulated TB\n",
        "-version             QEMU_VERSION         display version information and exit\n",
    );

    #[test]
    fn qemus_single_dash_long_options_keep_their_real_names() {
        let parsed = parse(QEMU_TABLE);
        for name in ["help", "cpu", "one-insn-per-tb", "version"] {
            let flag = flag_named(&parsed, name);
            assert!(flag.single_dash(), "-{name} is spelled with one dash");
            assert_eq!(flag.spelling(), format!("-{name}"));
            assert_eq!(flag.short(), None);
            assert_eq!(flag.value_name, None);
            assert_eq!(flag.value_kind, ValueKind::None);
        }
    }

    /// The false-positive case that matters most, and the reason the
    /// `qemu` table is carried whole rather than as the `-help` row alone:
    /// `-g port` stores a `value_name` exactly as `-help` stores `"elp"`,
    /// and only the space in the raw text tells them apart.
    #[test]
    fn qemus_genuine_valued_short_flags_on_adjacent_rows_are_left_alone() {
        let parsed = parse(QEMU_TABLE);
        let g = parsed
            .flags
            .iter()
            .find(|f| f.short() == Some('g'))
            .expect("-g must survive as a short flag");
        assert_eq!(
            g.long(),
            None,
            "-g port is a correct parse, not a long option"
        );
        assert_eq!(g.value_name.as_deref(), Some("port"));
        assert_eq!(g.value_kind, ValueKind::Required);
        // ...and the bare `-h` boolean the document also writes is still a
        // short flag in its own right. `-h` and `-help` are two different
        // flags of this tool and the repair must produce both.
        assert!(parsed
            .flags
            .iter()
            .any(|f| f.short() == Some('h') && f.value_kind == ValueKind::None));
    }

    /// The whole safety argument in one test: the GCC/Clang glued-value
    /// convention satisfies every condition but the case one, and every
    /// member of it is a **correct** parse that must survive untouched.
    /// Each token here is a real flag of a real tool, and `-oOUTFILE` is
    /// the one that forces the case test to read the whole token rather
    /// than only the tail.
    #[test]
    fn the_glued_value_convention_is_never_repaired() {
        for row in [
            "  -Zscript       an unstable flag\n",
            "  -Dname         define a macro\n",
            "  -Tutf8         set the output encoding\n",
            "  -Idirectory    add to the include path\n",
            "  -Olevel        set the optimization level\n",
            "  -oOUTFILE      write output here\n",
            "  -DMACRO        define a macro\n",
        ] {
            let parsed = parse(row);
            assert!(
                parsed.flags.iter().all(|f| f.long().is_none()),
                "a correct glued-value parse was destroyed by {row:?}: {:?}",
                parsed
                    .flags
                    .iter()
                    .map(|f| f.spelling())
                    .collect::<Vec<_>>()
            );
        }
    }

    /// `dbiprof`'s real option table, byte-exact from
    /// `corpus/dbiprof/1.643/help.txt` — the glued-`=value` rows and the
    /// value-less rows in one table, which is the whole `=`-split problem
    /// in five lines.
    const DBIPROF_TABLE: &str = concat!(
        "    -number=N        show top N, defaults to 10\n",
        "    -sort=S          sort by S, defaults to total\n",
        "    -reverse         reverse the sort\n",
        "    -match=K=V       for filtering, see docs\n",
        "    -exclude=K=V     for filtering, see docs\n",
        "    -case_sensitive  for -match and -exclude\n",
        "    -version         print version number and exit\n",
    );

    /// The defect the `=` split exists for: a single-dash long option
    /// carrying a glued value came out as its own first character plus a
    /// mangled value (`-number=N` → `-n` + `"umber=N"`), while the
    /// value-less rows of the *same table* came out right.
    #[test]
    fn dbiprofs_glued_value_long_options_keep_their_real_names() {
        let parsed = parse(DBIPROF_TABLE);
        for (name, value) in [
            ("number", "N"),
            ("sort", "S"),
            ("match", "K=V"),
            ("exclude", "K=V"),
        ] {
            let flag = flag_named(&parsed, name);
            assert!(flag.single_dash(), "-{name} is spelled with one dash");
            // `Flag::spelling` writes a required value with a space, the
            // same repo-wide display convention that renders `--output=FILE`
            // as `--output FILE`; what matters here is that the *name* is
            // whole and the value is the tool's own.
            assert_eq!(flag.spelling(), format!("-{name} {value}"));
            assert_eq!(flag.short(), None);
            // The document wrote the value spec on the token, so unlike the
            // spaced case it survives the repair. `-match=K=V` splits at the
            // *first* `=` and keeps the rest verbatim.
            assert_eq!(flag.value_name.as_deref(), Some(value));
            assert_eq!(flag.value_kind, ValueKind::Required);
        }
        // The value-less rows in the same table are unchanged by the split.
        for name in ["reverse", "version"] {
            let flag = flag_named(&parsed, name);
            assert!(flag.single_dash());
            assert_eq!(flag.value_kind, ValueKind::None);
        }
    }

    /// `gcc`'s `-foffload=<targets>`, stored as short `f` with `value_name`
    /// `offload=<targets>` — a real parser bug the human audit confirmed on
    /// `corpus/gcc/13.3.0`, and the same family as `dbiprof`'s. Carried as
    /// gcc's own rows so the uppercase value spec is exercised: the case
    /// test now reads the name half, and `<targets>` shouts on the other
    /// side of the `=`.
    #[test]
    fn gccs_glued_value_long_options_keep_their_real_names() {
        let parsed = parse(concat!(
            "  -foffload=<targets>      Specify offloading targets.\n",
            "  -print-file-name=<lib>   Display the full path to library <lib>.\n",
            "  -std=<standard>          Assume that the input sources are for <standard>.\n",
        ));
        for (name, value) in [
            ("foffload", "<targets>"),
            ("print-file-name", "<lib>"),
            ("std", "<standard>"),
        ] {
            let flag = flag_named(&parsed, name);
            assert!(flag.single_dash(), "-{name} is spelled with one dash");
            assert_eq!(flag.short(), None);
            assert_eq!(flag.value_name.as_deref(), Some(value));
        }
    }

    /// The inverse direction, and the reason condition 5 may look at the
    /// name half alone: the glued-value convention puts its shout to the
    /// **left** of the `=`, so every genuine glued short with a `key=value`
    /// argument is still rejected on exactly the signal it always was.
    /// Ghostscript's `-sDEVICE=` is the type specimen — a lowercase flag
    /// letter, which is what makes it the hard case.
    #[test]
    fn the_glued_value_convention_is_never_repaired_when_it_carries_an_equals() {
        for row in [
            "  -sDEVICE=png16m   select the output device\n",
            "  -sOutputFile=out.png   write output here\n",
            "  -DMACRO=value     define a macro\n",
            "  -Wl,-rpath=/usr/lib   pass to the linker\n",
            "  -Ttext=0x100      set the text segment address\n",
        ] {
            let parsed = parse(row);
            assert!(
                parsed.flags.iter().all(|f| f.long().is_none()),
                "a correct glued-value parse was destroyed by {row:?}: {:?}",
                parsed
                    .flags
                    .iter()
                    .map(|f| f.spelling())
                    .collect::<Vec<_>>()
            );
        }
    }

    /// The separator is still the whole difference: a **spaced** `key=value`
    /// argument stores byte-for-byte what `dbiprof`'s glued `-number=N`
    /// stores, and only condition 7's scan of the raw text tells them
    /// apart.
    #[test]
    fn a_spaced_key_value_argument_is_never_a_long_option() {
        for row in [
            "  -e var=value    set an environment variable\n",
            "  -o key=val      set a mount option\n",
            "  -v var=val      assign an awk variable\n",
        ] {
            let parsed = parse(row);
            assert!(
                parsed.flags.iter().all(|f| f.long().is_none()),
                "a spaced value was glued into a name by {row:?}: {:?}",
                parsed
                    .flags
                    .iter()
                    .map(|f| f.spelling())
                    .collect::<Vec<_>>()
            );
        }
    }

    /// `_` separates words inside an option name exactly as `-` does, and
    /// `dbiprof` proves it in one table: `-case_sensitive` sits between
    /// `-exclude=K=V` and `-version`, both of which this repair already
    /// recovered, and came out as `-c` carrying `"ase_sensitive"` — a
    /// short flag `dbiprof` does not document at all.
    #[test]
    fn an_underscored_name_is_recovered_from_the_table_it_shares() {
        let parsed = parse(DBIPROF_TABLE);
        let flag = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("case_sensitive"))
            .unwrap_or_else(|| {
                panic!(
                    "-case_sensitive was not recovered: {:?}",
                    parsed
                        .flags
                        .iter()
                        .map(|f| f.spelling())
                        .collect::<Vec<_>>()
                )
            });
        assert!(flag.single_dash(), "it is spelled with one dash");
        assert_eq!(flag.short(), None, "the fabricated -c is gone");
        assert_eq!(flag.value_kind, ValueKind::None);
        assert_eq!(
            flag.description.as_ref().map(|d| d.as_str()),
            Some("for -match and -exclude")
        );
        // The fabricated short must not survive under any other flag.
        assert!(
            !parsed
                .flags
                .iter()
                .any(|f| f.short() == Some('c') && f.long().is_none()),
            "the invented -c is not left behind"
        );
    }

    /// The ffmpeg `AVOption` table is 97% of this widening's population,
    /// and the thing that has to survive it is the **value spec**: these
    /// rows write `<int>`/`<flags>`/`<string>` in a space-separated column
    /// of their own, followed by a `.D.V..X....` capability column. Both
    /// already live in the *description* — the grammar never stored them
    /// in `value_name`, which held the swallowed name half instead — so
    /// the repair must move the name and leave the description untouched.
    ///
    /// Rows quoted byte-for-byte from `ffplay --help` (6.1.1-3ubuntu5).
    #[test]
    fn an_avoption_row_keeps_its_value_spec_and_capability_column() {
        const AVOPTIONS: &str = concat!(
            "AVCodecContext AVOptions:\n",
            "  -is_avc            <boolean>    .D.V..X.... is avc (default false)\n",
            "  -skip_top          <int>        .D.V....... number of macroblock rows at the top which are skipped (from INT_MIN to INT_MAX) (default 0)\n",
            "  -threads           <int>        ED.VA...... set the number of threads (from 0 to INT_MAX) (default 1)\n",
        );
        let parsed = parse(AVOPTIONS);
        for (name, spec) in [
            ("is_avc", "<boolean> .D.V..X.... is avc (default false)"),
            (
                "skip_top",
                "<int> .D.V....... number of macroblock rows at the top which are skipped (from INT_MIN to INT_MAX) (default 0)",
            ),
            // The control: no underscore, so this row is recovered on the
            // parser as it stands. Its description is what the two above
            // must now look like.
            (
                "threads",
                "<int> ED.VA...... set the number of threads (from 0 to INT_MAX) (default 1)",
            ),
        ] {
            let flag = parsed
                .flags
                .iter()
                .find(|f| f.long() == Some(name))
                .unwrap_or_else(|| {
                    panic!(
                        "-{name} was not recovered: {:?}",
                        parsed
                            .flags
                            .iter()
                            .map(|f| f.spelling())
                            .collect::<Vec<_>>()
                    )
                });
            assert!(flag.single_dash());
            assert_eq!(
                flag.description.as_ref().map(|d| d.as_str()),
                Some(spec),
                "-{name} lost its value spec or capability column"
            );
        }
    }

    /// The inverse, in the direction that matters: an underscore in the
    /// *swallowed* text is not on its own a licence to read a long option.
    /// Every one of these is a correct parse the widening must leave
    /// standing, and each is refused by a different condition.
    #[test]
    fn an_underscore_alone_never_buys_the_long_reading() {
        for (row, refused) in [
            // Condition 5: the GCC/Clang glued-value convention shouts,
            // and an underscored macro name shouts with it.
            ("  -DFOO_BAR         define a macro\n", "DFOO_BAR"),
            ("  -DMAX_PATH=4096   define a macro\n", "DMAX_PATH"),
            // Condition 5 again, via the whole token: only the argument
            // shouts, and that is exactly the `-oOUTFILE` shape.
            ("  -oOUT_FILE        write output here\n", "oOUT_FILE"),
            // Condition 7: a *spaced* underscored value stores the same
            // bytes a glued one would, and the raw text is what tells
            // them apart — `-o out_file` never occurs glued.
            ("  -o out_file       write output here\n", "out_file"),
            // Condition 3: the name half still may not carry value-spec
            // punctuation just because it also carries an underscore.
            ("  -d item_a[,...]   a list\n", "item_a"),
            ("  -b some_path/name a path\n", "some_path/name"),
            // Condition 4: one character of name is still not a name.
            ("  -s_               a stray\n", "s_"),
        ] {
            let parsed = parse(row);
            assert!(
                parsed.flags.iter().all(|f| f.long() != Some(refused)),
                "{row:?} was read as the long option -{refused}: {:?}",
                parsed
                    .flags
                    .iter()
                    .map(|f| f.spelling())
                    .collect::<Vec<_>>()
            );
        }
    }

    /// The two declared out-of-scope misses, asserted rather than
    /// described — a miss that is only written down in prose stops being
    /// checked the day the prose goes stale.
    #[test]
    fn the_declared_out_of_scope_misses_stay_missed() {
        // A tail that ends at the `=` with nothing after it: refused
        // outright rather than read as either a boolean or an empty value.
        let parsed = parse("  -foo=   an empty value spec\n");
        assert!(
            parsed.flags.iter().all(|f| f.long() != Some("foo")),
            "an empty value spec has no measured reading"
        );
        // `ip` writes a bracketed tail, so the grammar records
        // `ValueKind::Optional` — a value spec a human deliberately typed.
        let parsed = parse("OPTIONS := { -V[ersion] | -h[uman-readable] | -j[son] }\n");
        assert!(
            parsed
                .flags
                .iter()
                .all(|f| f.long() != Some("human-readable")),
            "ip's bracketed abbreviation is outside a Required-only fingerprint by construction"
        );
        // `sg_emc_trespass` glues the layout's own colon onto the flag, so
        // the tail is `"r:"` and is not an option name.
        let parsed = parse("    -hr: Set Honor Reservation bit\n");
        assert!(
            parsed.flags.iter().all(|f| f.long() != Some("hr")),
            "a tail carrying punctuation is not a name"
        );
    }

    /// A synopsis-sourced cluster is all-lowercase, unsorted, and
    /// indistinguishable from a long option on every condition but its
    /// source. Condition 1 is the only thing keeping the entire bundled-
    /// short population out of this repair.
    #[test]
    fn a_synopsis_sourced_bundle_is_never_read_as_a_long_option() {
        let parsed = parse("usage: rpcbind [-adhilswfr]\n");
        assert!(
            parsed.flags.iter().all(|f| f.long() != Some("adhilswfr")),
            "the bundle belongs to parse_bundled_shorts, not to this repair: {:?}",
            parsed
                .flags
                .iter()
                .map(|f| f.spelling())
                .collect::<Vec<_>>()
        );
    }

    /// A spaced value is indistinguishable from a glued one once
    /// [`parse_flag_spec`] has stored it, so the raw text is what decides
    /// — the same condition 7 the repeated-character repair leans on.
    #[test]
    fn a_spaced_value_is_never_read_as_a_long_option() {
        let parsed = parse("  -g port    wait gdb connection to 'port'\n");
        assert!(
            parsed.flags.iter().all(|f| f.long().is_none()),
            "only a glued token may be repaired: {:?}",
            parsed
                .flags
                .iter()
                .map(|f| f.spelling())
                .collect::<Vec<_>>()
        );
    }

    /// The two families that share the `short && !long && value_name`
    /// fingerprint stay disjoint: a repeated-character flag is handed to
    /// the other repair and a one-character tail is claimed by neither.
    #[test]
    fn the_repeat_and_short_tail_families_are_not_claimed_here() {
        // `-vvv` satisfies every other condition; condition 6 hands it off.
        let parsed = parse("  -vvv       even more verbose\n");
        assert!(
            parsed.flags.iter().all(|f| f.long().is_none()),
            "a repeated-character run is the other repair's, and only when it has its boolean"
        );
        // A one-character tail is the ambiguous population both repairs
        // decline: `rpcgen -Ss` and friends are half correct parses.
        let parsed = parse("  -ps        postscript\n");
        assert!(parsed.flags.iter().all(|f| f.long().is_none()));
    }

    #[test]
    fn is_option_name_tail_rejects_every_value_spec_shape() {
        assert!(is_option_name_tail("elp"));
        assert!(is_option_name_tail("one-insn-per-tb"));
        assert!(is_option_name_tail("utf8"));
        // `_` is a word separator inside a name, on the same footing as
        // `-`: `dbiprof`'s `-case_sensitive`, ffmpeg's `-pix_fmts`.
        assert!(is_option_name_tail("ase_sensitive"));
        assert!(is_option_name_tail("ix_fmts"));
        // Leading, trailing and doubled separators are still names — the
        // shape test is about the character set, and every other
        // condition is what makes the repair safe.
        assert!(is_option_name_tail("_err_detect"));
        // No letter at all is a glued numeric argument, not a name.
        assert!(!is_option_name_tail("4096"));
        assert!(!is_option_name_tail("_42"));
        assert!(!is_option_name_tail(""));
        // Every punctuation character a value spec leaks.
        for tail in [
            "r:",
            "tune=native",
            "item[,...]",
            "b{blocksize}",
            "a<b>",
            "path/name",
            "file.txt",
            "a,b",
        ] {
            assert!(!is_option_name_tail(tail), "{tail:?} is not an option name");
        }
    }

    #[test]
    fn token_is_uniformly_lowercase_reads_the_whole_token() {
        assert!(token_is_uniformly_lowercase("-help"));
        assert!(token_is_uniformly_lowercase("-one-insn-per-tb"));
        assert!(!token_is_uniformly_lowercase("-Zscript"));
        // The case the whole-token rule exists for: a lowercase flag
        // letter with a shouting argument glued on.
        assert!(!token_is_uniformly_lowercase("-oOUTFILE"));
    }

    #[test]
    fn tar_main_operation_mode_group_recovered() {
        let parsed = parse(TAR_HELP);
        let create = parsed.flags.iter().find(|f| f.long() == Some("create"));
        assert!(
            create.is_some(),
            "expected --create among {:?}",
            parsed.flags.iter().map(|f| f.long()).collect::<Vec<_>>()
        );
        assert_eq!(
            create.unwrap().group.as_deref(),
            Some("Main operation mode:")
        );
    }

    #[test]
    fn tar_flag_with_short_and_description() {
        let parsed = parse(TAR_HELP);
        let create = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("create"))
            .unwrap();
        assert_eq!(create.short(), Some('c'));
        assert!(create
            .description
            .as_ref()
            .unwrap()
            .as_str()
            .contains("create a new archive"));
    }

    #[test]
    fn tar_multiline_description_is_joined() {
        let parsed = parse(TAR_HELP);
        let occurrence = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("occurrence"))
            .unwrap();
        let desc = occurrence.description.as_ref().unwrap().as_str();
        assert!(desc.contains("NUMBERth occurrence"), "{desc:?}");
        assert!(
            desc.contains("conjunction with one of the subcommands"),
            "{desc:?}"
        );
    }

    /// Regression: a long-only flag (indented deeper than its short-form
    /// siblings, e.g. `--delete` at column 6 vs. `-c, --create` at column
    /// 2) must still be recovered as its own flag, not swallowed into the
    /// previous entry's description or misfired into a phantom heading.
    #[test]
    fn tar_long_only_flag_at_deeper_indent_is_its_own_flag() {
        let parsed = parse(TAR_HELP);
        let delete = parsed.flags.iter().find(|f| f.long() == Some("delete"));
        assert!(
            delete.is_some(),
            "expected --delete among {:?}",
            parsed.flags.iter().map(|f| f.long()).collect::<Vec<_>>()
        );
        assert_eq!(
            delete.unwrap().description.as_ref().unwrap().as_str(),
            "delete from the archive (not on mag tapes!)"
        );
    }

    /// A short-form flag appearing *after* a run of long-only flags at a
    /// shallower indent (tar's `-m, --touch` inside "Handling of file
    /// attributes", surrounded by long-only entries at a deeper column)
    /// must not be misread as a dedent back to heading level.
    #[test]
    fn tar_short_flag_after_long_only_run_is_recovered() {
        let parsed = parse(TAR_HELP);
        let touch = parsed.flags.iter().find(|f| f.long() == Some("touch"));
        assert!(
            touch.is_some(),
            "expected --touch among {:?}",
            parsed.flags.iter().map(|f| f.long()).collect::<Vec<_>>()
        );
        assert_eq!(touch.unwrap().short(), Some('m'));
    }

    #[test]
    fn tar_examples_section_does_not_produce_fake_subcommands() {
        let parsed = parse(TAR_HELP);
        assert!(
            !parsed.subcommands.iter().any(|c| c.name == "tar"),
            "Examples: section should not produce a fake 'tar' subcommand: {:?}",
            parsed
                .subcommands
                .iter()
                .map(|c| &c.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn tar_has_reasonable_confidence() {
        let parsed = parse(TAR_HELP);
        assert!(
            parsed.confidence > 0.5,
            "confidence was {}",
            parsed.confidence
        );
    }

    /// The core regression for [M-10]: `tar --help` has no `Commands:`-
    /// shaped heading anywhere, so the only correct answer is zero
    /// subcommands — not the 39 phantom entries (wrapped description
    /// fragments and `--format=`'s enum values) an earlier version of
    /// this parser produced.
    #[test]
    fn tar_produces_zero_subcommands() {
        let parsed = parse(TAR_HELP);
        assert!(
            parsed.subcommands.is_empty(),
            "expected zero subcommands, got {:?}",
            parsed
                .subcommands
                .iter()
                .map(|c| &c.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn empty_input_yields_low_confidence_and_no_panic() {
        let parsed = parse("");
        assert!(parsed.confidence < 0.5);
        assert!(parsed.flags.is_empty());
    }

    /// Core regression for [M-10]: `dd`, `less`, `sed`, and (this
    /// sandbox's) `find` (`bfs`) have no real subcommands. Each has at
    /// least one of the specific shapes that produced phantoms before:
    /// dd's bare `key=VALUE` operand list and two "Each ... symbol may
    /// be:" enum blocks, less's giant heavily-formatted command summary,
    /// sed's headingless flags block, and find/bfs's `Tests:`/`Actions:`
    /// flag sections (whose headings don't say "command" either).
    #[test]
    fn dd_less_sed_find_all_produce_zero_subcommands() {
        for (name, raw) in [
            ("dd", DD_HELP),
            ("less", LESS_HELP),
            ("sed", SED_HELP),
            ("find", FIND_HELP),
        ] {
            let parsed = parse(raw);
            assert!(
                parsed.subcommands.is_empty(),
                "{name}: expected zero subcommands, got {:?}",
                parsed
                    .subcommands
                    .iter()
                    .map(|c| &c.name)
                    .collect::<Vec<_>>()
            );
        }
    }

    /// Regression for a real crash the coverage harness (spec §13.1) found
    /// on the very first full run: a multi-byte character (e.g. a
    /// box-drawing glyph some real tool's `--help` output starts with)
    /// positioned so that byte offset 6 falls inside it made `&t[..6]`
    /// panic with "not a char boundary". A tier that can be crashed by one
    /// real tool's output is worse than a tier that just produces
    /// low-confidence output for it — this must degrade gracefully, not
    /// panic, no matter what bytes precede "usage:" or appear anywhere
    /// else in the input.
    #[test]
    fn multibyte_characters_near_the_start_of_output_do_not_panic() {
        // "12345" is 5 bytes, then U+2588 ('█') is a 3-byte UTF-8 sequence
        // spanning bytes 5..8 — byte offset 6 (the old `&t[..6]` slice
        // point) falls squarely inside it, reproducing the exact crash the
        // coverage harness found on its first real run.
        let raw =
            "12345█ some line\nUsage: weirdtool [OPTIONS]\n\n  -x, --example   an example flag\n";
        let parsed = parse(raw); // must not panic
        assert!(
            !parsed.usage.is_empty(),
            "should still recover the usage line: {parsed:?}"
        );
    }

    #[test]
    fn multibyte_characters_with_no_usage_line_do_not_panic() {
        // Also exercise the path with no "usage:" line at all, and with
        // multi-byte content positioned at various points.
        let raw = "日本語のヘルプ出力\n\n  --flag   description\n";
        let parsed = parse(raw); // must not panic
        let _ = parsed;
    }

    /// Regression found by the coverage harness (spec §13.1): `instmodsh`
    /// (a Perl REPL) ignores `--help` entirely and free-runs printing its
    /// own 3-line "Available commands are: l / m <module> / q" banner
    /// until the wall-clock cap kills it, producing several megabytes of
    /// near-exact repetition. Parsing that recovered 58,663 duplicate-name
    /// subcommands, which then took over two minutes just to bucket-merge
    /// downstream — a single degenerate tool making the whole pipeline
    /// slow. Same-named entries recovered twice must collapse to one, and
    /// the total accepted must stay bounded regardless of how many times
    /// the input repeats.
    #[test]
    fn repeated_identical_banner_does_not_explode_into_duplicate_subcommands() {
        // Non-blocking timing signal only — see `xtask::corpus::MAX_FIXTURE_PARSE_TIME`
        // and spec.md's "sweep-timing false transition" decision (D3): wall-clock
        // measured on shared/contended hardware is a statement about the machine,
        // not the parser, so it must never flip a correctness gate. This test false
        // -failed twice in review under concurrent-compile load (observed up to 7.5s,
        // against 4.3s/~1s alone on a quiet box) despite the parser being unchanged,
        // which is exactly the D3 pattern. `TIMING_BUDGET` below is set well above
        // every observed run (quiet or loaded) purely so a genuine reintroduction of
        // the O(n^2) blowup this test guards against — which would land in seconds
        // to minutes, not a borderline overage — prints a warning nobody could miss.
        // The real, blocking assertion is the subcommand count below.
        const TIMING_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);

        let block = "Available commands are:\n   l            - List all installed modules\n   q            - Quit the program\n\n";
        let raw = block.repeat(20_000);
        let start = std::time::Instant::now();
        let parsed = parse(&raw);
        let elapsed = start.elapsed();
        if elapsed > TIMING_BUDGET {
            eprintln!(
                "warning: parsing a repetitive input took {elapsed:?}, exceeding the \
                 {TIMING_BUDGET:?} non-blocking budget — likely a real regression rather \
                 than machine noise; investigate before dismissing"
            );
        }
        assert_eq!(
            parsed.subcommands.len(),
            2,
            "expected exactly one 'l' and one 'q', got {:?}",
            parsed
                .subcommands
                .iter()
                .map(|c| &c.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn entry_recovery_is_capped_even_when_every_entry_is_distinct() {
        let mut raw = String::from("Commands:\n");
        for i in 0..(MAX_RECOVERED_ENTRIES + 500) {
            raw.push_str(&format!("   cmd{i}   does a thing\n"));
        }
        let parsed = parse(&raw);
        assert!(
            parsed.subcommands.len() <= MAX_RECOVERED_ENTRIES,
            "got {} subcommands, expected at most {}",
            parsed.subcommands.len(),
            MAX_RECOVERED_ENTRIES
        );
    }

    #[test]
    fn is_command_name_shaped_rejects_prose_and_placeholders() {
        assert!(is_command_name_shaped("commit"));
        assert!(is_command_name_shaped("http-push"));
        assert!(is_command_name_shaped("sha3-256"));
        assert!(!is_command_name_shaped("treat them as errors"));
        assert!(!is_command_name_shaped("BYTES"));
        assert!(!is_command_name_shaped(""));
        assert!(!is_command_name_shaped("42start"));
    }

    #[test]
    fn mentions_commands_word_matches_whole_word_only() {
        assert!(mentions_commands_word("Commands:"));
        assert!(mentions_commands_word("Available Commands:"));
        assert!(mentions_commands_word(
            "These are common Git commands used in various situations:"
        ));
        assert!(mentions_commands_word("SUBCOMMANDS"));
        // "recommends" contains the substring "commands" but is not the
        // word "commands" — must not false-positive on substring match.
        assert!(!mentions_commands_word("This tool recommends caution."));
    }

    /// `jdeprscan --help` documents every option in its own flush-left
    /// prose paragraph, and its `options:` table has no description column
    /// at all: 8 flags, 0.0% with text before this. The paragraph names the
    /// option it documents, so the two can be associated.
    ///
    /// `--list` is the load-bearing case: the table row `-l    --list`
    /// loses its long spelling to a separate, still-unfixed bug, so the
    /// only way to reach that flag is the `(-l)` in the paragraph's own
    /// parenthetical.
    #[test]
    fn a_prose_paragraph_naming_an_option_supplies_its_description() {
        let help = "Usage: jdeprscan [options] {dir|jar|class} ...\n\
                    \n\
                    options:\n        \
                    --for-removal\n  \
                    -l    --list\n\
                    \n\
                    Scans each argument for usages of deprecated APIs.\n\
                    \n\
                    The --for-removal option limits scanning or listing to APIs that are\n\
                    deprecated for removal.\n\
                    \n\
                    The --list (-l) option prints out the set of deprecated APIs.\n";
        let parsed = parse(help);
        assert_eq!(
            parsed
                .flags
                .iter()
                .find(|f| f.long() == Some("for-removal"))
                .and_then(|f| f.description.as_ref())
                .map(|d| d.as_str()),
            Some(
                "The --for-removal option limits scanning or listing to APIs that are \
                 deprecated for removal."
            )
        );
        assert_eq!(
            parsed
                .flags
                .iter()
                .find(|f| f.short() == Some('l'))
                .and_then(|f| f.description.as_ref())
                .map(|d| d.as_str()),
            Some("The --list (-l) option prints out the set of deprecated APIs.")
        );
    }

    /// The backfill's two hard limits, which are what bound its cost:
    /// it may never invent a flag, and it may never overwrite a
    /// description the table itself supplied.
    ///
    /// Both cases are real. `apt-ftparchive`'s prose mentions
    /// `--source-override`, an option its table never lists; `apropos`
    /// describes `--regex` in its own table *and* mentions it in a
    /// trailing paragraph.
    #[test]
    fn the_prose_backfill_never_invents_a_flag_or_overwrites_a_description() {
        let help = "Usage: tool [options]\n\
                    \n\
                    Options:\n  \
                    -r, --regex                interpret each keyword as a regex\n\
                    \n\
                    The --regex option is enabled by default.\n\
                    \n\
                    The --source-override option can be used to specify a src override file\n";
        let parsed = parse(help);
        assert!(
            !parsed
                .flags
                .iter()
                .any(|f| f.long() == Some("source-override")),
            "a paragraph must never create a flag: {:?}",
            parsed.flags
        );
        assert_eq!(
            parsed
                .flags
                .iter()
                .find(|f| f.long() == Some("regex"))
                .and_then(|f| f.description.as_ref())
                .map(|d| d.as_str()),
            Some("interpret each keyword as a regex"),
            "the table's own description must win"
        );
    }

    /// A "The --x option ..." sentence *indented under another flag's row*
    /// is that flag's continuation text, not a standalone paragraph — so
    /// it must never be lifted out and attached to `--x`. Real shape:
    /// `java`, `jdeps` and `rg` all write such sentences inside a
    /// description column.
    #[test]
    fn an_indented_sentence_is_continuation_text_not_a_prose_paragraph() {
        let help = "Usage: tool [options]\n\
                    \n\
                    Options:\n      \
                    --dry-run\n      \
                    --validate-modules   Validate all modules.\n                  \
                    The --dry-run option may be useful for validating the\n                  \
                    command line.\n";
        let parsed = parse(help);
        assert_eq!(
            parsed
                .flags
                .iter()
                .find(|f| f.long() == Some("dry-run"))
                .map(|f| f.description.is_none()),
            Some(true),
            "an indented sentence belongs to the row above it: {:?}",
            parsed.flags
        );
    }
}
