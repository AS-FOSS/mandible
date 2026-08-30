//! Block scanners for bare-word content: operand tables, argparse subparser
//! lists, headingless invocation tables, and bare command tables.

use super::*;

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
pub(super) fn scan_bare_block<'a>(
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
pub(super) fn bare_block_end(lines: &[&str], start: usize) -> usize {
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
pub(super) fn scan_argparse_subparsers<'a>(
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
pub(super) fn scan_same_indent_entry_table<'a>(
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
pub(super) const MIN_INVOCATION_TABLE_ROWS: usize = 2;

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
pub(super) fn scan_headingless_invocation_table<'a>(
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
pub(super) fn invocation_table_row_run<'a>(
    trimmed: &'a str,
    tool_name: &str,
) -> Option<Vec<&'a str>> {
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
pub(super) fn token_occurs_literally(raw: &str, token: &str) -> bool {
    raw.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')))
        .any(|w| w == token)
}

pub(super) fn scan_comma_separated_commands<'a>(
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
pub(super) fn split_bare_command_table_row(line: &str) -> Option<(&str, Option<String>)> {
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
pub(super) fn scan_bare_command_table<'a>(
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

/// One row of a modifier table: the letter, the operand the table spells
/// beside it, and its description (spec §7 Tier B, "Modifier tables").
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ModifierRow {
    /// The modifier letter itself — `a` for `[a]`, `l` for `[l <text> ]`.
    pub letter: char,
    /// The operand written inside the brackets after the letter, if any:
    /// `<text>` in `ar`'s `[l <text> ]`.
    pub value_name: Option<String>,
    /// The row's description, with the separator that introduced it
    /// removed. Never empty — a row without one is not admitted at all.
    pub description: String,
}

/// The shortest run of rows that may be read as a modifier table.
///
/// Two, the same floor [`MIN_ATTESTED_SECTION_FLAGS`] uses and for the same
/// reason: one bracketed row is cheap for an unrelated document to produce
/// by accident, a run of them is a table. Measured against the 2,301 frozen
/// captures under `audit/queue-captures/`: the only length-2 run in the
/// fleet that [`split_modifier_table_row`]'s grammar looked at was
/// `pygettext3`'s reference footnotes (`[1] https://…`, `[2] https://…`),
/// and that shape is refused on two independent grounds inside the row
/// grammar itself, so no document in the fleet reaches this floor except a
/// genuine modifier table.
const MIN_MODIFIER_TABLE_ROWS: usize = 2;

/// Split one modifier-table row — `ar`'s `[a]          - put file(s) after
/// [member-name]`, `llvm-ar`'s `[a] - put [files] after [relpos]` — into a
/// [`ModifierRow`]. `None` for anything that is not that shape.
///
/// The grammar is deliberately narrow, because a bracketed token is common
/// punctuation and a modifier table is rare:
///
/// - The row **opens** with `[`, and the bracket closes on the same row.
/// - Inside the bracket, the first token is **exactly one ASCII letter**.
///   Not a digit: `pygettext3` writes its two reference footnotes as
///   `[1] https://…` / `[2] https://…`, consecutive rows that satisfy every
///   structural rule here, and a footnote marker is not a modifier. Not two
///   characters: `[ab]` is the optional-group notation a *command* row
///   spells its accepted modifiers with (`ar`'s `m[ab]`, already read by
///   [`strip_optional_modifier_suffix`]), and `[COMMON_OPTIONS]` is a usage
///   placeholder.
/// - Anything further inside the bracket is that letter's **operand**
///   (`[l <text> ]`), kept verbatim.
/// - After the bracket there must be an explicit **separator** — a ` - `
///   run, or a column gap of two or more spaces (or a tab) — and then a
///   non-empty description. A single space and then text is not a modifier
///   row; that is the footnote shape a second time, and it is the whole
///   difference between a table and a sentence that opens with a bracket.
///
/// The dash is punctuation *between two columns*, exactly what
/// [`split_at_dash`] already treats it as, so it is stripped rather than
/// left at the head of the description. It is looked for before the column
/// gap because both specimens write one, and only the dash reading gets
/// `llvm-ar`'s single-space rows apart into two columns at all.
pub(super) fn split_modifier_table_row(line: &str) -> Option<ModifierRow> {
    let trimmed = line.trim();
    let inner_and_rest = trimmed.strip_prefix('[')?;
    let close = inner_and_rest.find(']')?;
    let inner = &inner_and_rest[..close];
    let rest = &inner_and_rest[close + 1..];

    let mut tokens = inner.split_whitespace();
    let head = tokens.next()?;
    let mut head_chars = head.chars();
    let letter = head_chars.next()?;
    if head_chars.next().is_some() || !letter.is_ascii_alphabetic() {
        return None;
    }
    let operand = tokens.collect::<Vec<_>>().join(" ");
    let value_name = (!operand.is_empty()).then_some(operand);

    // `find_dash_separator` wants the space *before* the dash, which the
    // slice after `]` still carries. A row whose dash has no space in front
    // of it (`[a]- text`) is not two columns and is correctly refused here.
    let description = match find_dash_separator(rest) {
        Some(idx) => split_at_dash(rest, idx).1,
        None => {
            // [`find_multi_space_gap`] wants content *before* a gap, and
            // `rest` opens with the gap itself, so it cannot answer this
            // one. The leading run is measured directly instead, against
            // that function's own [`MIN_COLUMN_GAP_SPACES`] threshold and
            // its same "a tab is always a column gap" rule.
            let gap = rest.len() - rest.trim_start().len();
            let is_column_gap =
                gap >= MIN_COLUMN_GAP_SPACES || rest.get(..gap).is_some_and(|g| g.contains('\t'));
            if !is_column_gap {
                return None;
            }
            rest.trim().to_string()
        }
    };
    // A description has to *say* something. Emptiness is not a strong
    // enough test on its own: `[a]  -` (a row whose separator is the last
    // thing on the line) leaves the lone dash behind as the description,
    // since trimming the line first puts the dash out of
    // `find_dash_separator`'s reach and the column-gap branch then reads it
    // as content. Requiring one alphanumeric character refuses that and
    // every other punctuation-only remnant, while keeping real descriptions
    // that merely start with punctuation (`-1 means unlimited`).
    let description = description.trim();
    if !description.chars().any(|c| c.is_alphanumeric()) {
        return None;
    }
    Some(ModifierRow {
        letter,
        value_name,
        description: description.to_string(),
    })
}

/// Scan a run of modifier-table rows starting at `lines[start]` — the
/// `[a]`/`[b]`/`[D]` tables binutils `ar` prints under ` command specific
/// modifiers:` and ` generic modifiers:`, and `llvm-ar` under `MODIFIERS:`.
/// Returns the index just past the run and its rows, or `None` when the run
/// is shorter than [`MIN_MODIFIER_TABLE_ROWS`].
///
/// **The run must open at `lines[start]`.** That is what stops this from
/// reaching into the middle of somebody else's block and claiming a stray
/// bracketed line as a table: it is offered a heading's first content line,
/// and declines immediately if that line is not a modifier row.
///
/// It also **stops at the first line that is not one**, which is what makes
/// this safe to run ahead of the flags scanner rather than instead of it:
/// `ar`'s ` generic modifiers:` is seven bracket rows, then `@<file>`, then
/// four ordinary long options, and everything from `@<file>` onward is left
/// exactly where it was for the caller's existing flag-block handling to
/// read unchanged — including the group those four flags already carry.
///
/// A line indented past the run's own baseline folds into the previous
/// row's description, the same wrapped-continuation rule
/// [`split_entries`] and [`scan_bare_command_table`] both apply.
pub(super) fn scan_modifier_table(
    lines: &[&str],
    start: usize,
) -> Option<(usize, Vec<ModifierRow>)> {
    let baseline = leading_whitespace(lines[start]);
    let mut rows: Vec<ModifierRow> = Vec::new();
    let mut i = start;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            break;
        }
        let indent = leading_whitespace(line);
        if indent > baseline {
            let Some(last) = rows.last_mut() else {
                break;
            };
            last.description.push(' ');
            last.description.push_str(line.trim());
            i += 1;
            continue;
        }
        if indent < baseline {
            break;
        }
        let Some(row) = split_modifier_table_row(line) else {
            break;
        };
        rows.push(row);
        i += 1;
    }
    (rows.len() >= MIN_MODIFIER_TABLE_ROWS).then_some((i, rows))
}

