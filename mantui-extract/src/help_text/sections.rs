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

use super::grammar::{looks_like_flag_start, parse_flag_spec};
use mantui_core::{CommandNode, Flag, Positional, Provenance, Source, Text};

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
    /// Subcommand stubs recovered from bare-word blocks (not yet
    /// extracted themselves — `children_filled: false`).
    pub subcommands: Vec<CommandNode>,
    /// Fraction of recognized entry lines the grammar fully understood,
    /// in `[0.0, 1.0]`.
    pub confidence: f32,
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

/// Parse raw `--help` text (already selected as stdout-or-stderr by the
/// caller) into structured pieces.
pub fn parse(raw: &str) -> ParsedHelp {
    let lines: Vec<&str> = raw.lines().collect();
    let mut result = ParsedHelp::default();

    let mut i = 0;
    // 1. Usage block: one or more lines starting with (case-insensitive)
    // "usage:", plus indented continuations.
    if let Some(start) = lines.iter().position(|l| {
        let t = l.trim_start();
        t.len() >= 6 && t[..6].eq_ignore_ascii_case("usage:")
    }) {
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
            usage_lines.push(l.trim().to_string());
            i += 1;
        }
        result.positionals = extract_positionals(&usage_lines);
        result.usage = usage_lines;
    }

    // 2. Leading prose before the usage block (or before the first
    // section, if there's no usage block) becomes the description.
    let mut description_lines: Vec<&str> = Vec::new();
    let mut j = 0;
    while j < lines.len() && j < i.max(leading_prose_bound(&lines)) {
        let l = lines[j];
        if leading_whitespace(l) == 0 && !l.trim().is_empty() {
            let t = l.trim_start();
            let is_usage = t.len() >= 6 && t[..6].eq_ignore_ascii_case("usage:");
            if !is_usage {
                description_lines.push(l);
            }
        }
        j += 1;
    }
    if !description_lines.is_empty() {
        result.description = Some(description_lines.join(" "));
    }

    // 3. Section blocks: scan the rest of the output for a heading line
    // followed by more-indented content. "Heading" is a relative notion,
    // not "column 0": tar indents its own headings by one space
    // (` Main operation mode:`) while its entries sit at two, so a block
    // is recognized whenever some line is followed (after any blank
    // lines) by content indented *more than that line*, and it runs until
    // indentation drops back to less than the *entries'* own indent —
    // not the heading's indent, which can differ between sections
    // (tar's `Examples:` is at column 0, its next heading at column 1).
    let mut total_entries = 0usize;
    let mut clean_entries = 0usize;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
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
                    let (seen, clean) =
                        process_word_grid(&heading, &lines[grid_start..i], &mut result);
                    total_entries += seen;
                    clean_entries += clean;
                }
                continue;
            }
            // Not actually a heading; rewind to just past the original
            // line and continue scanning it as its own candidate.
            i = heading_idx + 1;
            continue;
        }
        let block_start = i;
        let entry_indent = leading_whitespace(lines[i]);
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
        let block_lines = &lines[block_start..i];
        if is_ignorable_heading(&heading) {
            continue;
        }
        let (entries_seen, entries_clean) = process_block(&heading, block_lines, &mut result);
        total_entries += entries_seen;
        clean_entries += entries_clean;
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

/// True if `line` looks like a row of a bare-name grid (openssl-style
/// `--help` output: `asn1parse   ca   ciphers   cmp`) rather than prose or
/// a flag spec — every whitespace-separated token is name-shaped (starts
/// with a letter, otherwise only alphanumerics/`-`/`_`), there are at
/// least two of them, and none starts with `-` (which would make it a
/// flag entry instead).
fn looks_like_word_grid_line(line: &str) -> bool {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.is_empty() {
        return false;
    }
    tokens.iter().all(|t| is_name_shaped_token(t))
}

