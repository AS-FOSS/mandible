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
    bracket_flag_row_content, is_bare_flag_spelling, is_bare_flag_token, is_dash_underline_token,
    looks_like_bracket_flag_row, looks_like_flag_start, looks_like_paren_alternation_open,
    looks_like_stanza_head_flag, paren_alternation_member_content, paren_depth_delta,
    parse_bundled_shorts, parse_flag_alternation, parse_flag_spec, split_alternatives, FlagSpec,
};
use super::profile::{heading_matches_markers, FrameworkProfile};
use mandible_core::{
    is_command_name_shaped, strip_escapes, CommandNode, Entity, Provenance, Source, Spelling, Text,
    ValueKind,
};

mod backfill;
mod emit;
mod entry;
mod flag_rows;
mod heading;
mod layout;
mod preamble;
mod repair;
mod scan;
mod spelling;
#[cfg(test)]
mod test_support;
mod usage;

use backfill::*;
pub use emit::*;
pub use entry::*;
use flag_rows::*;
pub use heading::*;
pub use layout::*;
use preamble::*;
use repair::*;
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
    /// Modifier letters recovered from a modifier table — `ar`'s `[a]`,
    /// `[D]`, `[l <text> ]` rows (spec §7 Tier B, "Modifier tables").
    pub modifiers: Vec<Entity>,
    /// Environment variables recovered from a row under an explicitly
    /// labeled environment heading — `bpftrace`'s `BPFTRACE_BTF`, `node`'s
    /// `NODE_DEBUG` (spec §7 Tier B, "Environment sections"). Never
    /// scavenged from `ALL_CAPS` prose or usage placeholders.
    pub env_vars: Vec<Entity>,
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

/// Minimum count of independently parsed flag rows a same-indent (or
/// deeper) block must produce before it is trusted as real structure over
/// worked-example prose. Shared by [`starts_attested_flag_section`] (a
/// headed block) and `heading::starts_attested_headingless_flag_block` (a
/// headingless one): both read this as the same evidence bar, just applied
/// to different shapes of the same question — "is this row run cheap for
/// an unrelated document to have produced, or does it need a real flag
/// table to explain it."
pub(super) const MIN_ATTESTED_SECTION_FLAGS: usize = 2;

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