#[cfg(test)]
mod modifier_tests {
    use super::*;

    fn row(line: &str) -> Option<ModifierRow> {
        split_modifier_table_row(line)
    }

    /// Both real spellings of a modifier row: `ar`'s column-padded
    /// dash-separated one and `llvm-ar`'s single-space dash-separated one.
    /// The letter, the description and the stripped separator come out the
    /// same either way — the two tools' formatting differs, what they
    /// document does not.
    #[test]
    fn both_real_spellings_of_a_modifier_row_read_the_same() {
        let padded = row("  [a]          - put file(s) after [member-name]").expect("ar row");
        let tight = row("  [a] - put [files] after [relpos]").expect("llvm-ar row");
        assert_eq!(padded.letter, 'a');
        assert_eq!(tight.letter, 'a');
        assert_eq!(padded.description, "put file(s) after [member-name]");
        assert_eq!(tight.description, "put [files] after [relpos]");
        assert_eq!(padded.value_name, None);
    }

    /// `ar`'s `[l <text> ]`: the operand inside the brackets is the
    /// modifier's value, not part of its letter and not part of its
    /// description.
    #[test]
    fn an_operand_inside_the_brackets_is_the_value() {
        let r = row("  [l <text> ]  - specify the dependencies of this library").expect("row");
        assert_eq!(r.letter, 'l');
        assert_eq!(r.value_name.as_deref(), Some("<text>"));
        assert_eq!(r.description, "specify the dependencies of this library");
    }