/// Stricter version used only to *start* a grid: requires 3+ columns, so
/// a two-word heading immediately above the grid (`"Standard commands"`)
/// is never itself mistaken for the first grid row. Once a grid has
/// started, [`looks_like_word_grid_line`] (which allows a trailing
/// single-token row, e.g. openssl's lone `x509` closing out a section) is
/// used to keep consuming it.
fn looks_like_word_grid_start(line: &str) -> bool {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    tokens.len() >= 3 && tokens.iter().all(|t| is_name_shaped_token(t))
}

fn is_name_shaped_token(t: &str) -> bool {
    let mut chars = t.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Turn a word-grid block into subcommand stubs, one per token, with no
/// per-entry description (the grid carries none) and `group` set to the
/// heading. Returns `(entries_seen, entries_cleanly_parsed)` — every token
/// here is "cleanly parsed" by construction (there's no grammar to fail).
fn process_word_grid(heading: &str, grid_lines: &[&str], out: &mut ParsedHelp) -> (usize, usize) {
    let mut count = 0;
    for line in grid_lines {
        for token in line.split_whitespace() {
            out.subcommands.push(CommandNode {
                group: Some(heading.to_string()),
                ..CommandNode::new(token, Provenance::single(Source::HelpText))
            });
            count += 1;
        }
    }
    (count, count)
}

/// Process one layout block: classify as flags or subcommands by whether
/// the first entry line looks like a flag, split entries at the block's
/// description column, and append results into `out`. Returns
/// `(entries_seen, entries_cleanly_parsed)`.
fn process_block(heading: &str, block_lines: &[&str], out: &mut ParsedHelp) -> (usize, usize) {
    let entries = split_entries(block_lines);
    if entries.is_empty() {
        return (0, 0);
    }
    let is_flags = looks_like_flag_start(entries[0].0);

    let mut seen = 0usize;
    let mut clean = 0usize;
    for (spec_text, desc_text) in entries {
        seen += 1;
        if is_flags {
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
                group: Some(heading.to_string()),
                description: non_empty_text(&desc_text),
                default: None,
                env_var: None,
                provenance: Provenance::single(Source::HelpText),
            });
        } else {
            let name = spec_text.trim();
            if name.is_empty() {
                continue;
            }
            clean += 1; // bare-word entries have no grammar to fail
            let mut node = CommandNode::new(name, Provenance::single(Source::HelpText));
            node.summary = non_empty_text(&desc_text);
            node.group = Some(heading.to_string());
            node.children_filled = false;
            out.subcommands.push(node);
        }
    }
    (seen, clean)
}

fn non_empty_text(s: &str) -> Option<Text> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(Text::sanitize(t))
    }
}

/// Split a block's raw lines into `(spec_fragment, description_fragment)`
/// pairs, one per entry, folding continuation lines into the preceding
/// entry's description.
///
/// Entries are distinguished from continuation lines by indentation: the
/// block's baseline indent is the minimum indentation among its non-blank
/// lines, and a line at or near that baseline starts a new entry, while a
/// line indented well past it continues the previous entry's description
/// (typical of a multi-line description wrapped under the description
/// column).
///
/// The description column is detected **per entry line**, not shared
/// across the whole block: real `--help` output is column-aligned only
/// approximately, and one over-long spec (tar's
/// `-A, --catenate, --concatenate`, `git`'s `restore` next to `mv`) breaks
/// strict alignment for its neighbors. A single shared column picked from
/// the shortest entry truncates longer flag names; a column picked from
/// the longest entry cuts into shorter entries' descriptions. Each line
/// knows its own gap correctly, so using it directly avoids both failure
/// modes; a block-wide column is unnecessary once continuation lines are
/// handled separately (they carry no gap of their own and are simply
/// trimmed and appended).
fn split_entries<'a>(block_lines: &[&'a str]) -> Vec<(&'a str, String)> {
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
            let (spec, desc) = split_at_column(line, find_description_gap(line));
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

    #[test]
    fn empty_input_yields_low_confidence_and_no_panic() {
        let parsed = parse("");
        assert!(parsed.confidence < 0.5);
        assert!(parsed.flags.is_empty());
    }
}
