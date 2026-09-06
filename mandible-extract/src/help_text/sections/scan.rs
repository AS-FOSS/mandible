//! Block scanners for bare-word content: operand tables, argparse subparser
//! lists, headingless invocation tables, and bare command tables.

use super::*;

/// Scan a bare-word block (subcommand names, enum values, ...) starting at
/// `lines[start]`. Returns the index past the block and the recovered
/// `(name, description)` pairs. Unlike [`scan_flags_block`] there is no `-`
/// marker to key off, so this is indentation-based: the first content
/// line's indent is the baseline, and a deeper line continues the
/// previous entry's description.
///
/// `allow_dash_separator` threads the ` - ` entry separator down to
/// [`split_entries`]; decided by the caller before scanning, not here.
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

/// Find the end of a bare-word block starting at `lines[start]`: the
/// block runs until a non-blank line dedents below its own baseline
/// indent **or a flag row resumes**, whichever comes first. Shared by
/// [`scan_bare_block`] and [`scan_argparse_subparsers`].
///
/// A bare-word block (e.g. an enum of values) can sit *inside* an options
/// table at an indent the table then resumes at, so dedent alone never
/// ends it and the resumed flag rows get consumed as fake choices. A flag
/// row therefore ends the block; the caller resumes its main loop at that
/// line and reads it as a flags block instead. See docs/shapes.md S-033
/// and corpus/sg_dd/audit-seed2, corpus/tar/1.35.
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

/// Recognizes argparse's `add_subparsers()` shape: a `{choice,choice,...}`
/// pseudo-entry followed by each real subcommand one indent level
/// *deeper* than it. The generic bare-word rule would otherwise fold
/// those subcommands into the pseudo-entry's own description. Gated on
/// the structural `{...}` pseudo-entry, never on heading text alone — an
/// ordinary `positional arguments:` block with no such entry returns
/// `None` and falls through to plain positional handling. See
/// docs/shapes.md S-073.
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
    // `false`: argparse renders subparser help column-aligned, never
    // `name - description`, so the dash separator never applies here.
    // See docs/shapes.md S-073.
    Some((end, split_entries(&sub_lines, false)))
}

/// Scan a busybox-shaped comma-separated applet block starting at
/// `lines[start]`, gated on
/// [`super::profile::FrameworkProfile::comma_separated_command_list`].
/// No name/description split exists here — the block is a flat,
/// wrapped `token, token, token,` run — so this returns `(name, "")`
/// pairs directly instead of delegating to [`split_entries`]. See
/// docs/shapes.md S-093.
/// Scan a command table at the *same* indent as its heading (`dnf`'s
/// flush-left command list). All-or-nothing: one non-column-aligned row
/// rejects the whole block rather than ending it early, since stopping
/// early would let prose get promoted to subcommands. See docs/shapes.md
/// S-050.
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

/// Fewest name-row / deeper-description-row pairs
/// [`scan_headingless_invocation_table`] requires before treating a run
/// as a real table rather than one stray line — shared with
/// [`nested_entry_table_starts_at`] and [`scan_same_indent_entry_table`].
/// See docs/shapes.md S-016.
pub(super) const MIN_INVOCATION_TABLE_ROWS: usize = 2;

/// Recognize a **headingless invocation table** starting at
/// `lines[start]`: rows of the tool's own invocation forms (`btrfs
/// balance start [options] <path>`), description one indent deeper, no
/// governing heading. Every row must start with the tool's own name at
/// a word boundary ([`starts_with_tool_name`]) — that supplies both the
/// heading-equivalent evidence and the nesting (`btrfs device add ...`
/// reads as child `device`, grandchild `add`). Requires at least
/// [`MIN_INVOCATION_TABLE_ROWS`] such rows; each emitted name is
/// checked ([`token_occurs_literally`]) against the raw text; only the
/// leading run of [`is_command_name_shaped`] tokens after the tool's
/// name contributes a child (`run[0]`) and grandchild (`run[1]`).
/// Returns `None` on refusal without partial consumption. Every emitted
/// node is `invocation_attested: true`, `heading_attested: false`. See
/// docs/shapes.md S-016.
// Ratchet: one table walk with interleaved lookahead; splitting it needs the pass split first. Listed in scripts/ratchet.txt.
#[allow(clippy::cognitive_complexity)]
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
    // with no real description still becomes a node, just undescribed)
    // and clear it.
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
            // Orphaned deeper line with nothing pending to attach to;
            // skip rather than risk misreading it as a new row.
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
            // Shouldn't happen by construction, but guard explicitly
            // (spec M-10) — refuse this row rather than trust it.
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

/// Leading run (up to two tokens) of [`is_command_name_shaped`] tokens in
/// `trimmed` after stripping `tool_name` from the front — read as
/// `(child, Option<grandchild>)`. `None` when `trimmed` doesn't start
/// with `tool_name`, or the first token after it isn't name-shaped. See
/// docs/shapes.md S-016.
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