/// True if `heading` mentions "operation"/"operations" as a whole word
/// (case-insensitive) — spec §7 Tier B rule 1's extended heading
/// vocabulary, `llvm-ar --help`'s `OPERATIONS:` table (issue: llvm-ar
/// operations table). An operation letter is an invocation verb exactly
/// the way a subcommand name is (`llvm-ar d archive.a file.o`), so a
/// heading naming a table of them is the same class of evidence rule 1
/// already accepts for "command(s)"/"subcommand(s)".
///
/// **Deliberately narrower in scope than [`mentions_commands_word`]: this
/// predicate feeds only [`is_recognized_command_heading`], never
/// [`command_mode_seed`].** The two vocabularies read as though they
/// should be the same list, and are not, on purpose — see this crate's
/// doc comment on `is_recognized_command_heading`'s call site for the
/// measurement behind the split.
fn mentions_operations_word(s: &str) -> bool {
    s.split(|c: char| !c.is_alphanumeric())
        .map(|w| w.to_lowercase())
        .any(|w| matches!(w.as_str(), "operation" | "operations"))
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
    // the section context at all.  While this is `Some((indent, _))`, the
    // marker was found in exactly that obscured shape and the whole region
    // is fenced before *any* headingless or headed emission path can see
    // its rows.  A physical dedent always closes the region; short of one,
    // only `obscured_fence_reopens` (issue #77 edge 1: an independently
    // attested flag section, headed or not, at the marker's indent or
    // deeper) may close it — a plain `Input:`/`Output:` label inside the
    // example may not, and neither may a bare dedent-free row of
    // unattested content.
    //
    // This state is deliberately separate from `in_ignorable_section`:
    // direct, correctly-recognized `Examples:` headings retain their
    // established behavior, while the stronger whole-region fence applies
    // only to markers that the prose-parent quirk would otherwise hide. The
    // tuple's second field is the value `in_ignorable_section` held the
    // instant before the fence opened — the close restores it (issue #77
    // edge 2) rather than clearing it outright, so a fence opened *inside*
    // an already-suppressed `EXAMPLES:` section cannot cancel that
    // suppression when it closes.
    let mut obscured_ignorable_indent: Option<(usize, bool)> = None;

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
        if let Some((marker_indent, prior_ignorable)) = obscured_ignorable_indent {
            if obscured_fence_reopens(&lines, i, marker_indent) {
                obscured_ignorable_indent = None;
                in_ignorable_section = prior_ignorable;
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
                emit_packed_flags(
                    None,
                    entries.into_iter().map(|(s, d, _)| (s, d)).collect(),
                    &mut result,
                );
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
                && is_obscured_fence_marker(lines[marker_idx].trim())
            {
                obscured_ignorable_indent =
                    Some((leading_whitespace(lines[marker_idx]), in_ignorable_section));
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
            // An environment section flush with its own heading — no
            // indent step at all between "Environment variables:" and its
            // first row. `node`'s real `--help` writes exactly this shape
            // (both at column 0), the same flush-left convention `dnf`'s
            // command table below uses, and without this check here the
            // heading-keyed recognizer below (which only ever runs once
            // content strictly deeper than the heading has been
            // established) never gets a chance to see rows that never
            // step in from their own heading at all.
            //
            // Gated on the row sitting at exactly `heading_indent`, the
            // same evidence bar the same-indent command table below
            // requires, so a row that dedents past its own heading (the
            // section has genuinely ended) is never swept in.
            if i < lines.len()
                && leading_whitespace(lines[i]) == heading_indent
                && is_environment_heading(&heading)
                && !is_ignorable_heading(&heading)
            {
                if let Some((end, rows)) = scan_env_var_table(&lines, i) {
                    i = end;
                    in_ignorable_section = false;
                    command_mode = false;
                    let (seen, clean) =
                        emit_env_vars(meaningful_flag_group(heading.clone()), rows, &mut result);
                    total_entries += seen;
                    clean_entries += clean;
                    continue;
                }
            }
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
        //
        // A stanza that carries its own description sentence directly
        // above its head line labels its group with that sentence instead
        // of the head line ([`stanza_description_above`] has the rule and
        // every clause's reasoning). The head line is not lost by that
        // swap: it is a usage form, so it goes where usage forms go —
        // `result.usage`, the verbatim synopsis section (§4.5) — which is
        // exactly where the stanzas this document's usage block *did*
        // reach already sit. Pushed here rather than in that block because
        // this is the one place that knows the head line was a stanza head
        // and that its own text is about to stop being the group label;
        // `extract_positionals` has already run by now, so nothing is
        // mined out of the added line.
        //
        // Capped, not deduplicated. `i` only ever advances, so no physical
        // line becomes `heading` twice and one document cannot repeat
        // itself here; two identical entries mean the tool printed the
        // stanza twice, which the synopsis section should say. A scan of
        // `usage` per heading would meanwhile be the quadratic shape
        // `MAX_RECOVERED_ENTRIES` exists for (`instmodsh`'s 8 MiB of
        // repeated banner).
        let stanza_label = if in_ignorable_section {
            None
        } else {
            stanza_description_above(&lines, heading_idx, tool_name).map(str::to_string)
        };
        if stanza_label.is_some() && result.usage.len() < MAX_RECOVERED_ENTRIES {
            result.usage.push(heading.clone());
        }
        if !in_ignorable_section {
            if let Some(mut flag) = recover_stanza_head_flag(&heading, tool_name) {
                if let Some(label) = stanza_label.clone() {
                    flag.group = Some(label);
                }
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

        // A modifier table — `ar`'s ` command specific modifiers:` and
        // ` generic modifiers:` blocks, `llvm-ar`'s `MODIFIERS:` — gets
        // first refusal on this heading's content, because its rows reach
        // none of the branches below as anything but noise: a `[a]` row is
        // not `looks_like_flag_start`, and under the recognized heading
        // `command specific modifiers:` (which contains the word "command")
        // it went to `emit_subcommands`, where every row failed the
        // name-shape test and was dropped as unattributable. `ar`'s
        // seventeen modifiers reached the tree as nothing at all.
        //
        // **Falls through rather than `continue`ing** when the block still
        // has content past the run. `ar`'s ` generic modifiers:` is seven
        // bracket rows, then `@<file>`, then `--target`/`--output`/
        // `--record-libdeps`/`--thin`; those four must go on being read by
        // the flags branch below *under this same heading*, so they keep
        // the `group` they already carry. Re-entering the loop instead
        // would reach them through the headingless-flags branch at the top,
        // which has no heading to name a group with, and would silently
        // strip it from four flags that have one today.
        if let Some((end, rows)) = scan_modifier_table(&lines, i) {
            i = end;
            if !is_ignorable_heading(&heading) {
                // A modifier table is positively-recognized structure, so
                // it clears the examples-region flag the same way a real
                // flags block does.
                in_ignorable_section = false;
                // ...and it positively contradicts a command list: `ar`'s
                // own ` command specific modifiers:` heading contains the
                // word "command", so without this the sticky chain would
                // carry `command_mode` on into the sections after it.
                command_mode = false;
                let (seen, clean) =
                    emit_modifiers(meaningful_flag_group(heading.clone()), rows, &mut result);
                total_entries += seen;
                clean_entries += clean;
            }
            if i >= lines.len() || leading_whitespace(lines[i]) <= heading_indent {
                continue;
            }
        }

        // An environment section — `bpftrace`'s `ENVIRONMENT:`, `node`'s
        // `Environment variables:`, `mksquashfs`'s `Environment:` — spec
        // §4.5's "strict-sections-only" rule made structural: unlike the
        // modifier table above, this is gated on the **heading itself**
        // first (`is_environment_heading`), never on row shape alone. A
        // bare identifier followed by a column gap and a description is
        // not, by itself, distinguishable from an ordinary bare-word block
        // or a flush-left config-variable table — [M-10]'s `mysqlslap`
        // specimen is exactly a table shaped like this that documents
        // settings, not environment variables — so here the heading is the
        // only reliable signal and the row grammar only has to clear an
        // ordinary bar once that signal has already fired.
        //
        // Falls through rather than `continue`ing, mirroring the modifier
        // branch, for the same reason: nothing in the measured fleet needs
        // it today, but a labeled environment heading whose block runs on
        // past its rows into ordinary flags should not lose those flags'
        // group any more than `ar`'s modifiers do.
        if is_environment_heading(&heading) && !is_ignorable_heading(&heading) {
            if let Some((end, rows)) = scan_env_var_table(&lines, i) {
                i = end;
                in_ignorable_section = false;
                command_mode = false;
                let (seen, clean) =
                    emit_env_vars(meaningful_flag_group(heading.clone()), rows, &mut result);
                total_entries += seen;
                clean_entries += clean;
                if i >= lines.len() || leading_whitespace(lines[i]) <= heading_indent {
                    continue;
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
            // A stanza's own description sentence outranks its head line
            // as the group's label — and only there: every other block in
            // the document still takes `meaningful_flag_group`'s answer
            // unchanged, including one whose heading merely happens to be
            // prose (which that predicate deliberately refuses to name a
            // group with, since a sentence promoted to a heading by
            // indentation alone is a defect rather than a label).
            let group = stanza_label
                .clone()
                .or_else(|| meaningful_flag_group(heading));
            if packed {
                let seen = entries.len();
                emit_packed_flags(
                    group,
                    entries.into_iter().map(|(s, d, _)| (s, d)).collect(),
                    &mut result,
                );
                total_entries += seen;
                clean_entries += seen;
            } else {
                let (seen, clean) = emit_flags(group, entries, &mut result);
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

    // --- issue #77: obscured-`Examples:` fence edges ---------------------

    /// Edge 1: a well-formed, positively-attested flag section indented
    /// *deeper* than the obscured marker must still be recoverable — the
    /// fence's only historical exits were a physical dedent, or an
    /// attested flag section at *exactly* the marker's indent, so a real
    /// `Options:` block sitting deeper than ` Examples:` (itself sitting
    /// deeper than the prose sentence that obscures it) was silently lost
    /// forever, with no dedent anywhere below to rescue it.
    #[test]
    fn obscured_fence_admits_a_deeper_attested_flag_section() {
        let raw = "\
This tool does many useful things for the busy user.
  Examples:
    widget run --now
      runs immediately without prompting
    widget stop
      halts all processing at once
    Options:
      --now      run immediately without prompting
      --stop     halt all processing at once
";
        let parsed = parse(raw);
        let names: Vec<_> = parsed.flags.iter().map(|f| f.long()).collect();
        assert!(
            parsed.flags.iter().any(|f| f.long() == Some("now")),
            "expected --now among {names:?}"
        );
        assert!(
            parsed.flags.iter().any(|f| f.long() == Some("stop")),
            "expected --stop among {names:?}"
        );
    }

    /// Edge 1: the same widened exit admits a *headingless* flag block —
    /// no heading vocabulary is possible when there is no heading, so the
    /// row-count floor alone (`starts_attested_headingless_flag_block`)
    /// must be the evidence.
    #[test]
    fn obscured_fence_admits_a_deeper_headingless_flag_block() {
        let raw = "\
This tool does many useful things for the busy user.
  Examples:
    widget run --now
      runs immediately without prompting
    --now      run immediately without prompting
    --stop     halt all processing at once
";
        let parsed = parse(raw);
        assert!(
            parsed.flags.iter().any(|f| f.long() == Some("now")),
            "expected --now among {:?}",
            parsed.flags.iter().map(|f| f.long()).collect::<Vec<_>>()
        );
        assert!(
            parsed.flags.iter().any(|f| f.long() == Some("stop")),
            "expected --stop among {:?}",
            parsed.flags.iter().map(|f| f.long()).collect::<Vec<_>>()
        );
    }

    /// Edge 2: the fence's close must *restore* whatever `in_ignorable_section`
    /// held before it opened, not clear it outright. Reproduced shape from
    /// the issue: a real, non-obscured `EXAMPLES:` heading (which
    /// legitimately suppresses `recover_stanza_head_flag` for the rest of
    /// its section) contains a prose sentence with an obscured ` Examples:`
    /// marker beneath it; the marker's own fence then closes on a physical
    /// dedent back to a stanza-head-shaped example line
    /// (`widget -x`). Clearing `in_ignorable_section` on that close cancels
    /// the outer `EXAMPLES:` heading's own, entirely legitimate,
    /// suppression — exactly the bpftrace fabrication class
    /// `in_ignorable_section` exists to prevent — and the dedented example
    /// line is misread as a real stanza head, fabricating a `-x` flag.
    #[test]
    fn obscured_fence_close_restores_rather_than_clears_suppression() {
        let raw = "\
EXAMPLES:

This tool does many useful things for the busy user.
  Examples:
widget -x
  run example x
";
        let parsed = parse_named(raw, "widget");
        assert!(
            !parsed.flags.iter().any(|f| f.short() == Some('x')),
            "fence close must not fabricate -x from the example: {:?}",
            parsed
                .flags
                .iter()
                .map(|f| (f.short(), f.long()))
                .collect::<Vec<_>>()
        );
    }

    /// Edge 3: the obscured-marker fence trigger must be stricter than
    /// `is_ignorable_heading` — heading-shaped and colon-terminated, not
    /// merely "starts with 'example' or contains 'report bugs'". Reproduced
    /// shape from the issue: a mid-document `Report bugs to <address>.`
    /// *sentence* (not a heading) sitting under a lower-indented prose
    /// sentence must never open the whole-region fence; if it does, it
    /// silently swallows every real command table after it (which the
    /// edge-1 rescue cannot see, since it only recognizes flag evidence)
    /// until a dedent that never comes.
    #[test]
    fn obscured_fence_trigger_requires_heading_shape() {
        let raw = "\
This tool supports many commands for the busy user.
  Report bugs to <maintainer@example.com>.
  start   begin processing
  stop    end processing
";
        let parsed = parse(raw);
        let names: Vec<_> = parsed.subcommands.iter().map(|c| &c.name).collect();
        assert!(
            parsed.subcommands.iter().any(|c| c.name == "start"),
            "expected 'start' among {names:?}"
        );
        assert!(
            parsed.subcommands.iter().any(|c| c.name == "stop"),
            "expected 'stop' among {names:?}"
        );
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

    // --- the "operations" heading vocabulary (llvm-ar operations table) ---

    #[test]
    fn mentions_operations_word_matches_whole_word_only() {
        assert!(mentions_operations_word("OPERATIONS:"));
        assert!(mentions_operations_word("Main operation modes:"));
        // "operational" contains the substring "operation" but is not the
        // word "operation" — must not false-positive on substring match.
        assert!(!mentions_operations_word("Operational readiness report:"));
    }

    /// `llvm-ar --help`'s real `OPERATIONS:` table (issue: llvm-ar
    /// operations table), byte-shaped from `corpus/llvm-ar-18/18.1.3`: a
    /// recognized-heading extension recovers it the same way `ar`'s own
    /// `commands:`-headed table already parses, with the ` - ` separator
    /// admissible because the heading is now `recognized`.
    #[test]
    fn llvm_ar_operations_table_recovered_as_subcommands() {
        let raw = concat!(
            "USAGE: llvm-ar [options] [-]<operation>[modifiers] [relpos] [count] <archive> [files]\n",
            "\n",
            "OPERATIONS:\n",
            "  d - delete [files] from the archive\n",
            "  m - move [files] in the archive\n",
            "  p - print contents of [files] found in the archive\n",
            "  q - quick append [files] to the archive\n",
            "  r - replace or insert [files] into the archive\n",
            "  s - act as ranlib\n",
            "  t - display list of files in archive\n",
            "  x - extract [files] from the archive\n",
        );
        let parsed = parse_named(raw, "llvm-ar");
        let names: Vec<&str> = parsed.subcommands.iter().map(|c| c.name.as_str()).collect();
        for op in ["d", "m", "p", "q", "r", "s", "t", "x"] {
            assert!(names.contains(&op), "missing operation {op:?}: {names:?}");
        }
        let delete = parsed.subcommands.iter().find(|c| c.name == "d").unwrap();
        assert_eq!(
            delete.summary.as_ref().map(|t| t.as_str()),
            Some("delete [files] from the archive")
        );
        // Every recovered operation is heading-attested — the heading text
        // itself said "OPERATIONS:", not merely a table row or a chain —
        // which is what spec §6 rule 0's closing paragraph gates probe
        // eligibility on.
        assert!(
            parsed.subcommands.iter().all(|c| c.heading_attested),
            "an operation letter under a recognized OPERATIONS: heading \
             must be heading_attested: {:?}",
            parsed.subcommands
        );
    }

    /// The measured false-positive candidate from spec §7 Tier B's
    /// "operations" extension doc comment: `mount --help`'s `Operations:`
    /// heading introduces an ordinary *flags* table (`-B, --bind`, `-M,
    /// --move`), not a command list. The extension must not fabricate
    /// subcommands from it — `flags_block_start` claims the block as
    /// flags before `is_recognized_command_heading` is ever consulted,
    /// and this test is the guard that the ordering keeps holding.
    #[test]
    fn flags_shaped_operations_heading_yields_no_subcommands() {
        let raw = concat!(
            "Operations:\n",
            " -B, --bind              mount a subtree somewhere else (same as -o bind)\n",
            " -M, --move              move a subtree to some other place\n",
            " -R, --rbind             mount a subtree and all submounts somewhere else\n",
        );
        let parsed = parse_named(raw, "mount");
        assert!(
            parsed.subcommands.is_empty(),
            "a flags table under an 'Operations:' heading must not become \
             subcommands: {:?}",
            parsed
                .subcommands
                .iter()
                .map(|c| c.name.clone())
                .collect::<Vec<_>>()
        );
        for spelling in ["bind", "move", "rbind"] {
            assert!(
                parsed.flags.iter().any(|f| f.long() == Some(spelling)),
                "missing --{spelling} in {:?}",
                parsed.flags
            );
        }
    }
}
