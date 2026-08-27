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
//!    heading between) becomes that flag's [`mandible_core::Flag::choices`],
//!    not subcommands. If no owning flag can be identified either, the
//!    block is dropped rather than guessed at.

use super::grammar::{
    bracket_flag_row_content, is_bare_flag_spelling, is_bare_flag_token,
    looks_like_bracket_flag_row, looks_like_flag_start, looks_like_paren_alternation_open,
    paren_alternation_member_content, paren_depth_delta, parse_bundled_shorts,
    parse_flag_alternation, parse_flag_spec, split_alternatives, FlagSpec,
};
use super::profile::{heading_matches_markers, FrameworkProfile};
use mandible_core::{
    is_command_name_shaped, strip_escapes, CommandNode, Flag, Positional, Provenance, Source, Text,
    ValueKind,
};

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
    pub positionals: Vec<Positional>,
    /// Flags recovered from dash-led blocks.
    pub flags: Vec<Flag>,
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
                usage_entries.push(trimmed);
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
    let description_lines: Vec<&str> = if drop_first_paragraph {
        paragraphs[1..].iter().flatten().copied().collect()
    } else {
        paragraphs.into_iter().flatten().collect()
    };
    if !description_lines.is_empty() {
        result.description = Some(description_lines.join(" "));
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
    // **Deliberately not `mandible_core::merge_flag_lists`.** A first cut
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
fn repair_repeated_character_flags(flags: &mut [Flag], glued_tokens: &GluedTokenIndex<'_>) {
    let booleans: Vec<char> = flags
        .iter()
        .filter(|f| f.value_kind == ValueKind::None)
        .filter_map(|f| f.short)
        .collect();
    for flag in flags.iter_mut() {
        let Some(short) = flag.short else { continue };
        if flag.long.is_some() || flag.value_kind != ValueKind::Required {
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
        // The name is the whole run, `long` holds it bare, and
        // `single_dash` is what puts one dash in front of it at display
        // time — see `mandible_core::Flag::single_dash`.
        flag.long = Some(token[1..].to_string());
        flag.single_dash = true;
        flag.short = None;
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
fn repair_single_dash_long_options(flags: &mut [Flag], glued_tokens: &GluedTokenIndex<'_>) {
    for flag in flags.iter_mut() {
        // 1. Option-table-sourced, never synopsis.
        if !flag.provenance.sources.contains(&Source::HelpText)
            || flag.provenance.sources.contains(&Source::HelpTextSynopsis)
        {
            continue;
        }
        // 2. A bare short flag carrying a required value.
        let Some(short) = flag.short else { continue };
        if flag.long.is_some() || flag.value_kind != ValueKind::Required {
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
        // The name is the run up to the `=`, `long` holds it bare, and
        // `single_dash` is what puts one dash in front of it at display
        // time — see `mandible_core::Flag::single_dash`.
        flag.long = Some(name_token[1..].to_string());
        flag.single_dash = true;
        flag.short = None;
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

/// Bound the leading-prose scan to before the first blank-line-preceded
/// section when there's no usage line at all (avoids treating the whole
/// output as "description" for tools with no `Usage:` line).
fn leading_prose_bound(lines: &[&str]) -> usize {
    for (idx, l) in lines.iter().enumerate() {
        if l.trim().is_empty() {
            return idx;
        }
    }
    lines.len()
}

/// True if `paragraph` (a blank-line-delimited run of leading, column-0
/// lines — see the description-collection block in
/// [`parse_with_profile`]) reads as a version/author/homepage banner rather
/// than descriptive prose.
///
/// Two independent signals, either sufficient on its own, both purely
/// structural — neither ever compares against the probed tool's own name,
/// which the hard constraint on this fix (spec §7 Tier B, generalized from
/// `zoxide`) requires:
///
/// 1. The paragraph's first line is *exactly* two tokens, `<name>
///    <version>` (clap's own template: `"zoxide 0.9.9"`). A longer first
///    line — even one that happens to contain a version-shaped word,
///    e.g. "Build v2 is faster than v1." — does not qualify: the two-token
///    shape is what a version banner actually looks like, and requiring it
///    exactly is what keeps ordinary prose from matching by accident.
/// 2. Any line in the paragraph carries a URL or an email address —
///    `zoxide`'s own author/homepage lines, and the general shape any
///    framework's templated banner uses for contact info.
///
/// Only ever consulted when a *later* paragraph exists to fall back to
/// (see the call site) — a lone paragraph that happens to match this shape
/// is kept rather than discarded, because degrading to "no description" is
/// worse than keeping a paragraph that looks unusual but is all there is.
fn is_banner_paragraph(paragraph: &[&str]) -> bool {
    match paragraph.first() {
        Some(first) if looks_like_name_version_line(first) => return true,
        _ => {}
    }
    paragraph.iter().any(|line| line_has_contact_info(line))
}

/// True if `line` is exactly two whitespace-separated tokens, a
/// name-shaped one followed by a version-shaped one — `"zoxide 0.9.9"`,
/// `"cargo 1.75.0"`. Exactly two tokens and no more: a sentence that merely
/// mentions a version number partway through does not qualify.
fn looks_like_name_version_line(line: &str) -> bool {
    let mut words = line.split_whitespace();
    let (Some(name), Some(version)) = (words.next(), words.next()) else {
        return false;
    };
    if words.next().is_some() {
        return false;
    }
    is_name_shaped_token(name) && looks_like_version_token(version)
}

/// True if `token` is shaped like a version number: an optional leading
/// `v`, then a run of digits/letters/`-`/`_`/`.` containing at least one
/// digit and at least one `.` — `0.9.9`, `v1.75.0`, `2.4.0-beta`. Digit and
/// dot are both required so a bare word (`x`) or a bare number with no dot
/// (`2020`, a copyright year) doesn't qualify.
fn looks_like_version_token(token: &str) -> bool {
    let rest = token.strip_prefix('v').unwrap_or(token);
    if rest.is_empty() {
        return false;
    }
    let mut has_digit = false;
    let mut has_dot = false;
    for c in rest.chars() {
        match c {
            '0'..='9' => has_digit = true,
            '.' => has_dot = true,
            c if c.is_ascii_alphabetic() || c == '-' || c == '_' => {}
            _ => return false,
        }
    }
    has_digit && has_dot
}

/// True if `line` contains a URL (`http://`/`https://`) or an
/// email-shaped token, as a whitespace-delimited word (common surrounding
/// punctuation — `<...>`, trailing `,`/`.` — stripped first, so
/// `"<98ajeet@gmail.com>"` and `"https://example.com,"` both match).
fn line_has_contact_info(line: &str) -> bool {
    line.split_whitespace().any(|word| {
        let trimmed = word.trim_matches(|c: char| matches!(c, '<' | '>' | ',' | '.' | '(' | ')'));
        trimmed.starts_with("http://")
            || trimmed.starts_with("https://")
            || looks_like_email(trimmed)
    })
}

/// True if `word` is shaped like an email address: a non-empty local part,
/// an `@`, and a domain part containing a `.` that doesn't start or end
/// with one, with both sides restricted to characters real addresses and
/// domains actually use. Deliberately simple — this only has to
/// distinguish "an address is present" from "one isn't," not validate one.
fn looks_like_email(word: &str) -> bool {
    let Some((local, domain)) = word.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && local
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
        && domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
}

/// True if `paragraph` (the same leading, blank-line-delimited unit
/// [`is_banner_paragraph`] is consulted on — see the description-collection
/// block in [`parse_with_profile`]) is the tool's own complaint about the
/// probe's argument, not descriptive prose, and should be dropped rather
/// than shown as the DESCRIPTION.
///
/// # The defect
///
/// A program built on a getopt-family option parser that has no `--help`
/// flag answers the probe by treating `--help` as an unrecognized option:
/// it prints its own one-line complaint, then (often, not always) still
/// manages to print a usage line. `ssh-keygen --help` writes exactly two
/// lines to stderr — `unknown option -- -` then its usage block — and
/// `c_rehash --help` writes *one* line and stops there: `Usage error; try
/// -h.`. Neither line describes what the tool does; both describe what the
/// probe did wrong. But step 2 above (leading prose becomes the
/// description) has no way to tell that apart from real descriptive
/// prose — `wpa_cli`'s root description was exactly this kind of banner
/// before that fix landed (see step 2's own comment) — and this shape
/// slipped through the same gap: it is the tool's *own* self-referential
/// complaint, not a version/author banner, so [`is_banner_paragraph`]'s
/// two signals (name-version line, contact info) never fire on it either.
///
/// # The rule
///
/// Every line in the paragraph must match [`is_option_error_line`] below.
/// Requiring *all* lines (not just the first) is what lets this fire on
/// `myisamlog`, whose probe apparently retried against several rejected
/// characters and printed four consecutive complaints
/// (`illegal option: "--"` / `"-h"` / `"-e"` / `"-l"`) with no blank line
/// between them — one paragraph, four lines, every one of them this exact
/// shape — while refusing a paragraph the moment any line in it is
/// something else, e.g. `crontab`'s second line (`crontab: usage error:
/// unrecognized option`, a *different* self-referential message this
/// predicate does not recognize as its own shape) or `vite`'s second line
/// (an unrelated Qt platform-plugin error). Both are still probably junk,
/// but this fix does not claim to know that; it only removes the lines it
/// can name with confidence, per this file's standing rule of narrow
/// predicates over broad ones.
///
/// # Why this can drop the *only* paragraph
///
/// [`is_banner_paragraph`] is only ever consulted when a later paragraph
/// exists to fall back to (see the call site's comment) — dropping a
/// tool's only leading paragraph there would trade a merely-unusual
/// description for no description at all, a worse outcome when the
/// paragraph might still be real prose that happens to look bannerish.
/// This predicate is not exposed to that risk: it recognizes a *complaint*,
/// which is never a description regardless of what else is or isn't
/// available, so it is checked before, and independently of, the
/// banner check — see the call site. `c_rehash`'s entire captured output
/// is its one-line complaint; dropping it leaves the node with no
/// description at all, which is the honest outcome — mandible does not
/// know what `c_rehash` does, and showing the probe's own error about
/// `--help` in the description pane is a worse answer than showing none.
fn is_option_error_paragraph(paragraph: &[&str]) -> bool {
    !paragraph.is_empty() && paragraph.iter().all(|line| is_option_error_line(line))
}

/// True if `line` (trimmed) is, on its own, one of the handful of
/// conventional getopt-family "you gave me a bad option" complaints —
/// see [`is_option_error_paragraph`] for why this exists and how it's used.
///
/// # The shape
///
/// An optional single-token `<name>: ` prefix (the invoking program's own
/// name or full path — `nginx: ...`, `/usr/sbin/rpcbind: ...`), then one of
/// four conventional complaints — `unknown`/`invalid`/`illegal`/
/// `unrecognized` `option`(s) — as the very first thing on the (post-prefix)
/// line, with at most a short, flag-shaped trailer (`-- '-'`, `: --help`,
/// `"--help"`); or, verbatim, busybox's `Usage error; try -h.`.
///
/// The prefix is stripped only when the text before the first `": "` has no
/// whitespace of its own — a bare name or path never contains a space, so
/// this is what tells a real `<progname>: ` prefix (`ping`'s
/// `/usr/bin/ping: invalid option -- '-'`) apart from a message that merely
/// *contains* a colon (`debconf-copydb`'s `Unknown option: help`, whose
/// pre-colon text, `"Unknown option"`, has a space and is therefore never
/// mistaken for a program name). Both shapes are handled by the same code
/// path: when the candidate prefix fails the no-whitespace test, stripping
/// is simply skipped and the *whole* line is checked against the four
/// complaints instead, which is exactly what `"Unknown option: help"`
/// needs (the message itself contains the `": "` that a real prefix would
/// have used).
///
/// The trailer bound (at most 24 characters, at most 3 whitespace-separated
/// words, and drawn only from ASCII letters/digits plus a small punctuation
/// set: space, `-`, `_`, `:`, `'`, `"`, `.`, `;`) is the safety argument
/// against the sentence reading: a real description that merely *mentions*
/// one of these phrases mid-clause (GNU tar's `--occurrence[=NUMBER]`
/// entry — hypothetical prose like "an invalid option combination here
/// raises an error" — never has the phrase open the line to begin with, so
/// it never reaches the trailer check at all; a line that *does* open with
/// the phrase but keeps going past a terse flag-shaped trailer (`socat`'s
/// `unknown option "--help"; use option "-h" for help`, whose trailer runs
/// well past both the length and word-count bound) is rejected there
/// instead.
///
/// # Measured
///
/// Over the 2,301 frozen captures in `audit/queue-captures/` (spec
/// §13.1d's frozen queue), measured the honest way — not by re-deriving
/// paragraph collection by hand (which drifts from the real usage-block
/// detection: an early attempt at this measurement undercounted because it
/// didn't recognize `nfsidmap: Usage: ...`'s name-prefixed usage line the
/// way the real scanner does), but by diffing [`parse_with_profile`]'s
/// actual `description` output with and without this predicate wired in,
/// over the same real call path: **116
/// tools** have their DESCRIPTION changed by this fix, among them `ssh`,
/// `ssh-keygen`, `ssh-keyscan`, `ssh-agent`, `sftp`, `slogin`,
/// `ssh-copy-id`, `c_rehash`, `nginx`, `myisamlog`, `ping`, `ping4`,
/// `ping6`, `reset`, `tput`, `tic`, `infocmp`, all fifteen probed `xfs_*`
/// tools, all four `fsck.ext{2,3,4}`/`mke2fs` variants, and the four
/// `debconf-*` tools (full list in this fix's PR description).
///
/// A **broader** shape — the same four keywords or "usage error" occurring
/// anywhere in the tool's raw leading text — additionally matches **52
/// tools** whose description this fix deliberately leaves untouched, each
/// excluded for one of three checked reasons rather than rounded into the
/// total:
///
/// 1. **The line never opens with a recognized phrase, even after
///    prefix-stripping** (9 tools): a multi-token prefix — a timestamp
///    and/or pid, not a bare name — on `filan`'s `2026/08/14 19:31:25
///    filan[18942] E unknown option --help`, `procan`, `socat`, `socat1`
///    (whose `; use option "-h" for help` continuation would also fail the
///    trailer bound on its own); an extra field between the prefix and the
///    message on `dash`/`sh`'s `/bin/dash: 0: Illegal option --` (the
///    shell's own `argv[0]: lineno: message` convention) and `ftp`/`tnftp`'s
///    `ftp: --: unknown option`; and a leading `*** ` marker on
///    `nslookup`'s `*** Invalid option: -help`.
/// 2. **A real banner or unrelated error precedes the complaint as the
///    paragraph's own first line** (7 tools): `debugfs`, `dumpe2fs`,
///    `e2image`, `resize2fs` (`e2image 1.47.0 (5-Feb-2023)`, a
///    three-token version line that also isn't quite
///    [`is_banner_paragraph`]'s two-token shape — a pre-existing,
///    separate gap, not one this fix claims to close), `ntfstruncate`
///    (version plus copyright), and `byobu-quiet`/`byobu-silent` (a `sed:
///    couldn't readlink ...` line ahead of the real `tmux: unknown option
///    -- X` complaint).
/// 3. **The first line matches but a later line in the same paragraph
///    carries real, distinct content** (36 tools) — `is_option_error_paragraph`'s
///    all-lines requirement (above) correctly refuses the whole paragraph
///    rather than guess which lines to drop: `crontab`'s second line
///    (`crontab: usage error: unrecognized option`, a different
///    self-referential message this predicate does not claim to
///    recognize), `sshd`'s second line (its own version banner), `lsof`'s
///    second/third lines (a different diagnostic, then its version
///    banner), `mkfs.xfs`'s second line (`unknown option -\0 `, a literal
///    embedded NUL that correctly fails the trailer's character-class
///    check), and 32 more of the same shape: `Xvfb`, `arptables-nft-save`,
///    `arptables-save`, `cgi-fcgi`, `cpgr`, `cppw`, `delv`, `devlink`,
///    `ebtables-nft-save`, `ebtables-save`, `fuser`, `ip6tables-legacy-save`,
///    `ip6tables-nft-save`, `ip6tables-save`, `iptables-legacy-save`,
///    `iptables-nft-save`, `iptables-save`, `lvmdump`, `mytop`, `nfsconf`,
///    `nfsidmap`, `nsupdate`, `pppoe-discovery`, `pptp`, `prtstat`,
///    `rsyslogd`, `socat-broker.sh`, `socat-chain.sh`, `socat-mux.sh`,
///    `vite`, `xfs_rtcp`, `zipdetails`.
fn is_option_error_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    let core = strip_option_error_progname_prefix(trimmed).unwrap_or(trimmed);
    let lower = core.to_ascii_lowercase();
    if lower == "usage error; try -h." || lower == "usage error; try -h" {
        return true;
    }
    const KEYWORDS: [&str; 4] = [
        "unknown option",
        "invalid option",
        "illegal option",
        "unrecognized option",
    ];
    for kw in KEYWORDS {
        let Some(mut tail) = lower.strip_prefix(kw) else {
            continue;
        };
        // Accept the plural ("options") too, without a separate keyword list.
        tail = tail.strip_prefix('s').unwrap_or(tail);
        return option_error_tail_is_shapely(tail);
    }
    false
}

/// Strips a leading `<token>: ` prefix from `line` when, and only when,
/// `<token>` itself contains no whitespace — see [`is_option_error_line`]
/// for why that single condition is what tells a genuine `<progname>: `
/// prefix apart from a message that merely contains a colon.
fn strip_option_error_progname_prefix(line: &str) -> Option<&str> {
    let (prefix, rest) = line.split_once(": ")?;
    if prefix.is_empty() || prefix.chars().count() > 64 || prefix.contains(char::is_whitespace) {
        return None;
    }
    Some(rest)
}

/// True if `tail` (everything after one of [`is_option_error_line`]'s four
/// keyword phrases) is short and flag-shaped rather than the start of a
/// longer sentence — see that function's doc comment for the false
/// positive (`socat`'s continuation clause) this bound exists to refuse.
fn option_error_tail_is_shapely(tail: &str) -> bool {
    let tail = tail.trim();
    if tail.is_empty() {
        return true;
    }
    if tail.chars().count() > 24 || tail.split_whitespace().count() > 3 {
        return false;
    }
    tail.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_' | ':' | '\'' | '"' | '.' | ';')
    })
}

/// The column a tab in leading whitespace advances to, from whatever
/// column it started at — the ordinary terminal tab-stop convention.
const TAB_STOP: usize = 8;

/// A line's leading indentation, as a **visual column**, not a raw
/// character count.
///
/// The two agree everywhere the fleet's overwhelming convention holds
/// (indentation built entirely from spaces): a run of `n` leading spaces
/// still measures `n` either way, so this is a byte-for-byte-identical
/// answer for that case, and every caller of this function that was
/// already correct for space-indented `--help` output stays exactly as
/// correct.
///
/// They disagree when leading whitespace mixes tabs and spaces, which is
/// where the plain character count actively lies: LVM's own emitter
/// (`vgck`, `vgextend`, `vgrename`, ...) indents its `Common options for
/// lvm:` heading two spaces and every flag row beneath it with **one
/// tab**. A raw count reads the tab as *one* column — narrower than the
/// heading's two spaces — so every "is this content indented more than
/// its heading" check in this module answered "no" and the entire block
/// (13+ flags per tool) was never even looked at as a candidate flags
/// table, regardless of anything `looks_like_flag_start` does or does not
/// accept. Expanding the tab to the next multiple of [`TAB_STOP`] (the
/// universal terminal convention, not an LVM-specific number) reads it as
/// column 8 — correctly deeper than the heading's column 2 — and every
/// downstream decision in this file that already trusted
/// `leading_whitespace`'s answer starts working for this shape too,
/// without being touched.
fn leading_whitespace(line: &str) -> usize {
    let mut col = 0usize;
    for c in line.chars() {
        if c == '\t' {
            col = (col / TAB_STOP + 1) * TAB_STOP;
        } else if c.is_whitespace() {
            col += 1;
        } else {
            break;
        }
    }
    col
}

/// True if `t` starts with `"usage:"`, case-insensitively.
///
/// Deliberately compares raw bytes rather than doing `&t[..6]` on the
/// `str` (which panics if byte offset 6 doesn't land on a UTF-8 character
/// boundary — a real crash the coverage harness found: some real-world
/// `--help` output puts a multi-byte character, e.g. a box-drawing glyph,
/// early in the first line). `[u8]::get` is bounds-checked and never
/// panics, and comparing ASCII bytes needs no UTF-8 decoding at all.
pub fn starts_with_usage_prefix(t: &str) -> bool {
    t.as_bytes()
        .get(..6)
        .map(|b| b.eq_ignore_ascii_case(b"usage:"))
        .unwrap_or(false)
}

/// True if `t` starts with `"or:"`, case-insensitively — GNU coreutils'
/// marker for a genuine *alternative* invocation form (`du`'s `Usage: du
/// [OPTION]... [FILE]...` / `  or:  du [OPTION]... --files0-from=F`), as
/// distinct from a wrapped continuation of the form above it. This is the
/// over-join guard: without recognizing this marker, a rule that joins
/// every more-indented line in the usage block onto the entry above it
/// (correct for a wrapped synopsis) would also swallow `or:`'s alternative
/// form, silently merging two real invocations into one and losing the
/// fact that `du` can be invoked either way.
///
/// Same bounds-checked byte comparison as [`starts_with_usage_prefix`], for
/// the same reason: never slice a `&str` derived from tool output at a raw
/// offset.
pub fn starts_with_or_marker(t: &str) -> bool {
    t.as_bytes()
        .get(..3)
        .map(|b| b.eq_ignore_ascii_case(b"or:"))
        .unwrap_or(false)
}

/// True if `t` (already trimmed of leading whitespace) begins with `name`
/// at a word boundary — either exactly `name`, or `name` followed by
/// whitespace. The "starts with the tool's own name" half of the
/// usage-block continuation discriminator (see the block's own comment):
/// a tool that lists alternative invocation forms *without* an `or:`/
/// `usage:` marker, by literally repeating itself —
///
/// ```text
/// Usage: prog foo
///        prog bar
/// ```
///
/// — must still read as two entries, not one continuation swallowing the
/// other.
///
/// Word-boundary checked (via `str::strip_prefix`, not a raw byte slice)
/// so a tool named `git` doesn't also claim a line that happens to start
/// with `gitk` or `git-foo`.
pub fn starts_with_tool_name(t: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    match t.strip_prefix(name) {
        Some(rest) => rest.is_empty() || rest.starts_with(char::is_whitespace),
        None => false,
    }
}

/// True if `t` (already trimmed of leading whitespace) is the C
/// `fprintf(stderr, "%s: Usage: ...", argv[0])` idiom's line: the tool's
/// own name, then a literal `": "`, then `usage:` case-insensitively —
/// `nfsidmap`'s `nfsidmap: Usage: nfsidmap [-vh] [-c || ...]`.
///
/// [`starts_with_usage_prefix`] tests the line's *start*, so a `usage:`
/// preceded by the program's own name (this ordinary `fprintf`
/// convention, framework-general rather than per-tool) is invisible to
/// it, and the whole document was previously rendered `verbatim` with
/// zero flags recovered.
///
/// Kept deliberately tight, per this fix's own hazard warning: the
/// `usage:` must be preceded by *only* the tool's own name and `": "` —
/// not scanned for anywhere inside the line, and not satisfied by the
/// name alone (an ordinary sentence starting `nfsidmap: ` followed by
/// prose must never match).
pub fn starts_with_name_prefixed_usage(t: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    t.strip_prefix(name)
        .and_then(|rest| rest.strip_prefix(": "))
        .is_some_and(starts_with_usage_prefix)
}

/// True if `t` (already trimmed of leading whitespace) opens with `name`
/// at a word boundary and its remainder reads as usage-synopsis grammar
/// rather than English prose — the **unlabelled** synopsis convention some
/// tools use in place of any `Usage:` line at all: `wpa_cli --help` simply
/// opens `wpa_cli [-p<path to ctrl sockets>] [-i<ifname>] [-hvBr] ...`,
/// with no marker anywhere.
///
/// This predicate is the entire risk this fix carries (its own doc
/// comment at the call site explains the guardrails around *where* it is
/// tried). A name match alone is not evidence of a synopsis — `"tar is an
/// archiving program that creates..."` starts with `tar` too — so two
/// independent, purely notational signals are both required:
///
/// - The remainder must contain at least one of the docopt-style group
///   delimiters spec §7 names (`[`, `<`, `{`) — the same notation
///   [`looks_like_usage_fragment`] keys on for usage-block continuation.
///   Prose describing a tool essentially never carries these characters;
///   a synopsis is built almost entirely out of them.
/// - The remainder must not read as an English sentence, reusing
///   [`is_prose_sentence`]'s own test (period-terminated, several words,
///   no multi-space column gap) — so a tool whose leading sentence
///   happens to mention a bracketed aside is still refused.
///
/// Measured on a full-`PATH` sweep before landing (see this fix's PR
/// description for the exact tool list this predicate moves).
pub fn looks_like_unlabeled_synopsis_line(t: &str, name: &str) -> bool {
    let Some(rest) = t.strip_prefix(name) else {
        return false;
    };
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        return false;
    }
    let rest = rest.trim_start();
    if rest.is_empty() {
        return false;
    }
    rest.contains(['[', '<', '{']) && !is_prose_sentence(rest)
}

/// True if `lines[idx]` is a bare own-name invocation line — no bracket
/// notation on the line itself — whose *very next* physical line is
/// unambiguous flag-row evidence: either [`looks_like_bracket_flag_row`]
/// (`vgck`'s `[ -d|--debug ]`) or [`looks_like_paren_alternation_open`]
/// (`vgchange`'s `( -l|--logicalvolume Number,`, the "any one of these is
/// required" convention). Shared by the unlabelled-synopsis entry point
/// (this file's `unlabelled_synopsis_start`) and the multi-stanza
/// continuation check in the usage-block loop below it — both need exactly
/// this one test, and a second copy would drift.
///
/// Narrow and structural, never keyed on any tool's name: a name-only line
/// only counts when the next line is itself unambiguous flag-row notation,
/// never merely because it comes right after a name match. LVM's own
/// emitter (`vgck`, `vgck --updatemetadata VG`, `vgextend VG PV ...`,
/// `vgchange`'s own paren-alternation first stanza) is the specimen this
/// was measured against, not a special case for it.
fn looks_like_bare_synopsis_head(lines: &[&str], idx: usize, name: &str) -> bool {
    let t = lines[idx].trim_start();
    starts_with_tool_name(t, name)
        && lines.get(idx + 1).is_some_and(|next| {
            let next = next.trim_start();
            looks_like_bracket_flag_row(next) || looks_like_paren_alternation_open(next)
        })
}

/// True if `lines[idx]` continues an already-open unlabelled synopsis into
/// a **later stanza**: a line opening with the tool's own name whose
/// remainder either carries a bare flag token directly (`vgck
/// --updatemetadata VG` — the flag notation itself, no bracket group at
/// all) or is followed by [`looks_like_bracket_flag_row`] evidence the same
/// way [`looks_like_bare_synopsis_head`] requires for the *first*
/// stanza. LVM's own emitter is the specimen throughout this doc comment;
/// nothing here keys on it — `adduser` and `pydoc3` hit this same
/// predicate on their own multi-stanza shapes (see this function's tests).
///
/// The first-stanza test alone cannot see this shape: `vgck`'s own second
/// stanza reads `vgck --updatemetadata VG` immediately followed by `[
/// COMMON_OPTIONS ]` — a placeholder token, not a flag row
/// ([`looks_like_bracket_flag_row`] requires the bracketed content to
/// *start* with `-`), so the next-line lookahead alone finds nothing here.
/// But the stanza head carries its own flag inline, which is strictly
/// stronger evidence than a lookahead row ever was — [`extract_usage_flags`]
/// already knows how to read a bare `--flag` sitting directly in a usage
/// line (the [M-15] grammar), so once this predicate admits the line as a
/// synopsis head, the existing machinery below recovers `--updatemetadata`
/// with no further change.
///
/// Deliberately a separate predicate from [`looks_like_bare_synopsis_head`]
/// rather than a widened copy of it: the entry point that opens the usage
/// block in the first place must stay exactly as measured (a name-only
/// line is accepted only on unambiguous *next-row* evidence), and loosening
/// it to "or carries its own flag token" would let it fire on a bare
/// tool-name-prefixed sentence that happens to mention a flag in prose —
/// a hazard the entry point's own doc comment already warns about for the
/// weaker, notation-only test. The continuation site carries no such risk:
/// it only ever runs immediately after a blank line inside an *already
/// open* unlabelled synopsis, never as a fresh scan of the whole document.
fn looks_like_stanza_continuation_head(lines: &[&str], idx: usize, name: &str) -> bool {
    let t = lines[idx].trim_start();
    let Some(rest) = t.strip_prefix(name) else {
        return false;
    };
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        return false;
    }
    let next = lines.get(idx + 1).map(|l| l.trim_start());
    let has_flag_token = rest.split_whitespace().any(is_bare_flag_token);
    let next_is_bracket_row = next.is_some_and(looks_like_bracket_flag_row);
    if !(has_flag_token || next_is_bracket_row) {
        return false;
    }
    // Guard against admitting a stanza whose own description **wraps
    // across more than one physical line**: the shared continuation loop
    // just below only ever recognizes a single physical line as "prose to
    // drop" ([`is_prose_sentence`] requires the line itself to end in a
    // period), so a description spanning two or more lines has its
    // interior line silently read as more usage notation instead — never
    // caught, because nothing about an unterminated mid-sentence line
    // says "not notation". `pydoc3`'s `-p`/`-b`/`-w` forms each carry a
    // two-line description ("Start an HTTP server on the given port on
    // the local machine.  Port" / "number 0 can be used ..."), and the
    // first line, not ending in a period, was silently glued onto the
    // synopsis text and mined for two fabricated positionals (`HTTP`,
    // `HTML`) before this guard existed. Refuse the stanza outright
    // rather than admit it and let it corrupt: whatever immediately
    // follows the head must be either absent/blank (no description at
    // all), itself more notation ([`looks_like_bracket_flag_row`] /
    // [`looks_like_usage_fragment`]), or a complete one-line sentence the
    // loop already knows how to drop — never an unterminated prose
    // fragment. `pydoc3`'s own `-k`/`-n` forms (each a single-line
    // description) pass this exactly the way `vgck`'s `[ COMMON_OPTIONS
    // ]` continuation and `lvextend`'s bracket rows do.
    match next {
        None => true,
        Some(n) if n.trim().is_empty() => true,
        Some(n) if looks_like_bracket_flag_row(n) || looks_like_usage_fragment(n) => true,
        Some(n) if is_prose_sentence(n) => true,
        _ => false,
    }
}

/// True if `t` (already trimmed of leading whitespace) opens with one of
/// the docopt-style usage grammar's own group delimiters — spec §7 Tier B:
/// "`[OPTIONS]`, `<required>`, `[optional]`, `...` for repetition, `|` for
/// alternatives, `{a|b|c}` for choices." A line that opens this way still
/// reads as more invocation syntax, not the next section starting.
///
/// This is the content-shape half of the usage-block continuation
/// discriminator, needed because indentation alone cannot separate a
/// genuine same-indent continuation (`lsof`'s `[-F [f]] [-g [s]] ...`,
/// wrapped at the exact same column as its own `usage:` marker) from a
/// same-indent line that legitimately ends the block (`du`'s trailing
/// "Summarize device usage of the set of FILEs..." sentence, immediately
/// after its `or:` line with no blank separator) — both sit at or below
/// the block's base indent, and neither is a marker or the tool's own
/// name, so only their content tells them apart: one is bracket/
/// angle-bracket syntax, the other is an English sentence starting with a
/// capital word.
fn looks_like_usage_fragment(t: &str) -> bool {
    matches!(t.as_bytes().first(), Some(b'[') | Some(b'<') | Some(b'{'))
}

/// True when `line` opens a BNF grammar production — `LABEL := ...`, with
/// or without a leading `where` keyword: iproute2's own convention,
/// `where OBJECT := { address | addrlabel | ... }` and
/// `OPTIONS := { -V[ersion] | ... }`.
///
/// This is what a column-0 line carrying a bare `:=` actually is once the
/// usage block above it has ended — grammar, not the tool's own
/// description — and without this test it silently became one: `ip`'s and
/// `vdpa`'s entire node `description` was exactly this one line
/// (`where OBJECT := { address | addrlabel | amt | ... }`), because it is
/// the first column-0 line in the document once `Usage:`'s block closes,
/// and nothing distinguished it from a genuine leading-prose sentence. The
/// production's own *wrapped continuation* lines never reach this test at
/// all — they sit indented under it, so the description-paragraph scan
/// (only ever column-0 lines) never considers them either; this closes the
/// one line that would otherwise have been read as prose.
///
/// The label before `:=` (after stripping a leading `where`) must be
/// short, plain words — the same shape [`is_section_heading_line`] already
/// trusts for an ordinary heading — so a sentence that merely happens to
/// contain a stray `:=` deep in prose (an environment-variable assignment
/// quoted in an example, say) is never mistaken for one.
fn looks_like_bnf_production_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    let trimmed = trimmed
        .strip_prefix("where")
        .map_or(trimmed, str::trim_start);
    let Some(op) = trimmed.find(":=") else {
        return false;
    };
    let label = trimmed[..op].trim_end();
    !label.is_empty()
        && label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ' ' || c == '-')
}

/// Rewrite every line that carries a section heading **and** the first row
/// of that section's own table into the two lines it means.
///
/// # The defect
///
/// `uconv --help` runs its heading straight into its first option row:
///
/// ```text
/// Options:  -h, --help                    print this message
///           -V, --version                 print the program version
/// ```
///
/// The section scanner promotes a line to a heading whole, so the entire
/// first line became the heading — `-h, --help` was never a flag under any
/// spelling a user could type, and every other flag in the block inherited
/// `group: "Options:  -h, --help                    print this message"`.
/// The audit reviewer's own words: "since the flag `-h` was in front of
/// `Options:` it got swallowed into the section header".
///
/// Measured over the 2,301 frozen captures in `audit/queue-captures/`:
/// **2 tools** (`uconv`, and `zipinfo`'s `main listing-format options:
/// -s  short Unix "ls -l" format (def.)`), each losing exactly the one row
/// that shares its heading's line. Small, and reported as measured rather
/// than rounded up — the *broad* shape (a heading label, a column gap, and
/// then anything at all) is 12 tools, but the other ten are second heading
/// columns (`awk`'s `POSIX options:\t\tGNU long options: (standard)`) or
/// wrapped prose, and rewriting those would invent rows rather than
/// recover them.
///
/// # A second shape: the BNF-grammar heading (`ip` and its iproute2 siblings)
///
/// `ip --help` writes its whole synopsis as a BNF grammar, and its `OPTIONS`
/// production opens the same way `uconv`'s did — heading and first row on
/// one physical line — except the label is glued to the row by `:=`, not by
/// a column of spaces:
///
/// ```text
/// where  OBJECT := { address | addrlabel | amt | fou | help | ila | ioam | l2tp |
///                    ...
///        OPTIONS := { -V[ersion] | -s[tatistics] | -d[etails] | -r[esolve] |
///                     -h[uman-readable] | -iec | -j[son] | -p[retty] |
///                     ...
/// ```
///
/// The original clause 3 (`MIN_COLUMN_GAP_SPACES` spaces right after the
/// colon) never fires here: the character right after `:` is `=`, not a
/// space, so the gap is zero and the whole line — `-V`, `-s`, `-d`, `-r`
/// included — was promoted to the heading string. `mandible --doctor ip`
/// read 8 flags before this fix; the group label was literally `OPTIONS :=
/// { -V[ERSION] | -S[TATISTICS] | -D[ETAILS] | -R[ESOLVE] |`.
///
/// Measured the same way as the shape above, over the same 2,301 captures:
/// **6 tools** gain their first `OPTIONS` row back — `bridge`, `dcb`,
/// `devlink`, `ip`, `rdma`, `vdpa` (all iproute2-family binaries; `dcb`'s
/// row opens `[ -V | --Version | ...`, the rest open `{ -V[ersion] | ...`).
/// A 7th tool, `ss`, matches the raw `:=\s*[{[]` grep this was measured
/// with but recovers nothing: every one of its BNF productions
/// (`FAMILY := {inet|inet6|...}`, `QUERY := {...}`, `STATE-FILTER := {...}`,
/// `connected := {...}`, `synchronized := {...}`, `bucket := {...}`,
/// `big := {...}`) opens on a bare word, not a flag, so clause 4 rejects
/// all of them — correctly: `ss` writes its actual flags one per line
/// already (`-h, --help          this message`), never sharing a heading.
/// `ip`'s own sibling `OBJECT := { address | addrlabel | ... }` production
/// is excluded the same way, by the same clause, for the same reason.
///
/// The *broader* version of this shape — drop the operator requirement and
/// accept any `label:` immediately followed by an opening bracket — matches
/// 36 tools, and the extra ones are exactly the false positives the operator
/// requirement exists to keep out. `pkgdata`'s `modes: (-m option)` is the
/// sharpest case: strip the label, the gap, and the `(` the same way the
/// bracket-only rule would, and the remainder is `-m option)` — which
/// *does* satisfy [`looks_like_flag_start`], so clause 4 does not save it.
/// The rest of the 36 are the same family of near-miss: usage-line
/// continuations (`lsof`'s own `usage:` line, already excluded elsewhere),
/// stack-trace fragments that happen to contain `[Errno 13]` (`dnf`, `ua`,
/// `pro`, `swift-recon-cron`), and parenthetical asides after a real heading
/// (`whiptail`'s `Options: (depend on box-option)`, `pkgdata`'s `modes: (-m
/// option)`, `mariadb-admin`'s `Where command is a one or more of: (Commands
/// may be shortened)`). Requiring the `=` is what tells a BNF assignment
/// apart from a colon that merely happens to be followed by a parenthesis.
///
/// # The rule
///
/// A line is split when **all** of these hold:
///
/// 1. its indentation is spaces only — a tab's width is a terminal
///    setting, so the recovered row's column could not be reproduced;
/// 2. the text up to and including its first `:` is a
///    [`is_section_heading_line`] label (short, plain words, colon-
///    terminated) and is not a `usage:` marker;
/// 3. **either** at least [`MIN_COLUMN_GAP_SPACES`] spaces follow the colon
///    (the `uconv`/`zipinfo` shape), **or** the colon is immediately
///    followed by a BNF `=` (making it read as `:=`), at least one space,
///    and optionally a single opening bracket (`{`/`[`/`(`) followed by at
///    least one more space (the `ip`-family shape) — see the section above
///    for why the operator, not the bracket, is the discriminator;
/// 4. what follows that gap [`looks_like_flag_start`].
///
/// Clause 4 is the safety argument. Clauses 1-3 alone are satisfied by
/// every "label, then a value" line in the fleet (`ntfs-3g`'s `Options:
/// ro (read-only mount), windows_names, uid=, gid=,`, `delv`'s `Where:
/// domain\t  is in the Domain Name System`), and splitting one of those
/// would hand the flags block a row that is not a flag. Requiring the
/// remainder to open like a flag spelling is what confines this to the
/// case where a real row is demonstrably being lost.
///
/// Returns `None` when no line matched, so the overwhelmingly common
/// document is parsed from its own borrowed `&str` with no allocation.
///
/// The returned [`HashSet`] names, by 0-indexed line number in the
/// *rewritten* text, every **row** line (never the heading line beside it)
/// this function recovered via the `:=` operator clause — never the plain
/// column-gap clause. This is the one piece of evidence
/// [`split_bnf_alternation_row`] is gated on: a BNF `:=` production and an
/// ordinary options table can both write a short/long pair joined by a
/// bare `|` (`btrfsck`'s own `-E|--subvol-extents <subvolid>` uses `|` as a
/// plain alias separator, no grammar involved), and only the *document* —
/// not the row's own text — says which is which. By the point
/// [`scan_flags_block`] sees a row, the operator itself is already gone
/// from both the heading and the row (this function's own job), so the set
/// is the only way that fact survives to reach it.
///
/// **Keyed on the row, not the heading**, because the engine does not
/// always recognize the heading `split_shared_heading_row` produced as a
/// heading in its own right before handing a block to
/// [`scan_flags_block`]. A `where OBJECT := { ... }` production that fits
/// on one physical line (`dcb`, `vdpa`) reads, to the general section
/// loop, as a heading of its own whose "content" is merely whatever is
/// indented more than column 0 — which the very next `OPTIONS :` heading
/// line always is, coincidentally, regardless of what it actually is. The
/// loop never revisits that line as a heading in its own right; it reaches
/// [`scan_flags_block`] straight from the *headingless* call site once the
/// bare-block scan dedents back out, with no heading index available to
/// check at all. The row itself, though, is always exactly the first line
/// [`scan_flags_block`] is asked to start from — [`flags_block_start`]
/// never skips ahead of an already-flag-shaped first line — so recording
/// row lines is what makes the gate reachable from *either* call site.
fn split_shared_heading_rows(raw: &str) -> Option<(String, std::collections::HashSet<usize>)> {
    let mut out = String::new();
    let mut split_any = false;
    let mut bnf_row_lines = std::collections::HashSet::new();
    let mut out_line_no = 0usize;
    for line in raw.lines() {
        match split_shared_heading_row(line) {
            Some((heading, row, is_bnf)) => {
                split_any = true;
                if is_bnf {
                    bnf_row_lines.insert(out_line_no + 1);
                }
                out.push_str(&heading);
                out.push('\n');
                out.push_str(&row);
                out.push('\n');
                out_line_no += 2;
            }
            None => {
                out.push_str(line);
                out.push('\n');
                out_line_no += 1;
            }
        }
    }
    split_any.then_some((out, bnf_row_lines))
}

/// One line's worth of [`split_shared_heading_rows`]: the heading line and
/// the row line it was carrying, the row re-indented to the column it
/// occupied in the original so the block below reads the same alignment it
/// always did.
///
/// Char-indexed throughout, never a byte-offset `&str` slice — AGENTS.md's
/// rule against slicing captured tool output at a raw byte offset.
fn split_shared_heading_row(line: &str) -> Option<(String, String, bool)> {
    let chars: Vec<char> = line.chars().collect();
    let indent = chars.iter().take_while(|c| c.is_whitespace()).count();
    if chars[..indent].iter().any(|c| *c != ' ') {
        return None;
    }
    let colon = chars.iter().position(|c| *c == ':')?;
    if colon <= indent {
        return None;
    }
    let label: String = chars[indent..=colon].iter().collect();
    if !is_section_heading_line(&label) || starts_with_usage_prefix(&label) {
        return None;
    }
    let mut row_start = colon + 1;
    // A BNF definition operator: the colon reads as `:=`, not a plain
    // section-heading colon. See this function's doc comment for why the
    // operator itself — not merely a bracket — is what widens clause 3.
    let has_bnf_operator = chars.get(row_start) == Some(&'=');
    if has_bnf_operator {
        row_start += 1;
    }
    let gap_start = row_start;
    while row_start < chars.len() && chars[row_start] == ' ' {
        row_start += 1;
    }
    let gap_spaces = row_start - gap_start;
    if has_bnf_operator {
        if gap_spaces == 0 || row_start >= chars.len() {
            return None;
        }
        // An optional opening bracket the grammar wraps its row in
        // (`ip`'s `{`, `dcb`'s `[`), skipped along with the space after it.
        if matches!(chars.get(row_start), Some('{') | Some('[') | Some('(')) {
            row_start += 1;
            let bracket_gap_start = row_start;
            while row_start < chars.len() && chars[row_start] == ' ' {
                row_start += 1;
            }
            if row_start - bracket_gap_start == 0 || row_start >= chars.len() {
                return None;
            }
        }
    } else if gap_spaces < MIN_COLUMN_GAP_SPACES || row_start >= chars.len() {
        return None;
    }
    let row: String = chars[row_start..].iter().collect();
    if !looks_like_flag_start(&row) {
        return None;
    }
    let heading: String = chars[..=colon].iter().collect();
    let mut row_line = " ".repeat(row_start);
    row_line.push_str(&row);
    Some((heading, row_line, has_bnf_operator))
}

/// Fewest whitespace-separated words a period-terminated single-field line
/// must carry before [`is_prose_sentence`] reads it as a sentence.
///
/// Five, chosen against the measured population rather than by taste: the
/// shortest real specimen in the fleet is `[`'s "Exit with the status
/// determined by EXPRESSION." (seven words) and `getent`'s "Get entries
/// from administrative database." (five), while the shortest *heading*
/// this must never claim is a two- or three-word label. Nothing between
/// four and five words was found on either side, so the boundary is not
/// load-bearing in the way a tighter one would be.
const MIN_PROSE_SENTENCE_WORDS: usize = 5;

/// True when `heading` is an English sentence rather than a section
/// heading — a single field (no column gap anywhere), several words long,
/// terminated by a full stop.
///
/// # The defect
///
/// The section scanner promotes a line to a heading on **indentation
/// alone**: any line whose next non-blank neighbour is indented further is
/// read as introducing that neighbour's block. A tool that closes its
/// preamble with a sentence and then indents its option table one column
/// therefore hands the scanner a sentence where a heading belongs, and
/// every flag in the block inherits it as [`mandible_core::Flag::group`] —
/// which the flags pane renders, uppercased, as a section header:
///
/// ```text
/// When a filename is '-', nano reads data from standard input.
///
///  Option         Long option             Meaning
///  -A             --smarthome             Enable smart home key
/// ```
///
/// Measured over the 2,301 frozen captures in `audit/queue-captures/`:
/// **205 tools**, 211 distinct (tool, line) pairs. It is overwhelmingly
/// the GNU convention — 56 tools inherit "Mandatory arguments to long
/// options are mandatory for short options too.", 13 inherit "With no
/// FILE, or when FILE is -, read standard input." — so it is a layout
/// fact about a whole family of `--help` writers, not a quirk of any one
/// tool.
///
/// # The rule, and what each clause keeps out
///
/// - **No column gap** ([`find_multi_space_gap`], deliberately *not*
///   [`find_description_gap`], whose sentence-start and `=`-separator
///   fallbacks would fire on the very prose this is trying to recognize).
///   A two-column line is a table row, not a sentence: it is what keeps
///   `arptables`' `[!] --version\t-V\t\tprint package version.` and
///   `fail2ban-client`'s `set logtarget <TARGET>   sets logging target to
///   <TARGET>.` — both period-terminated, neither prose — out.
/// - **Terminated by a full stop.** Headings are labels; they do not end
///   in a sentence terminator. This is what leaves every colon-terminated
///   heading alone, including the genuinely prose-shaped ones a stricter
///   wording test would have destroyed: `gcc`/`lto-dump` writes "The
///   following options are specific to just the language C:" and
///   `objdump` "At least one of the following switches must be given:",
///   and both are real headings over real blocks.
/// - **At least [`MIN_PROSE_SENTENCE_WORDS`] words**, so a short
///   period-carrying label can never qualify.
/// - **Not an ellipsis.** A trailing `...` is docopt-style usage notation
///   for repetition (`numactl`'s own `[--localalloc | -l] command args
///   ...`, `mkfontscale`'s `[-u] [-U] [-v] [ directory ]...`), never a
///   sentence terminator, but a naive `ends_with('.')` test reads its last
///   character the same way it reads a real full stop. Measured on the
///   usage-block continuation call site added alongside this clause:
///   without it, both lines above were misread as prose and (incorrectly)
///   ended the usage block early, silently dropping every flag the
///   synopsis still had left to name — 19 on `numactl`, 3 on
///   `mkfontscale`. Three dots, not one, so a single mid-notation period
///   (`<v1.0>`) is unaffected.
///
/// # What this does *not* touch
///
/// Two call sites use this to decide whether a line reads as English prose
/// rather than usage/heading notation ([`looks_like_unlabeled_synopsis_line`]
/// and the usage-block's own more-indented-continuation check in
/// [`parse_with_profile`], which stops a synopsis from swallowing
/// `sg_emc_trespass`'s trailing sentences and mining `LUN`/`SP`/`EMC` out of
/// them as fabricated positionals); the other two copy a heading into a
/// `group` and nowhere else — never by [`is_recognized_command_heading`],
/// never by `command_mode`, never by anything that sets
/// `CommandNode::heading_attested`. Spec §6's attestation gate reads
/// `heading_attested`, so the set of nodes eligible to become `<word>
/// --help` probe argv is bit-for-bit identical before and after this
/// change; `mandible-extract/tests/exec_policy.rs`'s
/// `prose_heading_suppression_does_not_widen_probe_eligibility` pins that.
fn is_prose_sentence(heading: &str) -> bool {
    let trimmed = heading.trim_end();
    if !trimmed.ends_with('.') || trimmed.ends_with("...") {
        return false;
    }
    if trimmed.split_whitespace().count() < MIN_PROSE_SENTENCE_WORDS {
        return false;
    }
    find_multi_space_gap(heading).is_none()
}

/// True when `heading` is the first half of a backslash-continued logical
/// line, and so cannot be a heading of anything: the tool has said, with
/// the shell's own continuation marker, that the line is not finished.
///
/// # The defect
///
/// The same indentation-alone promotion [`is_prose_sentence`] documents,
/// reached from the other direction. `update-xmlcatalog --help` writes its
/// synopsis as backslash-continued pairs:
///
/// ```text
///     update-xmlcatalog <options> --del --root --type <type> \
///                                                 --id <id>
/// ```
///
/// The second line is indented far past the first, so the first is read as
/// a heading and the second as its block — and the TUI renders
/// `UPDATE-XMLCATALOG <OPTIONS> --DEL --ROOT --TYPE <TYPE> \` as a section
/// header. Measured over the frozen captures: **7 tools**, 16 distinct
/// lines (`update-xmlcatalog`, `wpa_cli`, `zic`, and the four `bpfcc`
/// tracers, whose `EXAMPLES` sections wrap the same way).
///
/// Like [`is_prose_sentence`], this only suppresses the `group`; see that
/// function's "What this does not touch".
fn is_line_continuation_fragment(heading: &str) -> bool {
    heading.trim_end().ends_with('\\')
}

/// True when `heading` may be copied into a recovered entry's `group`.
///
/// The one predicate the three group-assigning sites share, so "what
/// counts as a heading for display purposes" is written down once instead
/// of three times. Both clauses are *subtractive*: a line either reads as
/// something that is positively not a heading, or it is left exactly as it
/// was before.
fn heading_can_name_a_group(heading: &str) -> bool {
    !is_prose_sentence(heading) && !is_line_continuation_fragment(heading)
}

/// Longest label this will accept before a `:` still counts as a section
/// heading. Real headings are a few words (`command specific modifiers:`,
/// `Available Commands:`); a long colon-terminated line is prose.
const MAX_HEADING_LABEL: usize = 60;

/// True if `t` (already trimmed of leading whitespace) is a section
/// heading: a short, colon-terminated label of plain words.
///
/// The plain-words test is what keeps usage grammar out. Every delimiter
/// the docopt-style synopsis grammar uses (`[`, `<`, `{`, `|`, `=`, `.`)
/// is excluded from the label, so a wrapped synopsis fragment can never
/// qualify however it is indented, while ` commands:` and ` generic
/// modifiers:` both do. The colon must terminate the whole line: a
/// synopsis carrying an interior colon (`host:port`) is untouched.
fn is_section_heading_line(t: &str) -> bool {
    let trimmed = t.trim_end();
    let Some(label) = trimmed.strip_suffix(':') else {
        return false;
    };
    if label.is_empty() || label.chars().count() > MAX_HEADING_LABEL {
        return false;
    }
    label
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_')
}

/// True if `line` looks like a row of a bare-name grid (openssl-style
/// `--help` output: `asn1parse   ca   ciphers   cmp`) rather than prose or
/// a flag spec — every column is name-shaped (starts with a letter,
/// otherwise only alphanumerics/`-`/`_`) and none starts with `-` (which
/// would make it a flag entry instead).
///
/// Used to *continue* a grid already started by
/// [`looks_like_word_grid_start`], so it accepts a lone trailing token
/// (openssl's final `x509` on its own line) as well as a multi-column
/// row. Multi-column rows are held to the same 2+-space column rule as
/// the start line: continuing on single-spaced prose is how a grid that
/// began legitimately would still end up swallowing a paragraph.
fn looks_like_word_grid_line(line: &str) -> bool {
    let columns = split_columns(line);
    if columns.is_empty() {
        return false;
    }
    columns.iter().all(|c| is_name_shaped_token(c))
}

/// Stricter version used only to *start* a grid: requires 3+ **columns**,
/// so a two-word heading immediately above the grid (`"Standard commands"`)
/// is never itself mistaken for the first grid row. Once a grid has
/// started, [`looks_like_word_grid_line`] (which allows a trailing
/// single-token row, e.g. openssl's lone `x509` closing out a section) is
/// used to keep consuming it.
///
/// "Column" means a field separated from its neighbours by a run of **two
/// or more** spaces, not merely by whitespace. That distinction is the
/// whole guard against reading a wrapped prose paragraph as a command
/// list: a real grid is laid out in aligned columns
/// (`asn1parse         ca                ciphers`), while prose separates
/// its words with exactly one space. Without it, `apt-get --help` gained
/// the subcommands *"and"*, *"information"*, *"about"*, *"them"*,
/// *"from"*, *"authenticated"* and *"sources"* — every word of its
/// description paragraph past the first line — because the sentence above
/// it ("apt-get is a **command** line interface for retrieval of
/// packages") contains the word "command" and so passed
/// [`is_recognized_command_heading`], and the paragraph's own lines are
/// all name-shaped words at a matching indent. That is [M-10] exactly:
/// fabricated structure a user cannot tell is wrong. Column alignment is
/// a structural property of the layout, so this stays a general rule
/// rather than anything keyed to a tool or a framework.
fn looks_like_word_grid_start(line: &str) -> bool {
    let columns = split_columns(line);
    columns.len() >= 3 && columns.iter().all(|c| is_name_shaped_token(c))
}

/// True if `lines` is a rendered man page rather than `--help` output.
///
/// The signal is the page banner every `man` renderer emits: a first line
/// carrying the same `NAME(section)` title at both the left and right
/// margins, e.g. `GIT-BISECT(1)    Git Manual    GIT-BISECT(1)`. That is a
/// property of the roff output format, not of any tool or framework, and
/// no `--help` summary looks like it.
fn looks_like_man_page(lines: &[&str]) -> bool {
    let Some(first) = lines.iter().find(|l| !l.trim().is_empty()) else {
        return false;
    };
    let trimmed = first.trim();
    let Some(head) = trimmed.split_whitespace().next() else {
        return false;
    };
    let Some(tail) = trimmed.split_whitespace().next_back() else {
        return false;
    };
    // Both margins must carry the identical `NAME(section)` token, and
    // there must be a centred title between them — a single repeated word
    // on its own is not a banner.
    head == tail
        && head.ends_with(')')
        && head.contains('(')
        && trimmed.split_whitespace().count() > 2
}

/// Public wrapper around [`looks_like_man_page`], for the coverage harness
/// (spec §13.1, [M-16]) to reuse rather than reimplement.
///
/// [M-16] proposes falling back to `-h` when `--help` renders a man page
/// (git's subcommands do this; its root does not, and that distinction is
/// exactly what this function exists to get right). Before that fallback
/// can be sent — an argv broadening the maintainer has ruled must be
/// measured first, not assumed — something has to enumerate which tools on
/// `PATH` would newly receive it. That enumeration must not spawn a second
/// probe of its own (spec §6: every invocation is measured, unmeasured
/// broadening is the exact hazard [M-16] is about), so it re-runs this
/// *same* detection over text the pipeline already captured — a tool's
/// `CommandNode::unparsed` line, set by [`super::build_node`] precisely
/// when this check fired (or when nothing else parsed for some other
/// reason; the caller re-checks here to tell those two apart) — instead of
/// touching the tool a second time.
///
/// Kept as a thin wrapper rather than inlined at the call site so there is
/// exactly one definition of "looks like a rendered man page": duplicating
/// the rule for a caller outside this module is how the two copies would
/// eventually drift, and this one is about to gate a safety decision.
pub fn is_man_page_banner(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().collect();
    looks_like_man_page(&lines)
}

/// Split `line` on runs of two or more spaces, discarding empty fields.
/// Fields keep any internal single spaces, so a prose fragment comes back
/// as one field containing whitespace — which `is_name_shaped_token`
/// then rejects.
fn split_columns(line: &str) -> Vec<&str> {
    line.trim()
        .split("  ")
        .map(|f| f.trim())
        .filter(|f| !f.is_empty())
        .collect()
}

fn is_name_shaped_token(t: &str) -> bool {
    let mut chars = t.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// True if `heading` is a recognized command-block introduction: either
/// spec §7 Tier B rule 1's literal generic test (mentions "command(s)" or
/// "subcommand(s)" as a word), or — when a framework was identified — one
/// of that framework's own extra heading markers
/// ([`FrameworkProfile::command_heading_markers`]). A framework profile
/// asserting [`FrameworkProfile::no_subcommand_concept`] overrides both:
/// it means this framework's help output structurally never has
/// subcommands, so no heading of any kind should ever be recognized here
/// — the direct fix for [M-10] (spec §7 Tier B rule 1: "must produce zero
/// subcommands"), made structural instead of incidental to which exact
/// words one tool's heading happens to use.
///
/// This is *not* the whole test — a heading can also qualify by being
/// part of a chain started by such a mention elsewhere (git's group
/// headings) — see [`command_mode_seed`] and `command_mode` in
/// [`parse_with_profile`].
fn is_recognized_command_heading(heading: &str, profile: Option<&FrameworkProfile>) -> bool {
    if let Some(p) = profile {
        if p.no_subcommand_concept {
            return false;
        }
        if heading_matches_markers(&heading.to_lowercase(), p.command_heading_markers) {
            return true;
        }
    }
    mentions_commands_word(heading)
}

/// True if `text` (prose introducing a heading chain, e.g. git's "These
/// are common Git commands used in various situations:") should seed
/// `command_mode` — same [`FrameworkProfile::no_subcommand_concept`]
/// override as [`is_recognized_command_heading`]: a framework with no
/// subcommand concept must never have `command_mode` turned on by a prose
/// mention either, since that mention almost certainly isn't about a
/// command list this framework doesn't have (e.g. a GNU-argp tool's
/// `--help` prose mentioning "commands" in an unrelated sentence).
fn command_mode_seed(text: &str, profile: Option<&FrameworkProfile>) -> bool {
    if profile.is_some_and(|p| p.no_subcommand_concept) {
        return false;
    }
    mentions_commands_word(text)
}

/// Find the index of the flag in `flags` that `heading` is most plausibly
/// "nested under" (spec §7 Tier B rule 4): first, a flag whose long or
/// short spelling is literally mentioned in the heading text (`"Valid
/// arguments for the --quoting-style option are:"` names `--quoting-style`
/// directly); failing that, the most recently emitted flag, since an
/// unlabeled enum list in `--help` output conventionally follows the flag
/// it enumerates with no other heading in between (tar's `--format=FORMAT`
/// immediately followed by `"FORMAT is one of the following:"`).
fn find_owning_flag_index(heading: &str, flags: &[Flag]) -> Option<usize> {
    let lower = heading.to_lowercase();
    if let Some(idx) = flags.iter().position(|f| {
        f.long
            .as_deref()
            .is_some_and(|l| lower.contains(&format!("--{}", l.to_lowercase())))
    }) {
        return Some(idx);
    }
    if flags.is_empty() {
        None
    } else {
        Some(flags.len() - 1)
    }
}

/// Turn a word-grid block into subcommand stubs (if `treat_as_commands`)
/// or drop it (spec §7 Tier B rule 1 — a word grid is layout, not by
/// itself evidence of a command list). Word grids carry no per-entry
/// description, so there is nothing sensible to route to `choices` here;
/// unattributed grids are simply dropped rather than guessed at.
fn process_word_grid(
    heading: &str,
    grid_lines: &[&str],
    treat_as_commands: bool,
    out: &mut ParsedHelp,
) -> (usize, usize) {
    let mut seen = 0usize;
    let mut clean = 0usize;
    for line in grid_lines {
        for token in line.split_whitespace() {
            seen += 1;
            if !is_command_name_shaped(token) {
                out.saw_unattributable_content = true;
                continue;
            }
            clean += 1;
            if treat_as_commands {
                out.try_push_subcommand(CommandNode {
                    group: heading_can_name_a_group(heading).then(|| heading.to_string()),
                    // `treat_as_commands` is only ever `true` when the
                    // grid's heading was `recognized` or the parser was
                    // already in `command_mode` (see the caller) — i.e.
                    // this entry has exactly the positive evidence spec
                    // issue #2 asks `structure_sanity` to trust, even
                    // though a word-grid entry carries no per-entry
                    // description (openssl's `asn1parse`, `ciphers`, ...).
                    heading_attested: true,
                    ..CommandNode::new(token, Provenance::single(Source::HelpText))
                });
            }
        }
    }
    if !treat_as_commands && seen > 0 {
        out.saw_unattributable_content = true;
    }
    (seen, clean)
}

/// Emit a flags block's entries as [`Flag`]s. `group` is `None` for a
/// headingless block (spec §7 Tier B rule 2's continuation handling
/// already folded wrapped descriptions in during scanning).
/// A flags block's heading as a display *group*, or `None` when the
/// heading is just the generic "here are the flags" label.
///
/// `Flag::group` exists to preserve meaningful subdivisions — tar's 171
/// flags under headings like "Main operation mode" are the difference
/// between a scannable pane and a wall of text. A heading that only says
/// "Options" or "Flags" subdivides nothing: it names the section the
/// detail pane already prints its own `FLAGS` heading for, so keeping it
/// rendered `FLAGS` twice in a row (visible on `gh`, whose help output
/// titles that section `FLAGS`).
fn meaningful_flag_group(heading: String) -> Option<String> {
    const GENERIC: [&str; 6] = [
        "options",
        "flags",
        "option",
        "flag",
        "optional arguments",
        "global flags",
    ];
    let normalized = heading.trim().trim_end_matches(':').to_lowercase();
    if GENERIC.contains(&normalized.as_str()) || !heading_can_name_a_group(&heading) {
        None
    } else {
        Some(heading)
    }
}

fn emit_flags(
    group: Option<String>,
    entries: Vec<(String, String)>,
    out: &mut ParsedHelp,
) -> (usize, usize) {
    let mut seen = 0usize;
    let mut clean = 0usize;
    for (spec_text, desc_text) in entries {
        if out.flags.len() >= MAX_RECOVERED_ENTRIES {
            break;
        }
        seen += 1;
        let spec = parse_flag_spec(&spec_text);
        if spec.fully_consumed {
            clean += 1;
        }
        if spec.short.is_none() && spec.long.is_none() {
            // Nothing recognizable as a flag at all; skip rather than
            // emit a garbage entry.
            continue;
        }
        out.flags.push(Flag {
            short: spec.short,
            long: spec.long,
            value_name: spec.value_name,
            value_kind: spec.value_kind,
            choices: Vec::new(),
            repeatable: false,
            required: false,
            negatable: spec.negatable,
            single_dash: false,
            hidden: false,
            deprecated: None,
            inherited: false,
            group: group.clone(),
            description: non_empty_text(&desc_text),
            default: None,
            env_var: None,
            provenance: Provenance::single(Source::HelpText),
        });
    }
    (seen, clean)
}

/// Emit a [`block_is_packed_flag_rows`]-shaped block's entries directly,
/// never through [`parse_flag_spec`]/[`emit_flags`]: that grammar's alias
/// loop reads `-wholename` as the *short* flag `-w` plus a required value
/// `"holename"` (`try_short` takes one character unconditionally before
/// `try_long`'s two-dash form ever gets a look), which is exactly the
/// reading `repair_single_dash_long_options` exists to correct elsewhere —
/// but here the second element of each entry is the flag's own *operand*,
/// never a description, and feeding it to `emit_flags` would show it as
/// one. Bypassing the grammar entirely for this narrow shape means the
/// spelling is decided the same way the repair pass already treats it
/// (one bare character is a short flag; anything longer is a single-dash
/// long option, spec's own `single_dash` field), and the operand text —
/// when this shape's notation resists any further, safer decomposition
/// (`-perm`'s `[-/]MODE`, a prefix bracket with a bare suffix glued
/// straight after it with no separator at all) — is kept exactly as the
/// tool wrote it rather than guessed at.
fn emit_packed_flags(group: Option<String>, entries: Vec<(String, String)>, out: &mut ParsedHelp) {
    // Scoped to this one block's own entries: GNU find's `-exec`/`-execdir`
    // document two invocation forms (`COMMAND ;` and `COMMAND {} +`) as two
    // separate packed entries sharing one spelling. One `Flag` per
    // spelling, not two identical-looking rows — the second form's operand
    // text is appended to the first's, verbatim, rather than dropped.
    let mut names: Vec<String> = Vec::new();
    let mut operands: Vec<String> = Vec::new();
    let mut index_of: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (spelling, operand) in entries {
        let name = spelling.trim_start_matches('-').to_string();
        if name.is_empty() {
            continue;
        }
        match index_of.get(&name) {
            Some(&idx) => {
                if !operand.is_empty() && operands[idx] != operand {
                    if !operands[idx].is_empty() {
                        operands[idx].push_str(" | ");
                    }
                    operands[idx].push_str(&operand);
                }
            }
            None => {
                index_of.insert(name.clone(), names.len());
                names.push(name);
                operands.push(operand);
            }
        }
    }
    for (name, operand) in names.into_iter().zip(operands) {
        if out.flags.len() >= MAX_RECOVERED_ENTRIES {
            break;
        }
        let mut chars = name.chars();
        let (short, long, single_dash) = match (chars.next(), chars.next()) {
            (Some(c), None) => (Some(c), None, false),
            _ => (None, Some(name), true),
        };
        let value_kind = if operand.is_empty() {
            ValueKind::None
        } else {
            ValueKind::Required
        };
        out.flags.push(Flag {
            short,
            long,
            value_name: (!operand.is_empty()).then_some(operand),
            value_kind,
            choices: Vec::new(),
            repeatable: false,
            required: false,
            negatable: false,
            single_dash,
            hidden: false,
            deprecated: None,
            inherited: false,
            group: group.clone(),
            description: None,
            default: None,
            env_var: None,
            provenance: Provenance::single(Source::HelpText),
        });
    }
}

/// Emit a recognized bare-word block's entries as subcommand stubs (spec
/// §7 Tier B rules 1 and 3). Entries failing the name-shape test are
/// dropped, not emitted — never fabricated.
fn emit_subcommands(
    heading: &str,
    entries: Vec<(&str, String)>,
    out: &mut ParsedHelp,
) -> (usize, usize) {
    let mut seen = 0usize;
    let mut clean = 0usize;
    for (spec_text, desc_text) in entries {
        // A trailing colon after the name (`"auth:        Authenticate..."`,
        // a real cobra-app template convention — captured directly from
        // `gh --help`, not invented) is punctuation, never part of the
        // name itself; strip it before the shape check below so this
        // common layout doesn't cause an otherwise perfectly good
        // subcommand name to be dropped as unattributable. Framework-
        // general (any framework's command list may format this way), not
        // gated on a specific one.
        let name = spec_text.trim().trim_end_matches(':').trim();
        let name = strip_optional_modifier_suffix(name);
        if name.is_empty() {
            continue;
        }
        seen += 1;
        if !is_command_name_shaped(name) {
            out.saw_unattributable_content = true;
            continue;
        }
        clean += 1;
        let mut node = CommandNode::new(name, Provenance::single(Source::HelpText));
        node.summary = non_empty_text(&desc_text);
        node.group = heading_can_name_a_group(heading).then(|| heading.to_string());
        node.children_filled = false;
        // Every call site of `emit_subcommands` is already gated on
        // positive evidence of a real command list — a recognized heading,
        // a `command_mode` chain started by one, or argparse's own
        // `{choice,...}` pseudo-entry shape — so an entry recovered here
        // is never "conjured from layout alone" (spec issue #2's
        // distinction). This is what lets the coverage harness's
        // structure-sanity check stop treating a description-less entry
        // as suspicious purely for being description-less.
        node.heading_attested = true;
        out.try_push_subcommand(node);
    }
    (seen, clean)
}

/// Emit a headed command table's rows — `wpa_cli`'s ` = `-separated
/// `commands:` block and `apt-ftparchive`'s operand-only `Commands:` table
/// (see [`scan_bare_command_table`] and `split_heading_inline_row`'s call
/// site) — as subcommand stubs with `invocation_attested: true,
/// heading_attested: false`, rather than routing through
/// [`emit_subcommands`], which always sets `heading_attested: true`.
///
/// # Why the weaker attestation bit (spec §6, "A second attestation bit
/// exists now")
///
/// `heading_attested` is spec §6 rule 0's gate for exactly one question:
/// is this word safe to send as `<tool> <word> --help` probe argv? These
/// two tables belong to C daemons and daemon-control clients whose
/// "commands" are runtime control verbs, not argv subcommands in the
/// clap/cobra sense — `wpa_cli terminate`, `wpa_cli quit`,
/// `wpa_cli reconfigure` act on a *running* `wpa_supplicant` the instant
/// they are invoked, and programs in this family commonly ignore a
/// trailing `--help` and just execute the verb. Probing
/// `wpa_cli terminate --help` therefore risks tearing down a real
/// supplicant rather than printing usage — exactly the risk
/// `invocation_attested` exists to flag as unproven, per
/// [`scan_headingless_invocation_table`]'s own precedent. The rows are
/// still strong *existence* evidence (each name is checked against
/// [`command_table_token_index`] below — the same whole-token existence
/// test [`token_occurs_literally`] makes, answered from one pass over the
/// raw text rather than one rescan per candidate, since a headed command
/// table can carry on the order of a hundred rows where
/// `scan_headingless_invocation_table`'s callers see a few dozen at most
/// — see that function's own doc comment, which the earlier commit
/// history of this file already applied to `help_text`'s glued-token
/// check for exactly this reason), just weak *safety* evidence, which is
/// the whole reason the bit is split rather than reused. And today these
/// tables yield nothing at all — every row is currently dropped as
/// unattributable — so withholding probe-eligibility costs no existing
/// behaviour: nothing that used to be probed stops being probed.
/// A headed command table's rows, already split into `(name,
/// description)` pairs by [`split_bare_command_table_row`] — named
/// because the plain tuple-of-tuple spelling trips clippy's
/// `type_complexity` lint at every one of this shape's several call
/// sites ([`scan_bare_command_table`], [`emit_headed_command_table`]).
type CommandTableEntries<'a> = Vec<(&'a str, Option<String>)>;

fn emit_headed_command_table(
    entries: CommandTableEntries<'_>,
    raw_tokens: &std::collections::HashSet<&str>,
    out: &mut ParsedHelp,
) -> (usize, usize) {
    let mut seen = 0usize;
    let mut clean = 0usize;
    for (name, desc) in entries {
        seen += 1;
        // `is_command_name_shaped` is true by construction here (every
        // name was produced by `leading_command_name`, which already
        // checked it), but spec [M-10]'s lesson is to check explicitly
        // rather than trust construction — same posture
        // `scan_headingless_invocation_table` takes for the identical
        // reason.
        if !is_command_name_shaped(name) || !raw_tokens.contains(name) {
            out.saw_unattributable_content = true;
            continue;
        }
        clean += 1;
        let mut node = CommandNode::new(name, Provenance::single(Source::HelpText));
        node.summary = desc.and_then(|d| non_empty_text(&d));
        node.invocation_attested = true;
        node.heading_attested = false;
        out.try_push_subcommand(node);
    }
    (seen, clean)
}

/// One-pass tokenization of `raw` into the maximal runs of
/// [`is_command_name_shaped`]'s own character class — the whole-token
/// existence index [`emit_headed_command_table`] checks each recovered
/// name against, built once per headed-command-table block rather than
/// re-scanning `raw` (as [`token_occurs_literally`] does) once per
/// candidate name. Same split predicate as that function; the two must
/// keep agreeing on what "occurs literally" means; a set entry here is
/// exactly a `true` answer there.
fn command_table_token_index(raw: &str) -> std::collections::HashSet<&str> {
    raw.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')))
        .filter(|w| !w.is_empty())
        .collect()
}

/// Strip trailing bracketed optional-modifier groups from a command entry's
/// name token: `m[ab]` names the command `m`, `r[ab][f][u]` names `r`.
///
/// This is the docopt-style optional-group convention spec §7 Tier B
/// already names (`[optional]`), applied where a command list uses it to
/// spell a command *and* the modifier letters it accepts in one token —
/// binutils `ar` writes its whole operation table that way. Purely
/// additive: a name carrying `[` can never pass
/// [`is_command_name_shaped`] as written, so every token this changes the
/// answer for was being dropped outright.
///
/// Returns the input untouched unless the suffix is *entirely* well-formed
/// `[...]` groups, so a token that merely contains a bracket
/// (`[a]`, `[l <text> ]`) keeps failing the shape check as before rather
/// than being trimmed down to something that passes.
pub fn strip_optional_modifier_suffix(name: &str) -> &str {
    let Some(open) = name.find('[') else {
        return name;
    };
    if open == 0 {
        return name;
    }
    let mut rest = &name[open..];
    while let Some(after_open) = rest.strip_prefix('[') {
        match after_open.find(']') {
            Some(close) => rest = &after_open[close + 1..],
            None => return name,
        }
    }
    if rest.is_empty() {
        &name[..open]
    } else {
        name
    }
}

/// Leading name from a headed command table row's name field — only the
/// row's very first whitespace token can ever be the command's name,
/// never a "run" of further name-shaped tokens.
///
/// This is deliberately capped at one token, unlike
/// [`invocation_table_row_run`]'s up-to-two-token run: `apt-ftparchive`'s
/// `sources srcpath [overridefile [pathprefix]]` row names one command,
/// `sources`, with `srcpath` as its first *operand* — and `srcpath` is
/// itself [`is_command_name_shaped`], so a "run of name-shaped tokens"
/// rule would wrongly promote it to a second command or a grandchild.
/// Taking only the first token sidesteps that ambiguity entirely: it is
/// always correct for a table whose rows carry no description at all
/// (there is nothing else the token stream could mean), and it is what
/// spec's headed-command-table subsection (§7 Tier B) requires.
///
/// Strips a trailing `:` and any `[...]` optional-modifier suffix first,
/// same as [`emit_subcommands`].
fn leading_command_name(field: &str) -> Option<&str> {
    let first = field.split_whitespace().next()?;
    let name = first.trim_end_matches(':');
    let name = strip_optional_modifier_suffix(name);
    is_command_name_shaped(name).then_some(name)
}

/// Split a heading line that carries its section table's **first row on
/// the heading's own physical line** (`apt-ftparchive`'s
/// `Commands: packages binarypath [overridefile [pathprefix]]`) into the
/// heading label (without its trailing colon) and the trailing row text.
///
/// Unlike [`split_shared_heading_row`] (which recovers the same shared-
/// line shape for a *flag* row and requires a real
/// [`MIN_COLUMN_GAP_SPACES`]-wide column gap after the colon, because a
/// flag table is column-aligned), this table's rows are single-spaced —
/// `apt-ftparchive` puts exactly one space after `Commands:` — so this
/// asks only for *some* non-empty text following the colon, and leaves it
/// to the call site's own [`is_recognized_command_heading`] /
/// [`leading_command_name`] checks to decide whether that text is really
/// a command row rather than an ordinary sentence that happens to contain
/// a colon.
///
/// [`is_section_heading_line`] is still the gate that keeps this from
/// firing on a colon buried in prose (`"Note: see the manual for
/// details"`) — a real section label is short and plain-worded, a
/// sentence generally is not. Returns `None` when no colon exists, the
/// text up to it doesn't read as a heading label, or nothing follows.
fn split_heading_inline_row(line: &str) -> Option<(&str, &str)> {
    let colon = line.find(':')?;
    let label = &line[..=colon];
    if !is_section_heading_line(label) || starts_with_usage_prefix(label) {
        return None;
    }
    let suffix = line[colon + 1..].trim_start();
    if suffix.is_empty() {
        return None;
    }
    Some((&line[..colon], suffix))
}

/// Route an unrecognized bare-word block into the `choices` of whichever
/// flag it's nested under (spec §7 Tier B rule 4), or drop it if no
/// plausible owning flag exists — fabricated structure is worse than
/// missing structure either way, so an unattributable block is simply
/// discarded rather than becoming subcommands by default.
fn emit_choices(
    heading: &str,
    entries: Vec<(&str, String)>,
    out: &mut ParsedHelp,
) -> (usize, usize) {
    let mut seen = 0usize;
    let mut clean = 0usize;
    let mut candidates: Vec<String> = Vec::new();
    for (spec_text, _desc_text) in &entries {
        if candidates.len() >= MAX_RECOVERED_ENTRIES {
            break;
        }
        // Real listings sometimes alias several values on one line
        // (`"none, off       never make backups"`); each comma-separated
        // fragment is its own candidate choice.
        for fragment in spec_text.split(',') {
            let name = fragment.trim();
            if name.is_empty() {
                continue;
            }
            seen += 1;
            if !is_command_name_shaped(name) {
                out.saw_unattributable_content = true;
                continue;
            }
            clean += 1;
            candidates.push(name.to_string());
        }
    }
    if candidates.is_empty() {
        return (seen, clean);
    }
    match find_owning_flag_index(heading, &out.flags) {
        Some(idx) => {
            for name in candidates {
                let text = Text::sanitize(&name);
                if !out.flags[idx].choices.contains(&text) {
                    out.flags[idx].choices.push(text);
                }
            }
        }
        None => {
            // No flag to attribute this to at all — drop rather than
            // guess. Still counted above so confidence reflects the
            // grammar not fully understanding this content.
            out.saw_unattributable_content = true;
        }
    }
    (seen, clean)
}

/// The bare name a positional-block row or a usage-synopsis token carries,
/// with the notation stripped: `[interval]` -> `interval`,
/// `<destination>` -> `destination`, `[rustfmt_options]...` ->
/// `rustfmt_options`. `None` for anything that is not a single
/// notation-wrapped word — a row whose first column is several words is
/// prose, not an operand name, and prose promoted to structure is [M-10].
///
/// The `<...>` rule is [`extract_positionals`]'s, character for character
/// (nearest `>`, not the outermost), so a name found in a declared block and
/// the same name found in the synopsis normalize identically and can be
/// matched against each other.
fn operand_name(token: &str) -> Option<String> {
    let token = token.trim();
    if token.is_empty() || token.split_whitespace().count() != 1 {
        return None;
    }
    let cleaned = token.trim_matches(|c| c == '[' || c == ']' || c == '.');
    let name = match cleaned.strip_prefix('<') {
        Some(stripped) => stripped.get(..stripped.find('>')?)?.to_string(),
        None => cleaned.to_string(),
    };
    // Never a flag (a `positional arguments:` block that somehow contains a
    // dash-led row is not the shape this reads), and never something with
    // no word content at all (`..]`, `|`, `{`).
    if name.starts_with('-') || !name.chars().any(char::is_alphanumeric) {
        return None;
    }
    Some(name)
}

/// The `(required, variadic)` shape the usage synopsis states for the
/// operand called `name`, or `None` if the synopsis never mentions it.
///
/// The declaring block says *which* tokens are operands but not whether
/// each is optional or repeatable — argparse's `positional arguments:` rows
/// are bare names with no notation on them at all. The synopsis states
/// exactly those two bits and nothing else useful, so this reads only them,
/// with the identical expressions [`extract_positionals`] uses (`[x]` is
/// optional; a trailing `...` is variadic) rather than a second opinion
/// about the same notation.
fn usage_operand_shape(usage_lines: &[String], name: &str) -> Option<(bool, bool)> {
    for line in usage_lines {
        for token in line.split_whitespace() {
            if operand_name(token).as_deref() != Some(name) {
                continue;
            }
            let required = !token.contains('[') && !line.contains(&format!("[{token}"));
            return Some((required, token.ends_with("...")));
        }
    }
    None
}

/// Emit a framework-declared positional block's rows as real positionals
/// (see [`FrameworkProfile::positional_heading_markers`] for why a declared
/// block is a different kind of evidence from a synopsis guess).
///
/// Merges rather than appends: the synopsis scan already ran, so an operand
/// written `<file>` in the synopsis *and* listed in the block is one
/// positional that gains a description, not two. Order follows the block,
/// which is the order the framework itself prints and the order the user
/// types them in.
///
/// Returns the `(seen, clean)` pair every `emit_*` returns, so a row this
/// refuses lowers the node's confidence instead of vanishing silently.
fn emit_declared_positionals(
    entries: Vec<(&str, String)>,
    usage_lines: &[String],
    out: &mut ParsedHelp,
) -> (usize, usize) {
    let mut seen = 0usize;
    let mut clean = 0usize;
    for (spec_text, desc_text) in entries {
        if out.positionals.len() >= MAX_RECOVERED_ENTRIES {
            break;
        }
        seen += 1;
        let Some(name) = operand_name(spec_text) else {
            // A row whose first column is not one operand-shaped word.
            // Counted above, dropped here, and flagged — the same "the
            // grammar did not understand this content" signal `emit_choices`
            // raises, never a guess at what it meant.
            out.saw_unattributable_content = true;
            continue;
        };
        clean += 1;
        let description = non_empty_text(&desc_text);
        if let Some(existing) = out.positionals.iter_mut().find(|p| p.name == name) {
            // The synopsis found this one first and has no description to
            // offer; the block does. Nothing else is overwritten — the
            // synopsis is the authority on `required`/`variadic` because it
            // is the only place that notation appears.
            if existing.description.is_none() {
                existing.description = description;
            }
            continue;
        }
        let (required, variadic) = usage_operand_shape(usage_lines, &name)
            // Not in the synopsis at all (a tool whose block is fuller than
            // its usage line): a declared positional is required unless
            // something says otherwise, and the block's own row may still
            // carry the notation even when the synopsis does not.
            .unwrap_or_else(|| {
                (
                    !spec_text.contains('['),
                    spec_text.trim_end().ends_with("..."),
                )
            });
        out.positionals.push(Positional {
            name,
            required,
            variadic,
            description,
            provenance: Provenance::single(Source::HelpText),
        });
    }
    (seen, clean)
}

/// Scan a flags block starting at `lines[start]` (already confirmed to
/// look like a flag entry). Returns the index just past the block and the
/// `(spec, description)` pairs recovered.
///
/// Classifies each line as a new entry or a continuation of the previous
/// one (spec §7 Tier B rule 2) using the *shape* of the line (does it look
/// like a flag?) combined with how far it's indented relative to the
/// shallowest entry seen so far in this block — not a single shared
/// indent floor. Real `--help` output routinely mixes two entry depths in
/// one block (a short+long flag at column 2, a long-only flag at column 6,
/// because the formatter aligns long names under where the long form
/// would start after a short one), and a floor derived from whichever
/// entry happened to come first breaks whichever depth didn't come first.
/// Once a `-`-led token has been seen deep inside the continuation zone
/// (tar's `--occurrence` description references `--delete, --diff,
/// --extract` while wrapped, itself dash-led), shape alone would
/// misclassify it as a new entry — the indent check catches that: a
/// deeply-indented dash-led line is still closer to the description
/// column than to any entry's name column, so it stays a continuation.
/// Where a flags block actually begins at or after `start`, or `None` if
/// this section is not a flags block at all.
///
/// Normally that is `start` itself. The exception this exists for: a tool
/// that documents a **positional** as the first row of its options table.
/// `kill --help` opens its `Options:` section with
///
/// ```text
///  <pid> [...]            send signal to every <pid> listed
///  -q, --queue <value>    integer value to be sent with the signal
/// ```
///
/// Deciding flags-vs-bare-words from row one alone sent that whole block
/// to the bare-word path, and `kill` reported **zero flags** — measured,
/// and confirmed by deleting just that row from the help text, after which
/// the same build read 6 flags at 100% described.
///
/// Bounded deliberately, because "look harder for flags" is how
/// fabrication starts. A row is skipped only when it sits at the block's
/// own indent (deeper lines are that row's own description) and only
/// [`MAX_SKIPPED_LEADING_ROWS`] of them, and there must still be a real
/// `-`-leading row at that same indent. A bare-word command table contains
/// no such row, so it is unaffected — the discriminator stays the `-`
/// marker, which is self-identifying in a way bare words never are.
/// True if `line`'s left-hand token can open neither a flag entry nor a
/// command entry — it starts with a character that is neither a flag
/// prefix (`-`, `+`) nor the start of a name (alphanumeric).
///
/// Such a row is structurally *undecidable*: `[c]`, `[l <text> ]`,
/// `@<file>` and `<pid>` are not flag spellings and not command names, so
/// they carry no evidence about which kind of block they sit in.
fn cannot_open_an_entry(line: &str) -> bool {
    match line.trim_start().chars().next() {
        Some(c) => !(c.is_ascii_alphanumeric() || c == '-' || c == '+'),
        None => true,
    }
}

fn flags_block_start(lines: &[&str], start: usize) -> Option<usize> {
    /// How many non-flag rows may precede the first flag row.
    const MAX_SKIPPED_LEADING_ROWS: usize = 3;

    if looks_like_flag_start(lines[start]) || looks_like_bracket_flag_row(lines[start]) {
        return Some(start);
    }
    let base = leading_whitespace(lines[start]);
    let mut skipped = 0;
    for (offset, line) in lines.iter().enumerate().skip(start + 1) {
        if line.trim().is_empty() {
            return None;
        }
        let indent = leading_whitespace(line);
        if indent > base {
            continue; // the previous row's own wrapped description
        }
        if indent < base {
            return None; // dedented out of the block
        }
        if looks_like_flag_start(line) || looks_like_bracket_flag_row(line) {
            return Some(offset);
        }
        // A row whose left token could not be *either* kind of entry does
        // not decide what kind of block this is, so it does not spend the
        // budget for finding out. binutils `ar` opens its ` generic
        // modifiers:` block with eight `[c]`/`[l <text> ]`/`@<file>` rows
        // before the first `--target=BFDNAME`, and charging those eight
        // against a budget of three lost every long flag in the section.
        // The guard this budget exists for is untouched: a bare-word
        // command table's rows *are* possible command names, so they still
        // charge, and a block of them still never becomes a flags block.
        if cannot_open_an_entry(line) {
            continue;
        }
        skipped += 1;
        if skipped > MAX_SKIPPED_LEADING_ROWS {
            return None;
        }
    }
    None
}

// --- Multi-column option tables (spec §7 Tier B, [M-10]'s sibling defect,
// `corpus/lsof/4.95.0`) --------------------------------------------------
//
// Some tools (`lsof`, `unzip`, `infocmp`, `zipinfo`) pack two or three
// flag+description *pairs* onto one physical line instead of one:
//
// ```text
//   -?|-h list help          -a AND selections (OR)     -b avoid kernel blocks
// ```
//
// Reading one description column per line here doesn't just lose `-a` and
// `-b` (under-extraction) — it attributes their descriptions to `-?`
// instead (misattribution), which is fabricated documentation at full
// confidence. This section detects that shape from the block's own layout
// — column alignment recurring across several rows, never a tool name or a
// framework — and splits each row into its real per-flag pairs before
// `emit_flags` ever sees it.
//
// The detection mechanism (cells → fields → recurring offsets) mirrors
// `xtask/src/misattribution.rs`'s `DefinitionIndex`, which was built and
// measured against this exact bug first and already carries the hardening
// against the false-positive classes below.
//
// The vocabulary functions — [`is_flag_shaped`], [`is_flag_char`],
// [`first_word`], [`cells`], [`MIN_COLUMN_RECURRENCE`], and
// [`is_value_placeholder_only`] — are `pub` and re-exported from
// `help_text::mod` (same pattern as [`pick_stream`](super::pick_stream))
// precisely so `xtask/src/misattribution.rs` imports these instead of
// restating them: that restatement was tried once, for `pick_stream`, and
// silently drifted past the openssl stream fix (spec §13.1c's K2 table),
// producing 200 of 656 fleet-wide fabrications from an oracle that no
// longer agreed with the parser it was auditing. `is_flag_shaped`/`cells`/
// `is_value_placeholder_only` are exactly the same hazard: if the splitter
// here and the misattribution oracle disagree on what counts as a
// flag-shaped token or a bare placeholder, the oracle stops measuring this
// parser and starts measuring its own, different guess at the same
// question.
//
// [`fields_in_line`] itself is **not** shared, and this is a real,
// load-bearing behavioral difference, not an oversight: `misattribution`'s
// copy is an *advisory* metric a human reads, so it can afford to
// under-suppress (its own doc comment names `arptables`'s `-A chain` as a
// known, accepted residual false positive). A splitter's mistakes are not
// advisory — they fabricate a flag that was never in the tool's own text —
// so the copy below is strictly more conservative: it never starts a new
// field on top of one that hasn't yet earned real description text of its
// own (see its doc comment), which is exactly what keeps `-A chain`/`-p
// NUM`-shaped rows (a value placeholder standing in for real trailing text,
// lower-case so `is_value_placeholder_only` can't recognize it as one) from
// being read as a second, independent flag. If this splitter's fold rule
// changes, `misattribution::fields_in_line` will not — check both, by hand,
// on a real change here.

/// Minimum number of distinct entry lines a secondary column offset must
/// recur at before a block is trusted as genuinely multi-column. Same
/// figure and same justification as
/// `xtask::misattribution::MIN_COLUMN_RECURRENCE`: real column bleed
/// (`lsof`'s two hidden columns) recurs 9 times over its ~10-line options
/// block; the worst accidental coincidence measured in this project's own
/// real-tool sample (`tar`'s `-T` cross-reference) recurs twice, at two
/// different offsets. `3` sits strictly between the two.
pub const MIN_COLUMN_RECURRENCE: usize = 3;

/// Minimum number of entry rows whose *second* spelling cell begins at the
/// same character offset before [`scan_flags_block`] reads that cell as an
/// aligned column of **alternate spellings** rather than as the row's
/// description (see [`spelling_run`]).
///
/// Two, where [`MIN_COLUMN_RECURRENCE`] is three, because the two
/// constants guard different questions and one of them is much harder to
/// trip by accident. `MIN_COLUMN_RECURRENCE` asks "is a second
/// flag+description pair hiding in this row?", where the rival reading —
/// ordinary prose that happens to mention a flag — is common and only a
/// count can separate them. This one asks "is this cell *nothing but*
/// another spelling of the option already named?", and the shape test
/// alone ([`is_spelling_only_cell`]) already excludes prose: every cell in
/// the run must be a flag spelling and, at most, a bare value placeholder,
/// with no words of its own. Recurrence here is only ruling out
/// *coincidental* alignment, so two rows is enough.
///
/// Both halves of that were measured over the 2,301 frozen captures in
/// `audit/queue-captures/` (2026-08-22):
///
/// - Three would exclude the shape's own reference case. `jdeprscan
///   --help` writes exactly two such rows — `  -l    --list` and
///   `  -v    --verbose` — and both long spellings were lost entirely
///   before this rule existed.
/// - The one measured false positive is excluded by *alignment*, not by
///   count, so lowering the count does not readmit it: `lto-dump --help`
///   prints a default-value column (`--param=prefetch-minimum-stride=
///   <TAB> -1`) whose `-1` would be read as a short spelling, but its
///   three rows have long names of three different lengths, so the `-1`
///   lands at three different offsets and no offset recurs even once.
///   [`block_has_aligned_spelling_column`]'s second arm — this same count
///   of *value-paired* rows ([`cells_name_the_same_value`]) — does not
///   readmit it either: `-1` names no value, so no row of that block is
///   value-paired.
const MIN_SPELLING_COLUMN_RECURRENCE: usize = 2;

/// True if `token` is shaped like a flag spelling: `-x`, `--word`, `+x`, or
/// `+|-x` — lsof spells several of its own flags with the `+` prefix
/// (`+d`, `+m`). Deliberately permissive about the character right after a
/// short prefix (`lsof`'s own `-?`).
pub fn is_flag_shaped(token: &str) -> bool {
    if let Some(rest) = token.strip_prefix("+|-") {
        return rest.chars().next().is_some_and(is_flag_char);
    }
    if let Some(rest) = token.strip_prefix("--") {
        return rest.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
    }
    if let Some(rest) = token.strip_prefix('+') {
        return rest.chars().next().is_some_and(is_flag_char);
    }
    if let Some(rest) = token.strip_prefix('-') {
        return rest.chars().next().is_some_and(is_flag_char);
    }
    false
}

/// The character class allowed immediately after a short flag's leading
/// `-`/`+`: alphanumerics cover the overwhelming majority, plus the small
/// punctuation set measured on real tools (`lsof -?`).
fn is_flag_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '?' | '#' | '@')
}

/// First whitespace-delimited word of `s`, or `""` for an all-whitespace
/// string.
pub fn first_word(s: &str) -> &str {
    s.split_whitespace().next().unwrap_or("")
}

/// Split `line` into cells at a column gap — a run of two or more spaces,
/// **or any tab** — character-indexed, never byte-indexed (AGENTS.md's rule
/// against slicing tool output at a raw byte offset applies to column math
/// here just as much as to parsing: a wide character earlier in a real
/// `--help` line would otherwise desync every offset after it). Returns
/// `(char offset, cell text)` pairs, trailing whitespace trimmed off each
/// cell.
///
/// A single tab is a boundary on its own — `debconf --help`'s real table is
/// tab-separated (`-o,  --owner=package\t\tSet the package...`), and only
/// requiring 2+ spaces would read the tab-glued alias-plus-description as
/// one cell.
pub fn cells(line: &str) -> Vec<(usize, String)> {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let is_gap_start = |i: usize| -> bool {
        chars[i] == '\t' || (chars[i] == ' ' && i + 1 < n && chars[i + 1] == ' ')
    };
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < n {
        while i < n && (chars[i] == ' ' || chars[i] == '\t') {
            i += 1;
        }
        if i >= n {
            break;
        }
        let start = i;
        let mut j = i;
        while j < n {
            if is_gap_start(j) {
                break;
            }
            j += 1;
        }
        let content: String = chars[start..j].iter().collect();
        out.push((start, content.trim_end().to_string()));
        i = j;
    }
    out
}

/// True if `s` is nothing but a single value-placeholder token — bracket-
/// wrapped (`<dir>`, `[NUMBER]`), fully upper-case (`NUM`, `FILE`), or an
/// upper-case name with a bracketed decoration (`BLOCKSIZE[bskK...]`) —
/// with no other words. Deliberately narrow: a lower-case placeholder
/// (`arptables`'s `-A chain`) is not recognized here, because a real
/// English word is not reliably distinguishable from real prose from one
/// cell alone. [`fields_in_line`]'s own fold-while-bare rule is what
/// actually protects that case (see its doc comment) — this check only
/// needs to catch the *unambiguous* placeholders, not every one.
pub fn is_value_placeholder_only(s: &str) -> bool {
    let mut words = s.split_whitespace();
    let Some(word) = words.next() else {
        return true;
    };
    if words.next().is_some() {
        return false;
    }
    let bracketed = matches!(
        (word.chars().next(), word.chars().last()),
        (Some('<'), Some('>')) | (Some('['), Some(']')) | (Some('{'), Some('}'))
    );
    let all_upper = word.chars().any(char::is_alphabetic)
        && word.chars().all(|c| !c.is_alphabetic() || c.is_uppercase());
    let upper_name_with_decoration = word.find(['[', '<', '{']).is_some_and(|i| {
        let name = &word[..i];
        !name.is_empty() && name.chars().all(|c| c.is_ascii_uppercase())
    });
    bracketed || all_upper || upper_name_with_decoration
}

/// One column entry recovered from a multi-column row.
struct Field {
    /// Character offset of the field's *first* flag-shaped cell — the
    /// position [`block_is_multi_column`] buckets recurrence counts by.
    /// Never updated once the field is created, even while later cells
    /// keep folding into it (see [`fields_in_line`]): it names where this
    /// logical column *starts*, not wherever it happens to still be
    /// absorbing text.
    offset: usize,
    /// Every flag-shaped spelling folded into this field — usually one,
    /// more when a row spells one option's short and long forms as
    /// adjacent cells sharing a single description (`nano --help`'s `-A
    /// --smarthome`), or when a value placeholder that looked like real
    /// text kept the field open (see [`fields_in_line`]).
    tokens: Vec<String>,
    /// Accumulated non-flag-shaped text following this field's token(s).
    /// Empty (or a bare value placeholder) means "not yet described" —
    /// see [`Field::is_bare`].
    trailing: String,
}

impl Field {
    /// True when this field carries no real descriptive text of its own
    /// yet. Never true of a genuine secondary column in an N-column table
    /// (every real column pairs a flag with a description, by the shape
    /// the bug report itself defines: "flag+description pairs"), so this
    /// is the discriminator [`fields_in_line`] uses to decide whether the
    /// *next* flag-shaped cell is a new column or just another spelling of
    /// the option still open.
    fn is_bare(&self) -> bool {
        let trailing = self.trailing.trim();
        trailing.is_empty() || is_value_placeholder_only(trailing)
    }
}

/// Group `line`'s cells (see [`cells`]) into [`Field`]s: one per *logical*
/// column entry, not one per raw cell.
///
/// **The fold-while-bare rule, and why it's stricter than
/// `misattribution::fields_in_line`.** Whenever the currently open field is
/// still bare (no real description attached yet), any further flag-shaped
/// cell is folded into it as another spelling of the *same* option —
/// regardless of whether that cell's own trailing text looks real. This is
/// what a genuine alias pair looks like (`nano`'s `-A  --smarthome  <shared
/// description>`, both cells bare until the real prose arrives), but it is
/// also what protects against the residual false-positive class the
/// misattribution detector documents and accepts rather than fixes:
/// `arptables --help`'s `--append  -A chain<TAB><TAB>Append to chain`. Read
/// cell-by-cell, `-A chain` has "real" trailing text (`chain`) that isn't a
/// recognized placeholder (lower-case, so [`is_value_placeholder_only`]
/// doesn't catch it) — but `--append`, the field already open when `-A`
/// arrives, is itself still bare, so this rule folds `-A` into it anyway,
/// and `chain` becomes an extension of the *shared* trailing text rather
/// than proof of a second, independent flag. A genuine N-column table never
/// needs this fold at all: its primary column always carries its own real
/// description (`lsof`'s `-?|-h list help  ...`), so the field it opens is
/// never bare when the next flag-shaped cell arrives, and a fresh field
/// starts exactly as it would without this rule.
fn fields_in_line(line: &str) -> Vec<Field> {
    let mut fields: Vec<Field> = Vec::new();
    for (offset, content) in cells(line) {
        let token = first_word(&content);
        if !is_flag_shaped(token) {
            // Plain prose: belongs to whichever field is currently open. A
            // line that starts with prose before any flag-shaped cell has
            // no open field yet, so that content is simply dropped — it
            // isn't part of any flag's definition.
            if let Some(last) = fields.last_mut() {
                if !last.trailing.is_empty() {
                    last.trailing.push(' ');
                }
                last.trailing.push_str(&content);
            }
            continue;
        }
        let own_trailing = content
            .strip_prefix(token)
            .unwrap_or(&content)
            .trim()
            .to_string();
        if let Some(last) = fields.last_mut() {
            if last.is_bare() {
                last.tokens.push(token.to_string());
                if last.trailing.trim().is_empty() {
                    last.trailing = own_trailing;
                } else if !own_trailing.is_empty() {
                    last.trailing.push(' ');
                    last.trailing.push_str(&own_trailing);
                }
                continue;
            }
        }
        fields.push(Field {
            offset,
            tokens: vec![token.to_string()],
            trailing: own_trailing,
        });
    }
    fields
}

/// True if `entry_lines` (a flags block's raw entry rows, one string per
/// physical line — never continuation lines, which carry no flag-shaped
/// cells of their own to align) shows real column alignment: a secondary
/// field recurring at the same character offset across at least
/// [`MIN_COLUMN_RECURRENCE`] rows. Mirrors
/// `misattribution::build_definition_index`'s recurrence check, scoped to
/// one block instead of a whole tool's raw text — the same signal, applied
/// where it can actually change how the block is parsed rather than only
/// audit it after the fact. Only the *secondary* fields (skipping each
/// row's own first/primary one) count, for the same reason
/// `misattribution` excludes a row's own leftmost field: a row's primary
/// entry legitimately cross-references another, real, single-column flag
/// in its own prose (`du --help`'s `-H` mentioning `-D`), and that must
/// never itself look like evidence of a second table column.
fn block_is_multi_column(entry_lines: &[&str]) -> bool {
    let mut offset_counts: std::collections::HashMap<usize, usize> =
        std::collections::HashMap::new();
    for line in entry_lines {
        let fields = fields_in_line(line);
        if fields.len() < 2 {
            continue;
        }
        for field in fields.iter().skip(1) {
            if field.is_bare() {
                continue;
            }
            *offset_counts.entry(field.offset).or_insert(0) += 1;
        }
    }
    offset_counts
        .values()
        .any(|&count| count >= MIN_COLUMN_RECURRENCE)
}

// --- packed flag rows: several bare entries share one physical line, ----
// --- with no per-entry description anywhere in the block ----------------
//
// GNU `find --help` writes its "Tests"/"Actions"/"Normal options" tables as
// several `-flag [ARG]` entries packed onto one physical line with single
// spaces, never one flag per line and never a description column at all:
//
// ```text
// Tests (N can be +N or -N or N):
//       -amin N -anewer FILE -atime N -cmin N -cnewer FILE -context CONTEXT
//       ...
//       -wholename PATTERN -size N[bcwkMG] -true -type [bcdpflsD] -uid N
// ```
//
// Neither [`block_is_multi_column`] (built for a block where every packed
// cell carries its *own* real description, e.g. `lsof`'s options table) nor
// the ordinary single-column path (`find_description_gap` + one flag per
// physical line) is the right tool: there is no description anywhere here
// to find a gap before, and reading the *whole* line as one flag's spec —
// what the single-column path falls back to when no gap is found — is what
// produced the corruption this shape exists to fix. `find_placeholder_
// boundary_gap` (a `]`/`>` followed by exactly one space, meant to recover
// a description a fixed-width table's long spelling overran) misreads
// `-size N[bcwkMG]`'s own bracketed unit suffix as exactly that shape and
// hands `parse_flag_spec` the front half of the *next* entries
// (`-true -type [bcdpflsD] -uid N`) as `-wholename`'s fabricated
// "description" — a flag invented text the tool never wrote as belonging
// to it. This block never reaches `find_description_gap` at all: see
// [`block_is_packed_flag_rows`]'s call site in [`scan_flags_block`].

/// True if `token` opens a new packed entry: a dash immediately followed
/// by an ASCII letter. Narrower than [`looks_like_flag_start`] (which also
/// accepts a bare `-` and a `{...}` alternation) because this is asked of
/// one whitespace-delimited token, many times per line, rather than of a
/// whole physical line once — a bare trailing `-` or a brace group never
/// opens a second entry mid-line in this shape, and admitting either here
/// would risk splitting a real operand token in two.
fn token_opens_packed_entry(token: &str) -> bool {
    let mut chars = token.chars();
    matches!(chars.next(), Some('-')) && matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
}

/// True if `token`, found between two packed entries, reads as the
/// previous entry's own operand rather than as prose — which would mean
/// the line is not this shape at all (see [`try_split_packed_row`]).
/// [`is_value_placeholder_only`] already recognizes a value placeholder
/// cell (`FILE`, `[bcdpflsD]`, an upper-case name with bracket decoration
/// like `N[bcwkMG]`); the three bare tokens `-exec`/`-execdir`/`-ok`/
/// `-okdir`'s own command-terminator convention writes with no other
/// decoration (`;`, `+`, `{}`) are added on top because none of them is
/// upper-case or bracket-wrapped as a whole token, and no other function in
/// this module already names them.
fn token_is_packed_operand(token: &str) -> bool {
    is_value_placeholder_only(token) || matches!(token, ";" | "+" | "{}")
}

/// Split one physical line into the packed `(spelling, operand)` entries
/// it carries — never a description, because this shape has none. Returns
/// `None` the moment a token is neither a new entry's own opening dash nor
/// the previous entry's operand: that means real prose is present and the
/// line is not this shape, so the caller must fall back to the ordinary
/// single-column reading rather than guess. A line with only one entry and
/// nothing following it (an ordinary lone boolean flag) still returns
/// `Some` with one entry — [`block_is_packed_flag_rows`] is what requires
/// at least one line in the block to carry two or more before any of them
/// is read this way, so a block that happens to wrap one flag onto its own
/// line is not refused outright just for that line.
fn try_split_packed_row(line: &str) -> Option<Vec<(String, String)>> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if !tokens.first().is_some_and(|t| token_opens_packed_entry(t)) {
        return None;
    }
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if !token_opens_packed_entry(tokens[i]) {
            return None;
        }
        let spelling = tokens[i].to_string();
        i += 1;
        let mut operand = String::new();
        while i < tokens.len() && !token_opens_packed_entry(tokens[i]) {
            if !token_is_packed_operand(tokens[i]) {
                return None;
            }
            if !operand.is_empty() {
                operand.push(' ');
            }
            operand.push_str(tokens[i]);
            i += 1;
        }
        entries.push((spelling, operand));
    }
    Some(entries)
}

/// True when `trimmed` is a continuation of a BNF alternation group whose
/// own line leads with the operator instead of a fresh flag spelling —
/// `dcb --help`'s `OPTIONS := [ ... ]` production wraps as
///
/// ```text
///        OPTIONS := [ -V | --Version | -i | --iec | -j | --json
///                   | -N | --Numeric | -p | --pretty
///                   | -s | --statistics | -v | --verbose]
/// ```
///
/// where the second and third physical lines open on the `|` that
/// separates them from the line above, not on a flag — every sibling in
/// this family (`ip`, `vdpa`, `bridge`, `rdma`, `devlink`) instead repeats
/// a flag spelling at the start of every wrapped line, which
/// [`looks_like_flag_start`] already recognizes as its own entry. Without
/// this, a `|`-led line is neither an entry (fails `looks_like_flag_start`,
/// which never accepts a leading `|`) nor read as a continuation of useful
/// shape — [`scan_flags_block`]'s continuation branch would still glue its
/// raw text onto the previous entry's `description`, but a BNF grammar row
/// carries no description to append to, so the text would sit there
/// unparsed rather than becoming the extra flags it names.
///
/// Requires flag-shaped content after the leading `|` and whitespace, not
/// just the `|` itself, so an unrelated line that happens to start a
/// physical line with `|` for some other reason (a table border, a
/// "pipe-or" example) is never swept in on the character alone.
fn looks_like_bnf_continuation_row(trimmed: &str) -> bool {
    trimmed
        .strip_prefix('|')
        .is_some_and(|rest| looks_like_flag_start(rest.trim_start()))
}

/// True when `token`, trimmed, is a short flag spelling and nothing else —
/// no abbreviation bracket, no value, no alias of its own
/// ([`is_bare_flag_spelling`]) — and specifically the *short* half of that
/// shape (a long spelling fails the "one character" arm of that predicate
/// on its own).
fn is_unadorned_short(token: &str) -> bool {
    let t = token.trim();
    !t.starts_with("--") && is_bare_flag_spelling(t)
}

/// The long-spelling counterpart to [`is_unadorned_short`]: `--name` and
/// nothing else.
fn is_unadorned_long(token: &str) -> bool {
    let t = token.trim();
    t.starts_with("--") && is_bare_flag_spelling(t)
}

/// The opening delimiter that would match a given closing one.
fn matching_open(close: char) -> char {
    match close {
        '}' => '{',
        ')' => '(',
        _ => '[',
    }
}

/// Trims a trailing closing bracket (`}`/`)`/`]`) that has no opening
/// counterpart earlier in the *same* segment — the residue of an
/// *enclosing* BNF alternation group's own closer landing on the row's
/// last alternative once the row has been split on `|`: `vdpa`'s
/// `-p[retty] }` (space-separated from its own abbreviation bracket) and
/// `dcb`'s `--verbose]` (glued straight onto the long spelling, closing the
/// `[` that `split_shared_heading_row` already consumed when it recognized
/// the block's opening line) are the same shape at two different
/// distances.
///
/// The no-matching-opener test is what tells this apart from a bracket
/// that really does belong to the segment: `-b[atch] [filename]`'s own
/// trailing `]` closes a `[` two tokens earlier in the *same* segment, so
/// it is left untouched, while `-c[olor]`'s own abbreviation bracket
/// (already closed by [`grammar::strip_short_abbrev_suffix`] before this
/// ever runs) never reaches this function without a group-closer glued
/// past it in the first place.
fn strip_trailing_stray_bracket(segment: &str) -> &str {
    let trimmed = segment.trim_end();
    let Some(last) = trimmed.chars().next_back() else {
        return trimmed;
    };
    if !matches!(last, '}' | ')' | ']') {
        return trimmed;
    }
    let before = &trimmed[..trimmed.len() - last.len_utf8()];
    if before.contains(matching_open(last)) {
        trimmed
    } else {
        before.trim_end()
    }
}

/// Read a flag-table row as a BNF alternation group listing several
/// distinct flags on one physical line — the shape iproute2's shared help
/// emitter writes for its `OPTIONS := { ... }` production, once
/// [`split_shared_heading_row`] has already separated the row from the
/// heading it opened on (`ip`, `vdpa`, `bridge`, `rdma`, `devlink`; `dcb`
/// opens its group with `[` rather than `{`, which that function already
/// treats the same way — see its own doc comment). Returns one `(spec,
/// description)` pair per recovered alternative, description always empty
/// — a BNF grammar row carries no prose at all, only spellings — or `None`
/// when the row does not conform closely enough to split without risking a
/// fabrication.
///
/// # Two shapes inside one row, told apart by a pairing rule
///
/// `ip`'s convention spells a flag's long form as a bracketed suffix
/// glued onto the same token (`-V[ersion]`), so every top-level
/// `|`-segment is already a complete, self-contained flag. `dcb` never
/// abbreviates this way and instead spells the short and long forms as two
/// adjacent alternatives (`-V | --Version`) — the ordinary alias-list
/// convention [`parse_flag_spec`] already reads via a comma, just spelled
/// with `|` here. A bare short immediately followed by a bare long —
/// neither carrying a bracket, value, or anything else of its own
/// ([`is_unadorned_short`]/[`is_unadorned_long`]) — is folded back into one
/// alias-list segment before anything else runs, so `dcb`'s six pairs
/// become six flags rather than twelve one-spelling fragments.
///
/// # The false-positive guard: every segment must fully, cleanly consume
///
/// A top-level `|`-split alone is not sufficient evidence: `sg_sanitize`'s
/// `--count=OC|-c OC  OC is overwrite count` also splits into two dash-led
/// segments, and the two are one flag with an alias and a shared value, not
/// two flags — kept together by [`parse_flag_spec`]'s own alias-
/// continuation grammar precisely so a naive splitter doesn't take them
/// apart (see `alias_follows`'s doc comment). Checked per segment after the
/// pairing step above:
///
/// 1. [`looks_like_flag_start`] — a segment that fails even to look like a
///    flag row on its own is never something worth guessing at.
/// 2. `parse_flag_spec` must report `fully_consumed`: nothing left over
///    once its own spelling/value grammar has run. `sg_sanitize`'s second
///    segment leaves `"is overwrite count"` unconsumed and fails this.
/// 3. A recovered value must (a) not itself start with `-` — a value is
///    never another flag's spelling, which is what `devlink`'s un-piped
///    `-v[erbose] -s[tatistics] -[he]x` tail would otherwise fabricate onto
///    `-v` — and (b) only exist where the segment actually has a
///    whitespace boundary before it: `ip`'s own un-bracketed multi-letter
///    abbreviations (`-iec`, `-ts[hort]`) glue a bare word directly onto
///    the short letter with nothing between them, and reading that glued
///    run as a value is exactly the fabrication
///    [`grammar::strip_short_abbrev_suffix`]'s own doc comment already
///    warns about one layer in.
/// 4. An *unpaired* segment (one raw `|`-fragment, not the short/long join
///    above) may carry only one flag-shaped word. Without this, `rdma`'s
///    `-p[retty] -r[aw]` — one `|`-segment, two flags run together by a
///    bare space, the "missing separator" shape this reader deliberately
///    stays out of — would not fail condition 2 at all:
///    [`parse_flag_spec`]'s alias loop silently *consumes* `-r[aw]` as a
///    discarded extra spelling rather than leaving it as leftover text, so
///    `fully_consumed` comes back `true` with no value and no visible sign
///    `-r` was ever there. Counting flag-shaped words is what catches a
///    swallow that leaves no other trace.
///
/// Any segment failing any of the three refuses the **whole row** — this
/// reader never partially splits a line, which would leave some of a row's
/// flags recovered as separate entries and others still glued into
/// whichever one happened to parse, with nothing distinguishing the two
/// outcomes for a reviewer. A row this conservative about is one where
/// `ip`'s own multi-letter-abbreviation defect (`corpus/ip/6.1.0/meta.toml`)
/// already stops the whole line from benefiting — reported, not silently
/// patched over.
fn split_bnf_alternation_row(line: &str) -> Option<Vec<(String, String)>> {
    let trimmed = line.trim();
    let raw_segments = split_alternatives(trimmed);
    if raw_segments.len() < 2 {
        return None;
    }
    // Clean each segment's own trailing stray bracket *before* the pairing
    // decision below, not after: `dcb`'s last pair (`-v`, `--verbose]`)
    // only reads as a bare short/long pair once `--verbose]`'s glued group
    // closer is gone — done any later, `-v` and `--verbose` would each end
    // up a separate one-spelling flag instead of one flag with both.
    let segments: Vec<&str> = raw_segments
        .iter()
        .map(|s| strip_trailing_stray_bracket(s))
        .collect();

    // `(text, was_paired)` — `was_paired` marks a group built by joining a
    // bare short and its bare long below, which legitimately carries two
    // flag-shaped words on purpose; every other group is one raw `|`-
    // segment and must carry exactly one.
    let mut groups: Vec<(String, bool)> = Vec::new();
    let mut idx = 0;
    while idx < segments.len() {
        let seg = segments[idx];
        if is_unadorned_short(seg)
            && segments
                .get(idx + 1)
                .is_some_and(|next| is_unadorned_long(next))
        {
            groups.push((format!("{} | {}", seg, segments[idx + 1]), true));
            idx += 2;
        } else {
            groups.push((seg.to_string(), false));
            idx += 1;
        }
    }
    if groups.len() < 2 {
        return None;
    }

    let mut entries = Vec::with_capacity(groups.len());
    for (candidate, was_paired) in &groups {
        if !looks_like_flag_start(candidate) {
            return None;
        }
        // A missing-separator hazard one layer finer than the top-level
        // `|`-split catches: `rdma`'s `-p[retty] -r[aw]` is a *single*
        // `|`-segment (no pipe inside it at all) whose two flags are
        // instead run together with a bare space — the exact shape
        // `parse_flag_spec`'s alias loop silently swallows, discarding
        // `-r[aw]` with no value, no leftover text, and therefore no
        // `fully_consumed` failure to catch it on. Refusing whenever an
        // *unpaired* group carries more than one flag-shaped word is what
        // stops that swallow from reaching this reader at all — the row
        // is refused rather than quietly losing a real flag.
        if !*was_paired
            && candidate
                .split_whitespace()
                .filter(|w| looks_like_flag_start(w))
                .count()
                > 1
        {
            return None;
        }
        let spec = parse_flag_spec(candidate);
        if !spec.fully_consumed {
            return None;
        }
        if let Some(value) = &spec.value_name {
            if value.starts_with('-') || !candidate.contains(char::is_whitespace) {
                return None;
            }
        }
        entries.push((candidate.clone(), String::new()));
    }
    Some(entries)
}

/// True when every entry row in a flags block splits cleanly via
/// [`try_split_packed_row`] (no row anywhere carries real prose this shape
/// would otherwise fabricate a boundary inside) **and** at least one row
/// actually packs two or more entries — proof this is genuinely the dense
/// shape and not just an ordinary one-flag-per-line block that happens to
/// have no description. Consulted only after [`block_is_multi_column`] and
/// the aligned-spelling check have both already declined the block, the
/// same subordination [`block_is_multi_column`]'s own doc comment
/// describes for that pair — never in front of either, since both are
/// already-working shapes this one must never compete with.
fn block_is_packed_flag_rows(entry_lines: &[&str]) -> bool {
    let mut any_multi = false;
    for line in entry_lines {
        match try_split_packed_row(line) {
            Some(entries) => any_multi |= entries.len() >= 2,
            None => return false,
        }
    }
    any_multi
}

/// One raw row within a flags block, before it's split into `(spec,
/// description)` — kept as a whole `&str` because the *splitting* decision
/// (one column vs. several — see [`block_is_multi_column`]) can't be made
/// per-line; it needs every entry row in the block at once.
enum FlagsBlockRow<'a> {
    /// Looks like the start of a new flag entry.
    Entry(&'a str),
    /// A continuation of the previous entry's description (`trim_end`ed
    /// text only — the row's own indentation has already done its job by
    /// this point).
    Continuation(&'a str),
}

fn scan_flags_block<'a>(
    lines: &[&'a str],
    start: usize,
    heading_is_bnf: bool,
) -> (usize, Vec<(String, String)>, bool) {
    const ENTRY_INDENT_TOLERANCE: usize = 10;
    let mut i = start;
    let mut rows: Vec<FlagsBlockRow<'a>> = Vec::new();
    let mut min_entry_indent: Option<usize> = None;
    let mut current_entry_line: Option<&'a str> = None;

    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        let indent = leading_whitespace(line);
        let trimmed = line.trim_start();

        let is_entry_start = (looks_like_flag_start(trimmed)
            || looks_like_bracket_flag_row(trimmed)
            // Gated the same way `split_bnf_alternation_row` is, and for
            // the identical reason: a leading `|` introduces a BNF
            // alternation's own wrapped continuation (`dcb`'s `| -N |
            // --Numeric | ...`) in exactly one shape this fleet has, but a
            // bare `|`-led line is also how `sg_write_x` wraps a single
            // alias onto its own line (`--generation=EOG,NOG` / `    |-G
            // EOG,NOG    and New ORWgeneration field to NOG`) — there the
            // leading `|` is a continuation *marker*, not grammar, and
            // reading it as a fresh entry split a real flag's own
            // continuation into a second, fabricated `-G`. Only a heading
            // known to be `:=`-shaped may read this leading-`|` shape as
            // anything other than the ordinary ("more indented, glue it
            // onto the entry above") continuation rule already handles.
            || (heading_is_bnf && looks_like_bnf_continuation_row(trimmed)))
            && min_entry_indent.is_none_or(|min| indent <= min + ENTRY_INDENT_TOLERANCE);

        if is_entry_start {
            rows.push(FlagsBlockRow::Entry(line));
            min_entry_indent = Some(min_entry_indent.map_or(indent, |m| m.min(indent)));
            current_entry_line = Some(line);
            i += 1;
            continue;
        }

        let is_continuation = !rows.is_empty() && min_entry_indent.is_some_and(|m| indent > m);
        if is_continuation {
            let entry_has_own_description =
                current_entry_line.is_some_and(entry_row_carries_own_description);
            if entry_has_own_description && nested_entry_table_starts_at(lines, i, indent) {
                break;
            }
            rows.push(FlagsBlockRow::Continuation(trimmed.trim_end()));
            i += 1;
            continue;
        }

        // Neither a new entry nor a continuation of one: this line dedents
        // back to (or below) the block's own entries without looking like
        // a flag — a genuinely new heading. Stop here.
        break;
    }

    // Whether this block packs more than one flag+description pair per
    // physical line (spec §7 Tier B, `lsof`'s options table) is a property
    // of the *block*, decided once from every entry row together — never
    // per line, which would let a block's ordinary single-column rows
    // (`lsof`'s own `-i select IPv[46] files`) get split as if a bare
    // second word were a second flag.
    let entry_lines: Vec<&str> = rows
        .iter()
        .filter_map(|r| match r {
            FlagsBlockRow::Entry(l) => Some(*l),
            FlagsBlockRow::Continuation(_) => None,
        })
        .collect();
    let multi_column = block_is_multi_column(&entry_lines);
    // Independent of, and subordinate to, `multi_column`: a block can pack
    // several flag+description pairs per line (that decision) *or* spell
    // one option across aligned spelling columns (this one). Only a block
    // the first decision did not claim consults the second.
    let aligned_spellings = !multi_column && block_has_aligned_spelling_column(&entry_lines);
    // Subordinate to both of the above, for the reason each of their own
    // doc comments gives for the other: a block already claimed by either
    // shape never reaches the packed-row reader, and the packed reader
    // itself refuses (see `block_is_packed_flag_rows`) rather than compete
    // for a row a working splitter already owns.
    let packed = !multi_column && !aligned_spellings && block_is_packed_flag_rows(&entry_lines);

    let mut entries: Vec<(String, String)> = Vec::new();
    for row in rows {
        match row {
            FlagsBlockRow::Entry(line) => {
                // A docopt bracket-group row (LVM's `[ -d|--debug ]`) is
                // one flag and nothing else — no description column
                // exists on the row at all. Neither the multi-column
                // splitter nor the aligned-spelling-column splitter below
                // is the right tool for a shape with no second column to
                // find, so this row is read directly: the content inside
                // the brackets is exactly what `split_single_column_entry`
                // would otherwise try to recover by looking for a
                // whitespace gap that isn't there.
                if let Some(content) = bracket_flag_row_content(line.trim()) {
                    entries.push((content.to_string(), String::new()));
                    continue;
                }
                // The packed shape (`find --help`'s "Tests"/"Actions"
                // tables — see the block comment above
                // `block_is_packed_flag_rows`): several bare entries per
                // line, never a description. `block_is_packed_flag_rows`
                // already proved every entry line in this block splits
                // cleanly, so this can only be `None` for a line that
                // reached here despite that (never for `find` itself) —
                // and even then it degrades to the single-column path
                // below rather than panicking or dropping the row.
                if packed {
                    if let Some(subs) = try_split_packed_row(line) {
                        entries.extend(subs);
                        continue;
                    }
                }
                // A BNF alternation group naming several distinct flags on
                // one physical line (iproute2's shared `OPTIONS := { ... }`
                // emitter) — see `split_bnf_alternation_row`'s own doc
                // comment for the shape and the false-positive guard that
                // keeps this from ever taking `sg_sanitize`'s alias-plus-
                // value `|` apart the same way.
                //
                // **Gated on `heading_is_bnf`.** A bare `|` alone is not
                // sufficient evidence, full stop — `btrfsck`'s own
                // `-E|--subvol-extents <subvolid>` (a normal short/long
                // pair, `|` as its plain alias separator, no grammar
                // anywhere near it) and the whole `lv*`/`vg*`/`pv*` family's
                // `-A|--autobackup y|n` convention both satisfy every
                // per-segment check below on their own — each half
                // independently `fully_consumed` — and only the *document*
                // says these are one flag, not the BNF shape this reader
                // targets. Measured full-fleet before this gate existed: 8
                // tools outside the iproute2 family (`btrfsck`, `dpkg`,
                // `mkfs.btrfs`, `pvchange`, `sg_get_config`, `sg_write_x`,
                // `update-java-alternatives`, `vgchange`) had a real,
                // previously-correct short/long pair torn into two
                // half-flags. `heading_is_bnf` is `true` only when *this*
                // block's own heading was produced by
                // `split_shared_heading_row`'s `:=`-operator clause — the
                // one piece of evidence that actually distinguishes the two
                // shapes — never inherited from context or guessed at from
                // the row's own text.
                //
                // Never both at once in practice: `packed` requires *every*
                // entry line in the block to split via `try_split_packed_row`,
                // which treats a bare `|` token as neither a new packed
                // entry (`token_opens_packed_entry` requires a dash) nor a
                // valid operand (`token_is_packed_operand`) — so a real BNF
                // alternation row (which always contains a `|`) fails
                // `try_split_packed_row` on sight and `packed` comes back
                // `false` for the whole block. The `if packed` branch above
                // is what would run first if a block ever did satisfy both,
                // but that block does not occur in the measured fleet.
                //
                // Tried after the bracket-row and packed-row cases above
                // (never competes with LVM's shape: that row's whole content
                // sits inside one outer bracket, so it has no *top-level*
                // `|` for this to find) and before the ordinary column
                // splitters below, which would otherwise read the row's
                // first flag only and bury every alternative after it in
                // that flag's own `description`.
                if heading_is_bnf {
                    if let Some(alternatives) = split_bnf_alternation_row(line.trim()) {
                        entries.extend(alternatives);
                        continue;
                    }
                }
                // `fields_in_line` can come back empty on a line
                // `looks_like_flag_start` accepted (bare `-` test) but
                // whose leading token isn't `is_flag_shaped` (a stricter,
                // narrower class — see that function). Never silently drop
                // the row when that happens: fall back to the ordinary
                // single-column split, same as a block that was never
                // multi-column at all.
                let split = multi_column
                    .then(|| fields_in_line(line))
                    .filter(|f| !f.is_empty());
                match split {
                    Some(fields) => {
                        for field in fields {
                            entries
                                .push((field.tokens.join(", "), field.trailing.trim().to_string()));
                        }
                    }
                    None if aligned_spellings => entries.push(split_aligned_spelling_entry(line)),
                    None => entries.push(split_single_column_entry(line)),
                }
            }
            FlagsBlockRow::Continuation(text) => {
                if let Some(last) = entries.last_mut() {
                    last.1.push(' ');
                    last.1.push_str(text);
                }
            }
        }
    }
    (i, entries, packed)
}

/// The fewest name/description pairs a deeper-indented run must show before
/// [`nested_entry_table_starts_at`] will read it as a table rather than an
/// ordinary wrapped description.
///
/// Two, for the same reason [`scan_same_indent_entry_table`]'s `MIN_ROWS`
/// is two: one ragged continuation line that happens to be followed by a
/// still-deeper line is unremarkable prose, and must not trip this on its
/// own. Only repetition — the same name/description shape recurring — is
/// evidence of a table.
const MIN_NESTED_TABLE_ROWS: usize = 2;

/// Look ahead from a candidate continuation line at `lines[start]` (indent
/// `indent`, already known to be deeper than the flags block's own entries)
/// for a **nested entry table** — command rows with their own
/// one-level-deeper descriptions — rather than an ordinary wrapped
/// description of the flag above it.
///
/// # Why this exists
///
/// [`scan_flags_block`]'s continuation rule used to be indentation alone:
/// *any* line deeper than the block's entries continues the previous
/// entry's description. That is right for a wrapped sentence and wrong for
/// a nested table, and nothing about indentation tells them apart — both
/// are "deeper than the flag rows above."
///
/// `btrfs --help` (`corpus/btrfs/audit-seed2/help.txt`) has both, back to
/// back. `Options for the main command only:` holds two ordinary flag rows
/// at indent 2 (`--help`, `--version`). A blank line later, at indent 4, a
/// large command table begins — `btrfs balance start [options] <path>`,
/// each row followed by its own description one indent deeper (indent 8):
///
/// ```text
///   --version         print version string
///
///     btrfs balance start [options] <path>
///         Balance chunks across the devices
///     btrfs balance pause <path>
///         Pause running balance
/// ```
///
/// Indentation alone reads every line of that table, and every one of its
/// descriptions, as more of `--version`'s own description — the whole
/// command table folds into one flag.
///
/// # The rule
///
/// A row is counted when a non-blank line sits at exactly `indent` and (a)
/// is not [`looks_like_flag_start`] — a real flag row at this indent is
/// business as usual for the block, not a nested table — and (b) is
/// immediately followed by a non-blank line indented deeper still. The
/// lookahead continues across blank lines and any line at or below
/// `indent`'s depth, stopping the moment a non-blank line dedents past it
/// (structure below `indent` belongs to whatever comes after the table, not
/// to this scan). At least [`MIN_NESTED_TABLE_ROWS`] such rows makes it a
/// table.
///
/// Returning `true` tells [`scan_flags_block`] to `break` at `start` — it
/// **re-routes rather than drops**, the same contract [`bare_block_end`]'s
/// flag-row break uses: the caller resumes its own scan at exactly this
/// line, so a wrong call here loses nothing, it just leaves the text where
/// it was for a later pass to read.
fn nested_entry_table_starts_at(lines: &[&str], start: usize, indent: usize) -> bool {
    let mut rows = 0usize;
    let mut j = start;
    while j < lines.len() {
        let line = lines[j];
        if line.trim().is_empty() {
            j += 1;
            continue;
        }
        let line_indent = leading_whitespace(line);
        if line_indent < indent {
            break;
        }
        if line_indent == indent && !looks_like_flag_start(line.trim_start()) {
            if let Some(next) = lines.get(j + 1) {
                if !next.trim().is_empty() && leading_whitespace(next) > indent {
                    rows += 1;
                    // Nothing past the floor can change the answer, and
                    // this scan runs once per candidate continuation line:
                    // returning as soon as it is decided keeps a positive
                    // match from walking the rest of a long table (the
                    // "never call an O(n) function from inside a loop"
                    // hazard in AGENTS.md §2). The negative case is
                    // already short — it stops at the first line that
                    // dedents past `indent`, which in an ordinary flags
                    // block is the very next entry row.
                    if rows >= MIN_NESTED_TABLE_ROWS {
                        return true;
                    }
                }
            }
        }
        j += 1;
    }
    rows >= MIN_NESTED_TABLE_ROWS
}

/// Does the flags-block entry row currently being continued already carry
/// its own, non-empty description **on its own line**?
///
/// # Why this exists
///
/// [`nested_entry_table_starts_at`] cannot tell apart two shapes that look
/// identical from indentation alone: a nested table that does not belong to
/// the flag above it (break away — `btrfs --help`, whose
/// `--version         print version string` already has its description
/// inline, and the command table below it is not that description), and a
/// value-choice list or keyword list that **is** the flag's description
/// (never break — pngfix's `--strip=[none|crc|unsafe|unused|...]:`
/// and pod2man's `--guesswork=rule[,rule...]` both carry nothing on their
/// own line; everything below, including any run that happens to look
/// table-shaped, is the only description that flag will ever have).
/// Breaking there does not mis-split, it deletes: `--strip` and
/// `--guesswork` both lost their entire description this way before this
/// gate existed, `--guesswork` also fabricating a bogus `choices` list from
/// whatever the parser found past the wrongly-ended block.
///
/// # The rule
///
/// The entry row already has real description text of its own only when a
/// conservative single-column split of that one line ([`split_single_column_entry`],
/// the same split an ungated single-column block would use) yields a
/// non-empty description. A row that instead looks multi-column-shaped
/// ([`fields_in_line`] finds more than one field) is read conservatively as
/// *not* having settled its description yet — this file's block-wide
/// multi-column decision ([`block_is_multi_column`]) isn't available yet
/// mid-scan, so a single-line probe here can't be trusted to say which
/// field, if any, is real; refusing the break is always safe, since the
/// worst case is the same as the pre-fix behaviour these two regressions
/// need to keep.
///
/// Evaluated once per candidate continuation line from the *entry row's own
/// text*, never from the continuation rows accumulated so far — so it gives
/// the same answer at the first continuation line and at the fiftieth, and
/// a description already underway can never be truncated part-way through.
fn entry_row_carries_own_description(entry_line: &str) -> bool {
    if fields_in_line(entry_line).len() > 1 {
        return false;
    }
    let (_, desc) = split_single_column_entry(entry_line);
    !desc.trim().is_empty()
}

/// A row's leading run of cells that are **nothing but option spellings**,
/// recovered by [`spelling_run`].
struct SpellingRun {
    /// Character offset of the run's *second* cell — the column
    /// [`block_has_aligned_spelling_column`] buckets recurrence counts by.
    second_offset: usize,
    /// The run's cells verbatim, value placeholder and all
    /// (`-C <dir>`, `--backupdir=<dir>`), so nothing a row spelled out is
    /// dropped on the way to the flag grammar.
    spellings: Vec<String>,
    /// Character offset where the first cell *past* the run begins, or
    /// `None` when the run consumed every cell on the line (a two-column
    /// table with no description column at all — `awk --help`).
    description_start: Option<usize>,
    /// True when every cell in the run that names a value at all names
    /// the *same* one (`-f progfile` / `--file=progfile`) — the evidence
    /// [`cells_name_the_same_value`] describes. Two things read it:
    /// [`block_has_aligned_spelling_column`], as evidence on its own that
    /// the block really is a two-column table, and
    /// [`split_aligned_spelling_entry`], which then emits that shared
    /// value exactly once.
    value_paired: bool,
}

/// True if `cell` holds one option spelling and nothing else: a
/// flag-shaped first word ([`is_flag_shaped`]) whose remainder is either
/// empty or a bare value placeholder ([`is_value_placeholder_only`]).
///
/// This is the whole discriminator against the inverse case, and it is
/// deliberately the strict half of the pair (`misattribution`'s and
/// [`fields_in_line`]'s fold-while-bare rule is the permissive half).
/// A description that legitimately *starts* with something flag-shaped —
/// `--foo is a synonym for --bar`, `-1 means unlimited` — is one cell with
/// real words in it, so it fails here and the row keeps its ordinary
/// single-column split. Only a cell that is a spelling and stops can be
/// mistaken for one, and that is the case this recovery is for.
fn is_spelling_only_cell(cell: &str) -> bool {
    let token = first_word(cell);
    // `+d`/`+|-x` (lsof) are flag-shaped but are never spelled as a second
    // aligned column, and admitting them would widen this rule past
    // anything measured. Plain `-`-initial spellings only.
    if !token.starts_with('-') || !is_flag_shaped(token) {
        return false;
    }
    let rest = cell.strip_prefix(token).unwrap_or("").trim();
    rest.is_empty() || is_value_placeholder_only(rest)
}

/// The value token a flag-spelling cell names, normalized so that a short
/// and a long spelling of the *same* option compare equal — `-f progfile`
/// and `--file=progfile` both yield `progfile`, `-d[file]` and
/// `--dump-variables[=file]` both yield `file`. `Some("")` means the cell
/// names no value at all (`--copyright`); `None` means the cell is not a
/// single `-`-initial flag spelling this rule models.
///
/// Normalization strips one layer of value punctuation — a leading `=`,
/// `[`, `<` or `{` and a trailing `]`, `>` or `}` — because that
/// punctuation is where the two spellings of one option legitimately
/// differ (`-d[file]` attaches an optional value with a bracket, its long
/// form with `[=`). Everything inside is compared verbatim, including
/// case, quotes and `|` alternations (`-L[fatal|invalid|no-ext]` /
/// `--lint[=fatal|invalid|no-ext]`).
///
/// A cell that names a value *twice* — attached to the token and again as
/// a following word — is refused outright rather than guessed at: nothing
/// measured writes that, so it is outside what this rule has evidence for.
fn value_token(cell: &str) -> Option<String> {
    let token = first_word(cell);
    if !token.starts_with('-') || !is_flag_shaped(token) {
        return None;
    }
    let dashes = token.chars().take_while(|&c| c == '-').count();
    let name_len = token
        .chars()
        .skip(dashes)
        .take_while(|&c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        .count();
    if name_len == 0 {
        // `-?`, `-#`: no name to end, so nothing after it is a value.
        return None;
    }
    let attached: String = token.chars().skip(dashes + name_len).collect();
    let detached = cell
        .chars()
        .skip(token.chars().count())
        .collect::<String>()
        .trim()
        .to_string();
    let raw = match (attached.is_empty(), detached.is_empty()) {
        (true, true) => return Some(String::new()),
        (false, true) => attached,
        (true, false) => detached,
        (false, false) => return None,
    };
    Some(
        raw.trim()
            .trim_start_matches(['=', '[', '<', '{'])
            .trim_end_matches([']', '>', '}'])
            .trim()
            .to_string(),
    )
}

/// A flag-spelling cell reduced to the spelling alone — `--file=progfile`
/// to `--file`, `-f progfile` to `-f` — using the same leading-dashes-plus-
/// name scan as [`value_token`], so the two can never disagree about where
/// a spelling ends and its value begins. Falls back to the cell's first
/// word for anything that scan does not model.
fn bare_spelling(cell: &str) -> String {
    let token = first_word(cell);
    let dashes = token.chars().take_while(|&c| c == '-').count();
    let name_len = token
        .chars()
        .skip(dashes)
        .take_while(|&c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        .count();
    if name_len == 0 {
        return token.trim_end_matches(',').to_string();
    }
    token.chars().take(dashes + name_len).collect()
}

/// The value a flag-spelling cell names, in the cell's *own* notation but
/// detached from the spelling — `--file=progfile` and `-f progfile` both
/// give `progfile`, `--dump-variables[=file]` and `-d[file]` both give
/// `[file]`, `--prompt=[prompt]` and `-P [prompt]` both give `[prompt]`.
/// `None` when the cell names no value.
///
/// Only the `=` that *attaches* a value to its spelling is removed;
/// brackets and angles are the value's own notation and are kept, because
/// the flag grammar reads `ValueKind` off exactly those (a bracketed value
/// is optional, a bare or `=`-attached one is required). That is the whole
/// reason this returns the written form rather than [`value_token`]'s
/// normalized one.
fn value_suffix(cell: &str) -> Option<String> {
    let token = first_word(cell);
    let dashes = token.chars().take_while(|&c| c == '-').count();
    let name_len = token
        .chars()
        .skip(dashes)
        .take_while(|&c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        .count();
    if name_len == 0 {
        return None;
    }
    let attached: String = token.chars().skip(dashes + name_len).collect();
    let raw = if attached.is_empty() {
        cell.chars()
            .skip(token.chars().count())
            .collect::<String>()
            .trim()
            .to_string()
    } else if let Some(rest) = attached.strip_prefix('=') {
        // `--file=progfile`
        rest.to_string()
    } else if let Some(rest) = attached.strip_prefix("[=") {
        // `--dump-variables[=file]` — the `=` is inside the bracket that
        // marks the value optional, so only the `=` goes.
        format!("[{rest}")
    } else {
        attached
    };
    (!raw.is_empty()).then_some(raw)
}

/// True when two adjacent cells name **the same, non-empty value token**
/// ([`value_token`]) — `-f progfile` beside `--file=progfile`.
///
/// This is the discriminator that lets [`spelling_run`] pair *valued*
/// cells without widening [`is_spelling_only_cell`], which must stay
/// narrow (see its doc comment, and `fields_in_line`'s, on `arptables`'s
/// `--append  -A chain`). Two cells restating one option's value is
/// evidence of one option spelled twice in a way that a flag followed by
/// unrelated text is not: `--append` names no value at all, so it can
/// never pair with `-A chain` here, and `lsof`'s genuine multi-column
/// rows (`-n no host names  -N select NFS files`) name different trailing
/// text in every cell.
///
/// Measured over all 2,301 frozen captures in `audit/queue-captures/`
/// (2026-08-22): 24 adjacent cell pairs in 5 tools (`awk`, `gawk`,
/// `nawk`, `ntfsmove`, `ntfswipe`) satisfy this and **every one of them
/// is a genuine short/long alias pair**; no capture pairs two independent
/// flags this way. The near misses it correctly refuses are `arptables`'s
/// `-A chain`, `lsof`'s three-column table, `objcopy`'s
/// `--strip-symbols <file>   -N for all symbols listed in <file>`
/// cross-reference (the second cell's "value" is a whole sentence, not
/// the first's token), and `prove`'s `-a,  --archive out.tgz Store ...`.
fn cells_name_the_same_value(a: &str, b: &str) -> bool {
    match (value_token(a), value_token(b)) {
        (Some(x), Some(y)) => !x.is_empty() && x == y,
        _ => false,
    }
}

/// Recover the leading run of alternate spellings from one flags-block
/// entry row laid out as an aligned **multi-column option table** — short
/// spelling in column 1, long spelling in column 2, description (if the
/// tool prints one at all) in column 3:
///
/// ```text
///  -A             --smarthome             Enable smart home key
///  -C <dir>       --backupdir=<dir>       Directory for saving unique backup files
///   -l    --list
///  -f progfile    --file=progfile
///  -c num         --count num             Number of times to write
/// ```
///
/// # Why this exists
///
/// [`find_description_gap`] cuts at the row's *first* 2+-space gap, which
/// in this layout is the gap before the long spelling — so the long
/// spelling is read as the start of the description. Measured on `main`
/// before this rule: `jdeprscan`'s `--list` and `--verbose` vanished from
/// the tree completely (their rows carry no description, so
/// [`is_synonym_not_description`] correctly refused to assert the spelling
/// as prose — and then there was nowhere else for it to go), and every one
/// of `nano`'s 52 flags kept its short spelling only, with its long
/// spelling glued onto the front of its own description
/// (`--smarthome Enable smart home key`). Both are the same cut in the
/// same place; the tools differ only in whether a third column follows.
///
/// # The rule
///
/// Returns `Some` only when **all** of the following hold:
///
/// - the row opens with at least two consecutive cells that are each
///   either an [`is_spelling_only_cell`] — a spelling that stops — or a
///   cell naming the same value token as the cell beside it
///   ([`cells_name_the_same_value`], the arm that admits `awk`'s
///   `-f progfile` / `--file=progfile`), and
/// - exactly one of them is a long (`--`) spelling.
///
/// The second condition is what keeps this from merging two *independent*
/// flags. A run of two longs (`--foo  --bar`) or two shorts (`-a  -b`) is
/// as easily a genuine two-column table of separate options as an alias
/// pair, and merging there would destroy a flag; short-plus-long is the
/// one combination that is an alias pair in every layout this project has
/// measured. Rows naming several shorts at once (`jdeprscan`'s
/// `-? -h --help`) are **not** in scope and are not touched here: they are
/// blocked one step earlier, because `-? -h` is a single cell with real
/// trailing text, and even if they were not, `mandible_core::Flag` has one
/// `short: Option<char>` and no field to hold the second.
///
/// The caller applies this only to a block that shows the column actually
/// recurring — see [`block_has_aligned_spelling_column`].
fn spelling_run(line: &str) -> Option<SpellingRun> {
    let cells = cells(line);
    let mut spellings: Vec<String> = Vec::new();
    let mut second_offset = None;
    let mut description_start = None;
    for (i, (offset, content)) in cells.iter().enumerate() {
        // A cell earns its place in the run either by being a spelling and
        // nothing else, or by naming the same value token as the cell
        // immediately before or after it (see
        // [`cells_name_the_same_value`]). The backward and forward arms are
        // both needed and they always agree: the run breaks at the first
        // rejected cell, so `i - 1` is by construction already in the run,
        // and a cell admitted by its successor admits that successor in
        // turn on the next iteration.
        let in_run = is_spelling_only_cell(content)
            || (i > 0 && cells_name_the_same_value(&cells[i - 1].1, content))
            || cells
                .get(i + 1)
                .is_some_and(|(_, next)| cells_name_the_same_value(content, next));
        if !in_run {
            description_start = Some(*offset);
            break;
        }
        if spellings.len() == 1 {
            second_offset = Some(*offset);
        }
        spellings.push(content.clone());
    }
    if spellings.len() < 2 {
        return None;
    }
    let longs = spellings
        .iter()
        .filter(|c| first_word(c).starts_with("--"))
        .count();
    if longs != 1 {
        return None;
    }
    let named: Vec<String> = spellings
        .iter()
        .filter_map(|cell| value_token(cell))
        .filter(|v| !v.is_empty())
        .collect();
    let value_paired = named.len() >= 2 && named.iter().all(|v| v == &named[0]);
    Some(SpellingRun {
        second_offset: second_offset?,
        spellings,
        description_start,
        value_paired,
    })
}

/// True if `entry_lines` (a flags block's raw entry rows) shows a real,
/// aligned column of alternate spellings: at least
/// [`MIN_SPELLING_COLUMN_RECURRENCE`] rows whose [`spelling_run`]'s second
/// cell starts at the same character offset. Same instrument, and same
/// reasoning, as [`block_is_multi_column`]'s recurrence check — a table is
/// evidenced by repetition, never by one suggestive row.
fn block_has_aligned_spelling_column(entry_lines: &[&str]) -> bool {
    let mut offset_counts: std::collections::HashMap<usize, usize> =
        std::collections::HashMap::new();
    let mut value_paired_rows = 0usize;
    for line in entry_lines {
        if let Some(run) = spelling_run(line) {
            *offset_counts.entry(run.second_offset).or_insert(0) += 1;
            if run.value_paired {
                value_paired_rows += 1;
            }
        }
    }
    offset_counts
        .values()
        .any(|&count| count >= MIN_SPELLING_COLUMN_RECURRENCE)
        || value_paired_rows >= MIN_SPELLING_COLUMN_RECURRENCE
}

/// Split one entry row of a block [`block_has_aligned_spelling_column`]
/// accepted, falling back to the ordinary single-column split for a row
/// that is not itself laid out that way (a block's occasional
/// `-x  description` line among its aligned ones).
fn split_aligned_spelling_entry(line: &str) -> (String, String) {
    let Some(run) = spelling_run(line) else {
        return split_single_column_entry(line);
    };
    // Rejoin as the flag grammar's own canonical alias separator, so the
    // recovered row reaches it as the `-A, --smarthome` it means. A cell's
    // own trailing comma (`-V,  --version`, where the padding after the
    // comma was wide enough to make it two cells) is dropped rather than
    // doubled.
    // A value-paired run names one value in every cell (`-f progfile`
    // `--file=progfile`). Rejoining both verbatim would hand the flag
    // grammar the same value twice, and a detached one (`-e
    // 'program-text', --source=...`) does not even survive that round trip
    // intact — the alias list terminates on it and the long spelling is
    // lost. So such a run is rejoined as *spellings, then the value once*:
    // every cell reduced to its bare spelling ([`bare_spelling`]), and the
    // shared value appended in the form the first cell that named it wrote
    // it ([`value_suffix`]).
    //
    // Taking the value's *form* from the first cell, rather than keeping
    // the last cell verbatim, is what makes this rewrite value-preserving.
    // `less --help` writes `-P [prompt]   --prompt=[prompt]`: the short
    // cell's brackets say the value is optional, and the long cell's `=`
    // says it is required. Keeping the long cell verbatim silently promoted
    // `-P` from `Optional` to `Required` and left the brackets stranded
    // inside the value's own name. The short cell's spelling of the same
    // value is the one both readings agree came first, and the flag grammar
    // reads `--prompt [prompt]` exactly as it read `-P [prompt]` before.
    let spec = if run.value_paired {
        let spellings = run
            .spellings
            .iter()
            .map(|cell| bare_spelling(cell))
            .collect::<Vec<_>>()
            .join(", ");
        match run.spellings.iter().find_map(|cell| value_suffix(cell)) {
            Some(value) => format!("{spellings} {value}"),
            None => spellings,
        }
    } else {
        run.spellings
            .iter()
            .map(|cell| cell.trim_end_matches(',').trim_end())
            .collect::<Vec<_>>()
            .join(", ")
    };
    // Character offsets, never byte offsets (AGENTS.md §2), and the
    // description's own internal spacing is preserved by slicing the line
    // rather than rejoining its cells.
    let description = match run.description_start {
        Some(start) => line.chars().skip(start).collect::<String>(),
        None => String::new(),
    };
    let description = strip_equals_separator(description.trim_end()).to_string();
    (spec, description)
}

/// The original (pre-multi-column) way to split one flags-block entry line:
/// one description column, detected once per line. Still the only path for
/// a block [`block_is_multi_column`] didn't flag, and the fallback for a
/// multi-column block's occasional line that doesn't itself split into
/// fields (see the call site).
fn split_single_column_entry(line: &str) -> (String, String) {
    let gap = find_description_gap(line);
    let (spec, desc) = split_at_column(line, gap);
    // `find_equals_separator_gap`/`find_multi_space_gap` may have cut at or
    // before a lone `=` separator token, leaving it attached to the front
    // of `desc` (`= be verbose`, `= a local filename`) — see
    // `strip_equals_separator`.
    let desc = strip_equals_separator(&desc).to_string();
    // `find_colon_separator_gap` leaves its own separator attached the
    // same way — see `strip_colon_separator`.
    let desc = strip_colon_separator(&desc).to_string();
    // A second column of *option spellings* is not a description (`awk
    // --help` prints POSIX short options beside their GNU long
    // equivalents) — see `is_synonym_not_description`. Blanked rather than
    // dropped, so a genuine continuation line below can still supply the
    // real text.
    let desc = if is_synonym_not_description(&desc) {
        String::new()
    } else {
        desc
    };
    (spec.to_string(), desc)
}

/// Scan a bare-word block (subcommand names, enum values, ...) starting at
/// `lines[start]`, whose heading sat at `heading_indent`. Returns the
/// index just past the block and the `(name, description)` pairs
/// recovered. Unlike [`scan_flags_block`], entries here have no `-`
/// marker to key off of, so this stays indentation-based: the block's
/// baseline is its first content line's own indent, and a line indented
/// well past that baseline continues the previous entry's description.
///
/// `allow_dash_separator` threads spec issue #3's ` - ` entry separator
/// down to [`split_entries`] — see the call site in
/// [`parse_with_profile`] for why this is decided *before* scanning
/// rather than inside it.
fn scan_bare_block<'a>(
    lines: &[&'a str],
    start: usize,
    heading_indent: usize,
    allow_dash_separator: bool,
) -> (usize, Vec<(&'a str, String)>) {
    let _ = heading_indent; // kept for documentation symmetry with the caller
    let end = bare_block_end(lines, start);
    (end, split_entries(&lines[start..end], allow_dash_separator))
}

/// Find the end of a bare-word block starting at `lines[start]`: its own
/// indentation is the baseline, and the block runs until a non-blank line
/// dedents below that baseline **or a flag row resumes**. Shared by
/// [`scan_bare_block`] and [`scan_argparse_subparsers`] so both agree on
/// where a block ends even though they disagree on how to split its
/// *entries*.
///
/// # Why a flag row ends a bare block
///
/// Indentation alone was the only test here, and it is not sufficient in
/// one direction that recurs: a tool nests a bare-word list *inside* its
/// options table and then resumes the table beneath it at an indent that
/// is still at or beyond the list's own. Dedent never happens, so the
/// block ran to the end of the table and every flag in it was consumed as
/// a *choice* (or, under a recognized heading, a subcommand).
///
/// This is not a new heuristic; it is the removal of an inconsistency.
/// The section engine's very first test already says a line that
/// [`looks_like_flag_start`] begins a flags block with no heading needed,
/// and the usage-block scan above already ends *its* block on the same
/// signal for the same reason (`curl`'s 13 flag rows run straight into the
/// synopsis). `bare_block_end` was the one place that overrode that, so a
/// flag row was structure everywhere except inside a bare block.
///
/// Breaking here **re-routes rather than drops**: the caller resumes the
/// main loop at exactly this line, whose headingless-flags-block branch
/// then reads the remainder as the flag table it is. Nothing is lost even
/// if the break is wrong.
///
/// Two real documents, one rule:
///
/// - `tar --help` opens a nested `FORMAT is one of the following:` enum
///   under `--format` at indent 4, then resumes its options table at
///   indent 6 — so `--old-archive`, `--pax-option` and `--posix` were read
///   as three more values of `FORMAT` rather than as the three real flags
///   they are (`corpus/tar/1.35`, which was green and snapshot-blessed
///   through all of it; found by residue ranking, spec §13.1f).
/// - `sg_dd --help`'s `where:` operand table at indent 4 ends with its own
///   flag rows at that same indent 4, so `--dry-run`, `--help`,
///   `--progress`, `--verbose`, `--verify` and `--version` were choices of
///   nothing, and the four that also appear in the synopsis reached the
///   tree stripped of every description
///   (`corpus/sg_dd/audit-seed2`, seed-2 verdict `wrong`).
fn bare_block_end(lines: &[&str], start: usize) -> usize {
    let mut i = start;
    let entry_indent = leading_whitespace(lines[start]);
    while i < lines.len() {
        if lines[i].trim().is_empty() {
            i += 1;
            continue;
        }
        if leading_whitespace(lines[i]) < entry_indent {
            break;
        }
        // Never the first line: `flags_block_start` has already had first
        // refusal on it, so reaching here means it is not a flag row —
        // and a zero-length block would loop forever.
        if i > start && looks_like_flag_start(lines[i].trim_start()) {
            break;
        }
        i += 1;
    }
    i
}

/// Argparse's subparser blocks are the one shape spec §7 Tier B's generic
/// bare-word engine cannot express as pure data — see
/// [`super::profile::FrameworkProfile::argparse_subparser_quirk`]'s doc
/// comment for the full rationale. In short: `add_subparsers()` renders a
/// `{choice,choice,...}` pseudo-entry (argparse's own metavar for the
/// whole choice group) immediately followed by each real subcommand one
/// indent level *deeper* than that pseudo-entry:
///
/// ```text
/// positional arguments:
///   {init,build,run}
///     init            Initialize a new widget
///     build           Build the widget
///     run             Run the widget
/// ```
///
/// The generic engine's own rule — "a line indented deeper than the
/// block's entries continues the previous entry's description" — would
/// fold `init`/`build`/`run` into the pseudo-entry's own description
/// instead of recovering them as their own entries, exactly backwards
/// from what's needed. And `positional arguments:` legitimately holds
/// *plain*, non-command positionals for any argparse tool that never
/// calls `add_subparsers()` at all, so the heading text alone is never
/// evidence of a command list (spec §7 Tier B rule 1) — only the presence
/// of a `{...}`-shaped pseudo-entry inside the block is. Returns `None`
/// (no evidence found; the caller falls through to ordinary bare-block/
/// choice handling, exactly as if this framework check hadn't run at all)
/// when the block contains no such pseudo-entry, so an ordinary
/// positional-argument list is never promoted to fake subcommands.
fn scan_argparse_subparsers<'a>(
    lines: &[&'a str],
    start: usize,
    heading_indent: usize,
) -> Option<(usize, Vec<(&'a str, String)>)> {
    let _ = heading_indent;
    let end = bare_block_end(lines, start);
    let block = &lines[start..end];

    let pseudo_indent = block.iter().find_map(|l| {
        if l.trim().is_empty() {
            return None;
        }
        l.trim_start()
            .starts_with('{')
            .then(|| leading_whitespace(l))
    })?;

    let sub_lines: Vec<&str> = block
        .iter()
        .filter(|l| !l.trim().is_empty() && leading_whitespace(l) > pseudo_indent)
        .copied()
        .collect();
    if sub_lines.is_empty() {
        return None;
    }
    // `false`: argparse's own template renders subparser help in a
    // column-aligned layout (`init            Initialize a new widget`),
    // never `name - description`, and issue #3's fix is scoped to the
    // shape it was actually observed in (apt-get-style recognized
    // headings), not extended here without a real fixture driving it.
    Some((end, split_entries(&sub_lines, false)))
}

/// Scan a busybox-shaped comma-separated applet block starting at
/// `lines[start]` (spec issue #1, gated on
/// [`super::profile::FrameworkProfile::comma_separated_command_list`]).
/// Unlike every other bare-word block this engine reads, there is no
/// name/description split at all — the block is a flat run of `token,
/// token, token,` entries wrapped across several lines purely for
/// terminal width, with nothing to key a per-entry description off of —
/// so this returns `(name, "")` pairs directly rather than delegating to
/// [`split_entries`]'s indentation-and-column logic, which doesn't apply
/// here. Reuses [`bare_block_end`] to find where the block ends (same
/// "dedents below the first content line" rule every bare block uses),
/// then just splits every non-blank line on `,`.
/// Scan a command table sitting at the *same* indent as its heading
/// (`dnf` 4's flush-left command list — see the call site for why this
/// exists and what it is guarded against).
///
/// **All-or-nothing on purpose.** A single row that is not column-aligned
/// rejects the whole block rather than ending it early. Stopping early
/// would accept a table with prose appended to it, and prose promoted to
/// subcommands is exactly [M-10]; refusing the block just leaves the text
/// where it was, which is the failure this project prefers. `None` here
/// simply falls through to the ordinary handling, same as any other tool.
fn scan_same_indent_entry_table<'a>(
    lines: &[&'a str],
    start: usize,
    indent: usize,
) -> Option<(usize, Vec<(&'a str, String)>)> {
    /// One row is as likely to be a stray sentence as a table.
    const MIN_ROWS: usize = 2;

    let mut end = start;
    let mut entries: Vec<(&'a str, String)> = Vec::new();
    while end < lines.len() {
        let line = lines[end];
        if line.trim().is_empty() || leading_whitespace(line) != indent {
            break;
        }
        let (name, description) = split_entry_line(line, false);
        if description.is_empty() || !is_name_shaped_token(name) {
            return None;
        }
        entries.push((name, description));
        end += 1;
    }
    (entries.len() >= MIN_ROWS).then_some((end, entries))
}

/// The fewest name-row / deeper-description-row pairs
/// [`scan_headingless_invocation_table`] requires before treating a run of
/// tool-name-prefixed rows as a real invocation table rather than one
/// stray line — the same floor [`nested_entry_table_starts_at`] and
/// [`scan_same_indent_entry_table`] each use, for the same reason: only
/// repetition is evidence of a table.
const MIN_INVOCATION_TABLE_ROWS: usize = 2;

/// Try to recognize and consume a **headingless invocation table**
/// starting at `lines[start]` (spec §7 Tier B's headingless-command-table
/// recognizer): a run of rows the tool prints of its own invocation forms
/// — `btrfs balance start [options] <path>`, each row's own description
/// one indent deeper — with **no governing heading at all**. Every other
/// command-recovery path in this file requires a *recognized heading*
/// (module doc rule 1); this one instead requires every row to start with
/// the tool's own name at a word boundary, which is what supplies the
/// positive evidence a heading would otherwise supply, and is also what
/// supplies the nesting: `btrfs device add ...` reads as child `device`,
/// grandchild `add`.
///
/// Returns `None` when the shape doesn't admit — too few qualifying rows,
/// or the very first row isn't tool-name-prefixed and name-shaped at all —
/// in which case the caller falls through to its ordinary heading-based
/// handling of `lines[start]` unchanged (this function never partially
/// consumes on a refusal). `Some((end, nodes, seen, clean))` otherwise:
/// `end` is the index just past the table, `nodes` the (already deduped,
/// up to two levels deep) direct-child nodes to emit, and `(seen, clean)`
/// feed the same total/clean-entry confidence accounting every other
/// command-recovery branch in [`parse_with_profile`] uses.
///
/// # Admission rules (conservative — zero new false positives beats recall)
///
/// 1. **Repetition shape**: at least [`MIN_INVOCATION_TABLE_ROWS`] rows
///    where a tool-name-prefixed, name-shaped row is immediately followed
///    (no blank line between) by a non-blank, deeper-indented line — the
///    same shape [`nested_entry_table_starts_at`] tests for, applied here
///    to decide *admission* rather than merely *where the flags block
///    ends*.
/// 2. **Every row starts with the tool's own name** at a word boundary
///    ([`starts_with_tool_name`]). A row that doesn't is never part of
///    this table — reaching one ends the scan.
/// 3. **Existence attestation** (spec [M-10]'s lesson): every emitted name
///    is checked ([`token_occurs_literally`]) to occur literally, as a
///    whole token, in the raw help text this table was scanned out of —
///    true by construction (a name here is always a token split directly
///    out of a real line), but the check is explicit rather than assumed,
///    per spec §6's closing paragraphs on attestation.
/// 4. **Name shape**: only the leading run of [`is_command_name_shaped`]
///    tokens after the tool's name contributes anything; the first
///    flag-shaped, bracketed, or placeholder-shaped token ends the run.
///    This is what keeps `tar -cf archive.tar files` (an `Examples:`-style
///    row — though that heading's own block is consumed before this
///    function is ever reached, see the call site) from contributing a
///    fabricated `cf`/`archive.tar` pair even if it somehow were reached:
///    `-cf` is flag-shaped, so the run stops at zero length and the row is
///    refused outright (rule 2's own row still needs a length->=1 run).
///
/// # Emission shape
///
/// For each admitted row, `run[0]` (the first name-shaped token after the
/// tool's name) is a direct child of the node being parsed; `run[1]`, if
/// the run is at least two tokens long, is a child of that child —
/// grandchildren go no deeper, matching spec's two-level shape. The row's
/// description (the deeper-indented line(s) immediately following)
/// belongs to the *deepest* name in the run — `run[1]` when present, else
/// `run[0]`. A run of consecutive name rows sharing one following
/// description block (btrfs's `device delete` / `device remove` pair) all
/// take that shared description. Every recovered node is
/// `invocation_attested: true`, `heading_attested: false` (spec §6: layout
/// evidence about a document is not a heading declaring a command list,
/// so this table's names are never sent as `--help` probe argv even
/// though they are existence-attested) — a parent gains
/// `children_filled: true` only when the table itself supplied at least
/// one of its children.
fn scan_headingless_invocation_table<'a>(
    lines: &[&'a str],
    start: usize,
    tool_name: &str,
    raw: &str,
) -> Option<(usize, Vec<CommandNode>, usize, usize)> {
    let base_indent = leading_whitespace(lines[start]);

    let mut children: Vec<CommandNode> = Vec::new();
    let mut child_index: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    // Rows collected since the last time a description was assigned —
    // possibly more than one when several sibling name rows in a row share
    // one following description block.
    let mut pending: Vec<(&'a str, Option<&'a str>)> = Vec::new();
    let mut qualifying_rows = 0usize;
    let mut seen = 0usize;
    let mut clean = 0usize;
    let mut i = start;

    // Finalize every row in `pending` with `desc` (possibly empty — a row
    // that never got a real description still becomes a node, just an
    // undescribed one, e.g. `btrfs subvolume snapshot` whose own next line
    // in the source is whitespace-only) and clear it.
    macro_rules! finalize_pending {
        ($desc:expr) => {{
            let desc: &str = $desc;
            for (child_name, grandchild_name) in pending.drain(..) {
                if children.len() >= MAX_RECOVERED_ENTRIES {
                    break;
                }
                let child_idx = *child_index
                    .entry(child_name.to_string())
                    .or_insert_with(|| {
                        let mut node =
                            CommandNode::new(child_name, Provenance::single(Source::HelpText));
                        node.invocation_attested = true;
                        node.heading_attested = false;
                        children.push(node);
                        children.len() - 1
                    });
                match grandchild_name {
                    Some(grandchild_name) => {
                        children[child_idx].children_filled = true;
                        let parent = &mut children[child_idx];
                        let existing = parent
                            .subcommands
                            .iter()
                            .position(|c| c.name == grandchild_name);
                        let gc_idx = match existing {
                            Some(idx) => idx,
                            None => {
                                if parent.subcommands.len() >= MAX_RECOVERED_ENTRIES {
                                    continue;
                                }
                                let mut node = CommandNode::new(
                                    grandchild_name,
                                    Provenance::single(Source::HelpText),
                                );
                                node.invocation_attested = true;
                                node.heading_attested = false;
                                parent.subcommands.push(node);
                                parent.subcommands.len() - 1
                            }
                        };
                        if parent.subcommands[gc_idx].summary.is_none() {
                            parent.subcommands[gc_idx].summary = non_empty_text(desc);
                        }
                    }
                    None => {
                        if children[child_idx].summary.is_none() {
                            children[child_idx].summary = non_empty_text(desc);
                        }
                    }
                }
            }
        }};
    }

    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            finalize_pending!("");
            i += 1;
            continue;
        }
        let indent = leading_whitespace(line);
        if indent < base_indent {
            break;
        }
        if indent > base_indent {
            // An orphaned deeper line with nothing pending to attach to
            // (shouldn't normally occur — the row branch below already
            // consumes an immediately-following description block). Skip
            // rather than risk misreading it as a new row.
            i += 1;
            continue;
        }

        let trimmed = line.trim_start();
        if !starts_with_tool_name(trimmed, tool_name) {
            break;
        }
        let Some(run) = invocation_table_row_run(trimmed, tool_name) else {
            break;
        };
        seen += 1;
        let child_name = run[0];
        let grandchild_name = run.get(1).copied();
        if !token_occurs_literally(raw, child_name)
            || grandchild_name.is_some_and(|g| !token_occurs_literally(raw, g))
        {
            // Should never happen by construction (the name was split
            // directly out of this very line), but the guard is explicit
            // (spec [M-10]) — refuse this row rather than trust it.
            i += 1;
            continue;
        }
        clean += 1;
        pending.push((child_name, grandchild_name));
        i += 1;

        if i < lines.len()
            && !lines[i].trim().is_empty()
            && leading_whitespace(lines[i]) > base_indent
        {
            let desc_start = i;
            while i < lines.len()
                && !lines[i].trim().is_empty()
                && leading_whitespace(lines[i]) > base_indent
            {
                i += 1;
            }
            let desc = lines[desc_start..i]
                .iter()
                .map(|l| l.trim())
                .collect::<Vec<_>>()
                .join(" ");
            qualifying_rows += pending.len();
            finalize_pending!(&desc);
        }
    }
    finalize_pending!("");

    if qualifying_rows < MIN_INVOCATION_TABLE_ROWS || children.is_empty() {
        return None;
    }
    Some((i, children, seen, clean))
}

/// The leading run (up to two tokens) of [`is_command_name_shaped`] tokens
/// in `trimmed` after stripping `tool_name` from the front — the token
/// shape [`scan_headingless_invocation_table`] reads as `(child,
/// Option<grandchild>)`. `None` when `trimmed` doesn't start with
/// `tool_name` at all, or the very first token after it isn't name-shaped
/// (a flag, a bracketed/placeholder token, or punctuation) — in either
/// case there is nothing here to promote.
fn invocation_table_row_run<'a>(trimmed: &'a str, tool_name: &str) -> Option<Vec<&'a str>> {
    let rest = trimmed.strip_prefix(tool_name)?;
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        return None;
    }
    let mut run = Vec::new();
    for token in rest.split_whitespace() {
        let name = token.trim_end_matches(':');
        let name = strip_optional_modifier_suffix(name);
        if is_command_name_shaped(name) {
            run.push(name);
            if run.len() == 2 {
                break;
            }
        } else {
            break;
        }
    }
    if run.is_empty() {
        None
    } else {
        Some(run)
    }
}

/// Whole-token occurrence check for spec [M-10]'s existence-attestation
/// lesson (spec §6): is `token` present in `raw` as a maximal run of
/// [`is_command_name_shaped`]'s own character class, rather than merely as
/// a substring of some longer word (`"sub"` must not "occur" inside
/// `"subvolume"`)? Splitting on everything outside that class and
/// comparing for an exact match is what gives "whole token" its meaning
/// here, matching the character class the names themselves are drawn from.
fn token_occurs_literally(raw: &str, token: &str) -> bool {
    raw.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')))
        .any(|w| w == token)
}

fn scan_comma_separated_commands<'a>(
    lines: &[&'a str],
    start: usize,
) -> (usize, Vec<(&'a str, String)>) {
    let end = bare_block_end(lines, start);
    let mut entries = Vec::new();
    for line in &lines[start..end] {
        for token in line.split(',') {
            let name = token.trim();
            if !name.is_empty() {
                entries.push((name, String::new()));
            }
        }
    }
    (end, entries)
}

fn non_empty_text(s: &str) -> Option<Text> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(Text::sanitize(t))
    }
}

/// Split a bare-word block's raw lines into `(name_fragment,
/// description_fragment)` pairs, one per entry, folding continuation
/// lines into the preceding entry's description.
///
/// Entries are distinguished from continuation lines by indentation: the
/// block's baseline indent is the minimum indentation among its non-blank
/// lines, and a line at or near that baseline starts a new entry, while a
/// line indented well past it continues the previous entry's description.
///
/// `allow_dash_separator` (spec issue #3): when true, a new-entry line
/// with no 2+-space column gap falls back to splitting on the first
/// ` - ` (space-dash-space) run instead — apt-get's own `"update -
/// Retrieve new lists of packages"` style. The column gap is always tried
/// first and wins when present, so this never changes how a tool that
/// already uses column alignment is read.
fn split_entries<'a>(
    block_lines: &[&'a str],
    allow_dash_separator: bool,
) -> Vec<(&'a str, String)> {
    let non_blank: Vec<&&str> = block_lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .collect();
    if non_blank.is_empty() {
        return Vec::new();
    }
    let baseline = non_blank
        .iter()
        .map(|l| leading_whitespace(l))
        .min()
        .unwrap_or(0);

    let mut entries: Vec<(&str, String)> = Vec::new();
    for line in block_lines {
        if line.trim().is_empty() {
            continue;
        }
        let indent = leading_whitespace(line);
        let is_new_entry = indent <= baseline + 1;
        if is_new_entry {
            let (spec, desc) = split_entry_line(line, allow_dash_separator);
            entries.push((spec, desc));
        } else if let Some(last) = entries.last_mut() {
            last.1.push(' ');
            last.1.push_str(line.trim());
        } else {
            // Malformed: a continuation with nothing to continue. Treat
            // as its own (spec-only) entry rather than dropping it.
            entries.push((line.trim(), String::new()));
        }
    }
    entries
}

/// Split one bare-block entry line into `(name, description)`: the usual
/// 2+-space column gap first, falling back to a ` - ` separator (spec
/// issue #3) only when `allow_dash_separator` is set and no column gap
/// was found.
fn split_entry_line(line: &str, allow_dash_separator: bool) -> (&str, String) {
    let (spec, description) = split_entry_line_raw(line, allow_dash_separator);
    if is_synonym_not_description(&description) {
        return (spec, String::new());
    }
    (spec, description)
}

/// True if `description` is a bare option spelling rather than prose — a
/// single token beginning with `-`.
///
/// Some tools lay out two *columns of flags* rather than flag-and-prose.
/// `awk --help` is the case that forced this: it prints POSIX short options
/// beside their GNU long equivalents, tab-separated, so reading the second
/// column as a description gives `-f progfile` the "description"
/// `--file=progfile`. That is not a description, it is the same option
/// spelled differently, and asserting it would be the fabrication §1
/// forbids — worse than the honest "no description" the tool actually
/// offers, and it would have been reported as **28 flags, 100% described**.
///
/// Deliberately narrow: only a lone token counts. Real descriptions that
/// merely *start* with a dash (`-1 means unlimited`) have more than one
/// word and are untouched.
fn is_synonym_not_description(description: &str) -> bool {
    let trimmed = description.trim();
    trimmed.starts_with('-') && !trimmed.contains(char::is_whitespace)
}

fn split_entry_line_raw(line: &str, allow_dash_separator: bool) -> (&str, String) {
    if let Some(col) = find_description_gap(line) {
        return split_at_column(line, Some(col));
    }
    if allow_dash_separator {
        if let Some(idx) = find_dash_separator(line) {
            return split_at_dash(line, idx);
        }
    }
    split_at_column(line, None)
}

/// Find the byte offset of a ` - ` (space-dash-space) entry separator in
/// `line`, if any — the alternative to [`find_description_gap`]'s column-
/// gap separator that apt-get-style `name - description` listings use
/// (spec issue #3). Returns the offset of the dash itself. A name's own
/// internal hyphens (`dist-upgrade`, `apt-get`) never match: they have no
/// space on at least one side, so only a genuine surrounding-space
/// separator is found.
fn find_dash_separator(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut i = 1;
    while i + 1 < bytes.len() {
        if bytes[i] == b'-' && bytes[i - 1] == b' ' && bytes[i + 1] == b' ' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Split `line` at a ` - ` separator found by [`find_dash_separator`]:
/// `dash_idx` is the dash's own byte offset, so the name is everything
/// before the space preceding it and the description is everything after
/// the space following it — the dash and its surrounding spaces are
/// punctuation, never part of either side.
fn split_at_dash(line: &str, dash_idx: usize) -> (&str, String) {
    let spec = line[..dash_idx].trim_end();
    let desc = line[dash_idx + 1..].trim_start().to_string();
    (spec, desc)
}

/// Find the byte offset of a ` = ` (space-equals-space) entry separator in
/// `line`, if any — [`find_dash_separator`]'s twin for a headed command
/// table that uses `=` instead of `-` (`wpa_cli`'s `commands:` block:
/// `status [verbose] = get current WPA/EAPOL/EAP status`). Same shape,
/// same reasoning: a token's own internal `=` (`payload=<hex dump of
/// payload>`, `dialog=<token>`) never matches, because it has no space on
/// at least one side, so only a genuine surrounding-space separator is
/// found. Distinct from [`find_equals_separator_gap`], which is
/// deliberately restricted to flag rows and a stricter "every token
/// before `=` is value-spec-shaped" test — see that function's doc
/// comment for why a bare-word block must never reach it. This one is
/// reached only from [`scan_bare_command_table`], itself gated on already
/// being headed for command emission (see that function's own doc
/// comment for the gate).
fn find_bare_equals_separator_gap(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut i = 1;
    while i + 1 < bytes.len() {
        if bytes[i] == b'=' && bytes[i - 1] == b' ' && bytes[i + 1] == b' ' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Split `line` at a ` = ` separator found by [`find_bare_equals_separator_gap`]:
/// `eq_idx` is the `=`'s own byte offset, so the name field is everything
/// before the space preceding it and the description is everything after
/// the space following it — mirrors [`split_at_dash`] exactly, substituting
/// the separator character.
fn split_at_bare_equals(line: &str, eq_idx: usize) -> (&str, String) {
    let name_field = line[..eq_idx].trim_end();
    let desc = line[eq_idx + 1..].trim_start().to_string();
    (name_field, desc)
}

/// Split one row of a headed command table into `(name, description)`:
/// [`find_bare_equals_separator_gap`]'s ` = ` separator when present
/// (`wpa_cli`'s ordinary row shape), otherwise the row's leading token as
/// the name with no description at all (`apt-ftparchive`'s
/// `sources srcpath [overridefile [pathprefix]]`, and the handful of
/// `wpa_cli` rows that carry no `=` at all — `wps_cancel Cancels the
/// pending WPS operation` — a real inconsistency in that tool's own
/// `--help` text, not something this parser should paper over by guessing
/// where the name ends and prose begins).
///
/// The no-separator branch deliberately never treats trailing words as a
/// description: for `apt-ftparchive` those words are the command's own
/// positional operands (`binarypath [overridefile [pathprefix]]`), and
/// reading them as prose would fabricate a description the tool never
/// wrote — the exact §1 violation this project exists to refuse. Losing
/// three real `wpa_cli` descriptions to the same rule is the honest price
/// of not being able to tell the two shapes apart from a single line.
///
/// `None` when the leading token isn't [`is_command_name_shaped`] — the
/// per-row refusal [`scan_bare_command_table`] relies on to skip a stray
/// line without rejecting the whole table.
fn split_bare_command_table_row(line: &str) -> Option<(&str, Option<String>)> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    match find_bare_equals_separator_gap(trimmed) {
        Some(eq_idx) => {
            let (name_field, desc) = split_at_bare_equals(trimmed, eq_idx);
            let name = leading_command_name(name_field)?;
            let desc = if desc.trim().is_empty() {
                None
            } else {
                Some(desc)
            };
            Some((name, desc))
        }
        None => {
            let name = leading_command_name(trimmed)?;
            Some((name, None))
        }
    }
}

/// Scan a headed command table whose rows carry no column-aligned
/// description at all — `wpa_cli`'s `commands:` block and
/// `apt-ftparchive`'s `Commands:` table (spec §7 Tier B, the
/// headed-command-table subsection). Reuses [`bare_block_end`] to find
/// the block, same as [`scan_bare_block`], but splits each row with
/// [`split_bare_command_table_row`] instead of [`split_entry_line`], and
/// its emission ([`emit_headed_command_table`]) is `invocation_attested`
/// rather than `heading_attested` — see that function's doc comment for
/// why.
///
/// # The column-gap/dash bail-out
///
/// Bails (`None`) outright — before reading a single row — if *any*
/// non-blank line in the block has a real column gap
/// ([`find_multi_space_gap`]) or a ` - ` separator ([`find_dash_separator`]).
/// Both are already-working shapes: a column-aligned table is read
/// correctly today via [`split_entry_line`]'s ordinary column-gap path,
/// and a dash-separated one via `allow_dash_separator`. Without this
/// guard, this function would compete with both — worse, it would *win*
/// by running first, silently discarding a working column-gap or dash
/// description in favor of this function's own "leading token, no
/// description" fallback. This is also what keeps this recognizer away
/// from `wpa_supplicant`'s own `drivers:` block (`nl80211 = Linux
/// nl80211/cfg80211`) even on the rare tool where that heading *would*
/// otherwise be recognized as a command list: a description-bearing
/// `name = description` bare block reads as a column gap the instant its
/// values line up, and the moment it doesn't, [`find_equals_separator_gap`]'s
/// own doc comment already explains why that block must be read as
/// `(name, value)`, never split apart here.
///
/// # Why bare single-word rows are excluded from the admission floor
///
/// A row that is just one bare word (no `=`, no further tokens) already
/// parses correctly through the ordinary heading-recognized path with
/// `heading_attested: true` — the strictly *more* trustworthy bit. Only
/// rows that are demonstrably not working today (an `=` separator, or
/// more than one token on the name side) count toward
/// [`MIN_INVOCATION_TABLE_ROWS`]'s floor, so this recognizer never fires
/// for a block that the existing engine already reads correctly, even
/// though a fired scan still emits every row it can (a single-word row
/// caught up in an otherwise-qualifying block is still worth recovering,
/// just with the weaker attestation bit, rather than being dropped
/// outright).
///
/// # The floor counts distinct names, not qualifying rows
///
/// Two rows that qualify but share one name do not meet the floor —
/// `trash-put --help`'s own "use one of these commands:" sentence (a real
/// false hit of `mentions_commands_word`/`is_recognized_command_heading`,
/// which read no further than "does the word 'commands' appear") followed
/// by the worked example `trash -- -foo` / `trash ./-foo`, two invocations
/// of one *different* program, not two commands of `trash-put`. Both rows
/// qualify by every other rule here (no column gap, no dash, a
/// name-shaped leading token, more than one token on the line), and
/// without this a distinct guard would have fabricated `trash` as a
/// subcommand of `trash-put`. A real command table lists several
/// *different* commands — that is the entire evidentiary point of
/// requiring repetition at all — so this is not a new restriction, only a
/// more precise statement of the one already documented above.
fn scan_bare_command_table<'a>(
    lines: &[&'a str],
    start: usize,
) -> Option<(usize, CommandTableEntries<'a>)> {
    let end = bare_block_end(lines, start);
    let block = &lines[start..end];

    if block.iter().any(|l| {
        !l.trim().is_empty()
            && (find_multi_space_gap(l).is_some() || find_dash_separator(l).is_some())
    }) {
        return None;
    }

    let non_blank: Vec<&&str> = block.iter().filter(|l| !l.trim().is_empty()).collect();
    if non_blank.is_empty() {
        return None;
    }
    let baseline = non_blank
        .iter()
        .map(|l| leading_whitespace(l))
        .min()
        .unwrap_or(0);

    let mut entries: CommandTableEntries<'a> = Vec::new();
    let mut qualifying_names: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for &line in block {
        if line.trim().is_empty() {
            continue;
        }
        let indent = leading_whitespace(line);
        if indent <= baseline + 1 {
            let Some((name, desc)) = split_bare_command_table_row(line) else {
                continue;
            };
            if desc.is_some() || line.split_whitespace().count() > 1 {
                qualifying_names.insert(name);
            }
            entries.push((name, desc));
        } else if let Some(last) = entries.last_mut() {
            // A deeper-indented continuation of the previous row's
            // description — same rule `split_entries` uses to fold
            // wrapped description lines. Never invents a description
            // where the entry row had none (an `apt-ftparchive` row is
            // never followed by a continuation line in the first place,
            // since none of its rows wrap), it simply starts one from
            // whatever real text the tool printed on the continuation
            // line.
            let cont = line.trim();
            match &mut last.1 {
                Some(d) => {
                    d.push(' ');
                    d.push_str(cont);
                }
                None => last.1 = Some(cont.to_string()),
            }
        }
    }

    // Distinct names, not raw qualifying rows: a real command table lists
    // several *different* commands, which is the whole point of it being a
    // table. Two rows sharing one name is exactly the shape a worked usage
    // example produces instead (`trash-put`'s own "use one of these
    // commands:" sentence — itself a false hit of `mentions_commands_word`,
    // not a real heading — followed by `trash -- -foo` / `trash ./-foo`,
    // two alternative invocations of one *different* program's example),
    // and admitting it fabricated `trash` as a "subcommand" of
    // `trash-put`. Requiring distinct names costs nothing on either real
    // fixture (`wpa_cli`'s ~180 rows and `apt-ftparchive`'s six are all
    // distinct) and closes this off structurally rather than by trying to
    // out-guess every future sentence that happens to contain "commands:".
    (qualifying_names.len() >= MIN_INVOCATION_TABLE_ROWS && !entries.is_empty())
        .then_some((end, entries))
}

/// Find the byte offset of the first column gap in `line`, if any, after
/// some non-whitespace content — a run of 2+ spaces, or any run containing
/// a tab.
///
/// A tab counts on its own because it is never decoration: it advances to
/// the next 8-column stop, so a single one already separates columns by at
/// least as much as the two spaces required of a space run. Ignoring tabs
/// meant a tab-aligned table had no gap at all and every row collapsed to
/// a name with no description. Measured on `mokutil --help`, which writes
/// `  --list-enrolled\t\t\t\tList the enrolled keys`: **38 flags, 0
/// described**, while the descriptions were sitting right there in the
/// output.
fn find_description_gap(line: &str) -> Option<usize> {
    if let Some(col) = find_multi_space_gap(line) {
        return Some(col);
    }
    // Only ever consulted when the rule above found nothing anywhere in
    // the line: a lone `=` token standing in for a column gap
    // (`update-xmlcatalog`'s `--verbose = be verbose`, `wpa_supplicant`'s
    // `-b = optional bridge interface name`) — see
    // `find_equals_separator_gap`'s own doc comment.
    if let Some(col) = find_equals_separator_gap(line) {
        return Some(col);
    }
    // Only ever consulted when the rules above found nothing anywhere in
    // the line: a colon standing in for a column gap, either as its own
    // token (`sg_emc_trespass`'s `-d : output debug`) or glued straight
    // onto the spec (`-hr: Set Honor Reservation bit`) — see
    // `find_colon_separator_gap`'s own doc comment.
    if let Some(col) = find_colon_separator_gap(line) {
        return Some(col);
    }
    // Only ever consulted when the rules above found nothing anywhere in
    // the line — see `find_placeholder_boundary_gap`'s own doc comment.
    if let Some(col) = find_placeholder_boundary_gap(line) {
        return Some(col);
    }
    // Same "no aligned column anywhere" precondition, one shape further
    // out: no placeholder either, just a sentence. See
    // `find_sentence_start_gap`.
    find_sentence_start_gap(line)
}

/// Second fallback for a flag row with no aligned column at all: the
/// description simply starts one space after the spec, and it is
/// recognizable because it starts an **English sentence** rather than
/// naming a value.
///
/// The shape is what a long flag name does to a fixed-width option table —
/// the name overruns the description column, and the formatter emits one
/// space instead of the padding it can no longer supply:
///
/// ```text
///   --md5 Control MD5 generation                    (apt-ftparchive)
///   --no-delink Enable delinking debug mode         (apt-ftparchive)
///   --allow-multiple-definition Allow multiple definitions of symbols   (ld)
///   -print-multi-os-directory Display the relative path to OS libraries (gcc)
/// ```
///
/// Without this, `find_description_gap` returns `None`, the *whole line*
/// is handed to `grammar::parse_flag_spec` as the spec, and its
/// ` VALUE` arm takes the first word of the prose as a `value_name` and
/// **discards the rest of the sentence** — `--md5` acquires the value
/// `Control` and the words "MD5 generation" are lost from the tree
/// entirely. Measured on `apt-ftparchive --help`: 9 flags, 2 of them
/// carrying another flag's job in their `value_name` and no description.
///
/// **Only ever consulted when both gap-finders above found nothing
/// anywhere in the line**, exactly as [`find_placeholder_boundary_gap`]
/// is, so no already-working split can move — this only recovers text
/// that would otherwise be dropped.
///
/// The predicate is deliberately narrow, because the inverse case is the
/// whole risk: ` VALUE` is a real and common shape (`--class-path PATH`,
/// `--release 7|8|9|…|17`, `--manifest-path <manifest-path>`), and reading
/// a value name as prose would delete a real field. Three conditions, all
/// required:
///
/// 1. The line starts with a flag (`-`). Bare-word blocks — subcommand
///    tables, enum-value lists — never reach this at all.
/// 2. The candidate token [`starts_a_sentence`]: an initial ASCII
///    uppercase letter followed by nothing but ASCII lowercase ones
///    (`Control`, `Enable`, `Display`). Every value placeholder shape this
///    project has measured fails that test — `PATH` and `MD5` are all-caps,
///    `<path>` and `[=WHEN]` are bracketed, `7|8|9` is punctuated.
/// 3. At least one more word follows it, so a lone trailing token
///    (`-v Verbose`, `--md5 Control`) is still read as a value. A single
///    word is genuinely ambiguous and stays with the pre-existing reading.
///
/// Scanning stops at the first token that is neither sentence-shaped nor
/// [`is_value_spec_token`] — so a *lowercase* metavar ends the search
/// rather than letting a capitalized word deeper in the line become a
/// false boundary (`--opt value do a Thing here` splits nowhere, instead
/// of splitting before `Thing`).
fn find_sentence_start_gap(line: &str) -> Option<usize> {
    if !line.trim_start().starts_with('-') {
        return None;
    }
    let bytes = line.as_bytes();
    let mut i = 0usize;
    let mut token_count = 0usize;
    let mut previous_token_end: Option<usize> = None;
    // A spec that already carries its own value (`--init-command=name`,
    // `--color[=WHEN]`) cannot take another one, so the boundary is fixed
    // at that first token and every word after it is description — even a
    // value-shaped one. Without this, `mariadb`'s
    // `--init-command=name SQL Command to execute ...` splits after `SQL`
    // instead, and the word "SQL" is dropped from the tree: the spec keeps
    // its `=name` value (first value wins) and nothing ever reads `SQL`
    // back out. Never discard a word the tool wrote.
    let mut spec_is_closed = false;
    while i < bytes.len() {
        if bytes[i] == b' ' || bytes[i] == b'\t' {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i] != b' ' && bytes[i] != b'\t' {
            i += 1;
        }
        // `start` and `i` only ever land on ASCII space/tab boundaries or
        // the ends of the string, so neither can fall inside a multi-byte
        // character — `get` rather than `[..]` regardless (AGENTS.md §2).
        let token = line.get(start..i)?;
        if token_count > 0 {
            let more_words_follow = line
                .get(i..)
                .is_some_and(|rest| rest.split_whitespace().next().is_some());
            if more_words_follow && starts_a_sentence(token) {
                return previous_token_end;
            }
            if !is_value_spec_token(token) {
                return None;
            }
        } else {
            spec_is_closed = token.contains('=') || token.contains('[');
        }
        // The spelling run's own token always sets the boundary; a closed
        // spec then freezes it there.
        if token_count == 0 || !spec_is_closed {
            previous_token_end = Some(i);
        }
        token_count += 1;
    }
    None
}

/// True if `token` reads as the first word of an English sentence rather
/// than as a value placeholder: an ASCII uppercase letter followed by at
/// least one, and nothing but, ASCII lowercase letters.
///
/// Checked against every distinct occurrence of the shape in this box's
/// 2,301 captured `--help` documents: all 108 of them are verbs
/// (`Use`, `Do`, `Disable`, `Allow`, `Print`, `Enable`, `Display`, …) and
/// none is a metavar. All-caps (`PATH`), mixed (`MD5`, `IPv6`) and
/// punctuated (`7|8|9`) tokens are excluded by construction.
fn starts_a_sentence(token: &str) -> bool {
    let mut chars = token.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_uppercase() {
        return false;
    }
    let mut saw_rest = false;
    for c in chars {
        if !c.is_ascii_lowercase() {
            return false;
        }
        saw_rest = true;
    }
    saw_rest
}

/// True if `token` can plausibly still be part of a flag *spec* rather
/// than of its description — used by [`find_sentence_start_gap`] to decide
/// how far it may keep looking for a sentence boundary.
///
/// Anything that names a spelling, a placeholder, or a metavar qualifies:
/// a leading `-`, any notation punctuation, a digit or a `-`/`_`/`.`
/// inside the word, or an all-caps run. A bare all-lowercase alphabetic
/// word does *not* — that is where the scan stops, which is what keeps a
/// capitalized word in the middle of a description from being mistaken for
/// its start.
fn is_value_spec_token(token: &str) -> bool {
    if token.starts_with('-') {
        return true;
    }
    if token.chars().any(|c| {
        matches!(
            c,
            '<' | '>' | '[' | ']' | '{' | '}' | '(' | ')' | '=' | '|' | ',' | '/' | ':'
        )
    }) {
        return true;
    }
    if token
        .chars()
        .any(|c| c.is_ascii_digit() || c == '-' || c == '_' || c == '.')
    {
        return true;
    }
    token.chars().all(|c| c.is_ascii_uppercase())
}

/// The fewest consecutive spaces that separate a row's columns rather than
/// merely decorating it — the boundary [`find_multi_space_gap`] cuts at.
///
/// Named rather than left as a literal `2` because it is what once put a
/// whole shape out of the flag-spec grammar's reach: `jdeprscan`'s
/// `  -l    --list` writes its two spellings four spaces apart, so the long
/// form arrived as a *description* and no fragment ever named both. A
/// detector that declares that shape out of its scope has to cite this
/// constant to say so structurally (`xtask`'s
/// `detector::Ground::AcrossDescriptionColumn`), and a retyped copy of the
/// value could drift away from the splitter it claims to describe.
///
/// **That shape is now recovered** — see [`spelling_run`], which reads a
/// second cell that is nothing but another spelling as the option's other
/// spelling rather than as its description. This constant still marks the
/// boundary the naive splitter cuts at; it is no longer the end of the
/// story for a row whose second column is itself a spelling.
pub const MIN_COLUMN_GAP_SPACES: usize = 2;

/// The original heuristic, unchanged: a run of two or more spaces, or any
/// run containing a tab, after some non-whitespace content.
fn find_multi_space_gap(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut seen_content = false;
    while i < bytes.len() {
        if bytes[i] == b' ' || bytes[i] == b'\t' {
            let mut j = i;
            let mut had_tab = false;
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                had_tab |= bytes[j] == b'\t';
                j += 1;
            }
            if seen_content && (had_tab || j - i >= MIN_COLUMN_GAP_SPACES) {
                return Some(i);
            }
            i = j;
        } else {
            seen_content = true;
            i += 1;
        }
    }
    None
}

/// Fallback for a line with no aligned column at all — some tools (`curl
/// --help all`, spec §6 rule 2b's own fixture, `corpus/curl/8.5.0-all`,
/// is what surfaced this) right-pad *short* specs to a fixed width but
/// simply run a single space after a *long* one:
///
/// ```text
///      --abstract-unix-socket <path> Connect via abstract Unix domain socket
///  -a, --append      Append to target file when uploading
/// ```
///
/// The second row has real column padding and [`find_multi_space_gap`]
/// finds it; the first has none at all, so without this fallback the
/// whole line — placeholder and description together — reads as the flag
/// spec with an empty description, and a real, present description is
/// silently lost (curl's `--help all` measured 25.2% described before this
/// fix, almost entirely from short flags that happened to have padding).
///
/// **Only ever consulted when [`find_multi_space_gap`] found no gap
/// anywhere in the line at all** — every line with a real aligned column
/// keeps taking that path completely unchanged, so this cannot move where
/// an already-working split happens; it only recovers a description that
/// would otherwise be lost entirely.
///
/// Splits right after the first `>` or `]` that closes a value-placeholder
/// -shaped token (`<value>`, `[value]` — the two spellings this project's
/// own flag grammar, `grammar.rs`, already recognizes) when it is
/// immediately followed by exactly one space and then more content.
/// Content-keyed on that closing-bracket shape, never on a tool name —
/// the same discipline every other layout heuristic in this file follows.
/// A `]` that closes a bracket *inside* a placeholder (`<[%]name=...>`)
/// is never mistaken for the boundary: nothing follows it but more of the
/// placeholder, never a single space, so it fails the "immediately
/// followed by exactly one space" test and scanning continues to the
/// placeholder's real closing `>`.
/// Fallback for a flag row with no aligned column at all, that separates
/// spec from description with a lone `=` token instead of whitespace or a
/// dash: `update-xmlcatalog`'s `--verbose = be verbose` and
/// `wpa_supplicant`'s `-b = optional bridge interface name` (spec §6, the
/// `=`-as-separator family). Before this fallback neither line has a 2+
/// space gap anywhere, so the whole row fell into
/// `grammar::parse_flag_spec` and the description was lost outright —
/// measured on `wpa_supplicant`, whose ~28 flags dropped to 9% parsed.
///
/// **Only ever consulted when [`find_multi_space_gap`] found no gap
/// anywhere in the line**, so an aligned row like `--file <file>       =
/// a local filename` keeps taking that path unchanged; this function never
/// even runs for it. That row's leftover `= ` prefix on the description is
/// [`strip_equals_separator`]'s job, not this one's.
///
/// Restricted to flag rows (`line.trim_start()` starts with `-`) — a
/// bare-word block using the same `name = description` shape
/// (`wpa_supplicant`'s `drivers:` block, `nl80211 = Linux
/// nl80211/cfg80211`) has no `-` anchor to key off of and is deliberately
/// left alone: that block never reaches [`find_description_gap`] through
/// this path anyway ([`scan_bare_block`] uses a different splitter), but
/// the guard also protects the (untested) day this function gets reused
/// from a different call site.
///
/// Scans tokens left to right. Every token before the `=` must satisfy
/// [`is_value_spec_token`] — the same predicate [`find_sentence_start_gap`]
/// uses to tell a still-open spec from prose — so `--file <file> = ...`
/// qualifies (`<file>` is a placeholder) but `--foo Set X = Y` does not:
/// `Set` is a bare lowercase-free word that stops the scan, and the
/// function returns `None` rather than treating `Set`'s `=` as the
/// separator. The candidate token itself must be **exactly** `=` — never
/// `=x`, `x=`, or `x=y` — which the whitespace tokenizer guarantees is
/// already surrounded by whitespace on both sides. Real in-spec and
/// in-description `=` usage (`--opt=VALUE`, `ffplay`'s "0 = disable, 1 =
/// enable", `bugpoint-18`'s "(default = off)") never reaches this function
/// bare: either `find_multi_space_gap` already cut the line, or the `=`
/// sits inside a token rather than standing alone as one.
///
/// Requires at least one non-whitespace character after the `=` — an
/// empty tail (`--flag =`) returns `None`, leaving today's behaviour (no
/// description) unchanged.
///
/// Returns the byte offset of the `=` character itself, so
/// [`split_at_column`] yields `spec = "--verbose"` and
/// `desc = "= be verbose"` — the leading `= ` is stripped afterward by
/// [`strip_equals_separator`], the same function symptom 2 uses.
fn find_equals_separator_gap(line: &str) -> Option<usize> {
    if !line.trim_start().starts_with('-') {
        return None;
    }
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b' ' || bytes[i] == b'\t' {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i] != b' ' && bytes[i] != b'\t' {
            i += 1;
        }
        // `start` and `i` only ever land on ASCII space/tab boundaries or
        // the ends of the string, so neither can fall inside a multi-byte
        // character — `get` rather than `[..]` regardless (AGENTS.md §2).
        let token = line.get(start..i)?;
        if token == "=" {
            let tail_has_content = line.get(i..).is_some_and(|rest| !rest.trim().is_empty());
            return if tail_has_content { Some(start) } else { None };
        }
        if !is_value_spec_token(token) {
            return None;
        }
    }
    None
}

/// Strip a flag row description's leading `= ` separator token: if `desc`
/// starts with `=` immediately followed by ASCII whitespace, drop the `=`
/// and that whitespace; otherwise return it unchanged.
///
/// Two call sites need this, both symptoms of the same `=`-as-separator
/// layout: [`find_equals_separator_gap`] deliberately returns the `=`
/// character's own offset (so [`split_at_column`] keeps the separator
/// attached to whichever side already worked), and a row whose column
/// *is* aligned (`--file <file>       = a local filename`) already splits
/// correctly at the space run **before** the `=` — [`find_multi_space_gap`]
/// gets there first — leaving the same leading `= ` on the description.
/// One strip serves both paths.
///
/// Only the separator token is removed. A second `=` inside the
/// description proper is text, not punctuation, and is left alone:
/// `--root              = the root XML catalog (= /etc/xml/catalog)`
/// strips to `the root XML catalog (= /etc/xml/catalog)`, keeping the
/// parenthetical's own `=`.
///
/// Apply this only where a flag row's description is produced — never to
/// a bare-word block's entries, where `name = description` names the
/// value itself (`wpa_supplicant`'s `drivers:` block) and stripping would
/// fabricate a different name.
fn strip_equals_separator(desc: &str) -> &str {
    match desc.strip_prefix('=') {
        Some(rest) if rest.starts_with(|c: char| c.is_ascii_whitespace()) => rest.trim_start(),
        _ => desc,
    }
}

/// Fallback for a flag row with no aligned column at all, that separates
/// spec from description with a **colon** instead of whitespace, `=`, or a
/// dash: `sg_emc_trespass`'s real `-d : output debug` (spaced) and
/// `-hr: Set Honor Reservation bit` / `-V: print version string then exit`
/// (glued, no space at all between the spec and the colon). Before this
/// fallback none of those rows have a 2+ space gap or a lone `=` token
/// anywhere, so the whole row fell into `grammar::parse_flag_spec`, whose
/// ` VALUE` arm took the colon (or the colon-terminated remainder of the
/// spec) as a required value — `-d` acquired the fabricated value `":"`,
/// and `-hr` was split into a fabricated `-h` carrying the value `"r:"`,
/// destroying the genuine two-character switch entirely.
///
/// **Only ever consulted when [`find_multi_space_gap`] and
/// [`find_equals_separator_gap`] both found no gap anywhere in the
/// line**, so any already-aligned table (or one using `=` as its
/// separator) keeps taking that path completely unchanged; this function
/// never even runs for it.
///
/// This is [`find_equals_separator_gap`]'s direct sibling, and it has to be
/// **tighter**, not looser: a colon is far more common in ordinary English
/// prose than a bare `=` ever is (`"(default: long)"`, `"Notes: see
/// below"`, `12:30`, `http://…`), so admitting every colon in the line
/// would read straight through a real sentence and invent a split inside
/// it. Two shapes are recognized, both requiring every token scanned
/// before them to satisfy [`is_value_spec_token`] — exactly the guard
/// [`find_equals_separator_gap`] uses, so prose can never be reached:
///
/// 1. **Spaced**: a lone `:` token, standing alone between whitespace —
///    `-d : output debug`. Identical in shape to the equals rule's lone
///    `=` token.
/// 2. **Glued**: a token whose *last* character is `:` and whose remainder
///    (the token with that trailing colon stripped) is itself
///    [`is_value_spec_token`]-shaped — `-hr:`, `-V:`, `-o,` followed by
///    `--output:`. This is the shape the equals rule has no analogue for,
///    because `=VALUE` never gets glued onto the *end* of a spelling the
///    way a colon separator does, and it is the riskier of the two: a
///    prose word that happens to end a sentence right before an inline
///    colon (`Options:`, `Notes:`) also "ends with a colon", so the
///    stripped remainder is checked against the very same predicate that
///    keeps the scan out of prose in the first place — `"Options"` is not
///    [`is_value_spec_token`]-shaped (no dash, no digit, no bracket, not
///    all-uppercase) and is refused, while `"-hr"` and `"-V"` are (they
///    start with `-`) and are accepted. A token that merely *contains* a
///    colon without ending in one (`<hh:mm>`, `0:30`, `http://host:port`)
///    never reaches either arm: it isn't `":"` and doesn't end with `:`,
///    so it is scanned past via the trailing [`is_value_spec_token`] check
///    like any other spec-shaped token, exactly as `find_equals_separator_
///    gap` scans past `<file>` on its way to a real `=`.
///
/// Both shapes require at least one non-whitespace character after the
/// colon — an empty tail (`--flag:`) returns `None`, leaving today's
/// behaviour (no description) unchanged, the same requirement
/// [`find_equals_separator_gap`] makes of `=`.
///
/// Returns the byte offset of the colon character itself (never the
/// character after it), so [`split_at_column`] keeps the separator
/// attached to the description side and [`strip_colon_separator`] removes
/// it afterward — the same two-step [`strip_equals_separator`] already
/// uses for `=`.
fn find_colon_separator_gap(line: &str) -> Option<usize> {
    if !line.trim_start().starts_with('-') {
        return None;
    }
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b' ' || bytes[i] == b'\t' {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i] != b' ' && bytes[i] != b'\t' {
            i += 1;
        }
        // `start` and `i` only ever land on ASCII space/tab boundaries or
        // the ends of the string, so neither can fall inside a multi-byte
        // character — `get` rather than `[..]` regardless (AGENTS.md §2).
        let token = line.get(start..i)?;
        if token == ":" {
            let tail_has_content = line.get(i..).is_some_and(|rest| !rest.trim().is_empty());
            return if tail_has_content { Some(start) } else { None };
        }
        if let Some(head) = token.strip_suffix(':') {
            // A bare `:` with nothing before it in the same token is
            // already handled above; an empty head here would only occur
            // if `token` were exactly `":"`, which can't reach this branch.
            if head.is_empty() || !is_value_spec_token(head) {
                return None;
            }
            let colon_offset = start + head.len();
            let tail_has_content = line.get(i..).is_some_and(|rest| !rest.trim().is_empty());
            return if tail_has_content {
                Some(colon_offset)
            } else {
                None
            };
        }
        if !is_value_spec_token(token) {
            return None;
        }
    }
    None
}

/// Strip a flag row description's leading `: ` separator token, the colon
/// analog of [`strip_equals_separator`]: if `desc` starts with `:`
/// immediately followed by ASCII whitespace, drop the `:` and that
/// whitespace; otherwise return it unchanged.
///
/// [`find_colon_separator_gap`] deliberately returns the `:` character's
/// own offset (so [`split_at_column`] keeps the separator attached to
/// whichever side already worked), leaving it on the front of the
/// description; this removes it.
///
/// Only the separator token is removed. A second `:` inside the
/// description proper is text, not punctuation, and is left alone:
/// `sg_emc_trespass`'s own `Send Short Trespass Command page (default:
/// long) (for FC series)` is untouched — it never had a leading `: ` to
/// begin with, since its separator was the spaced lone-`:` token, already
/// consumed by [`split_at_column`] on the spec side.
fn strip_colon_separator(desc: &str) -> &str {
    match desc.strip_prefix(':') {
        Some(rest) if rest.starts_with(|c: char| c.is_ascii_whitespace()) => rest.trim_start(),
        _ => desc,
    }
}

fn find_placeholder_boundary_gap(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b != b'>' && b != b']' {
            continue;
        }
        let after = i + 1;
        if bytes.get(after) != Some(&b' ') {
            continue;
        }
        // Exactly one space: a second space here would mean
        // `find_multi_space_gap` already matched above, so reaching this
        // function at all guarantees no run of 2+ spaces exists anywhere
        // in `line` — no need to re-check that this isn't a longer run.
        let desc_start = after + 1;
        if matches!(bytes.get(desc_start), Some(c) if *c != b' ') {
            return Some(after);
        }
    }
    None
}

fn split_at_column(line: &str, col: Option<usize>) -> (&str, String) {
    match col {
        Some(col) if col < line.len() => {
            let spec = line[..col].trim_end();
            let desc = line[col..].trim_start().to_string();
            (spec, desc)
        }
        _ => (line.trim(), String::new()),
    }
}

/// Usage-synopsis tokens that stand in for the tool's **own option list**
/// rather than naming an operand, matched case-insensitively after the
/// notation wrapper (`<>`, `[]`, `...`) is stripped.
///
/// A synopsis says where the flags go the same way it says where the
/// operands go, and it says it with a word: `tar [OPTION...] [FILE]...`,
/// `pkgconf [OPTIONS] [LIBRARIES]`, `dpkg-statoverride [<option> ...]
/// <command>`, `vim [arguments] [file ..]`. Only the *second* token in each
/// of those pairs is an argument the user supplies; the first is the
/// synopsis pointing at its own options table. Reading it as a positional
/// invents an operand no tool has — the fabrication class spec §7 Tier B
/// forbids, arrived at by a plausible-looking rule rather than by
/// mis-parsing anything.
///
/// **The anchor case is `vim`,** confirmed with the maintainer on
/// 2026-08-13: in `Usage: vim [arguments] [file ..]`, `[file ..]` is a real
/// variadic operand and `[arguments]` is the flag list. Today `arguments`
/// is skipped only incidentally — [`extract_positionals`] happens not to
/// accept bare lowercase words — so widening that rule for any reason at
/// all would silently start fabricating it. Naming the shape here makes the
/// exclusion survive such a change instead of depending on it not happening.
///
/// **`args`/`arg` are deliberately absent.** `git`'s `[<args>]` (the
/// arguments forwarded to the chosen subcommand) and every `sh -c
/// command_string [args]`-shaped synopsis use it as a genuine operand, so
/// excluding it would delete real structure to prevent a defect it does not
/// have. The list holds only words that name an option list and nothing
/// else.
pub(super) const OPTION_LIST_PLACEHOLDERS: &[&str] =
    &["option", "options", "flag", "flags", "arguments"];

/// True when `name` (already unwrapped from its notation) is one of
/// [`OPTION_LIST_PLACEHOLDERS`].
pub fn is_option_list_placeholder(name: &str) -> bool {
    OPTION_LIST_PLACEHOLDERS
        .iter()
        .any(|p| name.eq_ignore_ascii_case(p))
}

/// Pull placeholder tokens (`<value>`, bare `UPPERCASE` words not preceded
/// by `-`) out of usage lines as positionals. Best-effort: usage-line
/// grammar is genuinely varied (docopt-style `[OPTIONS]`, `<required>`,
/// `...`, `|`, `{a|b|c}`), so this recognizes the common placeholder
/// shapes rather than fully parsing the grammar.
///
/// What it recognizes is *inference from notation*, so it stays narrow, and
/// [`OPTION_LIST_PLACEHOLDERS`] carves out the one family of tokens whose
/// notation is indistinguishable from an operand's while its meaning is the
/// opposite. The declarative counterpart — a framework's own positional
/// block, which needs no inference at all — is
/// [`FrameworkProfile::positional_heading_markers`] and
/// [`emit_declared_positionals`].
/// The byte-offsets into `usage_lines` of every physical line that was
/// folded into the first recovered usage *entry* carrying real invocation
/// content — skipping a bare `Usage:`/`or:` label with nothing after the
/// colon on its own line (util-linux's `renice`: entry 0 is the literal
/// string `Usage:`, entry 1 — one physical line — is `renice
/// [-n|--priority|--relative] <priority> ...`), and following a wrapped
/// entry across every physical line it spans (`sg_sanitize`'s five-line
/// synopsis is one entry, entry 0, so all five lines qualify here).
///
/// `line_entry_index[i]` is the `entries` index physical line `i` was
/// folded into ([`parse_with_profile`]'s own bookkeeping, threaded through
/// because `extract_positionals` only ever sees the flattened
/// `usage_lines`, not which entry each line belongs to).
///
/// This is [`extract_positionals`]'s anchor for *where* the self-closed-
/// bracket-group refinement below is allowed to run — see that function's
/// own doc comment for why the refinement is scoped to exactly these lines
/// rather than every line.
fn primary_synopsis_lines(
    entries: &[String],
    line_entry_index: &[usize],
    line_count: usize,
) -> std::collections::HashSet<usize> {
    let primary_entry = entries.iter().position(|e| {
        let bare_label = e.trim().trim_end_matches(':').eq_ignore_ascii_case("usage")
            || e.trim().trim_end_matches(':').eq_ignore_ascii_case("or");
        !bare_label
    });
    let Some(primary_entry) = primary_entry else {
        return std::collections::HashSet::new();
    };
    (0..line_count)
        .filter(|&i| line_entry_index.get(i) == Some(&primary_entry))
        .collect()
}

/// `usage_lines`: every physical line of the recovered usage block, in
/// source order. `primary_lines`: [`primary_synopsis_lines`]'s pick of
/// which of them (by index) make up the tool's primary invocation form —
/// the self-closed-bracket-group refinement below only ever runs on one of
/// those; see its own comment for why.
fn extract_positionals(
    usage_lines: &[String],
    primary_lines: std::collections::HashSet<usize>,
) -> Vec<Positional> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (line_idx, line) in usage_lines.iter().enumerate() {
        // A value-shaped token immediately following a bare flag token
        // (`-C <path>`, `-c <name>=<value>`, argparse's `--config FILE`)
        // is that flag's *argument*, not a positional — a property of
        // usage-synopsis notation generally, true for every framework that
        // writes an option's value right after it rather than gluing it on
        // with `=`. `prev_cleaned` tracks the immediately preceding
        // token's cleaned spelling so the loop below can tell the two
        // apart; it resets every physical line, since a usage line's
        // tokens never continue onto the next one.
        let mut prev_cleaned: Option<&str> = None;
        // Whether the *immediately preceding raw token* was itself a
        // complete, self-closed bracket group (`[-v]`, `[-h]`) — opened
        // and closed with nothing else inside. A flag written that way has
        // already said everything about itself the notation can say; it is
        // never still waiting on a following token the way a *still-open*
        // group (`[-C`, expecting `<path>]` next) or a bare flag with no
        // brackets at all (`-C`, expecting `<path>` next) is.
        //
        // `sg_emc_trespass`'s own synopsis is the case that forced this:
        // `[-d] [-hr] [-s] [-V] DEVICE` closes `[-V]` completely before
        // `DEVICE` ever appears, so `DEVICE` is a real positional, not
        // `-V`'s argument — but the untracked version of this rule stripped
        // brackets off both ends of every token alike, so `[-v]` and a
        // genuinely value-expecting `-C` looked identical, and `DEVICE`
        // was silently swallowed as a fabricated flag argument (dropped
        // outright, no `positionals` entry at all, on a tool with no
        // declared positional block to fall back on).
        //
        // **Scoped to [`primary_synopsis_lines`], deliberately.**
        // A fleet sweep applying this fleet-wide moved two other shapes
        // this fix is not entitled to claim, both real:
        //
        // 1. A *later* alternate invocation form that repeats the tool's
        //    own name (`jps`'s second line, `jps [-q] [-mlvV] [<hostid>]`,
        //    under a first line that already carried its own content —
        //    `Usage: jps [--help]`). `xtask`'s existence oracle attests
        //    operands only from a line it recognizes as synopsis grammar,
        //    and it does not (yet) read a same-name repeat under that
        //    shape as one — a real, narrower gap than this fix owns.
        // 2. An **unlabelled** synopsis (no `Usage:`/`or:` anywhere in the
        //    document at all — `lvreduce`'s bare `lvreduce -L|--size
        //    [-]Size[m|UNIT] LV`, `dbus-cleanup-sockets`'s bare
        //    `dbus-cleanup-sockets [--version] [--help] <socketdir>`). The
        //    oracle's synopsis scanner does not read this convention at
        //    all yet, labelled or not, so *any* operand recovered from it
        //    — by this fix or any other — currently reports as invented.
        //
        // Both were measured on a full-`PATH` sweep (9 tools) and are
        // deliberately left as they were before this fix rather than
        // shipped as new false alarms in `xtask`'s own oracle; the primary
        // line is the one shape the oracle already attests correctly, and
        // it is also every real case this fix was written for
        // (`sg_emc_trespass`, `scsi_ready`'s whole `sg3-utils` family,
        // `lzgrep`/`xzgrep`, and `renice`'s own primary line, which is
        // entry 1 there, not entry 0 — see [`primary_synopsis_lines`]).
        let self_closed_recovery_applies = primary_lines.contains(&line_idx);
        let mut prev_was_self_closed_group = false;
        for token in line.split_whitespace() {
            let cleaned = token.trim_matches(|c| c == '[' || c == ']' || c == '.');
            // A flag already carrying its value inline (`--git-dir=<path>`)
            // has an `=` in `cleaned` and does not expect a following
            // token; a bare flag (`-C`, `-Zscript`) does — unless it was
            // already closed as its own complete bracket group, on the
            // one line this refinement is scoped to.
            let consumed_by_prior_flag = prev_cleaned
                .is_some_and(|p| p.starts_with('-') && !p.contains('='))
                && !(self_closed_recovery_applies && prev_was_self_closed_group);
            prev_cleaned = Some(cleaned);
            prev_was_self_closed_group = token.starts_with('[') && token.ends_with(']');

            if cleaned.starts_with('-') || consumed_by_prior_flag {
                continue;
            }
            let (name, variadic) = if let Some(stripped) = cleaned.strip_prefix('<') {
                // The *nearest* closing `>`, not the outermost one:
                // `<name>=<value>` (git's `-c <name>=<value>`, when not
                // already excluded above as a flag's own argument) must
                // yield `name`, not `name>=<value` from stripping only the
                // token's very last `>`.
                match stripped.find('>') {
                    Some(end) => (stripped[..end].to_string(), token.ends_with("...")),
                    None => continue,
                }
            } else if cleaned.chars().all(|c| c.is_uppercase() || c == '_') && cleaned.len() > 1 {
                (cleaned.to_string(), token.ends_with("..."))
            } else {
                continue;
            };
            if name.is_empty() || is_option_list_placeholder(&name) || !seen.insert(name.clone()) {
                continue;
            }
            let required = !token.contains('[') && !line.contains(&format!("[{token}"));
            out.push(Positional {
                name,
                required,
                variadic,
                description: None,
                provenance: Provenance::single(Source::HelpText),
            });
        }
    }
    out
}

/// Extract flag spellings from a usage-synopsis block (spec [M-15]:
/// "378 of 1,895 `ok` tools carry no flags at all", because usage-only
/// options — `git --help`'s `[-p | --paginate | -P | --no-pager]` and
/// friends — were never mined at all; [`extract_positionals`] (above)
/// reads the same block for positionals only, and nothing else reads it
/// for anything.
///
/// **The anti-fabrication property this relies on: a synopsis token
/// becomes a flag only if it starts with `-`.** That single character
/// class is the whole guard — there is no heading to misjudge, no
/// column-alignment ambiguity, no bare-word block that might be prose
/// (the failure mode [M-10] came in through four different ways in the
/// section-block scanner above). Prose cannot enter through a `-` prefix,
/// so this stays resistant to [M-10] by construction. Do not relax it to
/// recognize more shapes; spec §7 Tier B's rule is unconditional: never
/// fabricate.
///
/// Flags recovered here carry **no description** — a usage line documents
/// spellings and value shapes, never prose, and inventing one (by copying
/// the usage line's own text, or a neighbouring flag's description) is
/// exactly the fabrication spec §7 Tier B forbids. Reconciling a
/// same-spelling flag that *does* have a description (from an `Options:`-
/// style block elsewhere in the same output) is [`parse_with_profile`]'s
/// job, via [`flag_spelling_already_present`] — see that function's doc
/// comment for why a duplicate is *dropped* rather than merged.
fn extract_usage_flags(usage_lines: &[String]) -> Vec<Flag> {
    let mut out: Vec<Flag> = Vec::new();
    // Running depth of an open parenthesized alternation group (LVM's
    // "for options listed in parentheses, any one is required" convention),
    // tracked over these same physical lines the same way `parse_body`'s
    // own usage-block loop already tracked it while collecting them
    // (`grammar::paren_depth_delta`) — re-derived here rather than passed
    // in because `usage_lines` alone determines the same open/close
    // boundaries deterministically. Needed because a member row routinely
    // opens with `-` itself (`-p|--maxphysicalvolumes Number,`), which must
    // be read via `paren_alternation_member_content`, not the ordinary
    // segment walk below — that walk has no notion of a comma-terminated
    // alternative and would mis-tokenize it (`,` is not a recognized
    // separator anywhere in `usage_segments`).
    let mut paren_group_depth: i32 = 0;
    for line in usage_lines {
        let trimmed = line.trim();
        if paren_group_depth > 0 || looks_like_paren_alternation_open(trimmed) {
            paren_group_depth += paren_depth_delta(trimmed);
            if paren_group_depth < 0 {
                paren_group_depth = 0;
            }
            if let Some(content) = paren_alternation_member_content(trimmed) {
                push_usage_flag(&mut out, parse_flag_spec(content));
            }
            continue;
        }
        // A whole line that is one docopt bracket-group flag row (LVM's
        // `[ -A|--autobackup y|n ]`) is read directly by
        // `bracket_flag_row_content` + `parse_flag_spec`, never by the
        // generic segment walk below. `usage_segments` splits a group's
        // content on every top-level `|` unconditionally
        // (`split_top_level_pipe`), which is right for an alternation of
        // whole flags (`{-v|--version}`) but wrong here: it would read
        // `-A|--autobackup y|n` as three alternatives — `-A`,
        // `--autobackup y`, `n` — losing `--autobackup`'s real value
        // `y|n` down to just `y`. `parse_flag_spec` already resolves this
        // exact alias-vs-value ambiguity correctly (see
        // `bracket_flag_row_content`'s own doc comment), so this row
        // shape is diverted to it before the segment walk ever sees it.
        if let Some(content) = bracket_flag_row_content(line.trim()) {
            push_usage_flag(&mut out, parse_flag_spec(content));
            continue;
        }
        let segments = usage_segments(line);
        let mut seg_idx = 0usize;
        while seg_idx < segments.len() {
            if out.len() >= MAX_RECOVERED_ENTRIES {
                return out;
            }
            let segment = segments[seg_idx].clone();
            seg_idx += 1;
            match segment {
                UsageSegment::Group(members) => {
                    let mut flaggy: Vec<&str> = Vec::new();
                    for m in members {
                        if m.starts_with('-') {
                            flaggy.push(m);
                            continue;
                        }
                        // A member that is *itself* a delimited alternation
                        // of flag spellings, optionally followed by one
                        // shared operand — `xfs_io`'s `[[-c|-C] cmd]...`,
                        // whose outer group has exactly this one member.
                        // Neither `-c` nor `-C` reached the tree at all
                        // before this arm existed: the member does not start
                        // with `-`, so the filter above dropped it whole.
                        for spec in nested_alternation_specs(m) {
                            if out.len() >= MAX_RECOVERED_ENTRIES {
                                return out;
                            }
                            push_usage_flag(&mut out, spec);
                        }
                    }
                    // spec [M-15]'s conservative-pairing rule: within one
                    // bracket group, pair a short with a long only when the
                    // group has exactly one of each. `[-v | --version]`
                    // qualifies; `[-p | --paginate | -P | --no-pager]`
                    // (four alternatives) does not, and every spelling in
                    // it is emitted on its own rather than guessing which
                    // short goes with which long. A wrong pairing asserts a
                    // false equivalence a user would act on — worse than an
                    // unpaired entry, which is merely incomplete.
                    // A bundle is never one half of an alternation pair:
                    // `pair_short_and_long` would happily take `-2CDlNuVv`
                    // as the "short" side (it has a short and no long) and
                    // silently discard seven flags, so the cluster question
                    // is asked first.
                    if flaggy.len() == 2 && flaggy.iter().all(|m| parse_bundled_shorts(m).is_none())
                    {
                        let a = parse_flag_spec(flaggy[0]);
                        let b = parse_flag_spec(flaggy[1]);
                        if let Some(paired) = pair_short_and_long(a, b) {
                            push_usage_flag(&mut out, paired);
                            continue;
                        }
                    }
                    for m in flaggy {
                        push_usage_token(&mut out, m);
                    }
                }
                UsageSegment::Bare(tok) => {
                    if tok.starts_with('-') {
                        // A mandatory flag some tool's synopsis writes
                        // unbracketed (`ssh-keygen -D pkcs11`, `-M generate`,
                        // `-I certificate_identity`) is two bare tokens in a
                        // row: the flag, then its own required value with no
                        // separating group at all. `usage_segments` already
                        // pairs a flag with its value when a `[...]` group
                        // holds both (`bracket_flag_row_content`, the `Group`
                        // arm above); outside a group each bare token used to
                        // stand alone, so the flag's own value token was read
                        // as an unrelated, silently-dropped bare word and the
                        // flag came out looking like a boolean it isn't.
                        //
                        // Attaching is refused when `tok` is itself a
                        // recognized bundle of single-character switches
                        // (`parse_bundled_shorts`) — a bundle's members are
                        // booleans by construction, and the word that
                        // follows one in a synopsis is the next independent
                        // token, never a shared value. It is also refused
                        // when the next segment is missing, empty, or is
                        // itself flag-shaped (`-k -f krl_file`: `-k` stays a
                        // bare boolean because what follows it is another
                        // flag, not a value). It is also refused when the
                        // next word opens a parenthetical aside —
                        // `iptables`'s own `iptables -h (print this help
                        // information)` measured this exactly: `-h` takes no
                        // argument at all, and `(print` is the first word of
                        // a parenthetical explanation, not a value. No real
                        // value-placeholder convention this grammar
                        // recognizes anywhere else opens with `(` (`<value>`,
                        // `[value]`, `{a|b}`, or a bare word are the whole
                        // set), so refusing it costs no real recall.
                        let attach_value = parse_bundled_shorts(tok).is_none()
                            && matches!(
                                segments.get(seg_idx),
                                Some(UsageSegment::Bare(value))
                                    if !value.is_empty()
                                        && !value.starts_with('-')
                                        && !value.starts_with('(')
                            );
                        if attach_value {
                            if let Some(UsageSegment::Bare(value)) = segments.get(seg_idx) {
                                push_usage_flag(
                                    &mut out,
                                    parse_flag_spec(&format!("{tok} {value}")),
                                );
                            }
                            seg_idx += 1;
                            continue;
                        }
                        push_usage_token(&mut out, tok);
                    }
                }
            }
        }
    }
    out
}

/// The fewest alternatives a *nested* group must carry before
/// [`nested_alternation_specs`] will read it as one.
///
/// Two, and unlike `grammar::looks_like_flag_start`'s floor of one this is
/// not a judgment call about ambiguity — it is a statement about who already
/// owns the shape. A one-member nested group is `[[-v] file]`, an ordinary
/// optional flag inside an outer group, and [`usage_segments`] plus the
/// pairing rule above already read it correctly. Claiming it here would put
/// two rules on one shape for no recall at all.
const MIN_NESTED_ALTERNATIVES: usize = 2;

/// Read one member of a usage-synopsis group as a nested alternation of
/// flag spellings sharing a single operand — `xfs_io`'s `[[-c|-C] cmd]...`,
/// where the outer group's only member is the string `[-c|-C] cmd`.
///
/// Returns one [`FlagSpec`] per alternative, each carrying the shared
/// operand as a required value, or the *paired* single spec when the
/// alternatives are exactly one short and one long (spec [M-15]'s
/// conservative-pairing rule, the same one the caller applies to a flat
/// group — `{-i|--input} <file>` and `[-i|--input] <file>` must not disagree
/// about whether they name one flag or two). Empty when the member is not
/// this shape.
///
/// **The operand is refused unless it is one clean token.** `cmd` is taken;
/// anything with a second word, or that is itself flag-shaped, yields no
/// value rather than a guessed one — the alternatives are still emitted,
/// because their spellings are in the tool's own text either way, and
/// dropping real flags to avoid an unsure value spec would be the wrong
/// trade in the other direction. What is never done is inventing a value
/// name out of text this function could not read.
fn nested_alternation_specs(member: &str) -> Vec<FlagSpec> {
    let Some(alt) = parse_flag_alternation(member) else {
        return Vec::new();
    };
    if alt.members.len() < MIN_NESTED_ALTERNATIVES {
        return Vec::new();
    }
    let shared_value = shared_operand(&alt.rest);
    let specs: Vec<FlagSpec> = alt
        .members
        .iter()
        .map(|m| {
            let mut spec = parse_flag_spec(m);
            if let Some(value) = &shared_value {
                spec.value_name = Some(value.clone());
                spec.value_kind = ValueKind::Required;
            }
            spec
        })
        .collect();
    if let [a, b] = specs.as_slice() {
        if let Some(paired) = pair_short_and_long(a.clone(), b.clone()) {
            return vec![paired];
        }
    }
    specs
}

/// The operand a nested alternation's members share, when the text after
/// the group is one clean value token and nothing else.
///
/// `None` for empty text, for anything with a second word, and for a
/// flag-shaped token (`[[-a|-b] -c]` is not an operand, whatever it is).
fn shared_operand(rest: &str) -> Option<String> {
    let trimmed = rest.trim();
    if trimmed.is_empty() || trimmed.split_whitespace().nth(1).is_some() {
        return None;
    }
    if is_flag_shaped(trimmed) {
        return None;
    }
    Some(trimmed.to_string())
}

/// True if `candidate` shares a spelling — its short letter, or its long
/// name — with any flag already in `existing`.
///
/// **Deliberately loose in `existing`'s favor.** `candidate` is always a
/// usage-derived flag here (see the one call site in [`parse_with_profile`]),
/// so it never has a description to lose; matching on *either* spelling
/// (not requiring the same combination [`mandible_core::merge::flag_identity`]
/// would key on, which prefers a long name over a short one) is what
/// catches `arptables`' `--insert, -I` row against a bare `-I` mentioned
/// standalone elsewhere in the synopsis — a real duplicate that a stricter,
/// identity-string equality check would miss and add a second, spellingless
/// (no long, no description) time. The cost of the looser match is a
/// forgone enrichment (a usage flag's value shape is never folded into an
/// existing entry that lacks one) in exchange for the guarantee this
/// function exists to provide: an existing flag, right or wrong, is never
/// altered by anything found here — only ever left alone or joined by a
/// new one.
fn flag_spelling_already_present(candidate: &Flag, existing: &[Flag]) -> bool {
    existing.iter().any(|f| {
        (candidate.long.is_some() && f.long == candidate.long)
            || (candidate.short.is_some() && f.short == candidate.short)
    })
}

/// Push the flag(s) one synopsis token names: either a bundle of
/// single-character boolean switches, one [`Flag`] per member, or — for
/// every other shape — the single flag [`parse_flag_spec`] reads.
///
/// The bundle question is asked *here*, on the synopsis path only, and
/// never inside [`parse_flag_spec`]: an option-*table* row of the identical
/// shape is the GCC/Clang single-dash convention (`-fdump-scos`, `-Wall`,
/// `-Idirectory`), where the glued text genuinely is a value — thousands of
/// correct parses fleet-wide that splitting would destroy. Only a usage
/// synopsis writes a getopt cluster, so only this caller asks. See
/// [`parse_bundled_shorts`] for the five conditions and the two families
/// (single-dash long options, repeated-character flags) that share the
/// cluster's structural fingerprint and must not be split.
///
/// Members are emitted as bare booleans — no value, no description — which
/// is what they are: `[-2CDlNuVv]` says `-2`, `-C`, `-D`, `-l`, `-N`, `-u`,
/// `-V` and `-v` are eight switches and says nothing else about any of
/// them. Fabricating a description from the usage line's own text is the
/// same spec §7 Tier B violation [`extract_usage_flags`] forbids.
fn push_usage_token(out: &mut Vec<Flag>, token: &str) {
    if let Some(members) = parse_bundled_shorts(token) {
        for member in members {
            if out.len() >= MAX_RECOVERED_ENTRIES {
                return;
            }
            push_usage_flag(
                out,
                FlagSpec {
                    short: Some(member),
                    fully_consumed: true,
                    ..FlagSpec::default()
                },
            );
        }
        return;
    }
    push_usage_flag(out, parse_flag_spec(token));
}

/// Turn a [`FlagSpec`] into a [`Flag`] and push it, unless the spec
/// recognized nothing (`short`/`long` both `None` — a stray token like a
/// bare `-` or `--` option terminator). Mirrors `emit_flags`'s field
/// defaults exactly, except `group`/`description` are always `None`: see
/// [`extract_usage_flags`]'s doc comment for why a usage-derived flag must
/// never carry a description. Provenance is [`Source::HelpTextSynopsis`],
/// not the plain [`Source::HelpText`] `emit_flags` uses — same authority
/// (spec §4.4 is unaffected), but a distinct source so spec §13's
/// `pct_flags_with_text` can tell a structurally-undescribable flag apart from
/// one that merely wasn't described.
fn push_usage_flag(out: &mut Vec<Flag>, spec: FlagSpec) {
    if spec.short.is_none() && spec.long.is_none() {
        return;
    }
    out.push(Flag {
        short: spec.short,
        long: spec.long,
        value_name: spec.value_name,
        value_kind: spec.value_kind,
        choices: Vec::new(),
        repeatable: false,
        required: false,
        negatable: spec.negatable,
        single_dash: false,
        hidden: false,
        deprecated: None,
        inherited: false,
        group: None,
        description: None,
        default: None,
        env_var: None,
        provenance: Provenance::single(Source::HelpTextSynopsis),
    });
}

/// Pair a short-only and a long-only [`FlagSpec`] into one, or refuse
/// (`None`) if they are not exactly complementary (spec [M-15]'s
/// conservative pairing rule, applied by the caller to a bracket group
/// already known to have exactly one flaggy member of each kind).
///
/// Shape-similar to [`mandible_core::merge::pair_aliases`], but that
/// function pairs rows from the *same block* by matching description text
/// (two rows that happen to describe the same flag identically); nothing
/// here has a description to compare against, so the evidence is the
/// bracket group's own `|`-alternation instead, per spec's stated rule.
fn pair_short_and_long(a: FlagSpec, b: FlagSpec) -> Option<FlagSpec> {
    let (short_spec, long_spec) =
        if a.short.is_some() && a.long.is_none() && b.short.is_none() && b.long.is_some() {
            (a, b)
        } else if b.short.is_some() && b.long.is_none() && a.short.is_none() && a.long.is_some() {
            (b, a)
        } else {
            return None;
        };
    let long_had_value = long_spec.value_name.is_some();
    Some(FlagSpec {
        short: short_spec.short,
        long: long_spec.long,
        negatable: long_spec.negatable,
        value_kind: if long_had_value {
            long_spec.value_kind
        } else {
            short_spec.value_kind
        },
        value_name: long_spec.value_name.or(short_spec.value_name),
        fully_consumed: short_spec.fully_consumed && long_spec.fully_consumed,
    })
}

/// One token-level unit of a usage-synopsis line, as [`usage_segments`]
/// walks it: either a bracketed alternation group (spec [M-15]'s pairing
/// rule operates within one such group) or a bare token outside any
/// bracket.
#[derive(Clone)]
enum UsageSegment<'a> {
    /// The members of one top-level `[...]` group, already split on `|` at
    /// that group's own nesting depth — so `--exec-path[=<path>]`'s inner
    /// bracket (an optional value spec) is never mistaken for a second
    /// alternative.
    Group(Vec<&'a str>),
    /// A single top-level token outside any bracket (e.g. git's own
    /// `usage:`/`git` at the very start of the line, or a required flag
    /// some tool's synopsis writes unbracketed).
    Bare(&'a str),
}

/// Walk `line` into [`UsageSegment`]s.
///
/// Every substring boundary here comes from `char_indices`, never a raw
/// byte offset (`AGENTS.md`'s slicing rule) — safe even if a usage line
/// happens to carry a multi-byte character, with no separate UTF-8-
/// boundary reasoning required.
fn usage_segments(line: &str) -> Vec<UsageSegment<'_>> {
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let len = chars.len();
    let mut out = Vec::new();
    let mut idx = 0usize;
    while idx < len {
        let (byte_pos, c) = chars[idx];
        if c.is_whitespace() {
            idx += 1;
            continue;
        }
        // `{` opens a group on exactly the same terms as `[`. `eqn`'s
        // synopsis writes `usage: eqn {-v | --version}`, and while the
        // brackets-only version of this loop was running, the spaces around
        // that `|` split it into three bare tokens — `{-v` (discarded, it
        // does not start with `-`), `|`, and `--version}` (parsed as
        // `--version` carrying the literal value `"}"`). A brace group whose
        // members are *not* flag-shaped is unaffected: `{start|stop}` became
        // a bare token that was skipped before and becomes a group with no
        // flaggy member now, which `extract_usage_flags` skips just the same.
        if let Some(close) = group_close_delimiter(c) {
            if let Some((content_range, close_idx)) = matched_group(&chars, idx, c, close) {
                let content = &line[content_range.0..content_range.1];
                out.push(UsageSegment::Group(split_top_level_pipe(content)));
                idx = close_idx + 1;
                continue;
            }
        }
        let mut j = idx;
        while j < len && !chars[j].1.is_whitespace() {
            j += 1;
        }
        let end_byte = if j < len { chars[j].0 } else { line.len() };
        out.push(UsageSegment::Bare(&line[byte_pos..end_byte]));
        idx = j;
    }
    out
}

/// The closing delimiter that matches `c`, when `c` opens a synopsis group
/// at all. The two pairs a usage line uses for grouping; `<`/`>` is
/// deliberately absent, since it delimits a value placeholder and never a
/// group of alternatives.
fn group_close_delimiter(c: char) -> Option<char> {
    match c {
        '[' => Some(']'),
        '{' => Some('}'),
        _ => None,
    }
}

/// Find the byte range of the content strictly between `chars[open_idx]`
/// (an `open` delimiter) and its matching `close`, and the char-index of
/// that close — depth aware over that one pair, so
/// `[--exec-path[=<path>]]`'s inner `[...]` (an optional value spec on the
/// one alternative) is consumed as part of the outer group's content
/// instead of closing the group early, and `[[-c|-C] cmd]`'s inner `]` does
/// not end the outer group either. `None` when `open_idx`'s delimiter is
/// never closed (malformed input); the caller falls back to treating it as
/// an ordinary bare token.
fn matched_group(
    chars: &[(usize, char)],
    open_idx: usize,
    open: char,
    close: char,
) -> Option<((usize, usize), usize)> {
    let (open_byte, open_c) = chars[open_idx];
    let content_start = open_byte + open_c.len_utf8();
    let mut depth = 1i32;
    let mut j = open_idx + 1;
    while j < chars.len() {
        let (byte_pos, c) = chars[j];
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(((content_start, byte_pos), j));
            }
        }
        j += 1;
    }
    None
}

/// Split a group's content on `|` at that content's own nesting depth 0, so
/// a nested `[...]`/`{...}` (an optional value spec, or a value alternation,
/// on one of the alternatives) is never itself split on. Empty fragments (a
/// stray leading/trailing `|`, or `||`) are dropped.
///
/// Both delimiter pairs count toward the depth. Counting only brackets read
/// `[--color={always|never}]` as the two alternatives `--color={always` and
/// `never}`, emitting a flag whose value was the fragment `{always`.
fn split_top_level_pipe(content: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in content.char_indices() {
        match c {
            '[' | '{' => depth += 1,
            ']' | '}' => depth -= 1,
            '|' if depth == 0 => {
                out.push(content[start..i].trim());
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(content[start..].trim());
    out.retain(|s| !s.is_empty());
    out
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
fn backfill_prose_paragraph_descriptions(flags: &mut [Flag], lines: &[&str]) {
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
/// (`-print-sysroot`) against its `long` when [`Flag::single_dash`] says
/// that is how the tool spells it.
fn flag_answers_to_spelling(flag: &Flag, spelling: &str) -> bool {
    if let Some(long) = spelling.strip_prefix("--") {
        return !long.is_empty() && flag.long.as_deref() == Some(long) && !flag.single_dash;
    }
    let Some(rest) = spelling.strip_prefix('-') else {
        return false;
    };
    if rest.is_empty() {
        return false;
    }
    let mut chars = rest.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if flag.short == Some(c) {
            return true;
        }
    }
    flag.single_dash && flag.long.as_deref() == Some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- multi-stanza unlabelled synopsis (fix/multi-stanza-synopsis) ---

    /// `vgck --updatemetadata` — a *second* stanza, past the blank line
    /// this fix teaches the usage-block loop to look beyond — is now
    /// present as its own usage entry and its own flag, and the stanza's
    /// own prose head ("Rewrite VG metadata...") lands in neither.
    /// Reuses the existing [`VGCK_HELP`] fixture above (the real capture
    /// at `audit/queue-captures/vgck/0.stdout`) rather than a second copy.
    #[test]
    fn vgck_recovers_the_second_stanza_and_its_updatemetadata_flag() {
        let parsed = parse_with_profile(VGCK_HELP, None, Some("vgck"));
        assert_eq!(parsed.usage.len(), 2, "usage: {:?}", parsed.usage);
        assert!(parsed.usage[1].contains("--updatemetadata"));
        let updatemetadata = flag_named(&parsed, "updatemetadata");
        assert_eq!(updatemetadata.value_name.as_deref(), Some("VG"));
        for u in &parsed.usage {
            assert!(!u.contains("Rewrite VG metadata"));
        }
    }

    /// A tool with a genuine `Usage:` label must be completely unaffected
    /// by the multi-stanza continuation: `git`'s own wrapped hanging-indent
    /// synopsis, followed by an unrelated blank-line-separated paragraph
    /// that happens to open with `git` again, must never be read as a
    /// second usage entry.
    #[test]
    fn labelled_usage_block_does_not_reopen_on_a_later_blank_line() {
        let help = "Usage: git [--version] [--help] <command> [<args>]\n\n\
                     git clone is used to clone repositories.\n\
                     git clone [--bare] <repo>\n\
                     \t[--depth <n>]\n";
        let parsed = parse_with_profile(help, None, Some("git"));
        assert_eq!(parsed.usage.len(), 1, "usage: {:?}", parsed.usage);
    }

    /// A headingless invocation table (`corepack`'s own commander/oclif
    /// style — one `<tool> <subcommand> [flags] ...` row per blank-line-
    /// separated stanza, each followed by its own subcommand description)
    /// must not be reopened into more "usage": that would demote a real
    /// subcommand `scan_headingless_invocation_table` already recovers
    /// into fabricated synopsis text. The `...` ending each row defeats
    /// `is_prose_sentence`'s period check the same way a real LVM
    /// continuation's own remainder would, so this pins the guard that
    /// keeps the two shapes apart at the continuation site specifically
    /// (`looks_like_stanza_continuation_head`, not the wider
    /// `looks_like_unlabeled_synopsis_line`).
    #[test]
    fn headingless_invocation_table_stanzas_are_not_reopened_as_usage() {
        let help = "Corepack - 0.34.6\n\n  $ corepack <command>\n\nGeneral commands\n\n  \
                     corepack disable [--install-directory #0] ...\n    Remove the shims\n\n  \
                     corepack enable [--install-directory #0] ...\n    Add the shims\n";
        let parsed = parse_with_profile(help, None, Some("corepack"));
        // Whatever the pre-existing entry-point behavior does with the
        // first row, the second must never be folded into it as more
        // usage text — this fix must not widen that.
        assert!(
            parsed.usage.iter().all(|u| !u.contains("enable")),
            "usage: {:?}",
            parsed.usage
        );
    }

    /// A stanza whose own description wraps across more than one physical
    /// line (`pydoc3`'s `-p`/`-b`/`-w` forms) must not be admitted at all —
    /// the shared continuation loop only recognizes a *single* physical
    /// line as prose to drop, so an interior wrapped line would otherwise
    /// be silently read as more usage notation and mined for fabricated
    /// positionals (`HTTP`, `HTML` were invented from exactly this before
    /// the guard existed). A single-line description (`-k`, `-n`) is
    /// unaffected.
    #[test]
    fn stanza_with_wrapped_multi_line_description_is_refused() {
        let help = "pydoc - the Python documentation tool\n\npydoc3 <name> ...\n    Show text documentation on something.\n\npydoc3 -k <keyword>\n    Search for a keyword in the synopsis lines of all available modules.\n\npydoc3 -p <port>\n    Start an HTTP server on the given port on the local machine.  Port\n    number 0 can be used to get an arbitrary unused port.\n";
        let parsed = parse_with_profile(help, None, Some("pydoc3"));
        assert!(
            parsed.flags.iter().any(|f| f.short == Some('k')),
            "flags: {:?}",
            parsed.flags
        );
        assert!(
            !parsed.flags.iter().any(|f| f.short == Some('p')),
            "flags: {:?}",
            parsed.flags
        );
        assert!(
            !parsed
                .positionals
                .iter()
                .any(|p| p.name == "HTTP" || p.name == "HTML"),
            "positionals: {:?}",
            parsed.positionals
        );
    }

    // --- the parenthesized alternation stanza ---------------------------
    //
    // `headingless_invocation_table_stanzas_are_not_reopened_as_usage`
    // (above) and `stanza_with_wrapped_multi_line_description_is_refused`
    // (above) already pin the two hazards fix/multi-stanza-synopsis
    // guarded (`corepack`'s headingless invocation table, `pydoc3`'s own
    // multi-physical-line stanza descriptions) — both fixtures contain no
    // `(` at all, so `paren_group_depth` never leaves zero for either one
    // and this fix's own code path is never reached by them; re-run
    // unchanged by `cargo nextest run --workspace`, not duplicated here.

    /// The positive case: a bare `vgchange`-shaped synopsis whose first
    /// continuation opens a multi-line `(` group, one flag per physical
    /// line, closed by `)` on the last member's own line. Every member is
    /// recovered with a clean value (no leftover `,` from the shape's own
    /// separator) and the alias/value split `bracket_flag_row_content`
    /// already resolves for the bracket-row shape.
    #[test]
    fn paren_alternation_stanza_recovers_every_member_with_clean_values() {
        let help = "tool\n\
                     \t( -a|--aaa Number,\n\
                     \t  -b|--bbb,\n\
                     \t     --ccc y|n )\n";
        let parsed = parse_with_profile(help, None, Some("tool"));
        assert_eq!(parsed.usage.len(), 1, "usage: {:?}", parsed.usage);

        let aaa = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("aaa"))
            .unwrap_or_else(|| panic!("flags: {:?}", parsed.flags));
        assert_eq!(aaa.short, Some('a'));
        assert_eq!(aaa.value_name.as_deref(), Some("Number"));

        let bbb = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("bbb"))
            .unwrap_or_else(|| panic!("flags: {:?}", parsed.flags));
        assert_eq!(bbb.short, Some('b'));
        assert_eq!(bbb.value_name, None);

        let ccc = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("ccc"))
            .unwrap_or_else(|| panic!("flags: {:?}", parsed.flags));
        assert_eq!(ccc.value_name.as_deref(), Some("y|n"));

        for f in &parsed.flags {
            if let Some(v) = &f.value_name {
                assert!(
                    !v.ends_with(','),
                    "flag {:?} kept the shape's own trailing comma",
                    f.long
                );
            }
        }
    }

    /// `vgchange`'s own specimen wrinkle: the group's trailing bracket-row
    /// flag list sits after a blank line separating it from the group's
    /// closing `)` — still the *same* stanza, not a new one. This pins
    /// `just_closed_paren_group`, the narrowest branch this fix adds: the
    /// blank line must fold the bracket rows into the still-open usage
    /// entry rather than requiring fresh `looks_like_stanza_continuation_head`
    /// evidence (a bracket row is never a stanza head).
    #[test]
    fn trailing_bracket_rows_continue_across_the_blank_line_after_the_closing_paren() {
        let help = "tool\n\
                     \t( -a|--aaa Number,\n\
                     \t     --ccc y|n )\n\
                     \n\
                     \t[ -d|--ddd ]\n\
                     \t[ -e|--eee ]\n";
        let parsed = parse_with_profile(help, None, Some("tool"));
        assert_eq!(
            parsed.usage.len(),
            1,
            "the trailing bracket rows must join the one open stanza, not start a second: {:?}",
            parsed.usage
        );
        assert!(
            parsed.usage[0].contains("-d|--ddd") && parsed.usage[0].contains("-e|--eee"),
            "usage: {:?}",
            parsed.usage
        );
        assert!(
            parsed
                .flags
                .iter()
                .any(|f| f.long.as_deref() == Some("ddd")),
            "flags: {:?}",
            parsed.flags
        );
        assert!(
            parsed
                .flags
                .iter()
                .any(|f| f.long.as_deref() == Some("eee")),
            "flags: {:?}",
            parsed.flags
        );
    }

    /// A row using `|` as a plain alias separator with no paren group at
    /// all (an ordinary bracket-row flag list, no `(` anywhere in the
    /// document) must parse exactly as it always did — `paren_group_depth`
    /// stays at zero throughout, so this fix's own code path never fires.
    #[test]
    fn a_plain_alias_separator_row_with_no_paren_group_is_unaffected() {
        let help = "tool\n\
                     \t[ -a|--aaa Number ]\n\
                     \t[ -b|--bbb y|n ]\n";
        let parsed = parse_with_profile(help, None, Some("tool"));
        assert_eq!(parsed.usage.len(), 1, "usage: {:?}", parsed.usage);
        assert!(
            parsed
                .flags
                .iter()
                .any(|f| f.long.as_deref() == Some("aaa")),
            "flags: {:?}",
            parsed.flags
        );
        let bbb = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("bbb"))
            .unwrap_or_else(|| panic!("flags: {:?}", parsed.flags));
        assert_eq!(bbb.value_name.as_deref(), Some("y|n"));
    }

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

    // --- LVM's docopt bracket-group flag rows ---------------------------

    /// `vgck --help`, byte-exact (minus the `WARNING: Running as a
    /// non-root user` stderr banner, irrelevant here). A bare, unlabelled
    /// synopsis (`vgck` alone, no bracket notation on its own line) whose
    /// only flag — `--reportformat` — is documented on a continuation row,
    /// plus a real `Common options for lvm:` heading whose 18 rows are
    /// each one whole `[...]` group, tab-indented under a heading written
    /// with two spaces.
    const VGCK_HELP: &str = concat!(
        "  vgck - Check the consistency of volume group(s)\n",
        "\n",
        "  Read and display information about a VG.\n",
        "  vgck\n",
        "\t[    --reportformat basic|json ]\n",
        "\t[ COMMON_OPTIONS ]\n",
        "\t[ VG|Tag ... ]\n",
        "\n",
        "  Rewrite VG metadata to correct problems.\n",
        "  vgck --updatemetadata VG\n",
        "\t[ COMMON_OPTIONS ]\n",
        "\n",
        "  Common options for lvm:\n",
        "\t[ -d|--debug ]\n",
        "\t[ -h|--help ]\n",
        "\t[ -q|--quiet ]\n",
        "\t[ -v|--verbose ]\n",
        "\t[ -y|--yes ]\n",
        "\t[ -t|--test ]\n",
        "\t[    --commandprofile String ]\n",
        "\t[    --config String ]\n",
        "\t[    --driverloaded y|n ]\n",
        "\t[    --nolocking ]\n",
        "\t[    --lockopt String ]\n",
        "\t[    --longhelp ]\n",
        "\t[    --profile String ]\n",
        "\t[    --version ]\n",
        "\t[    --devicesfile String ]\n",
        "\t[    --devices PV ]\n",
        "\t[    --nohints ]\n",
        "\t[    --journal String ]\n",
        "\n",
        "  Use --longhelp to show all options and advanced commands.\n",
    );

    #[test]
    fn vgck_recovers_the_synopsis_continuation_flag() {
        let parsed = parse_with_profile(VGCK_HELP, None, Some("vgck"));
        let reportformat = flag_named(&parsed, "reportformat");
        assert_eq!(reportformat.short, None);
        assert_eq!(reportformat.value_name.as_deref(), Some("basic|json"));
    }

    #[test]
    fn vgck_recovers_every_common_option_from_the_headed_bracket_table() {
        let parsed = parse_with_profile(VGCK_HELP, None, Some("vgck"));
        let debug = flag_named(&parsed, "debug");
        assert_eq!(debug.short, Some('d'));
        assert_eq!(debug.value_name, None);

        let commandprofile = flag_named(&parsed, "commandprofile");
        assert_eq!(commandprofile.short, None);
        assert_eq!(commandprofile.value_name.as_deref(), Some("String"));

        let driverloaded = flag_named(&parsed, "driverloaded");
        assert_eq!(driverloaded.value_name.as_deref(), Some("y|n"));

        // Every one of the 18 rows under `Common options for lvm:`, plus
        // `--reportformat` from the first stanza's synopsis continuation
        // and `--updatemetadata` from the *second* stanza's own head
        // (fix/multi-stanza-synopsis: the blank line between the two no
        // longer ends the usage block before the second stanza is read).
        for long in [
            "debug",
            "help",
            "quiet",
            "verbose",
            "yes",
            "test",
            "commandprofile",
            "config",
            "driverloaded",
            "nolocking",
            "lockopt",
            "longhelp",
            "profile",
            "version",
            "devicesfile",
            "devices",
            "nohints",
            "journal",
            "reportformat",
            "updatemetadata",
        ] {
            flag_named(&parsed, long);
        }
        assert_eq!(parsed.flags.len(), 20, "{:#?}", parsed.flags);
    }

    /// The operand cross-references LVM writes in the identical bracket
    /// notation must never be read as flags: `[ COMMON_OPTIONS ]` names no
    /// dash at all, and `[ VG|Tag ... ]` / `[ VG PV ... ]` are positionals.
    #[test]
    fn vgck_never_fabricates_a_flag_from_an_operand_bracket() {
        let parsed = parse_with_profile(VGCK_HELP, None, Some("vgck"));
        assert!(parsed
            .flags
            .iter()
            .all(|f| f.long.as_deref() != Some("COMMON_OPTIONS")));
        assert!(parsed.flags.iter().all(|f| f.long.as_deref() != Some("VG")));
    }

    /// `vgextend`'s richer synopsis head (`vgextend VG PV ...`, still no
    /// bracket notation of its own) with the same value-vs-alias-vs-nested-
    /// bracket shapes this fix's own doc comments name: `-A|--autobackup
    /// y|n` (alias cluster plus a choice-list value) and `--metadatasize
    /// Size[m|UNIT]` (a value carrying its own nested brackets).
    #[test]
    fn vgextend_reads_the_alias_choice_and_nested_bracket_value_rows() {
        let raw = concat!(
            "  vgextend - Add physical volumes to a volume group\n",
            "\n",
            "  vgextend VG PV ...\n",
            "\t[ -A|--autobackup y|n ]\n",
            "\t[ -f|--force ]\n",
            "\t[    --metadatasize Size[m|UNIT] ]\n",
            "\t[ COMMON_OPTIONS ]\n",
        );
        let parsed = parse_with_profile(raw, None, Some("vgextend"));

        let autobackup = flag_named(&parsed, "autobackup");
        assert_eq!(autobackup.short, Some('A'));
        assert_eq!(autobackup.value_name.as_deref(), Some("y|n"));

        let force = flag_named(&parsed, "force");
        assert_eq!(force.short, Some('f'));
        assert_eq!(force.value_name, None);

        let metadatasize = flag_named(&parsed, "metadatasize");
        assert_eq!(metadatasize.value_name.as_deref(), Some("Size[m|UNIT]"));
    }

    // --- the tab-stop leading-indentation fix ---------------------------

    /// `sotruss --help`'s real specimen: a description that wraps onto a
    /// physical continuation line indented with three tabs, and that
    /// continuation's own trimmed text happens to start with a dash
    /// (`-f is also used`, referring to a different flag in prose). Byte-
    /// exact from a real capture.
    ///
    /// Before `leading_whitespace`'s tab-stop expansion, three raw tab
    /// characters measured as indent `3` — inside
    /// `scan_flags_block`'s `ENTRY_INDENT_TOLERANCE` (10) of the block's
    /// own two-space entries — so this continuation line was read as a
    /// **new** flag entry (`-f` carrying the fabricated value `is`)
    /// instead of a continuation, and `-o, --output`'s own description
    /// lost everything after "in case". Expanding the tabs to real
    /// terminal columns (24) is well outside the tolerance, so the line
    /// now correctly continues `-o`'s description and no phantom `-f`
    /// entry is created.
    const SOTRUSS_HELP: &str = concat!(
        "Usage: sotruss [OPTION...] [--] EXECUTABLE [EXECUTABLE-OPTION...]\n",
        "  -F, --from FROMLIST     Trace calls from objects on FROMLIST\n",
        "  -T, --to TOLIST         Trace calls to objects on TOLIST\n",
        "\n",
        "  -e, --exit              Also show exits from the function calls\n",
        "  -f, --follow            Trace child processes\n",
        "  -o, --output FILENAME   Write output to FILENAME (or FILENAME. in case\n",
        "\t\t\t  -f is also used) instead of standard error\n",
        "\n",
        "  -?, --help              Give this help list\n",
        "      --usage             Give a short usage message\n",
        "      --version           Print program version\n",
    );

    #[test]
    fn tab_indented_continuation_does_not_fabricate_a_flag() {
        let parsed = parse_with_profile(SOTRUSS_HELP, None, Some("sotruss"));
        // No phantom `-f` carrying the value `is` — only the one real
        // `-f, --follow` flag.
        let f_flags: Vec<_> = parsed
            .flags
            .iter()
            .filter(|f| f.short == Some('f'))
            .collect();
        assert_eq!(f_flags.len(), 1, "{:#?}", parsed.flags);
        assert_eq!(f_flags[0].long.as_deref(), Some("follow"));
        assert_eq!(f_flags[0].value_name, None);

        // `-o, --output`'s description is now whole, not truncated at the
        // point the continuation line used to be misread as a new entry.
        let output = flag_named(&parsed, "output");
        assert_eq!(
            output.description.as_ref().map(|d| d.to_string()).as_deref(),
            Some("Write output to FILENAME (or FILENAME. in case -f is also used) instead of standard error")
        );
    }

    // --- the alternation-with-mismatched-operands hazard ----------------

    /// `ethtool --help`'s real row: an alternation between two *different*
    /// flags, only one of which carries its own bracketed operands. Not
    /// LVM's shape at all — LVM's alias run never has a bare flag
    /// spelling reappear after the first whitespace gap — so
    /// `bracket_flag_row_content` must refuse the whole row rather than
    /// read `--all-groups` as carrying `--groups`'s operand and losing
    /// `--groups` outright (the exact fabrication this fix's own doc
    /// comment on `bracket_flag_row_content` names).
    #[test]
    fn bracket_row_with_a_second_alternatives_operands_is_refused() {
        assert_eq!(
            bracket_flag_row_content(
                "[ --all-groups | --groups [eth-phy] [eth-mac] [eth-ctrl] [rmon] ]"
            ),
            None
        );
    }

    #[test]
    fn ethtool_keeps_both_alternatives_unread_rather_than_fabricating() {
        let raw = concat!(
            "  ethtool DEVNAME\n",
            "\t[ --all-groups | --groups [eth-phy] [eth-mac] [eth-ctrl] [rmon] ]\n",
        );
        let parsed = parse_with_profile(raw, None, Some("ethtool"));
        assert!(parsed
            .flags
            .iter()
            .all(|f| f.long.as_deref() != Some("all-groups")));
    }

    fn flag_named(parsed: &ParsedHelp, long: &str) -> Flag {
        parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some(long))
            .unwrap_or_else(|| {
                panic!(
                    "no flag long=={long:?} in {:?}",
                    parsed
                        .flags
                        .iter()
                        .map(|f| f.spelling())
                        .collect::<Vec<_>>()
                )
            })
            .clone()
    }

    #[test]
    fn bpftraces_repeated_character_flags_become_single_dash_long_options() {
        let parsed = parse(BPFTRACE_TROUBLESHOOTING);
        for (name, description) in [
            ("vv", "more verbose messages (max 2)"),
            ("dd", "(dry run) verbose debug info"),
        ] {
            let flag = flag_named(&parsed, name);
            assert!(flag.single_dash, "-{name} is spelled with one dash");
            assert_eq!(flag.spelling(), format!("-{name}"));
            assert_eq!(flag.short, None);
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
                .find(|f| f.short == Some(short))
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
            parsed.flags.iter().all(|f| f.long.is_none()),
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
        assert!(flag_named(&parsed, "nn").single_dash);
    }

    /// A spaced value is indistinguishable from a glued one once
    /// [`parse_flag_spec`] has stored it, so the raw text is what decides.
    #[test]
    fn a_spaced_value_is_never_repaired() {
        let parsed = parse("  -v         verbose\n  -v v       take a v\n");
        assert!(
            parsed.flags.iter().all(|f| f.long.is_none()),
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
            parsed.flags.iter().all(|f| f.long.is_none()),
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
            assert!(flag.single_dash, "-{name} is spelled with one dash");
            assert_eq!(flag.spelling(), format!("-{name}"));
            assert_eq!(flag.short, None);
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
            .find(|f| f.short == Some('g'))
            .expect("-g must survive as a short flag");
        assert_eq!(
            g.long, None,
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
            .any(|f| f.short == Some('h') && f.value_kind == ValueKind::None));
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
                parsed.flags.iter().all(|f| f.long.is_none()),
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
            assert!(flag.single_dash, "-{name} is spelled with one dash");
            // `Flag::spelling` writes a required value with a space, the
            // same repo-wide display convention that renders `--output=FILE`
            // as `--output FILE`; what matters here is that the *name* is
            // whole and the value is the tool's own.
            assert_eq!(flag.spelling(), format!("-{name} {value}"));
            assert_eq!(flag.short, None);
            // The document wrote the value spec on the token, so unlike the
            // spaced case it survives the repair. `-match=K=V` splits at the
            // *first* `=` and keeps the rest verbatim.
            assert_eq!(flag.value_name.as_deref(), Some(value));
            assert_eq!(flag.value_kind, ValueKind::Required);
        }
        // The value-less rows in the same table are unchanged by the split.
        for name in ["reverse", "version"] {
            let flag = flag_named(&parsed, name);
            assert!(flag.single_dash);
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
            assert!(flag.single_dash, "-{name} is spelled with one dash");
            assert_eq!(flag.short, None);
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
                parsed.flags.iter().all(|f| f.long.is_none()),
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
                parsed.flags.iter().all(|f| f.long.is_none()),
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
            .find(|f| f.long.as_deref() == Some("case_sensitive"))
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
        assert!(flag.single_dash, "it is spelled with one dash");
        assert_eq!(flag.short, None, "the fabricated -c is gone");
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
                .any(|f| f.short == Some('c') && f.long.is_none()),
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
                .find(|f| f.long.as_deref() == Some(name))
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
            assert!(flag.single_dash);
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
                parsed
                    .flags
                    .iter()
                    .all(|f| f.long.as_deref() != Some(refused)),
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
            parsed
                .flags
                .iter()
                .all(|f| f.long.as_deref() != Some("foo")),
            "an empty value spec has no measured reading"
        );
        // `ip` writes a bracketed tail, so the grammar records
        // `ValueKind::Optional` — a value spec a human deliberately typed.
        let parsed = parse("OPTIONS := { -V[ersion] | -h[uman-readable] | -j[son] }\n");
        assert!(
            parsed
                .flags
                .iter()
                .all(|f| f.long.as_deref() != Some("human-readable")),
            "ip's bracketed abbreviation is outside a Required-only fingerprint by construction"
        );
        // `sg_emc_trespass` glues the layout's own colon onto the flag, so
        // the tail is `"r:"` and is not an option name.
        let parsed = parse("    -hr: Set Honor Reservation bit\n");
        assert!(
            parsed.flags.iter().all(|f| f.long.as_deref() != Some("hr")),
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
            parsed
                .flags
                .iter()
                .all(|f| f.long.as_deref() != Some("adhilswfr")),
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
            parsed.flags.iter().all(|f| f.long.is_none()),
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
            parsed.flags.iter().all(|f| f.long.is_none()),
            "a repeated-character run is the other repair's, and only when it has its boolean"
        );
        // A one-character tail is the ambiguous population both repairs
        // decline: `rpcgen -Ss` and friends are half correct parses.
        let parsed = parse("  -ps        postscript\n");
        assert!(parsed.flags.iter().all(|f| f.long.is_none()));
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

    // These two captures live once, as the corpus regression fixtures
    // (`corpus/tar/1.35/help.txt`, `corpus/git/2.43.0/help.txt` — see
    // corpus/README.md), rather than a byte-identical second copy under
    // this crate's own `tests/fixtures/`.
    const TAR_HELP: &str = include_str!("../../../corpus/tar/1.35/help.txt");
    const GIT_HELP: &str = include_str!("../../../corpus/git/2.43.0/help.txt");
    const LSOF_HELP: &str = include_str!("../../../corpus/lsof/4.95.0/help.stderr.txt");
    const UNZIP_HELP: &str = include_str!("../../../corpus/unzip/6.00/help.txt");
    const ZOXIDE_HELP: &str = include_str!("../../../corpus/zoxide/0.9.9/help.txt");

    const OPENSSL_HELP: &str = include_str!("../../tests/fixtures/help_text/openssl_help.stderr");
    const IP_HELP: &str = include_str!("../../tests/fixtures/help_text/ip_help.stderr");
    const DD_HELP: &str = include_str!("../../tests/fixtures/help_text/dd_help.stdout");
    const LESS_HELP: &str = include_str!("../../tests/fixtures/help_text/less_help.stdout");
    const SED_HELP: &str = include_str!("../../tests/fixtures/help_text/sed_help.stdout");
    const FIND_HELP: &str = include_str!("../../tests/fixtures/help_text/find_help.stdout");
    const CURL_HELP: &str = include_str!("../../tests/fixtures/help_text/curl_help.stdout");
    const APT_GET_HELP: &str = include_str!("../../tests/fixtures/help_text/apt_get_help.stdout");

    /// Regression for [M-10], found by reading the real TUI rather than a
    /// green test suite: `apt-get --help` gained the subcommands *"and"*,
    /// *"information"*, *"about"*, *"them"*, *"from"*, *"authenticated"*
    /// and *"sources"* — the words of its own description paragraph past
    /// the first line. The paragraph's opening sentence ("apt-get is a
    /// **command** line interface for retrieval of packages") satisfied
    /// the recognized-command-heading test, and the wrapped lines beneath
    /// it are all name-shaped words at a matching indent, so the
    /// bare-name grid parser (which exists for openssl's genuinely
    /// column-aligned command grid) consumed the prose.
    #[test]
    fn apt_get_description_prose_is_not_parsed_as_a_command_grid() {
        let parsed = parse(APT_GET_HELP);
        let names: Vec<&str> = parsed.subcommands.iter().map(|c| c.name.as_str()).collect();
        for fabricated in [
            "and",
            "information",
            "about",
            "them",
            "from",
            "authenticated",
            "sources",
        ] {
            assert!(
                !names.contains(&fabricated),
                "prose word {fabricated:?} was parsed as a subcommand: {names:?}"
            );
        }
    }

    /// Issue #3: `apt-get --help`'s real subcommands sit under a
    /// recognized heading (`"Most used commands:"`) in single-space
    /// `name - description` form, e.g. `"update - Retrieve new lists of
    /// packages"`. Before this fix the 2+-space column-gap requirement
    /// (needed to keep the regression above green) meant the whole line
    /// read as one ungapped field, failed the name-shape test, and was
    /// dropped — none of apt-get's real subcommands were recovered at
    /// all. This asserts they now are, *with* their descriptions.
    #[test]
    fn apt_get_dash_separated_commands_are_recovered_with_descriptions() {
        let parsed = parse(APT_GET_HELP);
        let names: Vec<&str> = parsed.subcommands.iter().map(|c| c.name.as_str()).collect();
        for want in [
            "update",
            "upgrade",
            "install",
            "remove",
            "purge",
            "autoremove",
            "dist-upgrade",
            "clean",
            "source",
        ] {
            assert!(names.contains(&want), "expected {want:?} among {names:?}");
        }
        let update = parsed
            .subcommands
            .iter()
            .find(|c| c.name == "update")
            .expect("update recovered");
        assert_eq!(
            update.summary.as_ref().map(|s| s.as_str()),
            Some("Retrieve new lists of packages")
        );
        let dist_upgrade = parsed
            .subcommands
            .iter()
            .find(|c| c.name == "dist-upgrade")
            .expect("dist-upgrade recovered — its own internal hyphen must not be mistaken for the separator");
        assert_eq!(
            dist_upgrade.summary.as_ref().map(|s| s.as_str()),
            Some("Distribution upgrade, see apt-get(8)")
        );
    }

    /// A bare ` - ` inside ordinary prose (not under a recognized command
    /// heading) must never manufacture a subcommand — the exact class of
    /// regression the column-gap rule was originally introduced to stop
    /// (spec [M-10]), just via this separator instead of that one.
    #[test]
    fn dash_separator_is_not_recognized_outside_a_command_heading() {
        let raw = "Usage: widget [OPTIONS]\n\nAbout this tool:\n  widget - a small utility for widgets\n  it does not have a Commands section at all - just prose\n";
        let parsed = parse(raw);
        assert!(
            parsed.subcommands.is_empty(),
            "expected zero subcommands from prose, got {:?}",
            parsed
                .subcommands
                .iter()
                .map(|c| &c.name)
                .collect::<Vec<_>>()
        );
    }

    /// Regression: `curl --help` runs its flag list straight into the
    /// usage line — indented one space, no blank line, no `Options:`
    /// heading. The usage block used to consume every indented line that
    /// followed, so all of curl's flags landed in `usage` and the tool
    /// reported *zero* flags while its status stayed `ok` (nothing was
    /// fabricated, so the structure-sanity check couldn't see it either).
    /// This fixture was checked in but never asserted on, which is how it
    /// survived; the assertion is what makes the fix stick.
    #[test]
    fn curl_flags_running_straight_into_the_usage_line_are_not_swallowed() {
        let parsed = parse(CURL_HELP);
        assert!(
            parsed.flags.len() > 100,
            "expected curl's full flag list, got {}",
            parsed.flags.len()
        );
        let longs: Vec<&str> = parsed
            .flags
            .iter()
            .filter_map(|f| f.long.as_deref())
            .collect();
        assert!(longs.contains(&"append"), "{longs:?}");
        assert!(longs.contains(&"anyauth"), "{longs:?}");
        // The usage block keeps its own line and stops before the flags.
        assert_eq!(parsed.usage.len(), 1);
        assert!(parsed.usage[0].starts_with("Usage: curl"));
        // And it must not have invented subcommands out of the flag rows.
        assert!(parsed.subcommands.is_empty(), "{:?}", parsed.subcommands);
    }

    /// The fabrication bug this module exists to fix: `git --help`'s usage
    /// synopsis is one invocation form wrapped across five physical lines
    /// (four continuations, all indented under `git`, none of them a
    /// marker). The old per-physical-line storage reported five separate
    /// `usage` entries, and the detail pane's `usage_signature` — which
    /// prepends the node name to any entry not already starting with it —
    /// turned the tail fragment into `git [--config-env=<name>=<envvar>]
    /// <command> [<args>]`, a complete-looking invocation git never
    /// documented. The fix must produce exactly one entry, with every
    /// fragment's own text intact and joined by a single space (not
    /// re-flowed — spec §7: usage is kept verbatim).
    #[test]
    fn git_wrapped_usage_synopsis_joins_into_one_entry() {
        let parsed = parse(GIT_HELP);
        assert_eq!(
            parsed.usage.len(),
            1,
            "git's five wrapped lines must join into one logical entry, got {:?}",
            parsed.usage
        );
        assert_eq!(
            parsed.usage[0],
            "usage: git [-v | --version] [-h | --help] [-C <path>] [-c <name>=<value>] \
             [--exec-path[=<path>]] [--html-path] [--man-path] [--info-path] \
             [-p | --paginate | -P | --no-pager] [--no-replace-objects] [--bare] \
             [--git-dir=<path>] [--work-tree=<path>] [--namespace=<name>] \
             [--config-env=<name>=<envvar>] <command> [<args>]"
        );
    }

    /// The over-join guard: `du --help` prints two *genuine* alternative
    /// invocation forms, joined by GNU coreutils' `or:` marker —
    ///
    /// ```text
    /// Usage: du [OPTION]... [FILE]...
    ///   or:  du [OPTION]... --files0-from=F
    /// ```
    ///
    /// — as opposed to one form wrapped across lines. The `or:` line is
    /// indented *more* than the block's base indent (0), so a rule that
    /// joins every more-indented line onto the entry above it — correct
    /// for git's wrapped synopsis above — would also swallow this one,
    /// silently merging two real invocations into a single fabricated
    /// line. The marker check is what keeps `or:` its own entry regardless
    /// of indentation.
    #[test]
    fn du_or_marker_stays_a_separate_usage_entry() {
        let raw = "Usage: du [OPTION]... [FILE]...\n  or:  du [OPTION]... --files0-from=F\nSummarize device usage of the set of FILEs, recursively for directories.\n";
        let parsed = parse(raw);
        assert_eq!(
            parsed.usage,
            vec![
                "Usage: du [OPTION]... [FILE]...".to_string(),
                "or:  du [OPTION]... --files0-from=F".to_string(),
            ],
            "or: must stay a separate entry, not join onto the line above"
        );
    }

    /// A tool that repeats the `usage:`/`Usage:` label itself for each
    /// form (rather than using `or:`) must also get one entry per label,
    /// not one entry per physical line and not everything joined into one.
    #[test]
    fn repeated_usage_label_at_base_indent_starts_a_new_entry() {
        let raw = "usage: widget run [OPTIONS]\nusage: widget stop [OPTIONS]\n";
        let parsed = parse(raw);
        assert_eq!(
            parsed.usage,
            vec![
                "usage: widget run [OPTIONS]".to_string(),
                "usage: widget stop [OPTIONS]".to_string(),
            ]
        );
    }

    /// The other discriminator spec §7's usage grammar allows for: a tool
    /// that lists alternative forms *without* any `usage:`/`or:` marker, by
    /// literally repeating its own name. This must read as two entries when
    /// the tool's name is known — the counterpart to the marker-based
    /// version just above, exercised via [`parse_named`] rather than
    /// [`parse`] since the discriminator only fires when a name is given.
    #[test]
    fn own_name_repeated_with_no_marker_starts_a_new_entry() {
        let raw = "Usage: prog foo\n       prog bar\n";
        let parsed = parse_named(raw, "prog");
        assert_eq!(
            parsed.usage,
            vec!["Usage: prog foo".to_string(), "prog bar".to_string()],
            "{:?}",
            parsed.usage
        );
        // Without a known name, the second line is still more indented
        // than the block's own base (7 spaces vs. 0) — the same hanging-
        // indent shape git's wrapped synopsis uses — so it (reasonably)
        // reads as a continuation instead, joining into one entry. This is
        // exactly why `tool_name` matters: the discriminator that tells
        // these two real shapes apart is the name, not the indent, which
        // looks identical in both.
        let unnamed = parse(raw);
        assert_eq!(unnamed.usage, vec!["Usage: prog foo prog bar".to_string()]);
    }

    /// The regression this batch exists to fix: `lsof -h`'s usage synopsis
    /// wraps across three physical lines, but — unlike `git`'s hanging
    /// indent — every continuation sits at *the same* column as the
    /// `usage:` marker itself (both indented by exactly one space):
    ///
    /// ```text
    ///  usage: [-?abhKlnNoOPRtUvVX] [+|-c c] [+|-d s] [+D D] [+|-E] [+|-e s] [+|-f[gG]]
    ///  [-F [f]] [-g [s]] [-i [i]] [+|-L [l]] [+m [m]] [+|-M] [-o [o]] [-p s]
    ///  [+|-r [t]] [-s [p:s]] [-S [t]] [-T [t]] [-u s] [+|-w] [-x [fl]] [--] [names]
    /// ```
    ///
    /// The indentation-only rule `f5f1183` shipped read `leading_whitespace
    /// <= base_indent` as "block already ended" for every one of these
    /// continuation lines, dropping them — and the six flags documented
    /// only in them (`-F`, `-g`, `+|-L`, `+m`, `+|-M`, `+|-r`, among others)
    /// never reached `extract_usage_flags` — before they ever joined
    /// `result.usage`. This must now recover as one logical entry, with
    /// every continuation-only flag still present.
    #[test]
    fn lsof_same_indent_continuations_join_into_one_entry() {
        let raw = " usage: [-?abhKlnNoOPRtUvVX] [+|-c c] [+|-d s] [+D D] [+|-E] [+|-e s] [+|-f[gG]]\n \
                    [-F [f]] [-g [s]] [-i [i]] [+|-L [l]] [+m [m]] [+|-M] [-o [o]] [-p s]\n \
                    [+|-r [t]] [-s [p:s]] [-S [t]] [-T [t]] [-u s] [+|-w] [-x [fl]] [--] [names]\n\
                    Defaults in parentheses; comma-separated set (s) items; dash-separated ranges.\n";
        let parsed = parse_named(raw, "lsof");
        assert_eq!(
            parsed.usage.len(),
            1,
            "lsof's three same-indent lines must join into one logical entry, got {:?}",
            parsed.usage
        );
        assert_eq!(
            parsed.usage[0],
            "usage: [-?abhKlnNoOPRtUvVX] [+|-c c] [+|-d s] [+D D] [+|-E] [+|-e s] [+|-f[gG]] \
             [-F [f]] [-g [s]] [-i [i]] [+|-L [l]] [+m [m]] [+|-M] [-o [o]] [-p s] \
             [+|-r [t]] [-s [p:s]] [-S [t]] [-T [t]] [-u s] [+|-w] [-x [fl]] [--] [names]"
        );
        // The over-join guard's counterpart: the trailing "Defaults in
        // parentheses..." sentence sits at that same column-0-or-less
        // position (no leading space at all) but is ordinary prose, not a
        // usage-grammar fragment, and must still end the block rather than
        // being swallowed onto the synopsis.
        assert!(!parsed.usage[0].contains("Defaults"), "{:?}", parsed.usage);
        let short_flags: Vec<Option<char>> = parsed.flags.iter().map(|f| f.short).collect();
        // Spot-check flags documented only in the two (previously dropped)
        // continuation lines — none of these appear in the first line's
        // own groups (verified by hand against `usage_segments`'
        // token-level behavior: `-o`, for instance, appears as a bare
        // character inside line one's bundled `-?abhKlnNoOPRtUvVX` blob,
        // but that whole blob parses as a single flag spelled `-?` with
        // the rest as its value shape, not as fourteen separate flags — so
        // `-o` is only ever actually recovered from continuation line
        // one's own explicit `[-o [o]]` group).
        for want in ['F', 'g', 'L', 'M', 'r', 'u'] {
            assert!(
                short_flags.contains(&Some(want)),
                "expected -{want} recovered from lsof's continuation lines, got {short_flags:?}"
            );
        }
    }

    /// Regression for spec [M-8]: `openssl --help` writes only to stderr,
    /// with no `Usage:` line and no indentation at all — commands are a
    /// same-indent word grid (`asn1parse   ca   ciphers   cmp`). A tier
    /// that only recognized indented blocks produced nothing here.
    #[test]
    fn openssl_word_grid_recovered_as_subcommands() {
        let parsed = parse(OPENSSL_HELP);
        let names: Vec<&str> = parsed.subcommands.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"asn1parse"), "{names:?}");
        assert!(names.contains(&"ciphers"), "{names:?}");
        assert!(names.contains(&"x509"), "{names:?}");
    }

    #[test]
    fn openssl_word_grid_entries_carry_their_heading_as_group() {
        let parsed = parse(OPENSSL_HELP);
        let asn1parse = parsed
            .subcommands
            .iter()
            .find(|c| c.name == "asn1parse")
            .unwrap();
        assert_eq!(asn1parse.group.as_deref(), Some("Standard commands"));
        let md5 = parsed.subcommands.iter().find(|c| c.name == "md5");
        assert!(md5.is_some(), "expected md5 among digest commands");
        assert!(md5
            .unwrap()
            .group
            .as_deref()
            .unwrap()
            .contains("Message Digest commands"));
    }

    /// Regression for spec [M-8]: `ip --help` writes only to stderr and
    /// exits 255. `ip`'s usage grammar (`OBJECT := { address | ... }`) is
    /// unusual enough that this just checks *something* structural comes
    /// back, not a specific shape.
    #[test]
    fn ip_stderr_help_produces_a_usage_line() {
        let parsed = parse(IP_HELP);
        assert!(
            !parsed.usage.is_empty(),
            "expected at least a Usage: line from ip's stderr help"
        );
    }

    #[test]
    fn tar_usage_line_recovered() {
        let parsed = parse(TAR_HELP);
        assert!(!parsed.usage.is_empty());
        assert!(parsed.usage[0].to_lowercase().contains("usage:"));
    }

    #[test]
    fn tar_main_operation_mode_group_recovered() {
        let parsed = parse(TAR_HELP);
        let create = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("create"));
        assert!(
            create.is_some(),
            "expected --create among {:?}",
            parsed.flags.iter().map(|f| &f.long).collect::<Vec<_>>()
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
            .find(|f| f.long.as_deref() == Some("create"))
            .unwrap();
        assert_eq!(create.short, Some('c'));
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
            .find(|f| f.long.as_deref() == Some("occurrence"))
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
        let delete = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("delete"));
        assert!(
            delete.is_some(),
            "expected --delete among {:?}",
            parsed.flags.iter().map(|f| &f.long).collect::<Vec<_>>()
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
        let touch = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("touch"));
        assert!(
            touch.is_some(),
            "expected --touch among {:?}",
            parsed.flags.iter().map(|f| &f.long).collect::<Vec<_>>()
        );
        assert_eq!(touch.unwrap().short, Some('m'));
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

    /// `--format=FORMAT`'s enum values (`gnu`, `oldgnu`, `pax`, ...) are
    /// documented in an unheaded list right after the flag, under a
    /// pseudo-heading ("FORMAT is one of the following:") that does not
    /// itself say "command". Spec §7 Tier B rule 4: these are the flag's
    /// `choices`, not subcommands.
    #[test]
    fn tar_format_enum_values_become_flag_choices_not_subcommands() {
        let parsed = parse(TAR_HELP);
        let format = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("format"))
            .expect("--format flag recovered");
        let choice_strs: Vec<&str> = format.choices.iter().map(|t| t.as_str()).collect();
        for want in ["gnu", "oldgnu", "pax", "posix", "ustar", "v7"] {
            assert!(choice_strs.contains(&want), "{choice_strs:?}");
        }
        assert!(!parsed.subcommands.iter().any(|c| c.name == "gnu"));
        // The other half of rule 4, and the half that was silently wrong:
        // the enum list *ends*, and the options table beneath it resumes.
        // Six values are documented; anything past `v7` is a flag row.
        assert_eq!(
            choice_strs,
            ["gnu", "oldgnu", "pax", "posix", "ustar", "v7"],
            "the enum swallowed the flag rows beneath it"
        );
    }

    /// The three GNU tar flags the `FORMAT is one of the following:` enum
    /// used to eat (tracker #41). They sit at indent 6 while the enum's own
    /// values sit at indent 4, so the block never dedented and ran straight
    /// through them — a green, snapshot-blessed fixture missing three real
    /// flags. `--portability` is *not* asserted: it is a second **long**
    /// alias on `--old-archive`'s row, and `Flag` has one `long` slot, so
    /// losing it is the `dropped-alias` family and not this one.
    #[test]
    fn tar_options_table_resumes_after_the_format_enum() {
        let parsed = parse(TAR_HELP);
        for want in ["old-archive", "pax-option", "posix"] {
            let flag = parsed
                .flags
                .iter()
                .find(|f| f.long.as_deref() == Some(want))
                .unwrap_or_else(|| panic!("--{want} consumed by the FORMAT enum"));
            assert!(
                !flag
                    .description
                    .as_ref()
                    .is_none_or(|d| d.as_str().is_empty()),
                "--{want} recovered without its description"
            );
        }
        // The row directly beneath the recovered ones must survive intact:
        // a break that re-routed too much would take `-V, --label=TEXT`
        // with it.
        assert!(
            parsed
                .flags
                .iter()
                .any(|f| f.long.as_deref() == Some("label") && f.short == Some('V')),
            "-V, --label lost"
        );
    }

    /// `sg_dd`'s shape: a `where:` operand table whose *own* rows are bare
    /// words, ending in flag rows at that same indent. Before the block
    /// learned to end at a flag row, all six became choices of nothing and
    /// `--progress`/`--verify` — documented nowhere else — were lost
    /// outright (seed-2 verdict `wrong`; `corpus/sg_dd/audit-seed2`).
    ///
    /// **More than three operand rows, deliberately.** A first cut of this
    /// test used two and passed with the fix reverted, which is worse than
    /// no test: [`flags_block_start`] already tolerates up to
    /// `MAX_SKIPPED_LEADING_ROWS` non-flag rows before the first flag row,
    /// so a short table is claimed as a flags block outright and never
    /// reaches [`bare_block_end`] at all. `sg_dd`'s real table is twenty
    /// rows; it is the tables *over* that budget that this rule exists for,
    /// and only those reproduce the defect.
    #[test]
    fn a_bare_operand_table_ends_where_its_flag_rows_begin() {
        let help = "\
Usage: prog [bs=BS] [--help]
  where:
    bs          logical block size (default is 512)
    count       number of blocks to copy
    ibs         input logical block size
    obs         output logical block size
    seek        block position to start writing to OFILE
    skip        block position to start reading from IFILE
    --progress    print progress report every 2 minutes
    --verify|-x    do verify/compare rather than copy
";
        let parsed = parse(help);
        for want in ["progress", "verify"] {
            assert!(
                parsed.flags.iter().any(|f| f.long.as_deref() == Some(want)),
                "--{want} consumed by the operand table: {:?}",
                parsed.flags.iter().map(|f| &f.long).collect::<Vec<_>>()
            );
        }
        // And the operands above them are still read as the bare block
        // they are, not promoted into flags or subcommands.
        assert!(!parsed.flags.iter().any(|f| f.long.as_deref() == Some("bs")));
        assert!(parsed.subcommands.is_empty(), "{:?}", parsed.subcommands);
    }

    /// The break is `i > start` for a reason: `flags_block_start` has
    /// already declined this block, and a block whose *first* line ended it
    /// would be zero-length and never advance. A bare-word list is
    /// unaffected by the rule when no flag row follows it.
    #[test]
    fn a_bare_block_with_no_flag_rows_is_unchanged() {
        let lines = ["  alpha   first", "  beta    second", "  gamma   third"];
        assert_eq!(bare_block_end(&lines, 0), 3);
    }

    /// `btrfs --help`'s shape (`corpus/btrfs/audit-seed2/help.txt`): a
    /// flags block at indent 2 (`--help`, `--version`), followed by a blank
    /// line and then a nested command table at indent 4 whose own rows are
    /// each followed by a description one indent deeper (indent 8). Before
    /// this test's fix, [`scan_flags_block`]'s continuation rule only
    /// checked "is this line indented past the block's entries?" — true for
    /// every row and every description in the table below — so the entire
    /// table folded into `--version`'s description instead of ending the
    /// flags block.
    ///
    /// Three table groups, deliberately more than the
    /// [`MIN_NESTED_TABLE_ROWS`] floor of two: a single ragged continuation
    /// followed by one deeper line must never trip this detector, only
    /// genuine repetition may.
    #[test]
    fn nested_command_table_does_not_swallow_into_a_flag_description() {
        let help = "\
Usage: widget [global] <group> <command> [<args>]

Options for the main command only:
  --help            print condensed help for all subcommands
  --version         print version string

    widget group one start
        Start the first task
    widget group one stop
        Stop the first task
    widget group two run
        Run the second task
";
        let parsed = parse(help);
        let version = flag_named(&parsed, "version");
        assert_eq!(
            version.description.as_ref().map(|t| t.as_str()),
            Some("print version string"),
            "--version's description must not absorb the command table below it"
        );
        for swallowed in [
            "Start the first task",
            "Stop the first task",
            "Run the second task",
        ] {
            assert!(
                !version
                    .description
                    .as_ref()
                    .is_some_and(|d| d.as_str().contains(swallowed)),
                "table row {swallowed:?} leaked into --version's description: {:?}",
                version.description
            );
        }
    }

    /// `pngfix --strip`'s shape (`corpus/pngfix/*/help.txt`): the flag row
    /// carries **no inline description at all** — it just ends in `:` — and
    /// everything below it, at one indent deeper, *is* that flag's
    /// description: a value-choice list whose own rows wrap onto a second,
    /// still-deeper physical line for the longer choices (`unsafe`,
    /// `unused`). That wrap is what [`nested_entry_table_starts_at`] misreads
    /// as table rows: two choices whose explanation happens to overflow onto
    /// a deeper-indented continuation line is exactly the "row at `indent`
    /// followed by something deeper" shape it looks for, even though this is
    /// ordinary wrapped prose, not a nested command table. Before the entry-
    /// row gate, the detector fired here too, breaking the flags block right
    /// at the first continuation line and leaving `--strip` with **no
    /// description at all** — the whole choice list, gone, not merely
    /// mis-split. Since the entry row itself has nothing on its own line,
    /// there is nowhere else for this text to go: the break must never
    /// trigger when the row being continued is bare like this.
    #[test]
    fn value_choice_list_with_wrapped_entries_is_not_read_as_a_nested_table() {
        let help = "\
Usage: widget [options] file

OPTIONS
    --strip=[none|crc|unsafe|unused]:
        none (default): Retain all chunks.
        crc: Remove chunks with a bad CRC.
        unsafe: Remove chunks that may be unsafe to retain if the image data
                is modified. This is set automatically if --max is given.
        unused: Remove chunks not used when decoding an image. This retains
                any chunks that might be used by transformations.
    --optimize (-o):
        Find the smallest deflate window size for the compressed data.
";
        let parsed = parse(help);
        let strip = flag_named(&parsed, "strip");
        let desc = strip.description.as_ref().map_or("", |t| t.as_str());
        assert!(
            desc.contains("Retain all chunks"),
            "--strip's own description must not be dropped: {desc:?}"
        );
        assert!(
            desc.contains("Remove chunks not used"),
            "--strip's own description must not be dropped: {desc:?}"
        );
    }

    /// `pod2man --guesswork`'s shape (`corpus/pod2man/*/help.txt`): the flag
    /// row also carries no inline description — an ordinary wrapped
    /// paragraph follows, then (this is the part that trips the detector) a
    /// genuine bare-word keyword list (`functions`, `manref`, `quoting`,
    /// `variables`), each keyword followed by its own explanation one indent
    /// deeper. That keyword list is real repetition, so
    /// [`nested_entry_table_starts_at`] is right that *something* table-
    /// shaped is down there — it is simply wrong that it's a *different*
    /// entry's table rather than more of `--guesswork`'s own description.
    /// Before the entry-row gate this broke the flags block at the very
    /// first continuation line, so `--guesswork` lost its paragraph *and*
    /// its keyword list both — its entire description, gone.
    #[test]
    fn guesswork_style_keyword_list_is_not_read_as_a_nested_table() {
        let help = "\
Usage: widget [options]

Options:
    --guesswork=rule[,rule...]
        By default, widget applies some default formatting rules based on
        guesswork. This option allows turning all or some of it off.

        Otherwise, the value of this option should be a comma-separated
        list of one or more of the following keywords:

        functions
            Convert function references like foo() to bold even if they
            have no markup.

        manref
            Make the first part of man page references like foo(1) bold
            even if they have no markup.

    --help
        Show this help.
";
        let parsed = parse(help);
        let guesswork = flag_named(&parsed, "guesswork");
        let desc = guesswork.description.as_ref().map_or("", |t| t.as_str());
        assert!(
            desc.contains("default formatting rules"),
            "--guesswork's own description must not be dropped: {desc:?}"
        );
        assert!(
            desc.contains("functions"),
            "--guesswork's keyword list must not be dropped: {desc:?}"
        );
    }

    /// `--quoting-style`'s valid arguments are introduced by a heading
    /// that names the flag directly (`"Valid arguments for the
    /// --quoting-style option are:"`) — the literal-name-match half of
    /// rule 4, distinct from the pure-adjacency case above.
    #[test]
    fn tar_quoting_style_values_become_flag_choices() {
        let parsed = parse(TAR_HELP);
        let quoting_style = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("quoting-style"))
            .expect("--quoting-style flag recovered");
        let choice_strs: Vec<&str> = quoting_style.choices.iter().map(|t| t.as_str()).collect();
        assert!(choice_strs.contains(&"literal"), "{choice_strs:?}");
        assert!(
            choice_strs.contains(&"shell-escape-always"),
            "{choice_strs:?}"
        );
    }

    #[test]
    fn git_command_groups_recovered_without_colon_headings() {
        let parsed = parse(GIT_HELP);
        let clone = parsed.subcommands.iter().find(|c| c.name == "clone");
        assert!(
            clone.is_some(),
            "expected clone among {:?}",
            parsed
                .subcommands
                .iter()
                .map(|c| &c.name)
                .collect::<Vec<_>>()
        );
        assert!(clone
            .unwrap()
            .group
            .as_deref()
            .unwrap()
            .contains("start a working area"));
    }

    #[test]
    fn git_subcommand_descriptions_recovered() {
        let parsed = parse(GIT_HELP);
        let add = parsed.subcommands.iter().find(|c| c.name == "add").unwrap();
        assert_eq!(
            add.summary.as_ref().unwrap().as_str(),
            "Add file contents to the index"
        );
    }

    /// Every one of git's group headings recovers its commands — the
    /// chain seeded by the leading blurb ("These are common Git commands
    /// used in various situations:") must survive across all five groups,
    /// not just the first.
    #[test]
    fn git_all_command_groups_recovered() {
        let parsed = parse(GIT_HELP);
        let names: Vec<&str> = parsed.subcommands.iter().map(|c| c.name.as_str()).collect();
        for want in ["clone", "add", "bisect", "branch", "fetch"] {
            assert!(names.contains(&want), "{names:?}");
        }
    }

    /// Regression for the two `extract_positionals` defects the
    /// `corpus/git/2.43.0` fixture held open under `[xfail]`: git's root
    /// usage line has `-C <path>` and `-c <name>=<value>` before its two
    /// real positionals, `<command>` and `[<args>]`.
    ///
    /// 1. Greedy bracket match: stripping only the *last* `>` off
    ///    `<name>=<value>` used to land past the value spec's own closing
    ///    bracket, producing a positional literally named `name>=<value`.
    /// 2. Flag arguments read as positionals: `-C <path>` and
    ///    `-c <name>=<value>` are option values, not positionals, but the
    ///    old scan had no awareness of what preceded a token.
    ///
    /// The fix must produce exactly `command` and `args` — neither
    /// `path` nor `name>=<value` (nor `options`/`file`/anything else) may
    /// leak in.
    #[test]
    fn git_root_positionals_are_exactly_command_and_args() {
        let parsed = parse(GIT_HELP);
        let names: Vec<&str> = parsed.positionals.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["command", "args"], "{names:?}");
    }

    /// The general shape behind the git-specific regression above, spelled
    /// out with a synthetic usage line so the rule reads as "any flag
    /// followed by its value token", not "git's own bytes": a bare flag's
    /// value — whether `<angle>`-bracketed or a bare `UPPERCASE` word
    /// (argparse's `--config FILE` convention) — must never become a
    /// positional, while a flag that already carries its value inline
    /// (`=`) leaves the *next* token free to be a real positional.
    #[test]
    fn flag_values_in_a_usage_line_are_never_positionals() {
        let parsed = parse("usage: widget [-C <dir>] [--tag=<name>] <target> [--config FILE]\n");
        let names: Vec<&str> = parsed.positionals.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["target"], "{names:?}");
    }

    /// The mirror-image case: a token right after a flag *is* a real
    /// positional when that flag was already written as its own complete,
    /// self-closed bracket group (`[-V]`, `[-v]`) rather than one still
    /// waiting on a following value (`[-C`, `-C`). `sg_emc_trespass`'s own
    /// synopsis, `[-d] [-hr] [-s] [-V] DEVICE`, is exactly this shape:
    /// `DEVICE` sits right after the fully-closed `[-V]` and is a real
    /// operand, not `-V`'s fabricated argument.
    #[test]
    fn a_token_after_a_self_closed_bracket_flag_is_a_real_positional() {
        let parsed = parse("Usage:  sg_emc_trespass [-d] [-hr] [-s] [-V] DEVICE\n");
        let names: Vec<&str> = parsed.positionals.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["DEVICE"], "{names:?}");

        // The general shape, with more than one self-closed flag ahead of
        // an uppercase operand.
        let parsed = parse("usage: widget [-h] [-v] FILE\n");
        let names: Vec<&str> = parsed.positionals.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["FILE"], "{names:?}");
    }

    /// `vim.basic`, the anchor case for [`OPTION_LIST_PLACEHOLDERS`],
    /// confirmed with the maintainer on 2026-08-13: in
    /// `Usage: vim [arguments] [file ..]`, `[arguments]` is the placeholder
    /// for vim's own 45-flag list and `[file ..]` is a real variadic
    /// operand. Extracting `arguments` would be a fabrication, not a recall
    /// gain, and this asserts it stays out no matter which of the two rules
    /// (bare-lowercase, or the placeholder list) is doing the work.
    #[test]
    fn a_usage_lines_option_list_placeholder_is_never_an_operand() {
        for line in [
            "Usage: vim [arguments] [file ..]\n",
            "usage: pkgconf [OPTIONS] [LIBRARIES]\n",
            "Usage: dpkg-statoverride [<option> ...] <command>\n",
            "Usage: tar [OPTION...] [FILE]...\n",
            "USAGE: widget [FLAGS] [OPTIONS] <input>\n",
        ] {
            let names: Vec<String> = parse(line)
                .positionals
                .into_iter()
                .map(|p| p.name)
                .collect();
            assert!(
                !names
                    .iter()
                    .any(|n| OPTION_LIST_PLACEHOLDERS.contains(&n.to_lowercase().as_str())),
                "{line:?} yielded {names:?}"
            );
        }
    }

    /// The other direction, and the reason `args`/`arg` are absent from
    /// [`OPTION_LIST_PLACEHOLDERS`]: an operand whose name merely *reads*
    /// like a generic word is still an operand, and the guard must not
    /// reach it.
    #[test]
    fn a_real_operand_is_not_mistaken_for_an_option_list_placeholder() {
        let parsed = parse("usage: git [<options>] <command> [<args>]\n");
        let names: Vec<&str> = parsed.positionals.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["command", "args"], "{names:?}");
    }

    /// The recall half: argparse declares its operands in a block, and the
    /// synopsis supplies only the notation. `uobjnew`'s real shape —
    /// `pid` required (bare in the synopsis), `interval` optional
    /// (bracketed), both described.
    #[test]
    fn a_declared_positional_block_supplies_names_the_synopsis_cannot() {
        let raw = "usage: uobjnew [-h] [-l {c,java}] [-v] pid [interval]\n\npositional \
                   arguments:\n  pid                   process id to attach to\n  interval        \
                   print every specified number of seconds\n\noptions:\n  -h, --help            \
                   show this help message and exit\n";
        let parsed = parse_with_profile(
            raw,
            Some(&crate::help_text::profile::profile(
                crate::framework::Framework::Argparse,
            )),
            None,
        );
        let shapes: Vec<(&str, bool, bool, Option<&str>)> = parsed
            .positionals
            .iter()
            .map(|p| {
                (
                    p.name.as_str(),
                    p.required,
                    p.variadic,
                    p.description.as_ref().map(|d| d.as_str()),
                )
            })
            .collect();
        assert_eq!(
            shapes,
            vec![
                ("pid", true, false, Some("process id to attach to")),
                (
                    "interval",
                    false,
                    false,
                    Some("print every specified number of seconds")
                ),
            ],
            "{shapes:?}"
        );
        // The identical bytes with no framework identified recover nothing:
        // this is a *declaration* being read, never a bare-lowercase-word
        // rule that would also invent `vim`'s `arguments`.
        assert!(parse(raw).positionals.is_empty());
    }

    /// The declared block must never cost the subparser scan its first
    /// refusal: argparse writes subcommands under the same heading, and
    /// those stay subcommands — with no positional invented from the
    /// `{...}` pseudo-entry or the rows beneath it.
    #[test]
    fn a_declared_block_holding_subparsers_still_yields_subcommands() {
        let raw = "usage: widget [-h] {init,build} ...\n\npositional arguments:\n  \
                   {init,build}\n    init          Initialize a new widget\n    build         \
                   Build the widget\n\noptions:\n  -h, --help    show this help message and \
                   exit\n";
        let parsed = parse_with_profile(
            raw,
            Some(&crate::help_text::profile::profile(
                crate::framework::Framework::Argparse,
            )),
            None,
        );
        let subs: Vec<&str> = parsed.subcommands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(subs, vec!["init", "build"], "{subs:?}");
        assert!(
            parsed.positionals.is_empty(),
            "{:?}",
            parsed
                .positionals
                .iter()
                .map(|p| &p.name)
                .collect::<Vec<_>>()
        );
    }

    /// A declared block whose first column is prose rather than one
    /// operand-shaped word recovers nothing from that row and says so
    /// (`saw_unattributable_content`) — the [M-10] refusal, applied to the
    /// one block this change newly reads.
    #[test]
    fn a_declared_block_never_promotes_prose_to_an_operand() {
        let raw = "usage: widget [-h]\n\npositional arguments:\n  the files you want to \
                   process\n\noptions:\n  -h, --help  show this help message and exit\n";
        let parsed = parse_with_profile(
            raw,
            Some(&crate::help_text::profile::profile(
                crate::framework::Framework::Argparse,
            )),
            None,
        );
        assert!(
            parsed.positionals.is_empty(),
            "{:?}",
            parsed
                .positionals
                .iter()
                .map(|p| &p.name)
                .collect::<Vec<_>>()
        );
        assert!(parsed.saw_unattributable_content);
    }

    /// [M-15]'s headline case, straight from the reference example in the
    /// work order: a synopsis with no `Options:`/`Flags:` block at all must
    /// still recover the flags it documents inline. Also exercises the
    /// conservative-pairing rule end to end: `[-v | --version]` (exactly
    /// one short, one long) becomes one flag with both spellings;
    /// `[-p | --paginate | -P | --no-pager]` (two of each) must not guess a
    /// pairing and instead emit all four spellings as separate entries.
    #[test]
    fn usage_synopsis_flags_are_recovered_with_conservative_pairing() {
        let raw = "usage: git [-v | --version] [-h | --help] [-C <path>] \
                   [-p | --paginate | -P | --no-pager] [--git-dir=<path>]\n";
        let parsed = parse(raw);

        let version = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("version"))
            .expect("--version recovered");
        assert_eq!(
            version.short,
            Some('v'),
            "exactly one short + one long in a group must pair"
        );

        let help = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("help"))
            .expect("--help recovered");
        assert_eq!(help.short, Some('h'));

        // Four alternatives: never guess which short goes with which long.
        // Every spelling is its own unpaired flag, with no cross-pairing.
        let spellings: Vec<(Option<char>, Option<&str>)> = parsed
            .flags
            .iter()
            .map(|f| (f.short, f.long.as_deref()))
            .collect();
        assert!(
            spellings.contains(&(Some('p'), None)),
            "expected an unpaired -p entry, got {spellings:?}"
        );
        assert!(
            spellings.contains(&(None, Some("paginate"))),
            "expected an unpaired --paginate entry, got {spellings:?}"
        );
        assert!(
            spellings.contains(&(Some('P'), None)),
            "expected an unpaired -P entry, got {spellings:?}"
        );
        assert!(
            spellings.contains(&(None, Some("no-pager"))),
            "expected an unpaired --no-pager entry, got {spellings:?}"
        );

        // None of these carry a description — a synopsis has spellings and
        // value shapes only, never prose (spec §7 Tier B: never fabricate).
        assert!(parsed.flags.iter().all(|f| f.description.is_none()));
    }

    /// spec §13's metric redefinition rests on this: a usage-synopsis-
    /// derived flag must carry `Source::HelpTextSynopsis`, not the plain
    /// `Source::HelpText` an options-table row gets, so `pct_flags_with_text`
    /// can exclude it from the denominator instead of counting it as an
    /// undescribed flag from a source that could have described it.
    #[test]
    fn usage_derived_flags_carry_the_synopsis_source_table_derived_do_not() {
        let raw =
            "usage: widget [--verbose] [<file>]\n\nOptions:\n  --loud    print extra output\n";
        let parsed = parse(raw);

        let verbose = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("verbose"))
            .expect("--verbose recovered from the synopsis");
        assert_eq!(
            verbose.provenance.sources.as_slice(),
            [Source::HelpTextSynopsis],
            "usage-only flag must be marked structurally undescribable"
        );
        assert!(!verbose.provenance.describable());

        let loud = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("loud"))
            .expect("--loud recovered from the Options: block");
        assert_eq!(loud.provenance.sources.as_slice(), [Source::HelpText]);
        assert!(loud.provenance.describable());
    }

    /// Value shapes the usage grammar recognizes: `-C <path>` (space-
    /// separated) and `--git-dir=<path>` (`=`-joined) are both a
    /// *required* value; `--exec-path[=<path>]` (bracketed) is *optional*.
    #[test]
    fn usage_synopsis_flag_value_shapes_are_captured() {
        let raw = "usage: git [-C <path>] [--exec-path[=<path>]] [--git-dir=<path>]\n";
        let parsed = parse(raw);

        let c = parsed
            .flags
            .iter()
            .find(|f| f.short == Some('C'))
            .expect("-C recovered");
        assert_eq!(c.value_name.as_deref(), Some("<path>"));
        assert_eq!(c.value_kind, mandible_core::ValueKind::Required);

        let exec_path = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("exec-path"))
            .expect("--exec-path recovered");
        assert_eq!(exec_path.value_kind, mandible_core::ValueKind::Optional);

        let git_dir = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("git-dir"))
            .expect("--git-dir recovered");
        assert_eq!(git_dir.value_kind, mandible_core::ValueKind::Required);
    }

    /// A mandatory flag some tool's synopsis writes unbracketed —
    /// `ssh-keygen`'s own `-D pkcs11`, `-M generate`, `-I
    /// certificate_identity -s ca_key` — is two bare tokens with no
    /// group around either. Before the bare-token lookahead in
    /// `extract_usage_flags` existed, each flag's own value word was a
    /// second, unrelated `Bare` segment that started with neither `-` nor
    /// anything else this scan reads, so it was silently dropped and the
    /// flag came out looking like a boolean it isn't.
    #[test]
    fn bare_synopsis_flags_recover_their_unbracketed_value() {
        let raw = "usage: ssh-keygen -D pkcs11\n       ssh-keygen -M generate [-O option] output_file\n       ssh-keygen -I certificate_identity -s ca_key [-hU]\n";
        let parsed = parse(raw);

        let d = parsed
            .flags
            .iter()
            .find(|f| f.short == Some('D'))
            .expect("-D recovered");
        assert_eq!(d.value_name.as_deref(), Some("pkcs11"));
        assert_eq!(d.value_kind, mandible_core::ValueKind::Required);

        let m = parsed
            .flags
            .iter()
            .find(|f| f.short == Some('M'))
            .expect("-M recovered");
        assert_eq!(m.value_name.as_deref(), Some("generate"));

        let i = parsed
            .flags
            .iter()
            .find(|f| f.short == Some('I'))
            .expect("-I recovered");
        assert_eq!(i.value_name.as_deref(), Some("certificate_identity"));

        let s = parsed
            .flags
            .iter()
            .find(|f| f.short == Some('s'))
            .expect("-s recovered");
        assert_eq!(s.value_name.as_deref(), Some("ca_key"));
    }

    /// `iptables --help`'s own synopsis line, byte-exact: `-h` takes no
    /// argument, and `(print` is the first word of a parenthetical aside,
    /// not a value. Measured during this fix's own fleet sweep — without
    /// the `(`-guard, `-h` picked up a fabricated value across the whole
    /// `iptables`/`arptables`/`ip6tables` family.
    #[test]
    fn a_parenthetical_aside_after_a_bare_flag_is_never_read_as_its_value() {
        let raw = "Usage: iptables -h (print this help information)\n";
        let parsed = parse(raw);
        let h = parsed
            .flags
            .iter()
            .find(|f| f.short == Some('h'))
            .expect("-h recovered");
        assert_eq!(h.value_name, None, "-h must stay boolean");
    }

    /// The lookahead must never attach a value onto a bare flag that is
    /// immediately followed by *another* flag rather than a value —
    /// `ssh-keygen -k -f krl_file`'s own `-k` takes no argument at all, and
    /// what follows it is `-f`, not `-k`'s value.
    #[test]
    fn a_bare_flag_followed_by_another_bare_flag_stays_boolean() {
        let raw = "usage: ssh-keygen -k -f krl_file\n";
        let parsed = parse(raw);
        let k = parsed
            .flags
            .iter()
            .find(|f| f.short == Some('k'))
            .expect("-k recovered");
        assert_eq!(k.value_name, None, "-k must stay boolean");
    }

    /// The bundled-short-flag collapse, end to end through `parse`:
    /// `tmux`'s real synopsis line, byte-exact. Its `[-2CDlNuVv]` must
    /// become eight boolean switches, and the five genuine value-taking
    /// short flags sharing the same physical line must be untouched — the
    /// only thing separating them from the cluster is a space, so this
    /// asserts both halves together or it asserts nothing useful.
    #[test]
    fn a_synopsis_short_flag_cluster_becomes_one_flag_per_member() {
        let raw = "usage: tmux [-2CDlNuVv] [-c shell-command] [-f file] [-L socket-name]\n            [-S socket-path] [-T features] [command [flags]]\n";
        let parsed = parse(raw);
        for member in "2CDlNuVv".chars() {
            let flag = parsed
                .flags
                .iter()
                .find(|f| f.short == Some(member))
                .unwrap_or_else(|| panic!("-{member} missing from {:?}", parsed.flags));
            assert_eq!(flag.value_name, None, "-{member} is a boolean switch");
            assert_eq!(
                flag.value_kind,
                mandible_core::ValueKind::None,
                "-{member} takes no value"
            );
            assert_eq!(flag.long, None);
            assert!(flag.description.is_none(), "a usage line describes nothing");
        }
        for (short, value) in [
            ('c', "shell-command"),
            ('f', "file"),
            ('L', "socket-name"),
            ('S', "socket-path"),
            ('T', "features"),
        ] {
            let flag = parsed
                .flags
                .iter()
                .find(|f| f.short == Some(short))
                .unwrap_or_else(|| panic!("-{short} missing from {:?}", parsed.flags));
            assert_eq!(flag.value_name.as_deref(), Some(value));
            assert_eq!(flag.value_kind, mandible_core::ValueKind::Required);
        }
    }

    /// The counterweight, and the reason the cluster question is asked on
    /// the *synopsis* path only: `filefrag`'s real usage line carries a
    /// cluster and a glued value spec side by side. `[-b{blocksize}[KMG]]`
    /// is synopsis-sourced and glued exactly like the cluster is, and must
    /// stay one valued flag.
    #[test]
    fn a_glued_value_spec_beside_a_cluster_stays_one_flag() {
        let raw = "Usage: /usr/sbin/filefrag [-b{blocksize}[KMG]] [-BeEksvxX] file ...\n";
        let parsed = parse(raw);
        let b = parsed
            .flags
            .iter()
            .find(|f| f.short == Some('b'))
            .expect("-b recovered");
        assert_eq!(b.value_name.as_deref(), Some("{blocksize}[KMG]"));
        for member in "BeEksvxX".chars() {
            assert!(
                parsed.flags.iter().any(|f| f.short == Some(member)),
                "-{member} missing from {:?}",
                parsed.flags
            );
        }
    }

    /// An option-*table* row of the identical shape is the GCC/Clang
    /// single-dash convention and is genuinely one flag with a glued
    /// value. Only the synopsis path splits, so a described row keeps its
    /// description and its value — splitting it would destroy thousands of
    /// correct fleet-wide parses to fix 58 tools.
    #[test]
    fn an_options_block_row_of_the_same_shape_is_never_split() {
        let raw = "Options:\n  -Zscript      run a script\n  -DMACRO       define a macro\n";
        let parsed = parse(raw);
        let z = parsed
            .flags
            .iter()
            .find(|f| f.short == Some('Z'))
            .expect("-Zscript recovered");
        assert_eq!(z.value_name.as_deref(), Some("script"));
        assert!(
            !parsed.flags.iter().any(|f| f.short == Some('s')),
            "-Zscript must not have been split: {:?}",
            parsed.flags
        );
    }

    /// A cluster in a synopsis whose members are *also* documented in an
    /// options block must not double-count: `flag_spelling_already_present`
    /// already drops a usage-derived duplicate, and expansion feeds it one
    /// candidate per member rather than one for the whole cluster. `od`'s
    /// real shape — a bundle in the usage line, the same switches described
    /// in a table below it.
    #[test]
    fn cluster_members_already_described_in_a_block_are_not_added_twice() {
        let raw = "Usage: od [-abcdfilosx]... [FILE]...\n\nOptions:\n  -a    named characters\n  -b    octal bytes\n";
        let parsed = parse(raw);
        for member in ['a', 'b'] {
            let matches: Vec<&Flag> = parsed
                .flags
                .iter()
                .filter(|f| f.short == Some(member))
                .collect();
            assert_eq!(matches.len(), 1, "-{member}: {matches:?}");
            assert!(
                matches[0].description.is_some(),
                "-{member} must keep the described version"
            );
        }
        // ...and the members the table never described are still recovered.
        for member in ['c', 'd', 'f', 'i', 'l', 'o', 's', 'x'] {
            assert!(
                parsed.flags.iter().any(|f| f.short == Some(member)),
                "-{member} missing from {:?}",
                parsed.flags
            );
        }
    }

    /// Do-not-double-count: a flag documented in *both* the usage synopsis
    /// and an `Options:` block must collapse to one entry, with the
    /// described version's fields — never two `Flag`s for the same
    /// spelling, and never the description dropped in favor of the
    /// synopsis's bare spelling.
    #[test]
    fn usage_and_options_block_duplicates_merge_into_one_described_flag() {
        let raw =
            "usage: widget [--verbose] [<file>]\n\nOptions:\n  --verbose    print extra output\n";
        let parsed = parse(raw);
        let verbose: Vec<&Flag> = parsed
            .flags
            .iter()
            .filter(|f| f.long.as_deref() == Some("verbose"))
            .collect();
        assert_eq!(
            verbose.len(),
            1,
            "expected exactly one flag, got {verbose:?}"
        );
        assert_eq!(
            verbose[0].description.as_ref().map(|d| d.as_str()),
            Some("print extra output")
        );
    }

    /// A synopsis with nothing dash-led at all (no `[OPTIONS]`-shaped
    /// bracket carries a real flag, just a positional-only usage line)
    /// must recover zero flags — the extractor must not invent structure
    /// from empty input, mirroring `apt-get`'s real-world shape (spec
    /// [M-15]: "apt-get's zero flags is correct").
    #[test]
    fn usage_synopsis_with_no_dash_tokens_yields_zero_flags() {
        let parsed = parse("Usage: mytool [FILE]... <target>\n");
        assert!(parsed.flags.is_empty(), "{:?}", parsed.flags);
    }

    /// A malformed/unmatched bracket in a usage line (never seen from a
    /// real tool, but the parser must not panic or misbehave on it) falls
    /// back to treating the stray `[` as part of an ordinary bare token —
    /// still gated by the same "starts with `-`" rule, so it recovers
    /// nothing rather than guessing.
    #[test]
    fn unmatched_bracket_in_usage_line_does_not_panic() {
        let parsed = parse("usage: widget [--flag <value>\n");
        // No panic is the primary assertion; the bracket does eventually
        // close over word boundaries in a way that still yields --flag,
        // since `[--flag` is bare (starts with `-`... actually with `[`)
        // and `<value>` is not flag-shaped. This just documents there is
        // no crash and no fabricated flag from the stray bracket itself.
        assert!(parsed.flags.iter().all(|f| f.long.as_deref() != Some("")));
    }

    /// Regression for the third defect found alongside the two above:
    /// `--[no-]name`, GNU getopt_long's negatable-boolean convention
    /// (git's own `--help` formatter uses it for every negatable boolean).
    /// Before the fix, `try_long` required an alphanumeric immediately
    /// after `--`, so `--[no-]staged` matched neither `try_short` nor
    /// `try_long`: a row with a short spelling (`-S, --[no-]staged`)
    /// rendered with its long name silently dropped, and a long-only row
    /// (`--[no-]ignore-unmerged`) was discarded entirely
    /// (`emit_flags`'s `short.is_none() && long.is_none()` skip). The fix
    /// must recover the *base* name, with `negatable` set and no `[`/`]`
    /// ever appearing in `long`.
    #[test]
    fn negatable_boolean_flags_are_recovered_with_base_names() {
        let raw = "Usage: restore [<options>]\n\nOptions:\n  -S, --[no-]staged     restore the index\n  --[no-]ignore-unmerged\n                        ignore unmerged entries\n  -2, --ours            checkout our version for unmerged files\n";
        let parsed = parse(raw);

        let staged = parsed
            .flags
            .iter()
            .find(|f| f.short == Some('S'))
            .expect("short-spelled negatable flag must not be dropped");
        assert_eq!(staged.long.as_deref(), Some("staged"));
        assert!(staged.negatable);

        let ignore_unmerged = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("ignore-unmerged"))
            .expect("long-only negatable flag must not be dropped entirely");
        assert!(ignore_unmerged.short.is_none());
        assert!(ignore_unmerged.negatable);
        assert_eq!(
            ignore_unmerged.description.as_ref().map(|d| d.as_str()),
            Some("ignore unmerged entries"),
            "the description on the following line must still attach"
        );

        // Control case: no `[no-]`, must be unaffected.
        let ours = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("ours"))
            .expect("non-negatable flag must still parse");
        assert!(!ours.negatable);

        for f in &parsed.flags {
            if let Some(long) = &f.long {
                assert!(
                    !long.contains('[') && !long.contains(']'),
                    "long name must never contain brackets: {long:?}"
                );
            }
        }
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

    /// sed's `--help` has no `Options:`/`Flags:` heading at all — the
    /// output starts directly with `-n, --quiet, --silent`. This must
    /// still be recovered as a (headingless) flags block, own-line
    /// descriptions and all, not silently dropped or misread as commands.
    #[test]
    fn sed_headingless_flags_block_is_recovered() {
        let parsed = parse(SED_HELP);
        let quiet = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("quiet"));
        assert!(
            quiet.is_some(),
            "expected --quiet among {:?}",
            parsed.flags.iter().map(|f| &f.long).collect::<Vec<_>>()
        );
        assert_eq!(quiet.unwrap().short, Some('n'));
        assert!(quiet
            .unwrap()
            .description
            .as_ref()
            .unwrap()
            .as_str()
            .contains("suppress automatic printing"));
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

    /// A tab-aligned entry table is a table. `find_description_gap` looked
    /// only for runs of 2+ *spaces*, so a tool separating its columns with
    /// tabs appeared to have no description column at all — measured on
    /// `mokutil --help`: **38 flags, 0 described**, with the descriptions
    /// plainly present in the output.
    #[test]
    fn tab_separated_entries_have_their_descriptions_recovered() {
        let help = "Usage: mokutil [options]\n\nOptions:\n  \
                    --list-enrolled\t\t\tList the enrolled keys\n  \
                    --import\t\t\t\tImport a key\n";
        let parsed = parse(help);
        let described: Vec<(&str, &str)> = parsed
            .flags
            .iter()
            .map(|f| {
                (
                    f.long.as_deref().unwrap_or(""),
                    f.description.as_ref().map(|d| d.as_str()).unwrap_or(""),
                )
            })
            .collect();
        assert!(
            described.contains(&("list-enrolled", "List the enrolled keys")),
            "{described:?}"
        );
        assert!(
            described.contains(&("import", "Import a key")),
            "{described:?}"
        );
    }

    /// The single-space fallback (spec §6 rule 2b's own fixture,
    /// `corpus/curl/8.5.0-all`, is what surfaced this): a tool that
    /// right-pads *short* specs to a fixed column but simply runs one
    /// space after a *long* one has no aligned column at all for those
    /// rows, so the original 2+-space/tab rule found nothing and the
    /// whole line — placeholder and description together — was read as
    /// the flag spec with an empty description. Measured on real
    /// `curl --help all`: 25.2% described before this fix, 77.1% after.
    #[test]
    fn a_single_space_after_a_value_placeholder_recovers_the_description() {
        let help = "Usage: curl [options...] <url>\n\
                    Options:\n  \
                    --abstract-unix-socket <path> Connect via abstract Unix domain socket\n  \
                    --anyauth     Pick any authentication method\n";
        let parsed = parse(help);
        let described: Vec<(&str, &str)> = parsed
            .flags
            .iter()
            .map(|f| {
                (
                    f.long.as_deref().unwrap_or(""),
                    f.description.as_ref().map(|d| d.as_str()).unwrap_or(""),
                )
            })
            .collect();
        assert!(
            described.contains(&(
                "abstract-unix-socket",
                "Connect via abstract Unix domain socket"
            )),
            "{described:?}"
        );
        // The ordinary, already-working padded row must be unaffected.
        assert!(
            described.contains(&("anyauth", "Pick any authentication method")),
            "{described:?}"
        );
    }

    /// A description one space after the spec, with no placeholder to key
    /// off of — the shape a long flag name forces on a fixed-width table
    /// when the name overruns the description column. Real
    /// `apt-ftparchive --help`: `--md5` used to arrive carrying
    /// `value_name: Control`, with the words "MD5 generation" discarded
    /// from the tree entirely.
    #[test]
    fn a_sentence_one_space_after_the_spec_is_a_description_not_a_value() {
        let help = "Usage: apt-ftparchive [options] command\n\nOptions:\n  \
                    -h    This help text\n  \
                    --md5 Control MD5 generation\n  \
                    --no-delink Enable delinking debug mode\n  \
                    --contents  Control contents file generation\n";
        let parsed = parse(help);
        let by_long = |name: &str| {
            parsed
                .flags
                .iter()
                .find(|f| f.long.as_deref() == Some(name))
                .unwrap_or_else(|| panic!("--{name} must be recovered"))
        };
        for (name, text) in [
            ("md5", "Control MD5 generation"),
            ("no-delink", "Enable delinking debug mode"),
        ] {
            let flag = by_long(name);
            assert_eq!(
                flag.description.as_ref().map(|d| d.as_str()),
                Some(text),
                "--{name}"
            );
            assert_eq!(flag.value_name, None, "--{name} takes no value");
            assert_eq!(flag.value_kind, ValueKind::None, "--{name} takes no value");
        }
        // The already-working padded rows must be untouched.
        assert_eq!(
            by_long("contents").description.as_ref().map(|d| d.as_str()),
            Some("Control contents file generation")
        );
        assert_eq!(
            parsed
                .flags
                .iter()
                .find(|f| f.short == Some('h'))
                .and_then(|f| f.description.as_ref())
                .map(|d| d.as_str()),
            Some("This help text")
        );
    }

    /// The inverse case, and the reason the predicate is what it is: a
    /// genuine ` VALUE` spec must keep parsing as a value. Every shape
    /// here is a real one this project already gets right —
    /// `jdeprscan`'s uppercase `PATH` and pipe-alternation
    /// `7|8|9|…`, `cargo-fmt`'s `<manifest-path>` — plus a lowercase
    /// metavar followed by a capitalized word deeper in the line, which
    /// must not be split at that word.
    #[test]
    fn a_real_value_placeholder_is_never_read_as_a_description() {
        let help = "Usage: tool [options]\n\nOptions:\n  \
                    --class-path PATH\n  \
                    --release 7|8|9|10|11\n  \
                    --manifest-path <manifest-path>\n  \
                    --opt value do a Thing here\n  \
                    --quiet Quiet\n";
        let parsed = parse(help);
        let by_long = |name: &str| {
            parsed
                .flags
                .iter()
                .find(|f| f.long.as_deref() == Some(name))
                .unwrap_or_else(|| panic!("--{name} must be recovered"))
        };
        for (name, value) in [
            ("class-path", "PATH"),
            ("release", "7|8|9|10|11"),
            ("manifest-path", "<manifest-path>"),
        ] {
            let flag = by_long(name);
            assert_eq!(flag.value_name.as_deref(), Some(value), "--{name}");
            assert_eq!(flag.description, None, "--{name} has no description");
        }
        // A lowercase metavar ends the scan: no split may happen at the
        // capitalized `Thing` three words later.
        assert_eq!(by_long("opt").value_name.as_deref(), Some("value"));
        assert_eq!(by_long("opt").description, None);
        // A lone capitalized trailing token is ambiguous and keeps the
        // pre-existing value reading — one word is not a sentence.
        assert_eq!(by_long("quiet").value_name.as_deref(), Some("Quiet"));
    }

    /// A spec that already carries its own value cannot take another one,
    /// so nothing between it and the sentence may be swallowed as a second
    /// value name and then discarded. Real `mariadb --help`:
    /// `--init-command=name SQL Command to execute ...` must keep the word
    /// "SQL", which the spec has no room for and which nothing downstream
    /// would ever read back out.
    #[test]
    fn a_self_valued_spec_keeps_every_word_of_its_description() {
        let help = "Usage: mariadb [OPTIONS]\n\nOptions:\n  \
                    --init-command=name SQL Command to execute when connecting to server.\n";
        let parsed = parse(help);
        let flag = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("init-command"))
            .expect("--init-command must be recovered");
        assert_eq!(flag.value_name.as_deref(), Some("name"));
        assert_eq!(
            flag.description.as_ref().map(|d| d.as_str()),
            Some("SQL Command to execute when connecting to server.")
        );
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
                .find(|f| f.long.as_deref() == Some("for-removal"))
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
                .find(|f| f.short == Some('l'))
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
                .any(|f| f.long.as_deref() == Some("source-override")),
            "a paragraph must never create a flag: {:?}",
            parsed.flags
        );
        assert_eq!(
            parsed
                .flags
                .iter()
                .find(|f| f.long.as_deref() == Some("regex"))
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
                .find(|f| f.long.as_deref() == Some("dry-run"))
                .map(|f| f.description.is_none()),
            Some(true),
            "an indented sentence belongs to the row above it: {:?}",
            parsed.flags
        );
    }

    /// The fallback must never fire when an ordinary aligned gap already
    /// exists — it is consulted only when [`find_multi_space_gap`] finds
    /// nothing anywhere in the line, so a `>`/`]` that happens to sit
    /// inside an already-correctly-split spec (value placeholder) must
    /// not move where the real split lands.
    #[test]
    fn the_single_space_fallback_never_overrides_an_existing_aligned_gap() {
        let help = "Usage: tool [options]\n\nOptions:\n  \
                    -o, --output <file>          Write to file instead of stdout\n";
        let parsed = parse(help);
        let flag = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("output"))
            .expect("--output must be recovered");
        assert_eq!(
            flag.description.as_ref().map(|d| d.as_str()),
            Some("Write to file instead of stdout")
        );
    }

    /// A closing `]` that sits *inside* a placeholder (`[%]` as part of a
    /// larger `<[%]name=...>` token) must never be mistaken for the real
    /// boundary — nothing follows it but more of the placeholder, never a
    /// single space, so scanning must continue to the placeholder's real
    /// closing `>`.
    #[test]
    fn a_bracket_nested_inside_a_placeholder_is_not_mistaken_for_the_boundary() {
        let help = "Usage: curl [options...] <url>\n\
                    Options:\n  \
                    --variable <[%]name=text/@file> Set variable\n";
        let parsed = parse(help);
        let flag = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("variable"))
            .expect("--variable must be recovered");
        assert_eq!(
            flag.value_name.as_deref(),
            Some("<[%]name=text/@file>"),
            "the nested `]` must not truncate the placeholder"
        );
        assert_eq!(
            flag.description.as_ref().map(|d| d.as_str()),
            Some("Set variable")
        );
    }

    /// The other half of tab handling: a second column of *option
    /// spellings* is not a description. `awk --help` prints POSIX short
    /// options beside their GNU long equivalents, so treating the tab as a
    /// description gap gave `-f progfile` the "description"
    /// `--file=progfile`. Reporting that would be **28 flags, 100%
    /// described** and every description a lie; the honest answer is that
    /// awk documents no descriptions here.
    #[test]
    fn a_second_column_of_option_spellings_is_not_a_description() {
        let help = "Usage: awk [options] -f progfile\n\n\
                    POSIX options:\t\tGNU long options: (standard)\n\t\
                    -f progfile\t\t--file=progfile\n\t\
                    -v var=val\t\t--assign=var=val\n";
        let parsed = parse(help);
        for flag in &parsed.flags {
            let desc = flag.description.as_ref().map(|d| d.as_str()).unwrap_or("");
            assert!(
                !desc.starts_with('-'),
                "a flag spelling was reported as a description: {:?} -> {desc:?}",
                flag.short
                    .or(flag.long.as_deref().and_then(|l| l.chars().next()))
            );
        }
    }

    /// A positional documented as the *first* row of an options table must
    /// not cost the whole table. `kill --help` opens `Options:` with
    /// `<pid> [...]`, and because the flags-vs-bare-words decision read
    /// only that first row, every flag below it was thrown away — measured
    /// at **0 flags**, confirmed by deleting just that row, after which
    /// the same build read 6 flags at 100% described.
    #[test]
    fn a_leading_positional_row_does_not_discard_the_options_table() {
        let help = "Usage:\n kill [options] <pid> [...]\n\nOptions:\n \
                    <pid> [...]            send signal to every <pid> listed\n \
                    -q, --queue <value>    integer value to be sent with the signal\n \
                    -L, --table            list all signal names in a nice table\n";
        let parsed = parse(help);
        let longs: Vec<&str> = parsed
            .flags
            .iter()
            .filter_map(|f| f.long.as_deref())
            .collect();
        assert!(longs.contains(&"queue"), "{longs:?}");
        assert!(longs.contains(&"table"), "{longs:?}");
        assert!(
            parsed.subcommands.is_empty(),
            "the positional row must not become a subcommand: {:?}",
            parsed
                .subcommands
                .iter()
                .map(|c| &c.name)
                .collect::<Vec<_>>()
        );
    }

    /// The bound on that lookahead is what keeps it from being "look
    /// harder until you find flags". A bare-word block containing no
    /// `-`-leading row at its own indent must still be a bare-word block.
    #[test]
    fn a_bare_word_block_is_not_reinterpreted_as_flags() {
        let help = "Usage: tool <command>\n\nCommands:\n  \
                    build    Build the thing\n  \
                    clean    Clean the thing\n  \
                    test     Test the thing\n";
        let parsed = parse(help);
        assert!(parsed.flags.is_empty(), "{:?}", parsed.flags);
        let names: Vec<&str> = parsed.subcommands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["build", "clean", "test"]);
    }

    // --- `is_man_page_banner` (spec [M-16] enumeration prerequisite) ---

    /// The exact shape `git bisect --help` renders (`man`'s own banner
    /// convention: identical `NAME(section)` token at both margins around
    /// a centred title) — a true positive.
    #[test]
    fn is_man_page_banner_true_positive_on_a_real_banner_shape() {
        let rendered = "GIT-BISECT(1)                Git Manual                GIT-BISECT(1)\n\n\
                         NAME\n       git-bisect - Use binary search to find the commit...\n";
        assert!(is_man_page_banner(rendered));
    }

    /// git's *root* `--help` is conventional help text, not a man page —
    /// [M-16]'s whole subtlety is that this must come back false. If this
    /// ever flips true, the detection is firing in the wrong place (spec
    /// §7 Tier B step 3 is meant for subcommands like `git bisect`, not
    /// the root `git --help`, which parses cleanly today).
    #[test]
    fn is_man_page_banner_is_false_on_gits_own_root_help() {
        assert!(!is_man_page_banner(GIT_HELP));
    }

    /// Ordinary `--help` output — even output that starts with a single
    /// all-caps word — is not a false positive: a repeated *single* word is
    /// not a banner (there must be a centred title between the two
    /// margins), and `tar`'s help doesn't repeat its own name at both ends
    /// of its first line at all.
    #[test]
    fn is_man_page_banner_is_false_on_ordinary_help_text() {
        assert!(!is_man_page_banner(TAR_HELP));
        assert!(!is_man_page_banner("USAGE USAGE\n"));
    }

    /// Public wrapper delegates to exactly the same rule the parser itself
    /// uses to decide whether to degrade to verbatim — not a second,
    /// possibly-drifted copy.
    #[test]
    fn is_man_page_banner_agrees_with_the_parsers_own_degradation_decision() {
        let man_page = "FOO(1)   Foo Manual   FOO(1)\n\nNAME\n     foo\n";
        assert!(is_man_page_banner(man_page));
        let parsed = parse(man_page);
        assert!(parsed.flags.is_empty());
        assert!(parsed.subcommands.is_empty());
        assert!(parsed.usage.is_empty());
    }

    // --- Multi-column option tables (corpus/lsof/4.95.0, corpus/unzip/6.00) ---

    /// The regression `corpus/lsof/4.95.0` was `[xfail]` for: lsof's
    /// options table packs three flag+description pairs onto one physical
    /// line. Before the column splitter, the generic parser read only the
    /// first flag on each row and swallowed the other two as its
    /// description — under-extracting `-a`/`-b`/`-l`/`-t`/`-v` entirely and
    /// telling a reader `-?` means "AND selections (OR)" (`-a`'s real
    /// text). Every flag here must now be present *and* carry its own
    /// text, not a neighbour's.
    #[test]
    fn lsof_three_column_options_table_is_split_per_flag() {
        let parsed = parse_named(LSOF_HELP, "lsof");
        let desc_of = |short: char| -> String {
            parsed
                .flags
                .iter()
                .find(|f| f.short == Some(short))
                .unwrap_or_else(|| panic!("expected -{short} to be recovered"))
                .description
                .as_ref()
                .map(|t| t.as_str().to_string())
                .unwrap_or_default()
        };
        assert_eq!(desc_of('?'), "list help");
        assert_eq!(desc_of('a'), "AND selections (OR)");
        assert_eq!(desc_of('b'), "avoid kernel blocks");
        assert_eq!(desc_of('l'), "list UID numbers");
        assert_eq!(desc_of('t'), "terse listing");
        assert_eq!(desc_of('v'), "list version info");
        // The misattribution shape itself: no flag's description contains
        // another flag's own spelling from this row.
        assert!(!desc_of('?').contains("-a"));
        assert!(!desc_of('?').contains("-b"));
    }

    /// A block with only *one* description column must still parse exactly
    /// as before — the splitter's block-level gate
    /// (`block_is_multi_column`) requires real, recurring column
    /// alignment, so an ordinary single-column table is untouched. `tar`'s
    /// 171-flag table is the existing net for this; this is a small,
    /// direct check that a ordinary two-word description doesn't get
    /// misread as a second flag+description pair.
    #[test]
    fn a_single_column_block_is_not_treated_as_multi_column() {
        let raw = "Options:\n\
                    \x20 -a, --all       do everything\n\
                    \x20 -b, --bare      minimal output\n\
                    \x20 -c, --count     print a count\n";
        let parsed = parse(raw);
        let all = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("all"))
            .unwrap();
        assert_eq!(all.description.as_ref().unwrap().as_str(), "do everything");
        assert_eq!(parsed.flags.len(), 3);
    }

    /// `nano`-shaped alias row: a short and long spelling of the *same*
    /// option, sharing one description, with nothing between them. Every
    /// row here folds into exactly one field per line (checked directly —
    /// `fields_in_line`'s alias fold), so the block never accumulates the
    /// column-recurrence evidence `block_is_multi_column` requires, and
    /// falls back to the ordinary single-column path for all three rows —
    /// the same path `nano`'s real 52-option table already went through
    /// before this change, unaffected by it. The bar here is what the
    /// false-positive class actually demands: no *phantom* fourth/fifth/
    /// sixth flag gets fabricated out of `--smarthome`/`--breezy`/`--calm`.
    #[test]
    fn an_alias_pair_sharing_one_description_is_not_split_into_two_flags() {
        let raw = "Options:\n\
                    \x20 -A  --smarthome  Enable smart home key\n\
                    \x20 -B  --breezy     Enable breezy mode\n\
                    \x20 -C  --calm       Enable calm mode\n";
        assert_eq!(
            fields_in_line(" -A  --smarthome  Enable smart home key").len(),
            1
        );
        let parsed = parse(raw);
        assert_eq!(parsed.flags.len(), 3, "{:?}", parsed.flags);
        for short in ['A', 'B', 'C'] {
            assert_eq!(
                parsed
                    .flags
                    .iter()
                    .filter(|f| f.short == Some(short))
                    .count(),
                1,
                "expected exactly one -{short}, got {:?}",
                parsed.flags
            );
        }
        assert!(
            !parsed.flags.iter().any(|f| f.short.is_none()),
            "a spellingless (fabricated) flag was emitted: {:?}",
            parsed.flags
        );
    }

    /// `iptables`/`patch`-shaped row: a bare short/long alias pair where
    /// the short spelling's own cell carries what looks like real trailing
    /// text but is actually just its value placeholder (`-p NUM`, `-A
    /// chain` — lower-case, so it isn't recognized by
    /// `is_value_placeholder_only`). Must fold into one field per line
    /// (checked directly), never fabricating a second flag out of the
    /// placeholder text.
    #[test]
    fn a_lowercase_value_placeholder_does_not_fabricate_a_second_flag() {
        let raw = "Options:\n\
                    \x20 --append  -A chain\tAppend to chain\n\
                    \x20 --check   -C chain\tCheck for the existence of a rule\n\
                    \x20 --delete  -D chain\tDelete matching rule from chain\n";
        assert_eq!(
            fields_in_line(" --append  -A chain\tAppend to chain").len(),
            1
        );
        let parsed = parse(raw);
        assert_eq!(parsed.flags.len(), 3, "{:?}", parsed.flags);
        for long in ["append", "check", "delete"] {
            assert_eq!(
                parsed
                    .flags
                    .iter()
                    .filter(|f| f.long.as_deref() == Some(long))
                    .count(),
                1,
                "expected exactly one --{long}, got {:?}",
                parsed.flags
            );
        }
        // No phantom `-A`/`-C`/`-D` split out as its own, separate flag —
        // each real spelling recovered here is the long form only (the
        // pre-existing single-column fallback's own limit on this 3-field
        // "short / long / description" shape, unrelated to and unchanged
        // by this batch), never a fabricated second entry.
        assert!(
            !parsed.flags.iter().any(|f| f.long.is_none()),
            "a spellingless (fabricated) flag was emitted: {:?}",
            parsed.flags
        );
    }

    /// `awk`-shaped row: two columns of option *spellings* (POSIX short
    /// beside GNU long), never flag+description. Must not read the second
    /// column as a real description, and must not split it out as a second
    /// flag either — `is_synonym_not_description`'s single-column check
    /// (unchanged by this batch) is what actually saves this shape, since
    /// the row's own lowercase value placeholder (`-f progfile`) keeps its
    /// primary field from reading as bare, which is exactly why this stays
    /// a block-level single-column fallback rather than a real second
    /// column — matching the existing `a_second_column_of_option_spellings_
    /// is_not_a_description` regression test above.
    #[test]
    fn two_columns_of_bare_option_spellings_are_not_read_as_two_flags() {
        let raw = "Options:\n\
                    \x20 -f progfile       --file=progfile\n\
                    \x20 -v var=val        --assign=var=val\n\
                    \x20 -F fs             --field-separator=fs\n";
        let parsed = parse(raw);
        assert_eq!(parsed.flags.len(), 3, "{:?}", parsed.flags);
        for short in ['f', 'v', 'F'] {
            assert_eq!(
                parsed
                    .flags
                    .iter()
                    .filter(|fl| fl.short == Some(short))
                    .count(),
                1,
                "expected exactly one -{short}, got {:?}",
                parsed.flags
            );
        }
        for flag in &parsed.flags {
            let desc = flag.description.as_ref().map(|d| d.as_str()).unwrap_or("");
            assert!(!desc.starts_with('-'), "{:?} -> {desc:?}", flag.short);
        }
    }

    /// The second independent multi-column net beyond `lsof`
    /// (`corpus/unzip/6.00`): a genuine two-column table, real flag on
    /// both sides of every row. Spot-checks one pair from each of unzip's
    /// two tables (the unlabeled top one and the "modifiers:" one) so a
    /// regression confined to either table or either physical column would
    /// still fail this test.
    #[test]
    fn unzip_two_column_options_table_is_split_per_flag() {
        let parsed = parse_named(UNZIP_HELP, "unzip");
        let desc_of = |short: char| -> String {
            parsed
                .flags
                .iter()
                .find(|f| f.short == Some(short))
                .unwrap_or_else(|| panic!("expected -{short} to be recovered"))
                .description
                .as_ref()
                .map(|t| t.as_str().to_string())
                .unwrap_or_default()
        };
        assert_eq!(desc_of('p'), "extract files to pipe, no messages");
        assert_eq!(desc_of('l'), "list files (short format)");
        assert_eq!(desc_of('n'), "never overwrite existing files");
        assert_eq!(desc_of('q'), "quiet mode (-qq => quieter)");
        assert!(!desc_of('p').contains("-l "));
        assert!(!desc_of('n').contains("-q "));
    }

    // --- Preamble bleeding into the root description (corpus/zoxide/0.9.9) ---

    /// The regression `corpus/zoxide/0.9.9` guards: clap's own `--help`
    /// template renders `<name> <version>` / author / homepage as one
    /// paragraph, a blank line, then the real description. Before the
    /// preamble fix, every leading column-0 line was concatenated
    /// regardless of that blank line, so the root description read "zoxide
    /// 0.9.9 Ajeet D'Souza <98ajeet@gmail.com> https://... A smarter cd
    /// command for your terminal". Nothing was fabricated or missing, so no
    /// existing gate caught it — this is a direct assertion on the text
    /// itself.
    #[test]
    fn zoxide_banner_is_dropped_and_real_description_kept() {
        let parsed = parse_named(ZOXIDE_HELP, "zoxide");
        assert_eq!(
            parsed.description.as_deref(),
            Some("A smarter cd command for your terminal")
        );
    }

    /// A tool with no banner at all — `tar`'s leading prose is a single
    /// paragraph, no blank line before `Usage:` — must be completely
    /// unaffected by the banner-drop logic: `paragraphs.len() > 1` never
    /// holds, so nothing is ever dropped.
    #[test]
    fn a_single_paragraph_description_is_never_dropped_as_a_banner() {
        let parsed = parse(TAR_HELP);
        let desc = parsed.description.as_deref().unwrap_or_default();
        assert!(desc.contains("GNU 'tar' saves many files together"));
    }

    /// A lone paragraph that *happens* to open with a version-shaped first
    /// line, with nothing after it to fall back to, must be kept rather
    /// than discarded — degrading to "no description" is worse than
    /// keeping a paragraph that merely looks unusual.
    #[test]
    fn a_banner_shaped_paragraph_with_no_fallback_is_kept() {
        let raw = "mytool 1.2.3\nDoes a thing.\n\nUsage: mytool [OPTIONS]\n";
        let parsed = parse(raw);
        assert_eq!(
            parsed.description.as_deref(),
            Some("mytool 1.2.3 Does a thing.")
        );
    }

    /// A banner detected purely by contact info (no name-version first
    /// line) is dropped the same way, and — the general-rule requirement —
    /// this must work without ever comparing against the tool's own name.
    #[test]
    fn a_contact_info_only_banner_is_dropped() {
        let raw = "Homepage: https://example.com/mytool\nSupport: help@example.com\n\n\
                    Does a thing well.\n\nUsage: mytool [OPTIONS]\n";
        let parsed = parse_named(raw, "mytool");
        assert_eq!(parsed.description.as_deref(), Some("Does a thing well."));
    }

    /// A multi-sentence banner-shaped first line (more than two tokens)
    /// must not be mistaken for a `<name> <version>` banner just because it
    /// contains a version-looking word partway through.
    #[test]
    fn a_sentence_merely_mentioning_a_version_number_is_not_a_banner() {
        let raw = "Build v2 is faster than v1.\n\nSee the changelog for details.\n\n\
                    Usage: mytool [OPTIONS]\n";
        let parsed = parse(raw);
        // Two *paragraphs* exist here (real fallback content follows), so
        // the banner check genuinely runs — and must say no, because the
        // first paragraph's line is a whole sentence (more than the two
        // bare tokens `<name> <version>` a real banner is), not merely
        // because there's nothing to fall back to.
        assert_eq!(
            parsed.description.as_deref(),
            Some("Build v2 is faster than v1. See the changelog for details.")
        );
    }

    // --- leading option-error line is not a description -------------------

    /// `ssh-keygen --help`'s exact defect, byte-for-byte: its own getopt
    /// complaint about the unrecognized `--help` probe, then its usage
    /// block, and nothing else. The complaint is the *only* leading
    /// paragraph — unlike the banner check above, this must still drop it,
    /// leaving no description at all rather than showing the tool's own
    /// error about the probe.
    #[test]
    fn a_leading_option_error_line_is_dropped_even_as_the_only_paragraph() {
        let raw = "unknown option -- -\nusage: ssh-keygen [-q] [-a rounds]\n";
        let parsed = parse_named(raw, "ssh-keygen");
        assert_eq!(parsed.description, None);
        assert!(!parsed.usage.is_empty(), "usage block must survive");
    }

    /// `c_rehash --help`'s degenerate case: the entire captured output is
    /// one line, busybox-style `Usage error; try -h.`, with no usage block
    /// to recover at all. Still dropped, for the same reason.
    #[test]
    fn a_lone_usage_error_line_is_dropped_with_nothing_left() {
        let parsed = parse_named("Usage error; try -h.\n", "c_rehash");
        assert_eq!(parsed.description, None);
    }

    /// A `<progname>: ` prefix (bare name or full path) is recognized and
    /// stripped before matching the four conventional complaints —
    /// `ping`'s real shape.
    #[test]
    fn a_progname_prefixed_option_error_line_is_dropped() {
        let raw = "/usr/bin/ping: invalid option -- '-'\n\nUsage: ping [options] <destination>\n";
        let parsed = parse_named(raw, "ping");
        assert_eq!(parsed.description, None);
    }

    /// `myisamlog`'s shape: several consecutive complaints, one per
    /// rejected character, with no blank line between them — one
    /// paragraph, several lines, every one of them this exact shape. All
    /// must match for the paragraph to be dropped.
    #[test]
    fn a_paragraph_of_several_option_error_lines_is_dropped() {
        let raw = "illegal option: \"--\"\nillegal option: \"-h\"\nillegal option: \"-e\"\n\nUsage: myisamlog\n";
        let parsed = parse_named(raw, "myisamlog");
        assert_eq!(parsed.description, None);
    }

    /// A real description that merely *contains* one of the four keyword
    /// phrases mid-sentence must never be dropped — the phrase has to open
    /// the (post-prefix) line, not merely occur in it. This is the
    /// `--occurrence`-style false positive the hard constraint on this fix
    /// calls out by name.
    #[test]
    fn a_sentence_mentioning_invalid_option_mid_clause_survives() {
        let raw = "An invalid option combination here raises an error, so check twice.\n\n\
                    Usage: mytool [OPTIONS]\n";
        let parsed = parse(raw);
        assert_eq!(
            parsed.description.as_deref(),
            Some("An invalid option combination here raises an error, so check twice.")
        );
    }

    /// A leading complaint followed by a *second, unrelated* line in the
    /// same paragraph (no blank line between them) must not be dropped —
    /// `is_option_error_paragraph` requires every line in the paragraph to
    /// match, and refuses to guess which lines to keep. `sshd`'s real
    /// shape: its own version banner sits directly under the complaint.
    #[test]
    fn a_mixed_paragraph_with_real_content_is_kept_whole() {
        let raw = "unknown option -- -\nOpenSSH_9.6p1 Ubuntu, OpenSSL 3.0.13\n\n\
                    usage: sshd [-46DdeGiqTtV]\n";
        let parsed = parse_named(raw, "sshd");
        assert_eq!(
            parsed.description.as_deref(),
            Some("unknown option -- - OpenSSH_9.6p1 Ubuntu, OpenSSL 3.0.13")
        );
    }

    /// A trailing continuation clause past the terse-flag bound must not
    /// qualify — `socat`'s real shape (minus its log-format prefix, which
    /// independently also disqualifies it; this isolates the trailer
    /// bound specifically).
    #[test]
    fn a_trailing_continuation_clause_is_not_a_shapely_trailer() {
        assert!(!is_option_error_line(
            "unknown option \"--help\"; use option \"-h\" for help"
        ));
    }

    /// The busybox `Usage error; try -h.` shape matches verbatim, but nothing
    /// that merely resembles it with extra words does.
    #[test]
    fn only_the_exact_usage_error_shape_matches() {
        assert!(is_option_error_line("Usage error; try -h."));
        assert!(!is_option_error_line(
            "Usage error occurred while parsing; try -h."
        ));
    }

    // --- headingless invocation table (spec §7 Tier B) -------------------

    fn find_subcommand<'a>(nodes: &'a [CommandNode], name: &str) -> &'a CommandNode {
        nodes.iter().find(|n| n.name == name).unwrap_or_else(|| {
            panic!(
                "no subcommand named {name:?} among {:?}",
                nodes.iter().map(|n| &n.name).collect::<Vec<_>>()
            )
        })
    }

    fn btrfs_help_txt() -> String {
        let path = format!(
            "{}/../corpus/btrfs/audit-seed2/help.txt",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
    }

    /// The real btrfs shape: two-level nesting (`device` -> `add`/`delete`/
    /// `replace`/...), a shared description across consecutive sibling
    /// rows (`device delete` / `device remove`), a single-level dedup
    /// across two rows that both name the same command (`receive` /
    /// `receive --dump`), a tab-indented description (`device replace`),
    /// and a row with no description at all in the source (`subvolume
    /// snapshot`) ending up genuinely empty rather than fabricating one.
    #[test]
    fn headingless_invocation_table_admits_the_btrfs_shape() {
        let raw = btrfs_help_txt();
        let parsed = parse_named(&raw, "btrfs");

        let balance = find_subcommand(&parsed.subcommands, "balance");
        assert!(balance.heading_attested.eq(&false));
        assert!(balance.invocation_attested);
        assert!(balance.children_filled, "balance's rows supplied children");
        for verb in ["start", "pause", "cancel", "resume", "status"] {
            let child = find_subcommand(&balance.subcommands, verb);
            assert!(child.invocation_attested);
            assert!(!child.heading_attested);
            assert!(
                child.summary.is_some(),
                "balance {verb} has a real description in the source"
            );
        }

        let device = find_subcommand(&parsed.subcommands, "device");
        assert!(device.summary.is_none(), "no row names `device` directly");
        let delete = find_subcommand(&device.subcommands, "delete");
        let remove = find_subcommand(&device.subcommands, "remove");
        assert_eq!(
            delete.summary.as_ref().map(|t| t.as_str()),
            Some("Remove a device from a filesystem"),
            "device delete/remove share one following description block"
        );
        assert_eq!(
            delete.summary.as_ref().map(|t| t.as_str()),
            remove.summary.as_ref().map(|t| t.as_str())
        );
        let replace = find_subcommand(&device.subcommands, "replace");
        assert_eq!(
            replace.summary.as_ref().map(|t| t.as_str()),
            Some("Replace a device (alias of \"btrfs replace\")"),
            "the tab-indented description must still be recovered"
        );

        // `receive` / `receive --dump` both name the single-level command
        // `receive`: `--dump` is flag-shaped, so the run stops at `receive`
        // for both rows, and they must dedup to one node, not two.
        let receive = find_subcommand(&parsed.subcommands, "receive");
        assert_eq!(
            receive.summary.as_ref().map(|t| t.as_str()),
            Some("Receive subvolumes from a stream")
        );
        assert!(receive.subcommands.is_empty());

        // `subvolume set-default` (two rows) shares its description; the
        // parent `subvolume` also carries `snapshot`, whose next line in
        // the source is blank — it must come out empty, not with a
        // fabricated description.
        let subvolume = find_subcommand(&parsed.subcommands, "subvolume");
        let set_default = find_subcommand(&subvolume.subcommands, "set-default");
        assert_eq!(
            set_default.summary.as_ref().map(|t| t.as_str()),
            Some("Set the default subvolume of the filesystem mounted as default.")
        );
        let snapshot = find_subcommand(&subvolume.subcommands, "snapshot");
        assert!(
            snapshot.summary.is_none(),
            "btrfs's own text never describes `subvolume snapshot` — honest emptiness, not fabrication"
        );

        // `help`/`version` are single-level leaves.
        let help = find_subcommand(&parsed.subcommands, "help");
        assert_eq!(
            help.summary.as_ref().map(|t| t.as_str()),
            Some("Display help information")
        );
        let version = find_subcommand(&parsed.subcommands, "version");
        assert_eq!(
            version.summary.as_ref().map(|t| t.as_str()),
            Some("Display btrfs-progs version")
        );
    }

    /// Pins the whole table against truncation, not just a sample of it.
    /// `btrfs device replace <command> [...]`'s description is **tab**-
    /// indented (`"    \tReplace a device..."`) — `leading_whitespace`
    /// counts it as part of the leading-whitespace *character* count (4
    /// spaces + 1 tab = 5), which is still deeper than the table's own row
    /// indent (4), so this must not end the scan early. Every group name in
    /// the source (verified independently by `grep`) must come out, in
    /// particular the ones physically **after** that tab-indented row —
    /// `filesystem` through `version` — proving the scan reads to the real
    /// end of the table (the column-0 "Use --help as an argument..." line)
    /// rather than stopping partway through.
    #[test]
    fn headingless_invocation_table_is_not_truncated_by_the_tab_indented_row() {
        let raw = btrfs_help_txt();
        let parsed = parse_named(&raw, "btrfs");
        let mut names: Vec<&str> = parsed.subcommands.iter().map(|n| n.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec![
                "balance",
                "check",
                "device",
                "filesystem",
                "help",
                "inspect-internal",
                "property",
                "qgroup",
                "quota",
                "receive",
                "replace",
                "rescue",
                "restore",
                "scrub",
                "send",
                "subvolume",
                "version",
            ],
            "the recovered top-level group set must be complete, not a prefix"
        );
    }

    /// [`MIN_INVOCATION_TABLE_ROWS`]'s floor counts rows that actually
    /// *received* a description, not merely rows that were seen: two rows
    /// where only one is ever followed by a deeper-indented line must not
    /// admit, because only one name-row/description-row pair exists.
    #[test]
    fn headingless_invocation_table_refuses_when_only_one_row_is_described() {
        let raw = "    mytool frob start <path>\n        Start frobbing\n    \
                    mytool frob stop <path>\n";
        let parsed = parse_named(raw, "mytool");
        assert!(
            parsed.subcommands.is_empty(),
            "only one row (start) ever got a description; the floor of two must not be met: \
             {:?}",
            parsed.subcommands
        );
    }

    /// A table whose rows do **not** start with the tool's own name is
    /// refused outright — this is the whole basis for the recognizer's
    /// evidence, not an optional extra.
    #[test]
    fn headingless_invocation_table_refuses_rows_not_naming_the_tool() {
        let raw = "  otherprog frobnicate [options] <path>\n      Frobnicate a path\n  \
                    otherprog defrobnicate [options] <path>\n      Defrobnicate a path\n";
        let parsed = parse_named(raw, "mytool");
        assert!(parsed.subcommands.is_empty());
    }

    /// A single name-row/description-row pair sits below the repetition
    /// floor and must not be promoted — one row is as likely to be a
    /// stray example as a table.
    #[test]
    fn headingless_invocation_table_refuses_a_single_pair() {
        let raw = "    mytool frob start <path>\n        Start frobbing\n";
        let parsed = parse_named(raw, "mytool");
        assert!(parsed.subcommands.is_empty());
    }

    /// `wpa_cli`'s real defect: a recognized `commands:` heading whose rows
    /// separate name and description with ` = ` instead of a column gap,
    /// with a couple of rows (a real inconsistency in the tool's own text)
    /// carrying no separator at all. Before this recognizer: 0 subcommands
    /// — every multi-word row failed [`is_command_name_shaped`] whole, and
    /// `note <text>`'s row happened to split at the placeholder boundary,
    /// leaving a description that still began with a literal `= `.
    #[test]
    fn bare_command_table_admits_the_wpa_cli_equals_shape() {
        let raw = "commands:\n  \
                    status [verbose] = get current WPA/EAPOL/EAP status\n  \
                    ifname = get current interface name\n  \
                    note <text> = add a note to wpa_supplicant debug log\n  \
                    log_level <level> [<timestamp>] = update the log level/timestamp\n  \
                    pmksa_add <network_id> <BSSID> <PMKID> <PMK> = store PMKSA cache entry\n  \
                    wps_cancel Cancels the pending WPS operation\n";
        let parsed = parse(raw);

        let status = find_subcommand(&parsed.subcommands, "status");
        assert!(status.invocation_attested);
        assert!(!status.heading_attested);
        assert_eq!(
            status.summary.as_ref().map(|t| t.as_str()),
            Some("get current WPA/EAPOL/EAP status"),
            "the `[verbose]` operand must never survive into the name or the description"
        );

        let log_level = find_subcommand(&parsed.subcommands, "log_level");
        assert_eq!(
            log_level.summary.as_ref().map(|t| t.as_str()),
            Some("update the log level/timestamp")
        );

        let pmksa_add = find_subcommand(&parsed.subcommands, "pmksa_add");
        assert_eq!(
            pmksa_add.summary.as_ref().map(|t| t.as_str()),
            Some("store PMKSA cache entry")
        );

        // No ` = ` at all on this row — the name is still recovered, but
        // never with a guessed description built from the trailing prose.
        let wps_cancel = find_subcommand(&parsed.subcommands, "wps_cancel");
        assert!(
            wps_cancel.summary.is_none(),
            "a row with no separator must come out honestly undescribed, not guessed at"
        );

        // The `= ` separator itself must never survive as description text.
        for name in ["status", "ifname", "note", "log_level", "pmksa_add"] {
            let node = find_subcommand(&parsed.subcommands, name);
            if let Some(summary) = &node.summary {
                assert!(
                    !summary.as_str().starts_with("= "),
                    "{name}'s summary still carries the separator: {summary:?}"
                );
            }
        }
    }

    /// The exact hazard `find_equals_separator_gap`'s own doc comment
    /// warns about, now guarding a second call site: a bare-word block
    /// that legitimately uses `name = description` for something that is
    /// *not* a command list (`wpa_supplicant`'s own `drivers:` block) must
    /// never be read by [`scan_bare_command_table`] — its heading isn't
    /// recognized and nothing put the parser in `command_mode`, so the
    /// gate at the call site refuses to even try.
    #[test]
    fn bare_command_table_never_touches_an_unrecognized_equals_block() {
        let raw = "drivers:\n  \
                    nl80211 = Linux nl80211/cfg80211\n  \
                    wext = Linux wireless extensions (generic)\n";
        let parsed = parse(raw);
        assert!(
            parsed.subcommands.is_empty(),
            "an unrecognized heading's `name = description` block must never become commands: \
             {:?}",
            parsed.subcommands
        );
    }

    /// Found on the real fleet while validating this recognizer (not a
    /// synthetic worry): `fail2ban-client --help`'s real `Command:` table
    /// is column-aligned throughout, but several rows' descriptions wrap
    /// entirely onto their own more-indented lines with nothing on the
    /// name row itself, and one row's own name-side text is itself
    /// column-gapped. The generic engine's "not actually a heading,
    /// rewind" path re-treats every such row as a fresh pseudo-heading
    /// once `command_mode` is stuck on — and a wrapped continuation block
    /// that ends up reachable on its own passes every guard
    /// [`scan_bare_command_table`] has (no column gap, no dash, a
    /// name-shaped leading token, more than one word) purely because
    /// ordinary English is indistinguishable from a command name by shape
    /// alone. Gating on `recognized` alone (never inherited through
    /// `command_mode`) closes this: none of these pseudo-headings
    /// themselves mention "command".
    #[test]
    fn bare_command_table_does_not_leak_through_a_sticky_command_mode_chain() {
        // The `BASIC` sub-heading is load-bearing: at indent 45 immediately
        // above rows at indent 4, it makes `bare_block_end` end the block
        // after `BASIC` alone (the real shape `fail2ban-client --help`
        // prints), which is what fragments the rest of the table into the
        // engine's row-by-row "not actually a heading, rewind" recursion —
        // without it, the whole block is read in one `scan_bare_block`
        // pass and this test does not exercise the hazard at all.
        let raw = "Command:\n                                             \
                    BASIC\n    \
                    start                                    starts the server and the jails\n    \
                    reload [--restart] [--unban] [--all]     reloads the configuration without\n                                             \
                    restarting of the server, the\n                                             \
                    option activates completely\n    \
                    stop                                     stops all jails and terminate the\n";
        let parsed = parse_named(raw, "fail2ban-client");
        for bogus in ["restarting", "option", "completely", "of", "the"] {
            assert!(
                parsed.subcommands.iter().all(|n| n.name != bogus),
                "{bogus:?} is a fragment of wrapped prose, never a command: {:?}",
                parsed.subcommands
            );
        }
    }

    /// Found on the real fleet: `trash-put --help` closes with "To remove
    /// a file whose name starts with a '-' ... use one of these
    /// commands:" — an ordinary sentence, not a section heading, that
    /// happens to satisfy `mentions_commands_word` because "commands"
    /// appears in it as a whole word. The two lines beneath it are a
    /// worked example of invoking a *different* program (`trash`, not
    /// `trash-put`) twice, not a table of `trash-put`'s own subcommands.
    /// Both example lines qualify by every other rule
    /// [`scan_bare_command_table`] has, but they share one name, and the
    /// distinct-names floor refuses to treat one repeated example as
    /// repetition evidence.
    #[test]
    fn bare_command_table_refuses_one_name_repeated_by_a_worked_example() {
        let raw = "use one of these commands:\n\n    \
                    trash -- -foo\n\n    \
                    trash ./-foo\n";
        let parsed = parse_named(raw, "trash-put");
        assert!(
            parsed.subcommands.is_empty(),
            "two invocations of one program are not two commands: {:?}",
            parsed.subcommands
        );
    }

    /// `apt-ftparchive`'s real defect: `Commands:` carries its first row on
    /// its own physical line, and the remaining rows are pure `name
    /// operand...` with no description at all. Before this recognizer: 0
    /// subcommands, and the whole line (including every continuation row)
    /// was absorbed into the root description/group text.
    #[test]
    fn heading_inline_row_admits_the_apt_ftparchive_shape() {
        let raw = "Usage: apt-ftparchive [options] command\n\
                    Commands: packages binarypath [overridefile [pathprefix]]\n          \
                    sources srcpath [overridefile [pathprefix]]\n          \
                    contents path\n          \
                    release path\n          \
                    generate config [groups]\n          \
                    clean config\n";
        let parsed = parse(raw);

        let mut names: Vec<&str> = parsed.subcommands.iter().map(|n| n.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec!["clean", "contents", "generate", "packages", "release", "sources"],
            "all six real commands, and nothing else, must be recovered: {names:?}"
        );

        // `sources srcpath` must never promote `srcpath` — itself
        // name-shaped — to a second command or a grandchild.
        assert!(
            parsed.subcommands.iter().all(|n| n.subcommands.is_empty()),
            "no row's operand may become a child command"
        );
        for name in [
            "srcpath",
            "binarypath",
            "overridefile",
            "pathprefix",
            "groups",
        ] {
            assert!(
                parsed.subcommands.iter().all(|n| n.name != name),
                "{name} is an operand, never a command"
            );
        }

        for node in &parsed.subcommands {
            assert!(node.invocation_attested, "{}", node.name);
            assert!(!node.heading_attested, "{}", node.name);
            assert!(
                node.summary.is_none(),
                "{}'s row carries only operands, never a description: {:?}",
                node.name,
                node.summary
            );
        }
    }

    /// pngfix's real near-miss (`corpus/pngfix/1.6.43/help.stderr.txt`):
    /// `--strip=[none|crc|...]:` carries no description on its own line,
    /// so [`entry_row_carries_own_description`] refuses to end the flags
    /// block there, and the whole value-list stays that flag's own
    /// description exactly as before this recognizer existed. None of
    /// these lines start with `pngfix`, so the new recognizer is never
    /// even reached.
    #[test]
    fn pngfix_strip_choice_list_stays_a_flag_description() {
        let raw = "Usage: pngfix {[options] png-file}\n\
                    OPTIONS\n\
                    \x20\x20\x20\x20--strip=[none|crc|unsafe|unused|transform|color|all]:\n\
                    \x20\x20\x20\x20\x20\x20\x20\x20none (default):   Retain all chunks.\n\
                    \x20\x20\x20\x20\x20\x20\x20\x20crc:    Remove chunks with a bad CRC.\n";
        let parsed = parse_named(raw, "pngfix");
        assert!(parsed.subcommands.is_empty());
        let strip = flag_named(&parsed, "strip");
        assert!(
            strip
                .description
                .as_ref()
                .is_some_and(|d| d.as_str().contains("Retain all chunks")),
            "the choice list must remain --strip's own description: {:?}",
            strip.description
        );
    }

    /// pod2man's real near-miss (`corpus/pod2man/5.01/help.txt`):
    /// `--guesswork=rule[,rule...]` likewise carries nothing on its own
    /// line, so its whole description (including the enum-shaped `all`/
    /// `none` rule names deep inside it) must stay attached to that one
    /// flag, never promoted to subcommands.
    #[test]
    fn pod2man_guesswork_value_list_stays_a_flag_description() {
        let raw = "Usage: pod2man [options]\n\
                    OPTIONS AND ARGUMENTS\n\
                    \x20\x20\x20\x20--guesswork=rule[,rule...]\n\
                    \x20\x20\x20\x20\x20\x20\x20\x20Adjust the guesswork applied.\n\
                    \x20\x20\x20\x20\x20\x20\x20\x20The special rule \"all\" enables all guesswork.\n";
        let parsed = parse_named(raw, "pod2man");
        assert!(parsed.subcommands.is_empty());
        let guesswork = flag_named(&parsed, "guesswork");
        assert!(
            guesswork
                .description
                .as_ref()
                .is_some_and(|d| d.as_str().contains("all guesswork")),
            "the value-list description must survive intact: {:?}",
            guesswork.description
        );
    }

    /// An ordinary prose paragraph, even one that happens to repeat the
    /// tool's own name at the start of successive sentences, must not be
    /// promoted: the sentences aren't name-shaped after the tool's name
    /// (they're prose), so the leading-run test refuses every row.
    #[test]
    fn a_prose_paragraph_is_not_promoted() {
        let raw = "    mytool is a tool that helps you manage things well.\n\
                    \x20\x20\x20\x20mytool also has a web site with more information.\n";
        let parsed = parse_named(raw, "mytool");
        assert!(parsed.subcommands.is_empty());
    }

    // --- existence attestation (spec [M-10]) ------------------------------

    #[test]
    fn token_occurs_literally_accepts_a_real_whole_token() {
        assert!(token_occurs_literally(
            "btrfs balance start [options] <path>",
            "balance"
        ));
    }

    /// A name must not "occur" merely as a substring of a longer token —
    /// this is the whole-token half of the guard, and the one a naive
    /// `raw.contains(name)` would get wrong.
    #[test]
    fn token_occurs_literally_rejects_a_mere_substring() {
        assert!(!token_occurs_literally("btrfs subvolume list", "sub"));
        assert!(!token_occurs_literally("btrfs subvolume list", "volume"));
    }

    #[test]
    fn token_occurs_literally_rejects_a_name_absent_from_the_text() {
        assert!(!token_occurs_literally(
            "btrfs balance start [options] <path>",
            "nonexistent"
        ));
    }

    // --- the aligned multi-column option table --------------------------
    //
    // `spelling_run` + `block_has_aligned_spelling_column`: a row whose
    // second column is itself another spelling of the same option, not the
    // start of its description. Every fixture below is byte-exact from a
    // real tool's own `--help`, and every one of the three positive tests
    // fails on the parent commit.

    /// `nano --help`, verbatim (`corpus/nano/7.2/help.txt`): short column,
    /// long column, description column, with and without a value.
    const NANO_TABLE: &str = concat!(
        " Option         Long option             Meaning\n",
        " -A             --smarthome             Enable smart home key\n",
        " -B             --backup                Save backups of existing files\n",
        " -C <dir>       --backupdir=<dir>       Directory for saving unique backup files\n",
        " -J <number>    --guidestripe=<number>  Show a guiding bar at this column\n",
    );

    /// `jdeprscan --help`, verbatim (`corpus/jdeprscan/audit-seed2/help.txt`):
    /// two columns and no description column at all. The `-? -h --help` row
    /// is included deliberately — it is the out-of-scope multi-short shape,
    /// and it must come through exactly as it did before.
    const JDEPRSCAN_TABLE: &str = concat!(
        "options:\n",
        "        --for-removal\n",
        "  -? -h --help\n",
        "  -l    --list\n",
        "  -v    --verbose\n",
    );

    /// `awk --help`, verbatim (`corpus/awk/5.2.1/help.txt`): the same shape
    /// aligned with tabs rather than spaces.
    const AWK_TABLE: &str = concat!(
        "Short options:\t\tGNU long options: (extensions)\n",
        "\t-b\t\t\t--characters-as-bytes\n",
        "\t-c\t\t\t--traditional\n",
        "\t-C\t\t\t--copyright\n",
        "\t-d[file]\t\t--dump-variables[=file]\n",
    );

    #[test]
    fn nanos_long_column_is_a_spelling_not_the_start_of_the_description() {
        let parsed = parse(NANO_TABLE);
        let a = flag_named(&parsed, "smarthome");
        assert_eq!(a.short, Some('A'));
        assert_eq!(
            a.description.as_ref().map(|t| t.as_str()),
            Some("Enable smart home key"),
            "the description must be the third column only — before this \
             rule it read `--smarthome Enable smart home key`"
        );
        let c = flag_named(&parsed, "backupdir");
        assert_eq!(c.short, Some('C'));
        assert_eq!(c.value_name.as_deref(), Some("<dir>"));
        assert_eq!(c.value_kind, ValueKind::Required);
        assert_eq!(
            c.description.as_ref().map(|t| t.as_str()),
            Some("Directory for saving unique backup files")
        );
        // Nothing invented from the table's own header row.
        assert!(
            !parsed
                .flags
                .iter()
                .any(|f| f.long.as_deref() == Some("option")),
            "the `Option  Long option  Meaning` header is not a flag"
        );
    }

    #[test]
    fn jdeprscans_two_column_table_recovers_the_long_form_it_used_to_drop() {
        let parsed = parse(JDEPRSCAN_TABLE);
        for (long, short) in [("list", 'l'), ("verbose", 'v')] {
            let flag = flag_named(&parsed, long);
            assert_eq!(flag.short, Some(short));
            assert_eq!(
                flag.description, None,
                "the row has no description column, and none may be invented"
            );
        }
        // The out-of-scope shape, unchanged: `-? -h --help` names two
        // shorts and `Flag::short` is one `Option<char>`, so the second is
        // still lost. Asserted rather than left implicit so that a future
        // data-model change has to come here and say so.
        let help = flag_named(&parsed, "help");
        assert_eq!(help.short, Some('?'));
        assert!(
            !parsed.flags.iter().any(|f| f.short == Some('h')),
            "`-h` is still dropped — see corpus/jdeprscan/audit-seed2"
        );
    }

    #[test]
    fn awks_tab_aligned_spelling_columns_are_read_as_spellings() {
        let parsed = parse(AWK_TABLE);
        assert_eq!(flag_named(&parsed, "characters-as-bytes").short, Some('b'));
        assert_eq!(flag_named(&parsed, "traditional").short, Some('c'));
        assert_eq!(flag_named(&parsed, "copyright").short, Some('C'));
        let d = flag_named(&parsed, "dump-variables");
        assert_eq!(d.short, Some('d'));
        assert_eq!(d.value_kind, ValueKind::Optional);
    }

    /// `awk --help`, verbatim (`corpus/awk/5.2.1/help.txt`): the same
    /// tab-aligned table, but every row's cells carry a **value**. The
    /// second cell of each row is the long spelling of the option the
    /// first cell names, and both spell out the same value token.
    const AWK_VALUED_TABLE: &str = concat!(
        "POSIX options:\t\tGNU long options: (standard)\n",
        "\t-f progfile\t\t--file=progfile\n",
        "\t-F fs\t\t\t--field-separator=fs\n",
        "\t-v var=val\t\t--assign=var=val\n",
    );

    #[test]
    fn awks_valued_columns_pair_on_the_value_they_both_name() {
        // The residual PR #21 left behind: the cells are spellings *plus a
        // value*, so `is_value_placeholder_only` (deliberately narrow, to
        // protect `arptables`'s `-A chain`) never recognized them and all
        // three long spellings were lost. Verified on the parent commit:
        // `-f`, `-F` and `-v` parse with no `long` at all.
        let parsed = parse(AWK_VALUED_TABLE);
        for (long, short, value) in [
            ("file", 'f', "progfile"),
            ("field-separator", 'F', "fs"),
            ("assign", 'v', "var=val"),
        ] {
            let flag = flag_named(&parsed, long);
            assert_eq!(flag.short, Some(short));
            assert_eq!(
                flag.value_name.as_deref(),
                Some(value),
                "the shared value is carried once, verbatim, never doubled"
            );
            assert_eq!(flag.value_kind, ValueKind::Required);
            assert_eq!(
                flag.description, None,
                "this table has no description column, and none may be invented"
            );
        }
        assert_eq!(parsed.flags.len(), 3, "no fourth flag invented");
    }

    #[test]
    fn a_valued_pair_keeps_an_optional_value_optional() {
        // `-d[file]` / `--dump-variables[=file]`, and the quoted and
        // alternation-valued rows beside them: the recovered flag must
        // carry the value's own *kind*, not merely its name. The bracket
        // is where a short and a long spelling of one option legitimately
        // differ (`[file]` against `[=file]`), which is why `value_token`
        // compares them with that punctuation stripped.
        let parsed = parse(concat!(
            "Short options:\t\tGNU long options: (extensions)\n",
            "\t-d[file]\t\t--dump-variables[=file]\n",
            "\t-e 'program-text'\t--source='program-text'\n",
            "\t-E file\t\t\t--exec=file\n",
            "\t-L[fatal|invalid|no-ext]\t--lint[=fatal|invalid|no-ext]\n",
        ));
        let d = flag_named(&parsed, "dump-variables");
        assert_eq!(d.short, Some('d'));
        assert_eq!(d.value_name.as_deref(), Some("file"));
        assert_eq!(d.value_kind, ValueKind::Optional);
        let e = flag_named(&parsed, "source");
        assert_eq!(e.short, Some('e'));
        assert_eq!(
            e.value_name.as_deref(),
            Some("'program-text'"),
            "a quoted value survives whole — rejoining both cells verbatim \
             used to leave `'program-text',` and lose `--source` entirely"
        );
        assert_eq!(e.value_kind, ValueKind::Required);
        let exec = flag_named(&parsed, "exec");
        assert_eq!(exec.short, Some('E'));
        assert_eq!(exec.value_name.as_deref(), Some("file"));
        assert_eq!(exec.value_kind, ValueKind::Required);
        let lint = flag_named(&parsed, "lint");
        assert_eq!(lint.short, Some('L'));
        assert_eq!(lint.value_name.as_deref(), Some("fatal|invalid|no-ext"));
        assert_eq!(lint.value_kind, ValueKind::Optional);
    }

    #[test]
    fn pairing_never_changes_the_value_a_row_already_parsed_to() {
        // `less --help`, verbatim with its overstrike bytes stripped: the
        // short cell's brackets say the value is optional and the long
        // cell's `=` says it is required, so the two cells disagree about
        // `ValueKind` while naming the same value.
        //
        // This one is a **guard, not a proof of the fix**, and says so:
        // stripped of the overstrike these cells are already
        // `is_value_placeholder_only`, so the parent commit pairs this row
        // by the older rule and reaches `Optional`/`prompt` on its own.
        // What it pins is the rejoin. An earlier draft of this change kept
        // the *last* cell of a value-paired run verbatim, which turned this
        // row into `-P, --prompt=[prompt]` and silently promoted it to
        // `Required` with `[prompt]` stranded inside the value's own name.
        // Real `less --help` does not strip the overstrike, and its raw
        // bytes desync the column offsets enough that the row only pairs at
        // all through `cells_name_the_same_value` — which is how that
        // promotion was caught, in a full-`PATH` sweep, on three tools that
        // were working fine.
        let parsed = parse(concat!(
            "  -p [pattern]  --pattern=[pattern]\n",
            "                  Start at pattern (from command line).\n",
            "  -P [prompt]   --prompt=[prompt]\n",
            "                  Define new prompt.\n",
        ));
        let prompt = flag_named(&parsed, "prompt");
        assert_eq!(prompt.short, Some('P'));
        assert_eq!(prompt.value_name.as_deref(), Some("prompt"));
        assert_eq!(
            prompt.value_kind,
            ValueKind::Optional,
            "the bracket the short cell wrote still decides the kind"
        );
        assert_eq!(
            prompt.description.as_ref().map(|t| t.as_str()),
            Some("Define new prompt.")
        );
        let pattern = flag_named(&parsed, "pattern");
        assert_eq!(pattern.short, Some('p'));
        assert_eq!(pattern.value_name.as_deref(), Some("pattern"));
        assert_eq!(pattern.value_kind, ValueKind::Optional);
    }

    #[test]
    fn a_valued_pair_with_a_third_description_column_keeps_the_description() {
        // `ntfsmove`/`ntfswipe --help`, verbatim: the same shape with the
        // value detached on *both* sides and a real description after it.
        let parsed = parse(concat!(
            "Options:\n",
            "    -c num   --count num   Number of times to write(default = 1)\n",
            "    -b list  --bytes list  List of values to write(default = 0)\n",
        ));
        let count = flag_named(&parsed, "count");
        assert_eq!(count.short, Some('c'));
        assert_eq!(count.value_name.as_deref(), Some("num"));
        assert_eq!(count.value_kind, ValueKind::Required);
        assert_eq!(
            count.description.as_ref().map(|t| t.as_str()),
            Some("Number of times to write(default = 1)")
        );
        let bytes = flag_named(&parsed, "bytes");
        assert_eq!(bytes.short, Some('b'));
        assert_eq!(bytes.value_name.as_deref(), Some("list"));
    }

    #[test]
    fn a_flag_followed_by_unrelated_text_is_never_paired_by_value() {
        // `arptables --help`, verbatim — the case `is_value_placeholder_only`
        // stays narrow for, and the reason the value test is *equality
        // between two cells* rather than "the cell has a trailing word".
        // `--append` names no value at all, so there is nothing for `-A
        // chain` to match, and the row keeps the reading it had.
        let parsed = parse(concat!(
            "Commands:\n",
            "--append  -A chain\t\tAppend to chain\n",
            "--delete  -D chain rulenum\t\tDelete rule rulenum from chain\n",
            "--insert  -I chain [rulenum]\t\tInsert in chain as rulenum\n",
        ));
        assert!(
            !parsed
                .flags
                .iter()
                .any(|f| f.value_name.as_deref() == Some("chain")
                    && f.long.as_deref() == Some("append")
                    && f.short == Some('A')),
            "`--append  -A chain` must not be merged into one valued flag: {:?}",
            parsed
                .flags
                .iter()
                .map(|f| f.spelling())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn two_cells_naming_different_values_are_not_paired() {
        // `objcopy --help`, verbatim: the second cell is a cross-reference
        // sentence that happens to start with a flag and end with the same
        // placeholder the first cell used. The value tokens differ (`<file>`
        // against a whole sentence), so no pairing happens and the sentence
        // stays the description it is.
        let parsed = parse(concat!(
            "Options:\n",
            "     --strip-symbols <file>        -N for all symbols listed in <file>\n",
            "     --keep-symbols <file>         -K for all symbols listed in <file>\n",
            "     --weaken-symbols <file>       -W for all symbols listed in <file>\n",
        ));
        let strip = flag_named(&parsed, "strip-symbols");
        assert_eq!(
            strip.short, None,
            "`-N for all symbols listed in <file>` is prose, not this flag's short spelling"
        );
        assert_eq!(strip.value_name.as_deref(), Some("<file>"));
    }

    #[test]
    fn a_description_that_merely_begins_with_a_flag_spelling_keeps_it() {
        // The inverse case, and the whole reason `is_spelling_only_cell`
        // requires the cell to be a spelling *and stop*: these second cells
        // carry real words, so they are descriptions and must survive whole.
        let parsed = parse(concat!(
            "options:\n",
            "  -x    --foo is a synonym for --bar\n",
            "  -y    --baz is a synonym for --qux\n",
            "  -z    -1 means unlimited here\n",
        ));
        let x = parsed
            .flags
            .iter()
            .find(|f| f.short == Some('x'))
            .expect("-x survives");
        assert_eq!(
            x.description.as_ref().map(|t| t.as_str()),
            Some("--foo is a synonym for --bar"),
            "a description beginning with a spelling is still a description"
        );
        assert!(
            !parsed
                .flags
                .iter()
                .any(|f| f.long.as_deref() == Some("foo")),
            "`--foo` here is prose about another flag, not this flag's own name"
        );
    }

    #[test]
    fn a_second_column_that_never_aligns_is_not_read_as_a_spelling() {
        // `lto-dump --help`, verbatim: the second column is a *default
        // value*, not a spelling, and the only thing separating it from a
        // real alias column is that it never lands twice at the same
        // offset. This is the one false positive the per-row shape test
        // alone admitted over all 2,301 frozen captures.
        let parsed = parse(concat!(
            "options:\n",
            "  --param=logical-op-non-short-circuit=<0,1> \t-1\n",
            "  --param=prefetch-minimum-stride= \t-1\n",
            "  --param=vect-max-peeling-for-alignment=<0,64> \t-1\n",
        ));
        assert!(
            !parsed.flags.iter().any(|f| f.short == Some('1')),
            "a misaligned default-value column must not become a short spelling: {:?}",
            parsed
                .flags
                .iter()
                .map(|f| f.spelling())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn two_independent_spellings_of_the_same_kind_are_never_merged() {
        // A run of two longs, or two shorts, is as easily a genuine
        // two-column table of separate options as an alias pair, and
        // merging there would destroy a flag — so `spelling_run` claims
        // only short-plus-long and leaves this shape exactly as it found
        // it.
        //
        // What it finds is *already* lossy, and that is not this rule's
        // doing: the single-column split cuts at the gap and
        // `is_synonym_not_description` then blanks the second spelling
        // rather than asserting it as prose, so `--beta`, `--delta` and
        // `--zeta` are dropped here on the parent commit too. This test
        // pins the no-merge promise, not that loss.
        let parsed = parse(concat!(
            "options:\n",
            "  --alpha    --beta\n",
            "  --gamma    --delta\n",
            "  --epsilon  --zeta\n",
        ));
        for long in ["alpha", "gamma", "epsilon"] {
            let flag = flag_named(&parsed, long);
            assert_eq!(
                flag.short, None,
                "--{long} must not absorb its neighbour as a spelling"
            );
            assert_eq!(
                flag.description, None,
                "--{long} must not absorb its neighbour as a description either"
            );
        }
        assert_eq!(
            parsed.flags.len(),
            3,
            "no flag invented and none merged away: {:?}",
            parsed
                .flags
                .iter()
                .map(|f| f.spelling())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn one_suggestive_row_is_not_a_column() {
        // Recurrence, not suggestion: a single row of the shape in an
        // otherwise ordinary block changes nothing.
        let parsed = parse(concat!(
            "options:\n",
            "  -a    do the first thing\n",
            "  -b    --beta\n",
            "  -c    do the third thing\n",
        ));
        assert!(
            !parsed
                .flags
                .iter()
                .any(|f| f.long.as_deref() == Some("beta")),
            "one row is not evidence of a column"
        );
    }

    /// Symptom 1 (`update-xmlcatalog`, `wpa_supplicant`): a bare `= `
    /// separator with only one space on each side has no 2+-space gap
    /// anywhere on the line, so before `find_equals_separator_gap` the
    /// whole row fell into `grammar::parse_flag_spec` and the description
    /// was lost outright. Measured on `wpa_supplicant --help`: "low
    /// confidence: 9% parsed" across its ~28 flags.
    #[test]
    fn a_lone_equals_token_with_single_spacing_recovers_the_description() {
        let help = "Options:\n    \
                    --verbose = be verbose\n    \
                    --sort = sorts the manipulated catalog content\n";
        let parsed = parse(help);
        assert_eq!(
            flag_named(&parsed, "verbose")
                .description
                .as_ref()
                .map(|d| d.as_str()),
            Some("be verbose")
        );
        assert_eq!(
            flag_named(&parsed, "sort")
                .description
                .as_ref()
                .map(|d| d.as_str()),
            Some("sorts the manipulated catalog content")
        );
        for flag in &parsed.flags {
            let desc = flag.description.as_ref().map(|d| d.as_str()).unwrap_or("");
            assert!(!desc.starts_with('='), "separator leaked into: {desc:?}");
        }
    }

    /// Same shape, short-flag form (`wpa_supplicant`'s `options:` block):
    /// `-b = optional bridge interface name`.
    #[test]
    fn a_lone_equals_token_recovers_a_short_flags_description() {
        let help = "options:\n  \
                    -b = optional bridge interface name\n  \
                    -B = run daemon in the background\n";
        let parsed = parse(help);
        let b = parsed
            .flags
            .iter()
            .find(|f| f.short == Some('b'))
            .expect("-b must be recovered");
        assert_eq!(
            b.description.as_ref().map(|d| d.as_str()),
            Some("optional bridge interface name")
        );
    }

    /// Symptom 2 (`update-xmlcatalog`'s `With:` block): the column *is*
    /// aligned, so `find_multi_space_gap` already cuts correctly, but the
    /// description keeps its leading `= ` (`= a local filename`). This is
    /// `strip_equals_separator`'s job, applied after the split rather than
    /// changing where the split happens.
    #[test]
    fn an_aligned_column_still_strips_its_leading_equals_separator() {
        let help = "With:\n    \
                    --file <file>       = a local filename\n    \
                    --id <id>           = catalog entry idenitifier\n";
        let parsed = parse(help);
        let file = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("file"))
            .expect("--file must be recovered");
        assert_eq!(
            file.description.as_ref().map(|d| d.as_str()),
            Some("a local filename")
        );
    }

    /// Only the *separator* `=` is stripped; a second `=` inside the
    /// description proper is text and must survive verbatim.
    /// `update-xmlcatalog`: `--root ... = the root XML catalog (=
    /// /etc/xml/catalog)`.
    #[test]
    fn a_second_equals_inside_the_description_is_left_alone() {
        let help = "With:\n    \
                    --root              = the root XML catalog (= /etc/xml/catalog)\n";
        let parsed = parse(help);
        let root = flag_named(&parsed, "root");
        assert_eq!(
            root.description.as_ref().map(|d| d.as_str()),
            Some("the root XML catalog (= /etc/xml/catalog)")
        );
    }

    /// `--flag =` with nothing after the separator keeps today's behaviour
    /// (no description invented from an empty tail).
    #[test]
    fn an_equals_separator_with_an_empty_tail_invents_no_description() {
        let help = "Options:\n  --flag =\n";
        let parsed = parse(help);
        let flag = flag_named(&parsed, "flag");
        assert_eq!(flag.description, None);
    }

    /// Inverse case: `=` deep inside a description, not a separator at
    /// all. `ffprobe --help`/`ffplay --help`:
    /// `-http_seekable <boolean> .D... Use HTTP partial requests, 0 =
    /// disable, 1 = enable, -1 = auto (default auto)`. A real aligned
    /// column gap exists before any `=`, so `find_multi_space_gap` must
    /// keep winning and `find_equals_separator_gap` must never run.
    #[test]
    fn equals_signs_inside_a_sentence_are_not_mistaken_for_a_separator() {
        // `-http_seekable` is one of the underscored single-dash long
        // options `repair_single_dash_long_options` recovers, so the name
        // is whole and the whole value-spec column stays in the
        // description. What matters *here* is orthogonal to both: the `=`
        // signs inside the sentence must not be mistaken for the glued
        // `=value` separator and cut the row short.
        let help = "Options:\n  \
                    -http_seekable     <boolean>    .D......... Use HTTP partial requests, 0 = disable, 1 = enable, -1 = auto (default auto)\n";
        let parsed = parse(help);
        let flag = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("http_seekable"))
            .expect("-http_seekable must be recovered as one single-dash long option");
        assert!(flag.single_dash);
        assert_eq!(
            flag.description.as_ref().map(|d| d.as_str()),
            Some("<boolean> .D......... Use HTTP partial requests, 0 = disable, 1 = enable, -1 = auto (default auto)")
        );
    }

    /// Inverse case: `llc-18`/`opt-18`/`bugpoint-18`'s
    /// `--enable-gvn-hoist ... - Enable the GVN hoisting pass (default =
    /// off)`. A huge aligned gap, then a ` - ` dash separator; the `=`
    /// inside `(default = off)` is deep in the description and must not
    /// move the cut.
    #[test]
    fn equals_signs_after_a_dash_separator_are_left_in_the_description() {
        let help = "Options:\n  \
                    --enable-gvn-hoist                                                    - Enable the GVN hoisting pass (default = off)\n";
        let parsed = parse(help);
        let flag = flag_named(&parsed, "enable-gvn-hoist");
        assert_eq!(
            flag.description.as_ref().map(|d| d.as_str()),
            Some("- Enable the GVN hoisting pass (default = off)")
        );
    }

    /// Inverse case: `ntfswipe --help`'s
    /// `-c num   --count num   Number of times to write(default = 1)` —
    /// an aligned multi-column row (two spellings, then description) whose
    /// description itself contains `=`.
    #[test]
    fn equals_signs_in_a_two_column_rows_description_are_untouched() {
        let help = "Options:\n    \
                    -c num   --count num   Number of times to write(default = 1)\n    \
                    -b list  --bytes list  List of values to write(default = 0)\n";
        let parsed = parse(help);
        let flag = flag_named(&parsed, "count");
        let desc = flag.description.as_ref().map(|d| d.as_str()).unwrap_or("");
        assert!(
            desc.contains("(default = 1)"),
            "description lost its own `=`: {desc:?}"
        );
    }

    /// Inverse case: `lvmpolld --help`'s
    /// `-t|--timeout     Time to wait in seconds before shutdown on idle
    /// (missing or 0 = inifinite)`.
    #[test]
    fn equals_signs_in_a_piped_alias_rows_description_are_untouched() {
        let help = "Options:\n   \
                    -t|--timeout     Time to wait in seconds before shutdown on idle (missing or 0 = inifinite)\n";
        let parsed = parse(help);
        let desc = parsed
            .flags
            .iter()
            .find_map(|f| f.description.as_ref().map(|d| d.as_str()))
            .unwrap_or("");
        assert!(
            desc.contains("(missing or 0 = inifinite)"),
            "description lost its own `=`: {desc:?}"
        );
    }

    /// Inverse case: `systemd --help`'s `--dump-core[=BOOL]          Dump
    /// core on crash` — the `=` is inside the spec's own bracket notation,
    /// never a standalone token, and a real aligned gap follows it anyway.
    #[test]
    fn a_bracketed_equals_inside_the_spec_is_not_a_separator() {
        let help = "Options:\n     \
                    --dump-core[=BOOL]          Dump core on crash\n";
        let parsed = parse(help);
        let flag = flag_named(&parsed, "dump-core");
        assert_eq!(flag.value_kind, ValueKind::Optional);
        assert_eq!(
            flag.description.as_ref().map(|d| d.as_str()),
            Some("Dump core on crash")
        );
    }

    /// `man --help`'s deeply-indented continuation line
    /// (`corpus`/queue-capture `man/0.stdout`):
    ///
    /// ```text
    ///   -X, --gxditview[=RESOLUTION]   use groff and display through gxditview
    ///                              (X11):
    ///                              -X = -TX75, -X100 = -TX100, -X100-12 = -TX100-12
    ///   -Z, --ditroff              use groff and force it to produce ditroff
    /// ```
    ///
    /// The `-X = -TX75, ...` line starts with `-` and would qualify for
    /// `find_equals_separator_gap` in isolation, but `scan_flags_block`
    /// never offers it that function: its indent (29) is far past
    /// `-X, --gxditview`'s own entry indent (2) plus
    /// `ENTRY_INDENT_TOLERANCE`, so it is read as a **continuation** line
    /// and appended verbatim to `--gxditview`'s description
    /// (`entries.last_mut()` in `scan_flags_block`) — it never reaches
    /// `split_single_column_entry` or `find_description_gap` at all.
    /// Confirmed unchanged by this fix: the continuation text, `=` signs
    /// included, survives byte-for-byte in the recovered description both
    /// before and after.
    #[test]
    fn mans_deeply_indented_continuation_line_is_unaffected() {
        let help = "Options:\n  \
                    -X, --gxditview[=RESOLUTION]   use groff and display through gxditview\n                             \
                    (X11):\n                             \
                    -X = -TX75, -X100 = -TX100, -X100-12 = -TX100-12\n  \
                    -Z, --ditroff              use groff and force it to produce ditroff\n";
        let parsed = parse(help);
        let gxditview = flag_named(&parsed, "gxditview");
        assert_eq!(
            gxditview.description.as_ref().map(|d| d.as_str()),
            Some(
                "use groff and display through gxditview (X11): -X = -TX75, -X100 = -TX100, -X100-12 = -TX100-12"
            ),
            "the continuation text (and its own `=` signs) must survive verbatim"
        );
    }

    #[test]
    fn find_equals_separator_gap_unit_cases() {
        assert_eq!(
            find_equals_separator_gap("  --verbose = be verbose"),
            Some("  --verbose ".len())
        );
        assert_eq!(
            find_equals_separator_gap("  -b = optional bridge interface name"),
            Some("  -b ".len())
        );
        // No content after the separator: unchanged behaviour.
        assert_eq!(find_equals_separator_gap("  --flag ="), None);
        // A token other than a bare spec/value-spec before the `=` stops
        // the scan (`mariadb`-style prose before an `=`).
        assert_eq!(find_equals_separator_gap("  --foo Set X = Y"), None);
        // Not a flag row at all (no leading `-`): never matched here.
        assert_eq!(
            find_equals_separator_gap("nl80211 = Linux nl80211/cfg80211"),
            None
        );
        // `=` glued to another character is not a lone separator token.
        assert_eq!(find_equals_separator_gap("  --foo =bar"), None);
        assert_eq!(find_equals_separator_gap("  --foo bar= baz"), None);
    }

    #[test]
    fn strip_equals_separator_unit_cases() {
        assert_eq!(strip_equals_separator("= be verbose"), "be verbose");
        assert_eq!(strip_equals_separator("=\tbe verbose"), "be verbose");
        assert_eq!(
            strip_equals_separator("the root XML catalog (= /etc/xml/catalog)"),
            "the root XML catalog (= /etc/xml/catalog)"
        );
        // No leading `=`: unchanged.
        assert_eq!(
            strip_equals_separator("no separator here"),
            "no separator here"
        );
        // `=` not followed by whitespace is not the separator shape.
        assert_eq!(strip_equals_separator("=bar"), "=bar");
        assert_eq!(strip_equals_separator("="), "=");
    }

    /// `sg_emc_trespass`'s real rows, byte-exact from its own capture: a
    /// spaced lone `:` token and two glued colon-terminated specs.
    #[test]
    fn find_colon_separator_gap_unit_cases() {
        assert_eq!(
            find_colon_separator_gap("-d : output debug"),
            Some("-d ".len())
        );
        assert_eq!(
            find_colon_separator_gap("-hr: Set Honor Reservation bit"),
            Some("-hr".len())
        );
        assert_eq!(
            find_colon_separator_gap("-V: print version string then exit"),
            Some("-V".len())
        );
        // A multi-alias row: the colon may sit on a later token, as long as
        // every token before it is spec-shaped.
        assert_eq!(
            find_colon_separator_gap("-o, --output: the output file"),
            Some("-o, --output".len())
        );
        // No content after the separator: unchanged behaviour, the same
        // requirement `find_equals_separator_gap` makes of `=`.
        assert_eq!(find_colon_separator_gap("--flag:"), None);
        assert_eq!(find_colon_separator_gap("--flag :"), None);
        // Not a flag row at all (no leading `-`): never matched here.
        assert_eq!(
            find_colon_separator_gap("Options: pick one of the below"),
            None
        );
        // A word-shaped token ending in `:` is prose (a heading-like
        // word), not a spec — refused, and the scan stops rather than
        // reading through it for a later colon.
        assert_eq!(
            find_colon_separator_gap("-a, --long Options: description"),
            None
        );
        // A colon *inside* a token, not terminating it, is never read as
        // the separator: `find_equals_separator_gap`'s own placeholder and
        // ratio/URL counter-examples, mirrored for `:`.
        assert_eq!(
            find_colon_separator_gap("--host <hh:mm> descriptive text"),
            None
        );
        assert_eq!(find_colon_separator_gap("--ratio 0:30 more text"), None);
        assert_eq!(
            find_colon_separator_gap("--proxy http://host:port do the thing"),
            None
        );
        // The real description's own inline colon (`(default: long)`) is
        // never reached: the scan already returns at the earlier, genuine
        // separator.
        assert_eq!(
            find_colon_separator_gap(
                "-s : Send Short Trespass Command page (default: long) (for FC series)"
            ),
            Some("-s ".len())
        );
    }

    #[test]
    fn strip_colon_separator_unit_cases() {
        assert_eq!(strip_colon_separator(": output debug"), "output debug");
        assert_eq!(
            strip_colon_separator(": Set Honor Reservation bit"),
            "Set Honor Reservation bit"
        );
        // No leading `:`: unchanged.
        assert_eq!(
            strip_colon_separator("no separator here"),
            "no separator here"
        );
        // A second `:` deeper in the description is text, not punctuation.
        assert_eq!(
            strip_colon_separator("Send Short Trespass Command page (default: long)"),
            "Send Short Trespass Command page (default: long)"
        );
        // `:` not followed by whitespace is not the separator shape.
        assert_eq!(strip_colon_separator(":bar"), ":bar");
        assert_eq!(strip_colon_separator(":"), ":");
    }

    /// `sg_emc_trespass --help`'s real capture, byte-exact, replayed
    /// end to end. Before this fix: `-d`/`-h`/`-s`/`-V` all fabricated a
    /// colon-shaped value (`-h` doubly so, since the colon glue also split
    /// the genuine two-character `-hr` switch into a fabricated `-h`), and
    /// the two prose sentences after the synopsis were folded into it and
    /// mined for three fabricated required positionals (`LUN`, `SP`,
    /// `EMC`). After: every flag is a clean boolean with its real
    /// description, and `DEVICE` is the only positional.
    #[test]
    fn sg_emc_trespasss_colon_rows_and_prose_synopsis_tail_are_recovered() {
        let help = concat!(
            "Unrecognized switch: --help\n",
            "Usage:  sg_emc_trespass [-d] [-hr] [-s] [-V] DEVICE\n",
            "  Change ownership of a LUN from another SP to this one.\n",
            "  EMC CLARiiON CX-/AX-family + FC5300/FC4500/FC4700.\n",
            "    -d : output debug\n",
            "    -hr: Set Honor Reservation bit\n",
            "    -s : Send Short Trespass Command page (default: long)\n",
            "         (for FC series)\n",
            "    -V: print version string then exit\n",
            "     DEVICE   sg or block device (latter in lk 2.6 or lk 3 series)\n",
            "        Example: sg_emc_trespass /dev/sda\n",
        );
        let parsed = parse_named(help, "sg_emc_trespass");

        // No fabricated `LUN`/`SP`/`EMC` operands; `DEVICE` survives as the
        // one real positional the usage line actually names.
        let positional_names: Vec<&str> =
            parsed.positionals.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(positional_names, vec!["DEVICE"], "{positional_names:?}");

        let flag = |short: char| -> Flag {
            parsed
                .flags
                .iter()
                .find(|f| f.short == Some(short))
                .unwrap_or_else(|| {
                    panic!(
                        "no flag short=={short:?} in {:?}",
                        parsed
                            .flags
                            .iter()
                            .map(|f| f.spelling())
                            .collect::<Vec<_>>()
                    )
                })
                .clone()
        };

        let d = flag('d');
        assert_eq!(d.value_name, None, "-d must not fabricate a value");
        assert_eq!(
            d.description.as_ref().map(|t| t.as_str()),
            Some("output debug")
        );

        let s = flag('s');
        assert_eq!(s.value_name, None, "-s must not fabricate a value");
        assert_eq!(
            s.description.as_ref().map(|t| t.as_str()),
            Some("Send Short Trespass Command page (default: long) (for FC series)")
        );

        let v = flag('V');
        assert_eq!(v.value_name, None, "-V must not fabricate a value");
        // The `DEVICE` operand row and its trailing example sit one column
        // deeper than `-V`'s own entry indent, so `scan_flags_block`'s
        // (unrelated, pre-existing) indentation-based continuation rule
        // still folds them into `-V`'s description — exactly as the task's
        // own "before" tree already showed for this row. Neither of this
        // fix's two causes touches that rule; asserted here only so a
        // future change to it is forced to notice this fixture.
        assert_eq!(
            v.description.as_ref().map(|t| t.as_str()),
            Some(
                "print version string then exit DEVICE sg or block device (latter in lk 2.6 or lk 3 series) Example: sg_emc_trespass /dev/sda"
            )
        );

        // `-hr` is the two-character switch this help text documents.
        // `Flag::short` can only ever hold one character, and the
        // remaining swallowed text (`"r"`, one character) sits below
        // `repair_single_dash_long_options`'s own `MIN_SWALLOWED_NAME_CHARS`
        // floor (2) — the same deliberate ambiguity that leaves `-Ss`,
        // `-ac` and `-it` unmerged elsewhere in this file, so this must
        // not be special-cased into a merge here either. What the colon
        // fix buys is that `-hr` no longer carries the punctuation-mangled
        // value `"r:"`: `-h`'s value is now the clean `"r"`, and its
        // spelling `-h` plus `-hr` (the fallback
        // `xtask::existence::short_candidates` reconstruction) both occur
        // literally in the raw text, so the fleet existence oracle no
        // longer reports it as invented.
        let h = flag('h');
        assert_eq!(h.value_name.as_deref(), Some("r"));
        assert_eq!(
            h.description.as_ref().map(|t| t.as_str()),
            Some("Set Honor Reservation bit")
        );
    }

    // --- over-eager headings: prose, wrapped synopsis, shared rows -------

    /// `nano 7.2`'s real preamble and the head of its option table,
    /// byte-exact from `corpus/nano/7.2/help.txt`.
    const NANO_PREAMBLE: &str = concat!(
        "Usage: nano [OPTIONS] [[+LINE[,COLUMN]] FILE]...\n",
        "\n",
        "To place the cursor on a specific line of a file, put the line number with\n",
        "a '+' before the filename.  The column number can be added after a comma.\n",
        "When a filename is '-', nano reads data from standard input.\n",
        "\n",
        " Option         Long option             Meaning\n",
        " -A             --smarthome             Enable smart home key\n",
        " -B             --backup                Save backups of existing files\n",
    );

    #[test]
    fn a_prose_sentence_above_an_option_table_names_no_group() {
        let parsed = parse_named(NANO_PREAMBLE, "nano");
        for long in ["smarthome", "backup"] {
            let flag = flag_named(&parsed, long);
            assert_eq!(
                flag.group, None,
                "-- {long} inherited nano's preamble sentence as its group"
            );
        }
        // The rows themselves are untouched: this suppresses a field, it
        // does not decline the block.
        assert_eq!(
            flag_named(&parsed, "smarthome")
                .description
                .as_ref()
                .map(|t| t.as_str()),
            Some("Enable smart home key")
        );
    }

    /// The GNU convention, and the largest single share of the family:
    /// 56 of the 205 affected tools in `audit/queue-captures/` inherit
    /// exactly this sentence.
    #[test]
    fn the_gnu_mandatory_arguments_sentence_names_no_group() {
        let raw = concat!(
            "Usage: head [OPTION]... [FILE]...\n",
            "Print the first 10 lines of each FILE to standard output.\n",
            "\n",
            "Mandatory arguments to long options are mandatory for short options too.\n",
            "  -c, --bytes=[-]NUM       print the first NUM bytes of each file\n",
            "  -n, --lines=[-]NUM       print the first NUM lines instead of the first 10\n",
        );
        let parsed = parse_named(raw, "head");
        assert_eq!(flag_named(&parsed, "bytes").group, None);
        assert_eq!(flag_named(&parsed, "lines").group, None);
    }

    /// The inverse direction, and the reason the prose test is anchored on
    /// the *full stop* rather than on wording: `gcc`/`lto-dump` writes
    /// section headings that are complete English sentences, and they are
    /// real headings over real blocks. A wording- or length-based test
    /// would have destroyed every one of them.
    #[test]
    fn a_prose_shaped_but_colon_terminated_heading_still_names_a_group() {
        let raw = concat!(
            "Usage: lto-dump [OPTION]... FILE\n",
            "\n",
            "The following options are specific to just the language C:\n",
            "  --std=c99                 conform to the C99 standard\n",
            "\n",
            "At least one of the following switches must be given:\n",
            "  --list                    list the objects\n",
        );
        let parsed = parse_named(raw, "lto-dump");
        assert_eq!(
            flag_named(&parsed, "std").group.as_deref(),
            Some("The following options are specific to just the language C:")
        );
        assert_eq!(
            flag_named(&parsed, "list").group.as_deref(),
            Some("At least one of the following switches must be given:")
        );
    }

    /// A period-terminated *row* is a table row, not a sentence — the
    /// column gap is what tells them apart. `arptables` writes both
    /// shapes in the same document.
    #[test]
    fn a_period_terminated_two_column_row_is_not_read_as_prose() {
        assert!(!is_prose_sentence(
            "[!] --version   -V      print package version."
        ));
        assert!(is_prose_sentence(
            "Either long or short options are allowed."
        ));
        // Too short to be a sentence.
        assert!(!is_prose_sentence("Main modes."));
        // Headings are labels; they do not end in a full stop.
        assert!(!is_prose_sentence("Available Commands:"));
    }

    /// `update-xmlcatalog --help`, byte-exact through its second
    /// invocation form. Two defects in one document: the wrapped tail
    /// begins with `--id`, which ended the usage block and lost `--del`
    /// with it, and the backslash-terminated line above it was then read
    /// as a section heading.
    const UPDATE_XMLCATALOG_USAGE: &str = concat!(
        "Usage:\n",
        "    update-xmlcatalog <options> --add --root --type <type> \\\n",
        "                                                --id <id> --package <package>\n",
        "    update-xmlcatalog <options> --del --root --type <type> \\\n",
        "                                                --id <id>\n",
    );

    #[test]
    fn a_backslash_wrapped_synopsis_keeps_the_flags_on_its_wrapped_tail() {
        let parsed = parse_named(UPDATE_XMLCATALOG_USAGE, "update-xmlcatalog");
        let spellings: Vec<String> = parsed.flags.iter().map(|f| f.spelling()).collect();
        assert!(
            spellings.iter().any(|s| s == "--del"),
            "--del is documented only on a backslash-continued usage line; \
             got {spellings:?}"
        );
        assert_eq!(
            parsed.usage,
            vec![
                "Usage:".to_string(),
                "update-xmlcatalog <options> --add --root --type <type> --id <id> --package <package>"
                    .to_string(),
                "update-xmlcatalog <options> --del --root --type <type> --id <id>".to_string(),
            ],
            "each wrapped form is one usage entry, with the continuation \
             marker consumed by the join it performed"
        );
    }

    #[test]
    fn a_backslash_continued_line_names_no_group() {
        // The same shape reached from the section scanner rather than the
        // usage block: a `bpfcc` tracer's EXAMPLES section.
        let raw = concat!(
            "USAGE message:\n",
            "\n",
            "argdist -p 2780 -z 120 \\\n",
            "        -C 'p:c:write(int fd):int:fd'\n",
        );
        let parsed = parse_named(raw, "argdist");
        for flag in &parsed.flags {
            assert_eq!(
                flag.group,
                None,
                "{} inherited a half-line as its group",
                flag.spelling()
            );
        }
        assert!(!is_line_continuation_fragment("Available Commands:"));
        assert!(is_line_continuation_fragment("argdist -p 2780 -z 120 \\"));
    }

    /// `uconv --help`, byte-exact: the heading and the first option row
    /// share one physical line, and before the split `-h, --help` was in
    /// the tree under no spelling at all.
    const UCONV_OPTIONS: &str = concat!(
        "Options:  -h, --help                    print this message\n",
        "          -V, --version                 print the program version\n",
        "          -s, --silent                  suppress messages\n",
    );

    #[test]
    fn a_heading_sharing_its_line_with_the_first_row_keeps_that_row() {
        let parsed = parse_named(UCONV_OPTIONS, "uconv");
        let help = flag_named(&parsed, "help");
        assert_eq!(help.short, Some('h'));
        assert_eq!(
            help.description.as_ref().map(|t| t.as_str()),
            Some("print this message")
        );
        // `Options:` is one of `meaningful_flag_group`'s generic labels,
        // so the recovered heading names no group — and neither does the
        // whole line any more.
        for flag in &parsed.flags {
            assert_eq!(flag.group, None, "{} kept a group", flag.spelling());
        }
    }

    #[test]
    fn a_heading_line_whose_remainder_is_not_a_flag_is_never_split() {
        // `ntfs-3g`'s real line: label, column gap, and then a *value*
        // list. Splitting it would hand the block a row that is not a row.
        assert_eq!(
            split_shared_heading_row("Options:  ro (read-only mount), windows_names, uid=, gid=,"),
            None
        );
        // `awk`'s second heading column, likewise not a row.
        assert_eq!(
            split_shared_heading_row("POSIX options:\t\tGNU long options: (standard)"),
            None
        );
        // The shape this does claim.
        assert_eq!(
            split_shared_heading_row("Options:  -h, --help    print this message"),
            Some((
                "Options:".to_string(),
                "          -h, --help    print this message".to_string(),
                false
            ))
        );
    }

    #[test]
    fn a_bnf_heading_carrying_its_first_flag_row_is_split() {
        // `ip`'s real line: the colon reads as `:=`, not a plain heading
        // colon, so the original column-gap clause (zero spaces right
        // after `:`) never fired and `-V`/`-s`/`-d`/`-r` were eaten by the
        // heading string. The recovered row is re-indented to column 20,
        // matching the continuation lines `ip` itself wraps to.
        // The opening bracket is stripped along with the operator, not kept
        // in the row: the continuation lines this heading introduces
        // (`-h[uman-readable] | -iec | ...`) never carry it either, and
        // downstream flag-row parsing expects a bare flag at the row's
        // start.
        assert_eq!(
            split_shared_heading_row(
                "       OPTIONS := { -V[ersion] | -s[tatistics] | -d[etails] | -r[esolve] |"
            ),
            Some((
                "       OPTIONS :".to_string(),
                "                    -V[ersion] | -s[tatistics] | -d[etails] | -r[esolve] |"
                    .to_string(),
                true
            ))
        );
        // `dcb`'s sibling shape: a `[`-bracket instead of `{`.
        assert_eq!(
            split_shared_heading_row("       OPTIONS := [ -V | --Version | -i | --iec ]"),
            Some((
                "       OPTIONS :".to_string(),
                "                    -V | --Version | -i | --iec ]".to_string(),
                true
            ))
        );
    }

    #[test]
    fn a_bnf_heading_whose_row_is_not_a_flag_is_never_split() {
        // `ip`'s own `OBJECT` production and `ss`'s grammar productions all
        // use the same `:=` operator but open on a bare word, never a flag
        // spelling — clause 4 must reject every one of them.
        assert_eq!(
            split_shared_heading_row(
                "where  OBJECT := { address | addrlabel | amt | fou | help | ila | ioam | l2tp |"
            ),
            None
        );
        assert_eq!(
            split_shared_heading_row(
                "       FAMILY := {inet|inet6|link|unix|netlink|vsock|tipc|xdp|help}"
            ),
            None
        );
        // `pkgdata`'s `modes: (-m option)`: a bracket immediately follows
        // the colon, but with no `=` — this is the false positive that a
        // bracket-without-operator version of clause 3 would invent, since
        // the remainder `-m option)` does satisfy `looks_like_flag_start`
        // on its own. Requiring the BNF operator keeps this line intact.
        assert_eq!(split_shared_heading_row("modes: (-m option)"), None);
    }

    /// `ip`'s own `OPTIONS := { -V[ersion] | -s[tatistics] | -d[etails] |
    /// -r[esolve] |` row: every top-level `|`-segment already carries its
    /// own abbreviation bracket, so each one is a complete, self-contained
    /// flag and all four are recovered.
    #[test]
    fn a_bnf_alternation_row_of_self_contained_abbreviated_flags_splits_fully() {
        let entries =
            split_bnf_alternation_row("-V[ersion] | -s[tatistics] | -d[etails] | -r[esolve] |")
                .expect("row splits");
        let specs: Vec<&str> = entries.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(
            specs,
            vec!["-V[ersion]", "-s[tatistics]", "-d[etails]", "-r[esolve]"]
        );
        assert!(entries.iter().all(|(_, desc)| desc.is_empty()));
    }

    /// `dcb`'s own convention: short and long spelled as two adjacent
    /// alternatives rather than one abbreviated token. A bare short
    /// immediately followed by a bare long folds back into one flag with
    /// both spellings, never two one-spelling fragments.
    #[test]
    fn a_bnf_alternation_row_pairs_bare_short_and_long_spellings() {
        let entries = split_bnf_alternation_row("-V | --Version | -i | --iec | -j | --json")
            .expect("row splits");
        assert_eq!(entries.len(), 3);
        let short = parse_flag_spec(&entries[0].0);
        assert_eq!(short.short, Some('V'));
        assert_eq!(short.long.as_deref(), Some("Version"));
    }

    /// `sg_sanitize`'s real `--count=OC|-c OC` (from `corpus/sg_sanitize`):
    /// splits into two top-level `|`-segments, and the two are one flag —
    /// an alias plus a shared value — never two, because the second
    /// segment (`-c OC  OC is overwrite count`) leaves real prose
    /// unconsumed and fails `fully_consumed` on its own, independent of
    /// any outer gating.
    ///
    /// This function alone cannot refuse every non-BNF `|`-joined pair,
    /// though — `btrfsck`'s real `-E|--subvol-extents <subvolid>` DOES
    /// split cleanly at this level (both halves are, on their own, a
    /// perfectly good `fully_consumed` flag), which is exactly why the
    /// per-segment checks here are not the whole guard: the caller
    /// ([`scan_flags_block`]) never invokes this function at all unless
    /// the block's own heading came from a `:=` production. See
    /// `a_plain_pipe_joined_flag_row_survives_parse_with_profile_unsplit`
    /// below for that gate exercised end to end, on the real shape that
    /// caught this before the gate existed.
    #[test]
    fn a_row_gluing_one_flags_alias_and_value_through_a_pipe_is_never_split() {
        assert_eq!(
            split_bnf_alternation_row("--count=OC|-c OC  OC is overwrite count"),
            None
        );
    }

    /// `rdma`'s real `-p[retty] -r[aw]}` and `devlink`'s real
    /// `-v[erbose] -s[tatistics] -[he]x`: two or three flags run together
    /// by a bare space inside one `|`-segment (the "missing separator"
    /// shape this reader deliberately refuses). Without the
    /// more-than-one-flag-shaped-word guard, `parse_flag_spec`'s own alias
    /// loop silently swallows the second flag with no leftover text at
    /// all, so `fully_consumed` alone cannot catch it.
    #[test]
    fn a_row_whose_segment_runs_two_flags_together_with_a_bare_space_is_refused() {
        assert_eq!(
            split_bnf_alternation_row("-V[ersion] | -d[etails] | -j[son] | -p[retty] -r[aw]}"),
            None
        );
        assert_eq!(
            split_bnf_alternation_row(
                "-V[ersion] | -n[o-nice-names] | -j[son] | -p[retty] | -v[erbose] -s[tatistics] -[he]x }"
            ),
            None
        );
    }

    /// `vdpa`'s real closing brace, one space after the last flag's own
    /// abbreviation bracket (`-p[retty] }`), and `dcb`'s real closing
    /// bracket glued directly onto a bare long spelling with no space at
    /// all (`--verbose]`) — the enclosing group's own closer landing on
    /// the row's last alternative two different ways. Both must vanish
    /// without being read as that flag's value.
    #[test]
    fn a_trailing_stray_group_closer_never_becomes_a_value() {
        assert_eq!(strip_trailing_stray_bracket("-p[retty] }"), "-p[retty]");
        assert_eq!(strip_trailing_stray_bracket("--verbose]"), "--verbose");
        // A bracket that closes something real in the *same* segment is
        // never touched.
        assert_eq!(
            strip_trailing_stray_bracket("-b[atch] [filename]"),
            "-b[atch] [filename]"
        );
    }

    /// End-to-end regression for the false-positive sweep
    /// `split_bnf_alternation_row`'s own doc comment describes: a real
    /// short/long pair joined by `|` outside any `:=` production must
    /// come through `parse_with_profile` completely unsplit, whether or
    /// not the document elsewhere contains a real BNF heading.
    #[test]
    fn a_plain_pipe_joined_flag_row_survives_parse_with_profile_unsplit() {
        let raw = "Usage: btrfsck [options] <device>\n\nOptions:\n    -Q|--qgroup-report        print a report on qgroup consistency\n    -E|--subvol-extents <subvolid>\n                              print subvolume extents and sharing state\n";
        let result = parse_with_profile(raw, None, Some("btrfsck"));
        let e = result
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("subvol-extents"))
            .expect("subvol-extents recovered as one flag");
        assert_eq!(e.short, Some('E'));
        assert_eq!(e.value_name.as_deref(), Some("<subvolid>"));
        assert!(
            !result
                .flags
                .iter()
                .any(|f| f.short.is_none() && f.long.is_none()),
            "no half-flag left behind"
        );
    }

    /// `where OBJECT := { address | ... }` is BNF grammar, not the tool's
    /// own description — the leak `looks_like_bnf_production_line` closes.
    #[test]
    fn a_bnf_production_line_never_becomes_the_description() {
        assert!(looks_like_bnf_production_line(
            "where  OBJECT := { address | addrlabel | amt }"
        ));
        assert!(looks_like_bnf_production_line("OPTIONS := { -V | -s }"));
        // An ordinary sentence that happens to start with the same word
        // must stay eligible to become the description.
        assert!(!looks_like_bnf_production_line(
            "where possible, prefer the short spelling"
        ));
    }

    /// Spec §6's attestation gate reads `CommandNode::heading_attested`,
    /// which is what decides whether a recovered word may become
    /// `<word> --help` probe argv. Group suppression must not touch it in
    /// either direction, and above all must never make a word probe-
    /// eligible that was not.
    ///
    /// The pair below is the proof: the same block under a real command
    /// heading and under a prose sentence. The real heading attests its
    /// entries; the prose sentence recovers no commands at all, before
    /// this change or after it. Nothing this change does can move a node
    /// from the second document into the first.
    #[test]
    fn group_suppression_does_not_widen_probe_eligibility() {
        const BLOCK: &str = concat!(
            "  clone     Clone a repository\n",
            "  init      Create one\n",
        );
        let attested = parse_named(&format!("Commands:\n{BLOCK}"), "prog");
        assert_eq!(attested.subcommands.len(), 2);
        assert!(
            attested.subcommands.iter().all(|c| c.heading_attested),
            "a recognized command heading still attests its entries"
        );

        let prose = parse_named(
            &format!("Copy standard input to each FILE, and also to standard output.\n{BLOCK}"),
            "prog",
        );
        assert!(
            prose.subcommands.is_empty(),
            "a prose sentence attests nothing: {:?}",
            prose
                .subcommands
                .iter()
                .map(|c| c.name.clone())
                .collect::<Vec<_>>()
        );
    }

    /// [`try_split_packed_row`] unit cases: the sharp operand-vs-next-entry
    /// boundaries spec calls out for GNU `find --help`'s own shape.
    #[test]
    fn try_split_packed_row_unit_cases() {
        assert_eq!(
            try_split_packed_row("-anewer FILE -atime N"),
            Some(vec![
                ("-anewer".to_string(), "FILE".to_string()),
                ("-atime".to_string(), "N".to_string()),
            ])
        );
        // A prefix-bracket-then-bare-suffix value spec with no separator
        // (`-perm`'s own convention) is kept as one operand, verbatim,
        // rather than decomposed further.
        assert_eq!(
            try_split_packed_row("-perm [-/]MODE -regex PATTERN"),
            Some(vec![
                ("-perm".to_string(), "[-/]MODE".to_string()),
                ("-regex".to_string(), "PATTERN".to_string()),
            ])
        );
        // Two operands on one entry (`-fprintf FILE FORMAT`) are kept
        // together, not split into a second flag.
        assert_eq!(
            try_split_packed_row("-fprintf FILE FORMAT -print"),
            Some(vec![
                ("-fprintf".to_string(), "FILE FORMAT".to_string()),
                ("-print".to_string(), String::new()),
            ])
        );
        // The `-exec`/`-ok` command-terminator convention: bare `;` and
        // `{} +` are operand tokens, never new entries (neither starts
        // with a dash-plus-letter).
        assert_eq!(
            try_split_packed_row("-exec COMMAND ; -exec COMMAND {} +"),
            Some(vec![
                ("-exec".to_string(), "COMMAND ;".to_string()),
                ("-exec".to_string(), "COMMAND {} +".to_string()),
            ])
        );
        // Real prose between two dash-looking tokens is not this shape at
        // all — refuse rather than guess where the entry boundary is.
        assert_eq!(
            try_split_packed_row("-foo Enable the foo behavior -bar"),
            None
        );
        // A lone boolean flag alone on its line still succeeds (one
        // entry, empty operand) — `block_is_packed_flag_rows` is what
        // requires at least one *other* line in the block to pack two or
        // more before this one is read as part of the shape.
        assert_eq!(
            try_split_packed_row("-readable"),
            Some(vec![("-readable".to_string(), String::new())])
        );
    }

    /// Reproduces the corruption this shape reader fixes: before it
    /// existed, `find_placeholder_boundary_gap` misread `-size N[bcwkMG]`'s
    /// own bracketed unit suffix as a description boundary and handed
    /// `-wholename` the front of the *next* entries on the line
    /// (`-true -type [bcdpflsD] -uid N`) as a fabricated description.
    /// `-wholename` must come out with its real value (`PATTERN`) and no
    /// description at all — never text belonging to a different flag.
    #[test]
    fn find_style_packed_tests_block_recovers_every_entry_with_no_fabricated_description() {
        let raw = concat!(
            "Usage: find [-H] [-L] [-P] [path...] [expression]\n",
            "\n",
            "Tests (N can be +N or -N or N):\n",
            "      -wholename PATTERN -size N[bcwkMG] -true -type [bcdpflsD] -uid N\n",
        );
        let parsed = parse_named(raw, "find");
        let wholename = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("wholename"))
            .expect("-wholename recovered");
        assert_eq!(wholename.value_name.as_deref(), Some("PATTERN"));
        assert!(
            wholename.description.is_none(),
            "no description exists in this document; must not be fabricated: {:?}",
            wholename.description
        );
        assert!(wholename.single_dash);
        for name in ["size", "true", "type", "uid"] {
            assert!(
                parsed.flags.iter().any(|f| f.long.as_deref() == Some(name)),
                "expected {name} to be recovered as its own flag, not folded into -wholename"
            );
        }
    }

    /// GNU find's real `-exec`/`-execdir` write two packed entries under
    /// one spelling (a `;`-terminated form and a `{} +`-terminated form).
    /// They must merge into one `Flag`, not appear twice.
    #[test]
    fn find_style_packed_actions_block_merges_repeated_spellings() {
        let raw = concat!(
            "Actions:\n",
            "      -exec COMMAND ; -exec COMMAND {} + -ok COMMAND ;\n",
        );
        let parsed = parse_named(raw, "find");
        let exec_flags: Vec<_> = parsed
            .flags
            .iter()
            .filter(|f| f.long.as_deref() == Some("exec"))
            .collect();
        assert_eq!(
            exec_flags.len(),
            1,
            "one -exec flag, not two: {exec_flags:?}"
        );
        assert_eq!(
            exec_flags[0].value_name.as_deref(),
            Some("COMMAND ; | COMMAND {} +")
        );
    }

    /// A block with real prose descriptions must never be read as packed,
    /// even if one of its rows happens to carry two dash-looking tokens.
    #[test]
    fn packed_row_reader_never_claims_a_block_with_real_descriptions() {
        let raw = concat!(
            "Other common options:\n",
            "      --help                   display this help and exit\n",
            "      --version                output version information and exit\n",
        );
        let parsed = parse_named(raw, "find");
        let help = parsed
            .flags
            .iter()
            .find(|f| f.long.as_deref() == Some("help"))
            .expect("--help recovered");
        assert_eq!(
            help.description.as_ref().map(|t| t.as_str()),
            Some("display this help and exit")
        );
    }
}