/// Whole-token occurrence check for existence attestation (spec M-10,
/// §6): is `token` present in `raw` as a maximal run of
/// [`is_command_name_shaped`]'s character class, not merely a substring
/// of a longer word (`"sub"` must not "occur" inside `"subvolume"`)?
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
/// (`wpa_cli`'s row shape), else the leading token as the name with no
/// description (`apt-ftparchive`'s `sources srcpath [overridefile
/// [pathprefix]]`). The no-separator branch never treats trailing words
/// as a description — for `apt-ftparchive` those are positional
/// operands, and reading them as prose would fabricate a description the
/// tool never wrote (spec §1). `None` when the leading token isn't
/// [`is_command_name_shaped`], letting [`scan_bare_command_table`] skip a
/// stray line without rejecting the whole table. See docs/shapes.md
/// S-017.
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
/// `apt-ftparchive`'s `Commands:` table. Reuses [`bare_block_end`] like
/// [`scan_bare_block`], splits each row with
/// [`split_bare_command_table_row`], and emits `invocation_attested`
/// rather than `heading_attested`.
///
/// Bails outright if any row has a real column gap
/// ([`find_multi_space_gap`]) or a ` - ` separator
/// ([`find_dash_separator`]) — those shapes already parse correctly
/// elsewhere and must not be discarded in favor of this table's weaker
/// "leading token, no description" fallback. The admission floor
/// ([`MIN_INVOCATION_TABLE_ROWS`]) counts distinct qualifying names, not
/// rows: `trash-put`'s own worked example repeats one program's name
/// twice and must not count twice. See docs/shapes.md S-017.
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
            // description, same rule `split_entries` uses to fold
            // wrapped description lines. Never invents a description
            // where the entry row had none.
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
    // several different commands. Two rows sharing one name is the shape
    // a worked usage example produces instead. See docs/shapes.md S-017.
    (qualifying_names.len() >= MIN_INVOCATION_TABLE_ROWS && !entries.is_empty())
        .then_some((end, entries))
}

/// One row of a modifier table: the letter, its operand, and its
/// description. See docs/shapes.md S-020.
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

/// Shortest run of rows read as a modifier table — same floor
/// [`MIN_ATTESTED_SECTION_FLAGS`] uses: one bracketed row is cheap to
/// produce by accident, a run of them is a table. `pygettext3`'s
/// reference footnotes are the fleet's only near-miss, and are refused
/// on independent grounds inside the row grammar itself. See
/// docs/shapes.md S-020.
const MIN_MODIFIER_TABLE_ROWS: usize = 2;

/// Split one modifier-table row — `ar`'s `[a]          - put file(s) after
/// [member-name]`, `llvm-ar`'s `[a] - put [files] after [relpos]` — into a
/// [`ModifierRow`]. `None` for anything not that shape.
///
/// Narrow grammar: opens with `[`, closes on the same row; inside, the
/// first token must be exactly one ASCII letter (not a digit —
/// `pygettext3`'s footnotes `[1] https://…` look identical otherwise; not
/// two letters — `[ab]` is a command row's optional-group notation).
/// Anything further inside the bracket is the operand (`[l <text> ]`).
/// After the bracket, an explicit ` - ` or column-gap separator plus a
/// non-empty description is required — a single space is the footnote
/// shape again, not a modifier row. See docs/shapes.md S-020.
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

    // `find_dash_separator` wants the space before the dash. A row whose
    // dash has none (`[a]- text`) is not two columns, refused here.
    let description = match find_dash_separator(rest) {
        Some(idx) => split_at_dash(rest, idx).1,
        None => {
            // `find_multi_space_gap` wants content before a gap, but
            // `rest` opens with the gap itself, so measure it directly
            // against [`MIN_COLUMN_GAP_SPACES`] instead.
            let gap = rest.len() - rest.trim_start().len();
            let is_column_gap =
                gap >= MIN_COLUMN_GAP_SPACES || rest.get(..gap).is_some_and(|g| g.contains('\t'));
            if !is_column_gap {
                return None;
            }
            rest.trim().to_string()
        }
    };
    // Emptiness alone isn't a strong enough test: `[a]  -` leaves the
    // lone dash as the description. Requiring one alphanumeric character
    // refuses that while keeping descriptions starting with punctuation
    // (`-1 means unlimited`).
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

/// One row of an environment section: the variable name and its
/// description. See docs/shapes.md S-023.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EnvVarRow {
    /// The variable name itself, e.g. `NODE_DEBUG`.
    pub name: String,
    /// The row's description, with the separator that introduced it
    /// removed. Never empty — a row without one is not admitted at all.
    pub description: String,
}