    /// A column gap with no dash at all is still a two-column row.
    #[test]
    fn a_column_gap_alone_separates_a_modifier_row() {
        let r = row("  [v]     be verbose").expect("row");
        assert_eq!(r.letter, 'v');
        assert_eq!(r.description, "be verbose");
    }

    /// **The refusals, one per rule.** `pygettext3`'s reference footnotes
    /// are the fleet's only near-miss (measured over the 2,301 frozen
    /// captures under `audit/queue-captures/`) and they fail twice over: a
    /// digit is not a letter, and one space is not a separator. The rest
    /// are the notations a bracketed token otherwise carries in real help
    /// text, none of which documents a modifier.
    #[test]
    fn the_shapes_that_are_not_modifier_rows_are_refused() {
        // pygettext3's footnotes — a digit, and a single space.
        assert_eq!(
            row(" [1] https://www.python.org/workshops/1997-10/proceedings/loewis.html"),
            None
        );
        assert_eq!(row(" [1]   https://example.invalid/paper"), None, "a digit");
        assert_eq!(row("  [a] one space only"), None, "no separator");
        // Multi-character brackets: an optional-group suffix's own group,
        // and a usage placeholder.
        assert_eq!(row("  [ab]  - put file(s) somewhere"), None);
        assert_eq!(row("  [COMMON_OPTIONS]  - the usual"), None);
        // A bracketed *flag*, which is usage notation, not a modifier.
        assert_eq!(row("  [-a]  - some flag"), None);
        // A row with a letter and a separator but nothing after it.
        assert_eq!(row("  [a]  -   "), None);
        // Not a bracket row at all.
        assert_eq!(row("  -a   some flag"), None);
        assert_eq!(row("  [a  - never closed"), None);
    }

    /// A modifier table is a *run*: one bracketed row on its own is not
    /// enough evidence, which is what keeps an isolated bracketed line in
    /// somebody else's block from becoming a one-row MODIFIERS section.
    #[test]
    fn one_row_is_not_a_table() {
        let lines = ["  [a]  - put file(s) after", "  something else entirely"];
        assert_eq!(scan_modifier_table(&lines, 0), None);
    }

