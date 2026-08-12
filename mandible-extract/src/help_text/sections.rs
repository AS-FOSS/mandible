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

use super::grammar::{looks_like_flag_start, parse_flag_spec, FlagSpec};
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
    if let Some(start) = lines
        .iter()
        .position(|l| starts_with_usage_prefix(l.trim_start()))
    {
        i = start;
        let base_indent = leading_whitespace(lines[i]);
        usage_lines.push(lines[i].trim().to_string());
        let mut usage_entries = vec![lines[i].trim().to_string()];
        i += 1;
        while i < lines.len() {
            let l = lines[i];
            if l.trim().is_empty() {
                break;
            }
            let trimmed_start = l.trim_start();
            let is_marker =
                starts_with_usage_prefix(trimmed_start) || starts_with_or_marker(trimmed_start);
            let is_own_name =
                tool_name.is_some_and(|name| starts_with_tool_name(trimmed_start, name));
            let starts_new_entry = is_marker || is_own_name;

            if !starts_new_entry {
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
                last.push(' ');
                last.push_str(&trimmed);
            }
            i += 1;
        }
        result.positionals = extract_positionals(&usage_lines);
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
            if !starts_with_usage_prefix(t) {
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
    let description_lines: Vec<&str> =
        if paragraphs.len() > 1 && is_banner_paragraph(&paragraphs[0]) {
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

        // Peek the first content lines to decide flags vs. bare-word. Not
        // just the *first*: some tools document a positional at the top of
        // their options table, and keying the whole decision off row one
        // threw the rest of the block away. See `flags_block_start`.
        if let Some(flags_start) = flags_block_start(&lines, i) {
            let (end, entries) = scan_flags_block(&lines, flags_start);
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
fn starts_with_or_marker(t: &str) -> bool {
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
fn starts_with_tool_name(t: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    match t.strip_prefix(name) {
        Some(rest) => rest.is_empty() || rest.starts_with(char::is_whitespace),
        None => false,
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
fn flags_block_start(lines: &[&str], start: usize) -> Option<usize> {
    /// How many non-flag rows may precede the first flag row.
    const MAX_SKIPPED_LEADING_ROWS: usize = 3;

    if looks_like_flag_start(lines[start]) {
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
        if looks_like_flag_start(line) {
            return Some(offset);
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
// against the false-positive classes below — deliberately duplicated here
// (like `help_text::pick_stream`/`misattribution::pick_stream` already are)
// rather than sharing code with that module, which this task's own
// instructions rule out touching. One difference is load-bearing, not
// incidental: that module is an *advisory* metric a human reads, so it can
// afford to under-suppress (its own doc comment names `arptables`' `-A
// chain` as a known, accepted residual false positive). A splitter's
// mistakes are not advisory — they fabricate a flag that was never in the
// tool's own text — so [`fields_in_line`] below is strictly more
// conservative: it never starts a new field on top of one that hasn't yet
// earned real description text of its own (see its doc comment), which is
// exactly what keeps `-A chain`/`-p NUM`-shaped rows (a value placeholder
// standing in for real trailing text, lower-case so
// `is_value_placeholder_only` can't recognize it as one) from being read as
// a second, independent flag.

/// Minimum number of distinct entry lines a secondary column offset must
/// recur at before a block is trusted as genuinely multi-column. Same
/// figure and same justification as
/// `xtask::misattribution::MIN_COLUMN_RECURRENCE`: real column bleed
/// (`lsof`'s two hidden columns) recurs 9 times over its ~10-line options
/// block; the worst accidental coincidence measured in this project's own
/// real-tool sample (`tar`'s `-T` cross-reference) recurs twice, at two
/// different offsets. `3` sits strictly between the two.
const MIN_COLUMN_RECURRENCE: usize = 3;

/// True if `token` is shaped like a flag spelling: `-x`, `--word`, `+x`, or
/// `+|-x` — lsof spells several of its own flags with the `+` prefix
/// (`+d`, `+m`). Deliberately permissive about the character right after a
/// short prefix (`lsof`'s own `-?`).
fn is_flag_shaped(token: &str) -> bool {
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
fn first_word(s: &str) -> &str {
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
fn cells(line: &str) -> Vec<(usize, String)> {
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
fn is_value_placeholder_only(s: &str) -> bool {
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

fn scan_flags_block<'a>(lines: &[&'a str], start: usize) -> (usize, Vec<(String, String)>) {
    const ENTRY_INDENT_TOLERANCE: usize = 10;
    let mut i = start;
    let mut rows: Vec<FlagsBlockRow<'a>> = Vec::new();
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
            rows.push(FlagsBlockRow::Entry(line));
            min_entry_indent = Some(min_entry_indent.map_or(indent, |m| m.min(indent)));
            i += 1;
            continue;
        }

        let is_continuation = !rows.is_empty() && min_entry_indent.is_some_and(|m| indent > m);
        if is_continuation {
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

    let mut entries: Vec<(String, String)> = Vec::new();
    for row in rows {
        match row {
            FlagsBlockRow::Entry(line) => {
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
    (i, entries)
}

/// The original (pre-multi-column) way to split one flags-block entry line:
/// one description column, detected once per line. Still the only path for
/// a block [`block_is_multi_column`] didn't flag, and the fallback for a
/// multi-column block's occasional line that doesn't itself split into
/// fields (see the call site).
fn split_single_column_entry(line: &str) -> (String, String) {
    let gap = find_description_gap(line);
    let (spec, desc) = split_at_column(line, gap);
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
    // the line — see `find_placeholder_boundary_gap`'s own doc comment.
    find_placeholder_boundary_gap(line)
}

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
            if seen_content && (had_tab || j - i >= 2) {
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

/// Pull placeholder tokens (`<value>`, bare `UPPERCASE` words not preceded
/// by `-`) out of usage lines as positionals. Best-effort: usage-line
/// grammar is genuinely varied (docopt-style `[OPTIONS]`, `<required>`,
/// `...`, `|`, `{a|b|c}`), so this recognizes the common placeholder
/// shapes rather than fully parsing the grammar.
fn extract_positionals(usage_lines: &[String]) -> Vec<Positional> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for line in usage_lines {
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
        for token in line.split_whitespace() {
            let cleaned = token.trim_matches(|c| c == '[' || c == ']' || c == '.');
            // A flag already carrying its value inline (`--git-dir=<path>`)
            // has an `=` in `cleaned` and does not expect a following
            // token; a bare flag (`-C`, `-Zscript`) does.
            let consumed_by_prior_flag =
                prev_cleaned.is_some_and(|p| p.starts_with('-') && !p.contains('='));
            prev_cleaned = Some(cleaned);

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
    for line in usage_lines {
        for segment in usage_segments(line) {
            if out.len() >= MAX_RECOVERED_ENTRIES {
                return out;
            }
            match segment {
                UsageSegment::Group(members) => {
                    let flaggy: Vec<&str> =
                        members.into_iter().filter(|m| m.starts_with('-')).collect();
                    // spec [M-15]'s conservative-pairing rule: within one
                    // bracket group, pair a short with a long only when the
                    // group has exactly one of each. `[-v | --version]`
                    // qualifies; `[-p | --paginate | -P | --no-pager]`
                    // (four alternatives) does not, and every spelling in
                    // it is emitted on its own rather than guessing which
                    // short goes with which long. A wrong pairing asserts a
                    // false equivalence a user would act on — worse than an
                    // unpaired entry, which is merely incomplete.
                    if flaggy.len() == 2 {
                        let a = parse_flag_spec(flaggy[0]);
                        let b = parse_flag_spec(flaggy[1]);
                        if let Some(paired) = pair_short_and_long(a, b) {
                            push_usage_flag(&mut out, paired);
                            continue;
                        }
                    }
                    for m in flaggy {
                        push_usage_flag(&mut out, parse_flag_spec(m));
                    }
                }
                UsageSegment::Bare(tok) => {
                    if tok.starts_with('-') {
                        push_usage_flag(&mut out, parse_flag_spec(tok));
                    }
                }
            }
        }
    }
    out
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
        if c == '[' {
            if let Some((content_range, close_idx)) = matched_bracket_group(&chars, idx) {
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

/// Find the byte range of the content strictly between `chars[open_idx]`
/// (a `[`) and its matching `]`, and the char-index of that `]` —
/// bracket-depth aware, so `[--exec-path[=<path>]]`'s inner `[...]` (an
/// optional value spec on the one alternative) is consumed as part of the
/// outer group's content instead of closing the group early. `None` when
/// `open_idx`'s bracket is never closed (malformed input); the caller
/// falls back to treating it as an ordinary bare token.
fn matched_bracket_group(
    chars: &[(usize, char)],
    open_idx: usize,
) -> Option<((usize, usize), usize)> {
    let (open_byte, open_c) = chars[open_idx];
    let content_start = open_byte + open_c.len_utf8();
    let mut depth = 1i32;
    let mut j = open_idx + 1;
    while j < chars.len() {
        let (byte_pos, c) = chars[j];
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(((content_start, byte_pos), j));
                }
            }
            _ => {}
        }
        j += 1;
    }
    None
}

/// Split a bracket group's content on `|` at that content's own nesting
/// depth 0, so a nested `[...]` (an optional value spec on one of the
/// alternatives) is never itself split on. Empty fragments (a stray
/// leading/trailing `|`, or `||`) are dropped.
fn split_top_level_pipe(content: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in content.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => depth -= 1,
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