/// True for a token shaped like a POSIX shell identifier: an ASCII
/// letter or underscore, then any run of ASCII letters, digits or
/// underscores. Refuses a flag spelling (`--thin`) or prose; admits
/// both `ALL_CAPS` and lowercase (`http_proxy`) since case isn't part of
/// the shape. See docs/shapes.md S-023.
fn is_env_var_name_shaped(token: &str) -> bool {
    let mut chars = token.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// True when `heading` is one of the small set of exact, explicit
/// spellings a tool uses to introduce its own environment-variable
/// documentation.
///
/// Heading-keyed, not row-keyed — the opposite choice from
/// [`split_modifier_table_row`]/[`scan_modifier_table`]: a bare
/// identifier/separator/description row alone is indistinguishable from
/// an ordinary config-variable table (`mysqlslap`'s flush-left settings
/// list, spec M-10), so the heading is the only reliable signal and is
/// what's keyed on. Normalizes by trimming and dropping one optional
/// trailing colon only — a wrapped sentence that *ends* with
/// "environment variable." keeps its period and is correctly refused.
/// See docs/shapes.md S-023.
pub(super) fn is_environment_heading(heading: &str) -> bool {
    let normalized = heading.trim().trim_end_matches(':').trim().to_lowercase();
    matches!(
        normalized.as_str(),
        "environment" | "environment variable" | "environment variables"
    )
}

/// Split one environment-section row — `bpftrace`'s
/// `BPFTRACE_BTF  [default: none] BTF file`, `node`'s `NODE_DEBUG  ','-
/// separated list of core modules`, `mksquashfs`'s tab-separated
/// `SOURCE_DATE_EPOCH\tIf set, ...` — into an [`EnvVarRow`]. `None` for
/// anything not that shape.
///
/// Reuses [`split_entry_line_raw`]'s separator grammar (column gap via
/// [`find_multi_space_gap`], else ` - ` via [`find_dash_separator`]). The
/// first token must be [shell-identifier-shaped][is_env_var_name_shaped],
/// which refuses a flag row (`--thin`). See docs/shapes.md S-023 and
/// corpus/bpftrace, corpus/node.
pub(super) fn split_env_var_row(line: &str) -> Option<EnvVarRow> {
    let trimmed = line.trim();
    let name_end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
    let name = &trimmed[..name_end];
    if !is_env_var_name_shaped(name) {
        return None;
    }
    let description = if let Some(idx) = find_dash_separator(trimmed) {
        split_at_dash(trimmed, idx).1
    } else {
        let gap = find_multi_space_gap(trimmed)?;
        trimmed[gap..].trim().to_string()
    };
    // A description has to *say* something, the same "at least one
    // alphanumeric character" test `split_modifier_table_row` applies —
    // refuses a row whose separator is the last thing on the line.
    let description = description.trim();
    if !description.chars().any(|c| c.is_alphanumeric()) {
        return None;
    }
    Some(EnvVarRow {
        name: name.to_string(),
        description: description.to_string(),
    })
}

/// [`split_env_var_row`]'s permissive twin: a lone space is accepted as
/// the separator, the shape a name too long for its table's column
/// convention falls back to (`node`'s `NODE_PENDING_PIPE_INSTANCES set
/// the number of pending pipe instance`).
///
/// Never called for a candidate first row — only from inside
/// [`scan_env_var_table`]'s loop once a stricter row has already
/// confirmed the table, since one space alone is too cheap a signal to
/// open a table on. See docs/shapes.md S-023.
pub(super) fn split_env_var_row_single_space_fallback(line: &str) -> Option<EnvVarRow> {
    let trimmed = line.trim();
    let name_end = trimmed.find(char::is_whitespace)?;
    let name = &trimmed[..name_end];
    if !is_env_var_name_shaped(name) {
        return None;
    }
    let rest = &trimmed[name_end..];
    let after_one_space = rest.strip_prefix(' ')?;
    // More than one space here is already `find_multi_space_gap`'s shape,
    // which `split_env_var_row` would have taken care of — this function
    // exists only for the *exactly one* case.
    if after_one_space.starts_with(char::is_whitespace) {
        return None;
    }
    let description = after_one_space.trim();
    if description.is_empty() || !description.chars().any(|c| c.is_alphanumeric()) {
        return None;
    }
    Some(EnvVarRow {
        name: name.to_string(),
        description: description.to_string(),
    })
}

/// Shortest run of rows [`scan_env_var_table`] accepts: **one**,
/// deliberately lower than [`MIN_MODIFIER_TABLE_ROWS`]'s floor of two.
/// A modifier table has no reliable heading, so row repetition is its
/// only evidence; an environment section's heading is already narrow,
/// explicit evidence, so one row need only clear
/// [`split_env_var_row`]'s ordinary bar. `ebtables`/`ebtables-nft` label
/// the heading with zero rows beneath it, producing no section either
/// way. See docs/shapes.md S-023.
const MIN_ENV_VAR_TABLE_ROWS: usize = 1;

/// Scan a run of environment-section rows starting at `lines[start]`,
/// immediately after a heading [`is_environment_heading`] already
/// accepted. Returns the index past the run and its rows, or `None`
/// below [`MIN_ENV_VAR_TABLE_ROWS`].
///
/// Skips at most one leading non-row line before the run must open —
/// `gprofng`'s `Environment:` heading is followed by an intro sentence,
/// then a blank line, then its real rows; safe because the heading is
/// already positive evidence, unlike [`scan_modifier_table`] which must
/// open at the exact offered line.
///
/// A deeper-indented line folds into the previous row's description,
/// same rule as [`scan_modifier_table`]. The run stops at the first
/// blank line: `gprofng`'s real section blank-separates its two
/// variables as two paragraphs, so only the first is recovered — a
/// documented miss (spec §13.1e), not a special case. See
/// docs/shapes.md S-023.
pub(super) fn scan_env_var_table(lines: &[&str], start: usize) -> Option<(usize, Vec<EnvVarRow>)> {
    let mut idx = start;
    if idx < lines.len() && !lines[idx].trim().is_empty() && split_env_var_row(lines[idx]).is_none()
    {
        idx += 1;
        while idx < lines.len() && lines[idx].trim().is_empty() {
            idx += 1;
        }
    }
    if idx >= lines.len() {
        return None;
    }
    let baseline = leading_whitespace(lines[idx]);
    let mut rows: Vec<EnvVarRow> = Vec::new();
    let mut i = idx;
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
        let row = match split_env_var_row(line) {
            Some(row) => row,
            None => {
                // A name too long to leave room for a separator on its own
                // line — `node`'s own `NODE_TLS_REJECT_UNAUTHORIZED`, whose
                // description begins entirely on the folded continuation
                // beneath it, the same shape a flags block's own longest
                // spellings use. Accepted only when the line is *nothing
                // but* an identifier-shaped name and a deeper-indented
                // line immediately follows to supply the description —
                // never merely because a row failed to split, which would
                // turn this into a second, laxer row grammar.
                let name = line.trim();
                let has_continuation = lines
                    .get(i + 1)
                    .is_some_and(|l| !l.trim().is_empty() && leading_whitespace(l) > baseline);
                if is_env_var_name_shaped(name) && has_continuation {
                    EnvVarRow {
                        name: name.to_string(),
                        description: String::new(),
                    }
                } else if !rows.is_empty() {
                    // A single space where an unusually long name overflowed
                    // its own column convention's alignment (`node`'s own
                    // `NODE_PENDING_PIPE_INSTANCES set the number of...`,
                    // one space, not the 2+ every shorter row in the same
                    // table gets). `split_env_var_row` requires an explicit
                    // separator everywhere else in this grammar, and still
                    // does for a *candidate first row* — the heading alone
                    // is not enough to trust a bare single space there. It
                    // is enough once a real row has already been recovered
                    // under this same heading: a second, independent piece
                    // of evidence for the same table earns the row after it
                    // the single-space fallback, and only that row.
                    match split_env_var_row_single_space_fallback(line) {
                        Some(row) => row,
                        None => break,
                    }
                } else {
                    break;
                }
            }
        };
        rows.push(row);
        i += 1;
    }
    (rows.len() >= MIN_ENV_VAR_TABLE_ROWS).then_some((i, rows))
}