    /// The scan stops at the first row that is not a modifier row, leaving
    /// the rest of the block where it was. This is what lets `ar`'s
    /// ` generic modifiers:` keep its four long options — and their group —
    /// while its seven bracket rows become modifiers.
    #[test]
    fn the_scan_stops_at_the_first_row_that_is_not_one() {
        let lines = [
            "  [c]          - do not warn if the library had to be created",
            "  [s]          - create an archive index (cf. ranlib)",
            "  @<file>      - read options from <file>",
            "  --thin       - make a thin archive",
        ];
        let (end, rows) = scan_modifier_table(&lines, 0).expect("two rows is a table");
        assert_eq!(end, 2, "stopped before @<file>");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].letter, 's');
    }

    /// A wrapped description folds into the row above it, the same rule
    /// every other block scanner in this module applies.
    #[test]
    fn a_wrapped_description_folds_into_its_row() {
        let lines = [
            "  [u]  - only replace files that are newer",
            "         than current archive contents",
            "  [v]  - be verbose",
        ];
        let (end, rows) = scan_modifier_table(&lines, 0).expect("table");
        assert_eq!(end, 3);
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0].description,
            "only replace files that are newer than current archive contents"
        );
    }

    /// The run has to open at the line the scan is offered: it is never
    /// allowed to hunt forward into a block for something bracket-shaped.
    #[test]
    fn the_run_must_open_at_the_offered_line() {
        let lines = [
            "  ordinary text here",
            "  [a]  - put file(s) after",
            "  [b]  - put file(s) before",
        ];
        assert_eq!(scan_modifier_table(&lines, 0), None);
    }

    /// End to end through the engine, on `ar`'s own shape: the modifier
    /// rows become modifiers, and the four long options that follow the
    /// bracket rows *inside the same section* keep both their spellings and
    /// the group that section names. The second half is the whole reason
    /// the call site falls through instead of restarting the loop.
    #[test]
    fn a_modifier_table_leaves_the_flags_beneath_it_alone() {
        let help = "\
Usage: ar [-]{dmpqrstx}[abcDfilMNoOPsSTuvV] archive-file file...
 command specific modifiers:
  [a]          - put file(s) after [member-name]
  [b]          - put file(s) before [member-name] (same as [i])
  [N]          - use instance [count] of name
 generic modifiers:
  [c]          - do not warn if the library had to be created
  [l <text> ]  - specify the dependencies of this library
  @<file>      - read options from <file>
  --target=BFDNAME - specify the target object format as BFDNAME
  --thin       - make a thin archive
";
        let parsed = parse_named(help, "ar");

        let letters: Vec<&str> = parsed
            .modifiers
            .iter()
            .map(mandible_core::Entity::primary_name)
            .collect();
        assert_eq!(letters, ["a", "b", "N", "c", "l"], "{letters:?}");

        // The operand row keeps its value, and the letter is not part of it.
        let l = parsed
            .modifiers
            .iter()
            .find(|m| m.primary_name() == "l")
            .expect("[l <text> ]");
        assert_eq!(l.value_name.as_deref(), Some("<text>"));

        // Each table's own heading names its group.
        assert_eq!(
            parsed.modifiers[0].group.as_deref(),
            Some("command specific modifiers:")
        );
        assert_eq!(
            parsed.modifiers[3].group.as_deref(),
            Some("generic modifiers:")
        );

        // ...and the flags after the bracket rows are untouched, group
        // included.
        for want in ["target", "thin"] {
            let f = parsed
                .flags
                .iter()
                .find(|f| f.long() == Some(want))
                .unwrap_or_else(|| panic!("--{want} lost"));
            assert_eq!(f.group.as_deref(), Some("generic modifiers:"));
        }
        // A modifier is never also a flag.
        assert!(!parsed.flags.iter().any(|f| f.short() == Some('a')));
    }

    /// `llvm-ar`'s spelling of the same table: an explicit `MODIFIERS:`
    /// heading and single-space dash separators. Nothing about the
    /// recognizer is keyed on either tool — this is the second document
    /// that proves it.
    #[test]
    fn the_other_tools_modifier_table_reads_the_same_way() {
        let help = "\
OVERVIEW: LLVM Archiver

USAGE: llvm-ar [options] [-]<operation>[modifiers] <archive> [files]

MODIFIERS:
  [a] - put [files] after [relpos]
  [b] - put [files] before [relpos] (same as [i])
  [c] - do not warn if archive had to be created
";
        let parsed = parse_named(help, "llvm-ar");
        let letters: Vec<&str> = parsed
            .modifiers
            .iter()
            .map(mandible_core::Entity::primary_name)
            .collect();
        assert_eq!(letters, ["a", "b", "c"]);
        assert_eq!(
            parsed.modifiers[0].description.as_ref().map(|d| d.as_str()),
            Some("put [files] after [relpos]")
        );
        assert_eq!(parsed.modifiers[0].group.as_deref(), Some("MODIFIERS:"));
    }

    /// The fleet's one near-miss, end to end: `pygettext3`'s reference
    /// footnotes must not become a two-entry MODIFIERS section on a tool
    /// that has no modifiers at all.
    #[test]
    fn reference_footnotes_never_become_modifiers() {
        let help = "\
Usage: pygettext [options] inputfile ...

Options:
  -h, --help    print this help message and exit

 [1] https://www.python.org/workshops/1997-10/proceedings/loewis.html
 [2] https://www.gnu.org/software/gettext/gettext.html
";
        let parsed = parse_named(help, "pygettext");
        assert!(parsed.modifiers.is_empty(), "{:?}", parsed.modifiers);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                parsed.flags.iter().any(|f| f.long() == Some(want)),
                "--{want} consumed by the operand table: {:?}",
                parsed.flags.iter().map(|f| f.long()).collect::<Vec<_>>()
            );
        }
        // And the operands above them are still read as the bare block
        // they are, not promoted into flags or subcommands.
        assert!(!parsed.flags.iter().any(|f| f.long() == Some("bs")));
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
}
