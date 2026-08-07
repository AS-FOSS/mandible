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

use super::grammar::{looks_like_flag_start, parse_flag_spec};
use super::profile::{heading_matches_markers, FrameworkProfile};
use mandible_core::{
    is_command_name_shaped, CommandNode, Flag, Positional, Provenance, Source, Text,
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
/// Equivalent to `parse_with_profile(raw, None)`. `#[cfg(test)]`: the one
/// production caller (`help_text::build_node`) always has a definite
/// answer to "was a framework identified?" and calls
/// [`parse_with_profile`] directly with `None` or `Some(..)`; this
/// zero-argument spelling exists only because most of this module's own
/// (pre-batch-6-part-4) test suite below calls it, and its behavior must
/// stay exactly what it always was.
#[cfg(test)]
pub fn parse(raw: &str) -> ParsedHelp {
    parse_with_profile(raw, None)
}

/// Same engine as [`parse`], but consulting `profile`'s framework-specific
/// heading vocabulary and subcommand-concept knowledge when present (spec
/// §7 Tier B step 1, "framework identified"). `None` reproduces [`parse`]'s
/// generic behavior exactly — this is what keeps the two degradation
/// levels (spec §7 Tier B: identified vs. unidentified) sharing one engine
/// instead of forking into two.
pub fn parse_with_profile(raw: &str, profile: Option<&FrameworkProfile>) -> ParsedHelp {
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
    // 1. Usage block: one or more lines starting with (case-insensitive)
    // "usage:", plus indented continuations.
    if let Some(start) = lines
        .iter()
        .position(|l| starts_with_usage_prefix(l.trim_start()))
    {
        i = start;
        let mut usage_lines = vec![lines[i].trim().to_string()];
        i += 1;
        while i < lines.len() {
            let l = lines[i];
            if l.trim().is_empty() {
                break;
            }
            if leading_whitespace(l) == 0 {
                break;
            }
            // A continuation line that itself reads as a flag entry ends
            // the usage block, even though it is indented and unseparated
            // by a blank line. A usage continuation is an *alternative
            // invocation form* (`   curl [options...] <url>`); it never
            // begins with a dash. Tools that run their flag list straight
            // into the usage line with no blank separator and no
            // `Options:` heading are common enough that not stopping here
            // silently swallowed every flag they have: `curl --help`
            // indents its 13 flag rows by one space directly under
            // `Usage:`, and all 13 landed in `usage` with zero flags
            // parsed — reported as `ok` at "no flags to describe", which
            // is the same class of confidently-wrong result as [M-10].
            // This is a layout fact, true of every framework, so it lives
            // in the shared engine rather than in any profile.
            if looks_like_flag_start(l.trim_start()) {
                break;
            }
            usage_lines.push(l.trim().to_string());
            i += 1;
        }
        result.positionals = extract_positionals(&usage_lines);
        result.usage = usage_lines;
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
    let mut description_lines: Vec<&str> = Vec::new();
    let mut j = 0;
    while j < lines.len() && j < description_bound {
        let l = lines[j];
        if leading_whitespace(l) == 0 && !l.trim().is_empty() {
            let t = l.trim_start();
            if !starts_with_usage_prefix(t) {
                description_lines.push(l);
            }
        }
        j += 1;
    }
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
            let (end, entries) = scan_flags_block(&lines, i);
            i = end;
            let (seen, clean) = emit_flags(None, entries, &mut result);
            total_entries += seen;
            clean_entries += clean;
            command_mode = false;
            continue;
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

        // Peek the first content line to decide flags vs. bare-word.
        if looks_like_flag_start(lines[i]) {
            let (end, entries) = scan_flags_block(&lines, i);
            i = end;
            if is_ignorable_heading(&heading) {
                command_mode = false;
                continue;
            }
            let (seen, clean) = emit_flags(meaningful_flag_group(heading), entries, &mut result);
            total_entries += seen;
            clean_entries += clean;
            command_mode = false;
            continue;
        }

        // Argparse's subparser blocks (spec §7 Tier B, batch 6 part 4) are
        // structurally distinct from every other framework's bare-word
        // block — see `scan_argparse_subparsers`'s doc comment — so they
        // get first refusal here, gated on the profile explicitly opting
        // in and the heading plausibly being argparse's own (usually
        // undecorated) `"positional arguments:"`. A miss (no `{...}`
        // pseudo-entry evidence found) falls straight through to the
        // ordinary bare-block handling below, same as any other tool.
        if profile.is_some_and(|p| p.argparse_subparser_quirk)
            && heading.to_lowercase().contains("positional arguments")
        {
            if let Some((end, entries)) = scan_argparse_subparsers(&lines, i, heading_indent) {
                i = end;
                command_mode = false;
                let (seen, clean) = emit_subcommands(&heading, entries, &mut result);
                total_entries += seen;
                clean_entries += clean;
                continue;
            }
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

    result.confidence = compute_confidence(total_entries, clean_entries, !result.usage.is_empty());
    result
}

fn compute_confidence(total_entries: usize, clean_entries: usize, had_usage: bool) -> f32 {
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

fn leading_whitespace(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// True if `t` starts with `"usage:"`, case-insensitively.
///
/// Deliberately compares raw bytes rather than doing `&t[..6]` on the
/// `str` (which panics if byte offset 6 doesn't land on a UTF-8 character
/// boundary — a real crash the coverage harness found: some real-world
/// `--help` output puts a multi-byte character, e.g. a box-drawing glyph,
/// early in the first line). `[u8]::get` is bounds-checked and never
/// panics, and comparing ASCII bytes needs no UTF-8 decoding at all.
fn starts_with_usage_prefix(t: &str) -> bool {
    t.as_bytes()
        .get(..6)
        .map(|b| b.eq_ignore_ascii_case(b"usage:"))
        .unwrap_or(false)
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
                    group: Some(heading.to_string()),
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
    if GENERIC.contains(&normalized.as_str()) {
        None
    } else {
        Some(heading)
    }
}

fn emit_flags(
    group: Option<String>,
    entries: Vec<(&str, String)>,
    out: &mut ParsedHelp,
) -> (usize, usize) {
    let mut seen = 0usize;
    let mut clean = 0usize;
    for (spec_text, desc_text) in entries {
        if out.flags.len() >= MAX_RECOVERED_ENTRIES {
            break;
        }
        seen += 1;
        let spec = parse_flag_spec(spec_text);
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
        node.group = Some(heading.to_string());
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
fn scan_flags_block<'a>(lines: &[&'a str], start: usize) -> (usize, Vec<(&'a str, String)>) {
    const ENTRY_INDENT_TOLERANCE: usize = 10;
    let mut i = start;
    let mut entries: Vec<(&str, String)> = Vec::new();
    let mut min_entry_indent: Option<usize> = None;

    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        let indent = leading_whitespace(line);
        let trimmed = line.trim_start();

        let is_entry_start = looks_like_flag_start(trimmed)
            && min_entry_indent.is_none_or(|min| indent <= min + ENTRY_INDENT_TOLERANCE);

        if is_entry_start {
            let gap = find_description_gap(line);
            let (spec, desc) = split_at_column(line, gap);
            entries.push((spec, desc));
            min_entry_indent = Some(min_entry_indent.map_or(indent, |m| m.min(indent)));
            i += 1;
            continue;
        }

        let is_continuation = !entries.is_empty() && min_entry_indent.is_some_and(|m| indent > m);
        if is_continuation {
            let last = entries.last_mut().expect("checked non-empty above");
            last.1.push(' ');
            last.1.push_str(trimmed.trim_end());
            i += 1;
            continue;
        }

        // Neither a new entry nor a continuation of one: this line dedents
        // back to (or below) the block's own entries without looking like
        // a flag — a genuinely new heading. Stop here.
        break;
    }
    (i, entries)
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
/// dedents below that baseline. Shared by [`scan_bare_block`] and
/// [`scan_argparse_subparsers`] so both agree on where a block ends even
/// though they disagree on how to split its *entries*.
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

/// Find the byte offset of the first run of 2+ spaces in `line`, if any,
/// after some non-whitespace content.
fn find_description_gap(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut seen_content = false;
    while i < bytes.len() {
        if bytes[i] == b' ' {
            let mut j = i;
            while j < bytes.len() && bytes[j] == b' ' {
                j += 1;
            }
            if seen_content && j - i >= 2 {
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

/// Pull placeholder tokens (`<value>`, bare `UPPERCASE` words not preceded
/// by `-`) out of usage lines as positionals. Best-effort: usage-line
/// grammar is genuinely varied (docopt-style `[OPTIONS]`, `<required>`,
/// `...`, `|`, `{a|b|c}`), so this recognizes the common placeholder
/// shapes rather than fully parsing the grammar.
fn extract_positionals(usage_lines: &[String]) -> Vec<Positional> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for line in usage_lines {
        for token in line.split_whitespace() {
            let cleaned = token.trim_matches(|c| c == '[' || c == ']' || c == '.');
            if cleaned.starts_with('-') {
                continue;
            }
            let (name, variadic) = if let Some(stripped) = cleaned.strip_prefix('<') {
                match stripped.strip_suffix('>') {
                    Some(inner) => (inner.to_string(), token.ends_with("...")),
                    None => continue,
                }
            } else if cleaned.chars().all(|c| c.is_uppercase() || c == '_') && cleaned.len() > 1 {
                (cleaned.to_string(), token.ends_with("..."))
            } else {
                continue;
            };
            if name.is_empty() || !seen.insert(name.clone()) {
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

#[cfg(test)]
mod tests {
    use super::*;

    const TAR_HELP: &str = include_str!("../../tests/fixtures/help_text/tar_help.stdout");
    const GIT_HELP: &str = include_str!("../../tests/fixtures/help_text/git_help.stdout");
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
        let block = "Available commands are:\n   l            - List all installed modules\n   q            - Quit the program\n\n";
        let raw = block.repeat(20_000);
        let start = std::time::Instant::now();
        let parsed = parse(&raw);
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "parsing a repetitive input took {:?}, expected it to stay fast",
            start.elapsed()
        );
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
}