#[cfg(test)]
mod modifier_tests {
    use super::*;

    fn row(line: &str) -> Option<ModifierRow> {
        split_modifier_table_row(line)
    }

    /// `ar`'s column-padded and `llvm-ar`'s single-space dash-separated
    /// modifier rows parse to the same letter/description shape. See
    /// docs/shapes.md S-020.
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

    /// `ar`'s `[l <text> ]`: the bracketed operand is the modifier's
    /// value, not part of its letter or description.
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

    /// The refusals, one per rule. `pygettext3`'s footnotes are the
    /// fleet's only near-miss, refused on two independent grounds: a
    /// digit isn't a letter, and one space isn't a separator. See
    /// docs/shapes.md S-020.
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

    /// A modifier table is a run: one bracketed row alone is not enough
    /// evidence. See docs/shapes.md S-020.
    #[test]
    fn one_row_is_not_a_table() {
        let lines = ["  [a]  - put file(s) after", "  something else entirely"];
        assert_eq!(scan_modifier_table(&lines, 0), None);
    }

    /// The scan stops at the first row that is not a modifier row,
    /// leaving `ar`'s long options after the bracket rows untouched.
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

    /// A wrapped description folds into the row above it.
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

    /// The run must open at the offered line; it never hunts forward
    /// for a bracket-shaped line.
    #[test]
    fn the_run_must_open_at_the_offered_line() {
        let lines = [
            "  ordinary text here",
            "  [a]  - put file(s) after",
            "  [b]  - put file(s) before",
        ];
        assert_eq!(scan_modifier_table(&lines, 0), None);
    }

    /// End to end on `ar`'s own shape: modifier rows become modifiers,
    /// and the long options following them in the same section keep
    /// their spelling and group. See docs/shapes.md S-020.
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

    /// `llvm-ar`'s spelling: explicit `MODIFIERS:` heading, single-space
    /// dash separators. Nothing here is keyed on tool name.
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

    /// `pygettext3`'s two reference footnotes must not become a
    /// MODIFIERS section. Set under a heading here (unlike the real
    /// document, where they sit in an ungoverned prose region) so the
    /// row grammar is actually exercised rather than trivially passing.
    /// See docs/shapes.md S-020.
    #[test]
    fn reference_footnotes_never_become_modifiers() {
        let help = "\
Usage: pygettext [options] inputfile ...

References:
  [1] https://www.python.org/workshops/1997-10/proceedings/loewis.html
  [2] https://www.gnu.org/software/gettext/gettext.html
";
        let parsed = parse_named(help, "pygettext");
        assert!(parsed.modifiers.is_empty(), "{:?}", parsed.modifiers);
    }
}

#[cfg(test)]
mod env_var_tests {
    use super::*;

    fn row(line: &str) -> Option<EnvVarRow> {
        split_env_var_row(line)
    }

    /// Three real column shapes read the same way: wide column-gap
    /// (`bpftrace`), tab-separated (`mksquashfs`), narrow column-gap
    /// (`fzf`). See docs/shapes.md S-023.
    #[test]
    fn every_real_column_shape_reads_the_same() {
        let wide =
            row("    BPFTRACE_CACHE_USER_SYMBOLS       [default: auto] enable user symbol cache")
                .expect("bpftrace row");
        assert_eq!(wide.name, "BPFTRACE_CACHE_USER_SYMBOLS");
        assert_eq!(wide.description, "[default: auto] enable user symbol cache");

        let tabbed = row("SOURCE_DATE_EPOCH\tIf set, this is used as the filesystem creation")
            .expect("mksquashfs row");
        assert_eq!(tabbed.name, "SOURCE_DATE_EPOCH");
        assert_eq!(
            tabbed.description,
            "If set, this is used as the filesystem creation"
        );

        let narrow = row("    FZF_DEFAULT_COMMAND    Default command to use when input is tty")
            .expect("fzf row");
        assert_eq!(narrow.name, "FZF_DEFAULT_COMMAND");
        assert_eq!(
            narrow.description,
            "Default command to use when input is tty"
        );
    }

    /// A dash-separated row is accepted, pinning a branch no fleet tool
    /// happens to exercise for an environment row.
    #[test]
    fn a_dash_separated_row_is_accepted() {
        let r = row("PAGER - the pager used to display long output").expect("dash row");
        assert_eq!(r.name, "PAGER");
        assert_eq!(r.description, "the pager used to display long output");
    }

    /// The refusals: a flag row (opens with `-`), a row with no separator
    /// at all, and a row whose separator has nothing after it.
    #[test]
    fn the_shapes_that_are_not_env_var_rows_are_refused() {
        assert_eq!(row("  --thin       - make a thin archive"), None, "a flag");
        assert_eq!(row("NODE_DEBUG single space only"), None, "no separator");
        assert_eq!(row("NODE_DEBUG    "), None, "nothing after the gap");
        assert_eq!(
            row("3AMPLE   not an identifier"),
            None,
            "leads with a digit"
        );
    }

    /// A labeled section with exactly one row is still read: the
    /// heading is the evidence, unlike a modifier table's floor of two.
    #[test]
    fn one_row_is_enough_for_an_environment_section() {
        let lines = ["EDITOR    the editor to invoke"];
        let (end, rows) = scan_env_var_table(&lines, 0).expect("one row is enough");
        assert_eq!(end, 1);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "EDITOR");
    }

    /// `gprofng`'s shape: an intro sentence between the heading and the
    /// real rows is skipped. `gprofng`'s real text blank-separates its
    /// two variables into two paragraphs, so only the first is
    /// recovered — a recorded miss. See docs/shapes.md S-023.
    #[test]
    fn a_single_introductory_sentence_is_skipped() {
        let lines = [
            "The following environment variables are supported:",
            "",
            " GPROFNG_MAX_CALL_STACK_DEPTH  set the depth of the call stack (default is 256).",
            "",
            " GPROFNG_USE_JAVA_OPTIONS      may be set when profiling a C/C++ application",
            "                               that uses dlopen() to execute Java code.",
        ];
        let (end, rows) = scan_env_var_table(&lines, 0).expect("table after the sentence");
        assert_eq!(
            end, 3,
            "stops at the blank line separating the two variables"
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "GPROFNG_MAX_CALL_STACK_DEPTH");
    }

    /// A wrapped description folds into the row above it.
    #[test]
    fn a_wrapped_description_folds_into_its_row() {
        let lines = [
            "FORCE_COLOR                 when set to 'true', 1, 2, 3, or an",
            "                            empty string causes NO_COLOR to be ignored.",
            "NO_COLOR                    Alias for NODE_DISABLE_COLORS",
        ];
        let (end, rows) = scan_env_var_table(&lines, 0).expect("table");
        assert_eq!(end, 3);
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0].description,
            "when set to 'true', 1, 2, 3, or an empty string causes NO_COLOR to be ignored."
        );
    }

    /// A real ENVIRONMENT section becomes `EntityKind::EnvVar` entities;
    /// flags elsewhere in the same document stay untouched.
    #[test]
    fn an_environment_section_reads_end_to_end() {
        let help = "\
Usage: bpftrace [options] filename\n\nOPTIONS:\n    -f FORMAT      output format\n\nENVIRONMENT:\n    BPFTRACE_BTF                      [default: none] BTF file\n    BPFTRACE_CACHE_USER_SYMBOLS       [default: auto] enable user symbol cache\n";
        let parsed = parse_named(help, "bpftrace");
        let names: Vec<&str> = parsed
            .env_vars
            .iter()
            .map(mandible_core::Entity::primary_name)
            .collect();
        assert_eq!(names, ["BPFTRACE_BTF", "BPFTRACE_CACHE_USER_SYMBOLS"]);
        assert_eq!(parsed.env_vars[0].group.as_deref(), Some("ENVIRONMENT:"));
        // Unrelated flags block elsewhere stays untouched.
        assert!(!parsed.flags.is_empty(), "{:?}", parsed.flags);
    }

    /// `node`'s shape: heading and rows sit flush at the same indent
    /// (unlike bpftrace's or fzf's stepped-in rows), needing the
    /// same-indent branch (mirroring `dnf`'s table) to be read at all.
    #[test]
    fn a_flush_left_environment_section_is_still_read() {
        let help = "\
Usage: node [options] [ V8 options] [<program-entry-point> | -e \"script\" | -] [--] [arguments]

Options:
  -e, --eval=...             evaluate script

Environment variables:
FORCE_COLOR                 when set to 'true', 1, 2, 3, or an
                            empty string causes NO_COLOR to be ignored.
NO_COLOR                    Alias for NODE_DISABLE_COLORS
NODE_PENDING_PIPE_INSTANCES set the number of pending pipe instance
                            handles on Windows
NODE_TLS_REJECT_UNAUTHORIZED
                            set to 0 to disable TLS certificate
                            validation
";
        let parsed = parse_named(help, "node");
        let names: Vec<&str> = parsed
            .env_vars
            .iter()
            .map(mandible_core::Entity::primary_name)
            .collect();
        // Needs both fallbacks: a name overflowing to a single-space
        // separator, and one whose description starts on the next line.
        assert_eq!(
            names,
            [
                "FORCE_COLOR",
                "NO_COLOR",
                "NODE_PENDING_PIPE_INSTANCES",
                "NODE_TLS_REJECT_UNAUTHORIZED",
            ],
            "{names:?}"
        );
        assert_eq!(
            parsed.env_vars[2].description.as_ref().map(|d| d.as_str()),
            Some("set the number of pending pipe instance handles on Windows")
        );
        assert_eq!(
            parsed.env_vars[3].description.as_ref().map(|d| d.as_str()),
            Some("set to 0 to disable TLS certificate validation")
        );
        assert!(!parsed.flags.is_empty(), "{:?}", parsed.flags);
    }

    /// [`split_env_var_row_single_space_fallback`] accepts exactly one
    /// space, refusing what [`split_env_var_row`] already handles or
    /// refuses.
    #[test]
    fn the_single_space_fallback_accepts_only_a_lone_space() {
        assert_eq!(
            split_env_var_row_single_space_fallback(
                "NODE_PENDING_PIPE_INSTANCES set the number of pending pipe instance"
            ),
            Some(EnvVarRow {
                name: "NODE_PENDING_PIPE_INSTANCES".to_string(),
                description: "set the number of pending pipe instance".to_string(),
            })
        );
        assert_eq!(
            split_env_var_row_single_space_fallback("NODE_DEBUG    two spaces already"),
            None,
            "already split_env_var_row's own shape"
        );
        assert_eq!(
            split_env_var_row_single_space_fallback("NODE_DEBUG"),
            None,
            "nothing after the name at all"
        );
        assert_eq!(
            split_env_var_row_single_space_fallback("NODE_DEBUG "),
            None,
            "a lone trailing space with nothing after it"
        );
        assert_eq!(
            split_env_var_row_single_space_fallback("3AMPLE not an identifier"),
            None,
            "leads with a digit"
        );
    }

    /// The single-space fallback is never consulted for a *candidate first
    /// row* of a table: a single-space-only line offered as the table's
    /// opening line is not read as a row by the strict grammar, so the
    /// one-line-of-slack intro skip (for `gprofng`'s own prose sentence)
    /// takes it instead — dropped, not fabricated, and never contributing
    /// a name. The genuine row after it, in the ordinary two-space shape,
    /// still opens the table and is recovered.
    #[test]
    fn the_single_space_fallback_never_opens_a_table_on_its_own() {
        let lines = [
            "NODE_OPTIONS set default CLI options",
            "NODE_DEBUG    a real row",
        ];
        let (end, rows) = scan_env_var_table(&lines, 0).expect("second row opens the table");
        assert_eq!(end, 2);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "NODE_DEBUG");
    }

    /// A heading merely mentioning "environment" in prose never becomes
    /// an environment section — only the exact labeled forms do. See
    /// docs/shapes.md S-023.
    #[test]
    fn a_heading_that_only_mentions_environment_is_refused() {
        assert!(!is_environment_heading("Specify the target environment"));
        assert!(!is_environment_heading("environment variable."));
        assert!(!is_environment_heading("ENV"));
        assert!(!is_environment_heading("Environment Commands:"));
    }

    /// ALL_CAPS placeholder prose with no labeled heading produces zero
    /// `EnvVar` entities (spec §13.1e's fabrication class).
    #[test]
    fn all_caps_prose_never_becomes_env_vars() {
        let help = "\
Usage: mytool [OPTIONS] FILE\n\nOPTIONS:\n  -o, --output FILE   write output to FILE\n\nmytool reads PATH and TERM from its caller but does not document them\nas configuration; FILE above is a positional placeholder, not a variable.\n";
        let parsed = parse_named(help, "mytool");
        assert!(parsed.env_vars.is_empty(), "{:?}", parsed.env_vars);
    }

    /// A heading merely mentioning "env" is a subcommand list, not an
    /// environment section — [`is_environment_heading`]'s strict set
    /// excludes bare `env`/`Env Commands` spellings.
    #[test]
    fn a_bare_word_list_under_an_env_mentioning_heading_never_becomes_env_vars() {
        let help = "\
Usage: mytool [command]\n\nCommands:\n  env       print resolved environment\n  build     build the project\n  test      run tests\n";
        let parsed = parse_named(help, "mytool");
        assert!(parsed.env_vars.is_empty(), "{:?}", parsed.env_vars);
    }

    /// A real flags block under an environment-ish heading is still
    /// read as flags, not folded into an environment section.
    #[test]
    fn a_flags_block_under_an_environment_ish_heading_is_still_flags() {
        let help = "\
Usage: mytool [OPTIONS]\n\nEnvironment overrides:\n  -e, --env-file FILE   load environment overrides from FILE\n  -q, --quiet           suppress output\n";
        let parsed = parse_named(help, "mytool");
        assert!(parsed.env_vars.is_empty(), "{:?}", parsed.env_vars);
        assert_eq!(parsed.flags.len(), 2, "{:?}", parsed.flags);
    }
}

/// Fewest characters allowed on the left side of a comma before it stops
/// reading as a short alias: pnpm's own aliased root commands are all 1 or
/// 2 letters (`i`, `ln`, `rm`, `up`, `ls`, `c`). Kept small deliberately —
/// a wider bound risks reading an unrelated two-word row as an alias pair.
/// See docs/shapes.md S-104.
const MAX_RAGGED_ALIAS_CHARS: usize = 3;

/// A short-alias prefix on a ragged command row's own name field (`"i,
/// install"`, `"ln, link"`): pnpm's own convention for its 6 aliased root
/// commands. Returns `(primary, Some(alias))` when the text splits on one
/// comma into two [`is_command_name_shaped`] tokens, the left one at most
/// [`MAX_RAGGED_ALIAS_CHARS`] characters and strictly shorter than the
/// right — otherwise `(name, None)` unchanged, which also covers the
/// ordinary unaliased case (`"add"`, `"init"`). See docs/shapes.md S-104.
fn split_ragged_alias_prefix(name: &str) -> (&str, Option<&str>) {
    let Some((left, right)) = name.split_once(',') else {
        return (name, None);
    };
    let (left, right) = (left.trim(), right.trim());
    if is_command_name_shaped(left)
        && is_command_name_shaped(right)
        && left.chars().count() <= MAX_RAGGED_ALIAS_CHARS
        && left.chars().count() < right.chars().count()
    {
        (right, Some(left))
    } else {
        (name, None)
    }
}

/// One ragged-indent command-table row: a bare or short-alias-prefixed
/// name, its own description-column gap, and a gap-free description (no
/// further [`find_multi_space_gap`] — refuses `less --help`'s packed
/// key-binding rows). Folds its own deeper-indented, gap-free
/// continuations. Called only under `st.command_mode`, and only ever
/// accepted in a run of [`MIN_RAGGED_RUN`]+ via [`scan_ragged_command_run`].
/// See docs/shapes.md S-103, S-104; corpus/pnpm/11.22.0.
fn try_ragged_command_row(
    lines: &[&str],
    start: usize,
    group: Option<&str>,
) -> Option<(usize, CommandNode)> {
    let line = *lines.get(start)?;
    let trimmed = line.trim_start();
    if trimmed.is_empty() || looks_like_flag_start(trimmed) {
        return None;
    }
    let gap = find_description_gap(line)?;
    let (name_field, desc) = split_at_column(line, Some(gap));
    let name_field = name_field.trim();
    if name_field.is_empty() {
        return None;
    }
    let desc = desc.trim().to_string();
    if desc.is_empty() || find_multi_space_gap(&desc).is_some() {
        return None;
    }
    let (primary, alias) = split_ragged_alias_prefix(name_field);
    if !is_command_name_shaped(primary) {
        return None;
    }

    let row_indent = leading_whitespace(line);
    let mut end = start + 1;
    let mut full_desc = desc;
    while end < lines.len() {
        let cont = lines[end];
        if cont.trim().is_empty() {
            break;
        }
        if leading_whitespace(cont) <= row_indent {
            break;
        }
        if find_description_gap(cont).is_some() || looks_like_flag_start(cont.trim_start()) {
            break;
        }
        full_desc.push(' ');
        full_desc.push_str(cont.trim());
        end += 1;
    }

    let mut node = CommandNode::new(primary, Provenance::single(Source::HelpText));
    if let Some(alias) = alias {
        node.aliases.push(alias.to_string());
    }
    node.summary = non_empty_text(&full_desc);
    node.group = group.map(str::to_string);
    node.children_filled = false;
    node.heading_attested = true;
    Some((end, node))
}

/// Shortest run of [`try_ragged_command_row`] matches, in strict physical
/// adjacency, admitted as a real command table rather than noise — see
/// that function's own doc comment, gate 3.
const MIN_RAGGED_RUN: usize = 2;

/// Every [`try_ragged_command_row`] match starting at `lines[start]`,
/// requiring immediate adjacency (each row starts exactly where the
/// previous one's own span, continuations included, ended — no blank line
/// and no non-matching line between them) and at least [`MIN_RAGGED_RUN`]
/// of them. `None` on a lone match or no match at all; the caller then
/// falls through to the ordinary heading/block scanners exactly as if
/// this function did not exist. See docs/shapes.md S-103, S-104.
pub(super) fn scan_ragged_command_run(
    lines: &[&str],
    start: usize,
    group: Option<&str>,
) -> Option<(usize, Vec<CommandNode>)> {
    let mut nodes = Vec::new();
    let mut i = start;
    while let Some((end, node)) = try_ragged_command_row(lines, i, group) {
        nodes.push(node);
        i = end;
    }
    // `timedatectl`'s `Commands:` block mixes bare-name rows this grammar
    // reads (`status`, `show`) with rows carrying a trailing operand
    // (`set-time TIME`) it does not — and does not claim to (that operand
    // is no part of any command name). A run that stops mid-block, on a
    // non-blank line it simply does not recognize, must decline whole
    // rather than return a partial result and abandon everything after
    // it: the caller has no other path back to those rows once this one
    // returns. Accepted only when the run reaches a real boundary —
    // end of input or a blank line — the same signal `bare_block_end`
    // reads a block's own end from.
    let reached_boundary = i >= lines.len() || lines[i].trim().is_empty();
    (nodes.len() >= MIN_RAGGED_RUN && reached_boundary).then_some((i, nodes))
}

#[cfg(test)]
mod ragged_command_row_tests {
    use super::*;

    #[test]
    fn an_alias_prefixed_row_yields_its_primary_name_and_alias() {
        let lines = ["   i, install              Install all dependencies for a project"];
        let (end, node) = try_ragged_command_row(&lines, 0, None).unwrap();
        assert_eq!(end, 1);
        assert_eq!(node.name, "install");
        assert_eq!(node.aliases, vec!["i".to_string()]);
        assert_eq!(
            node.summary.as_ref().unwrap().as_str(),
            "Install all dependencies for a project"
        );
    }

    #[test]
    fn an_unaliased_row_folds_its_own_continuation() {
        let lines = [
            "      unlink               Unlinks a package. Like yarn unlink but pnpm",
            "                           re-installs the dependency after removing the",
            "                           external link",
        ];
        let (end, node) = try_ragged_command_row(&lines, 0, None).unwrap();
        assert_eq!(end, 3);
        assert_eq!(node.name, "unlink");
        assert!(node.aliases.is_empty());
        assert_eq!(
            node.summary.as_ref().unwrap().as_str(),
            "Unlinks a package. Like yarn unlink but pnpm re-installs the dependency after \
             removing the external link"
        );
    }

    /// The `less` false positive this detector must never fire on: a
    /// key-binding row whose own first 2+-space gap lands right after a
    /// single letter, exactly the way a real row's name field would, but
    /// whose "description" is itself another aligned column.
    #[test]
    fn a_packed_multi_column_reference_row_is_refused() {
        let lines = ["  e  ^E  j  ^N  CR  *  Forward  one line   (or _N lines)."];
        assert!(try_ragged_command_row(&lines, 0, None).is_none());
    }

    #[test]
    fn a_row_with_no_description_gap_is_refused() {
        let lines = ["  bareword"];
        assert!(try_ragged_command_row(&lines, 0, None).is_none());
    }

    #[test]
    fn a_flag_row_is_never_read_as_a_command_row() {
        let lines = ["  -x, --example         An ordinary flag, not a command"];
        assert!(try_ragged_command_row(&lines, 0, None).is_none());
    }

    /// pnpm's own ragged run: a shallower aliased row beside two deeper
    /// unaliased ones, admitted together because the run has 3 members.
    #[test]
    fn a_run_of_ragged_rows_is_admitted_together() {
        let lines = [
            "   i, install              Install all dependencies for a project",
            "  ln, link                 Connect the local project to another one",
            "      unlink               Unlinks a package.",
        ];
        let (end, nodes) = scan_ragged_command_run(&lines, 0, Some("Manage:")).unwrap();
        assert_eq!(end, 3);
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, vec!["install", "link", "unlink"]);
        assert!(nodes.iter().all(|n| n.group.as_deref() == Some("Manage:")));
    }

    /// `less`'s lone `v` row: it passes every single-row gate on its own,
    /// but the row before it (`s _f_i_l_e`) is not a match, so the run
    /// never reaches [`MIN_RAGGED_RUN`] and nothing is emitted.
    #[test]
    fn a_lone_matching_row_is_not_a_run() {
        let lines = ["  v                    Edit the current file with $VISUAL or $EDITOR."];
        assert!(scan_ragged_command_run(&lines, 0, None).is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `sg_dd`'s shape: a `where:` operand table whose own rows are bare
    /// words, ending in flag rows at the same indent. See docs/shapes.md
    /// S-033 and corpus/sg_dd/audit-seed2.
    ///
    /// Uses more than three operand rows deliberately: a short table is
    /// claimed outright by `flags_block_start`'s
    /// `MAX_SKIPPED_LEADING_ROWS` tolerance and never reaches
    /// [`bare_block_end`], so a two-row test would pass even reverted.
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

    /// The break is `i > start` for a reason: a block whose first line
    /// ended it would be zero-length and never advance.
    #[test]
    fn a_bare_block_with_no_flag_rows_is_unchanged() {
        let lines = ["  alpha   first", "  beta    second", "  gamma   third"];
        assert_eq!(bare_block_end(&lines, 0), 3);
    }

    /// The lookahead bound keeps this from "look harder until you find
    /// flags": a bare-word block with no `-`-leading row at its own
    /// indent stays a bare-word block.
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

    /// Real btrfs shape: two-level nesting, a shared description across
    /// sibling rows, single-level dedup of two rows naming the same
    /// command, a tab-indented description, and a row with no
    /// description staying genuinely empty. See docs/shapes.md S-016
    /// and corpus/btrfs/audit-seed2.
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

    /// The table is not truncated by btrfs's tab-indented `device
    /// replace` description row — its character-count indent is still
    /// deeper than the table's row indent, so the scan must not end
    /// early there. See docs/shapes.md S-016.
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

    /// [`MIN_INVOCATION_TABLE_ROWS`]'s floor counts described rows, not
    /// merely seen rows: two rows where only one gets a deeper-indented
    /// description line must not admit.
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

    /// A table whose rows do not start with the tool's own name is
    /// refused outright — that is the recognizer's whole evidentiary
    /// basis. See docs/shapes.md S-016.
    #[test]
    fn headingless_invocation_table_refuses_rows_not_naming_the_tool() {
        let raw = "  otherprog frobnicate [options] <path>\n      Frobnicate a path\n  \
                    otherprog defrobnicate [options] <path>\n      Defrobnicate a path\n";
        let parsed = parse_named(raw, "mytool");
        assert!(parsed.subcommands.is_empty());
    }

    /// A single name-row/description-row pair sits below the repetition
    /// floor and must not be promoted.
    #[test]
    fn headingless_invocation_table_refuses_a_single_pair() {
        let raw = "    mytool frob start <path>\n        Start frobbing\n";
        let parsed = parse_named(raw, "mytool");
        assert!(parsed.subcommands.is_empty());
    }

    /// `wpa_cli`'s shape: a recognized `commands:` heading whose rows
    /// separate name and description with ` = ` instead of a column gap;
    /// a few rows carry no separator at all. See docs/shapes.md S-017
    /// and corpus/wpa_cli.
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

    /// `wpa_supplicant`'s own `name = value` `drivers:` block must never
    /// become commands: its heading isn't recognized and nothing put the
    /// parser in `command_mode`, so the call site's gate refuses to try.
    /// See docs/shapes.md S-017.
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

    /// `fail2ban-client`'s wrapped rows: once `command_mode` is stuck
    /// on, the "not actually a heading, rewind" path re-treats a wrapped
    /// continuation as a fresh pseudo-heading, and it passes every other
    /// guard by shape alone. Gating on `recognized` alone, never
    /// inherited through `command_mode`, closes this. See
    /// docs/shapes.md S-017 and corpus/fail2ban-client.
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

    /// `trash-put`'s "use one of these commands:" sentence is an
    /// ordinary sentence, not a heading, and the worked example beneath
    /// it invokes a different program (`trash`) twice — the
    /// distinct-names floor refuses to treat that repetition as
    /// evidence. See docs/shapes.md S-017 and corpus/trash-put.
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

    /// pngfix's `--strip=[none|crc|...]:` row carries no description on
    /// its own line, so the value-list stays that flag's own
    /// description. See docs/shapes.md S-051 and
    /// corpus/pngfix/1.6.43/help.stderr.txt.
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

    /// pod2man's `--guesswork=rule[,rule...]` likewise carries nothing on
    /// its own line, so its whole value-list description must stay
    /// attached to that one flag. See docs/shapes.md S-051 and
    /// corpus/pod2man/5.01/help.txt.
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

    /// An ordinary prose paragraph repeating the tool's name is not
    /// promoted to subcommands: the words after the name aren't
    /// name-shaped, so the leading-run test refuses every row.
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

    /// A name must not "occur" merely as a substring of a longer token
    /// — what a naive `raw.contains(name)` would get wrong.
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
