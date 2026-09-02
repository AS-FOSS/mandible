//! Turning recovered rows into [`ParsedHelp`] entries — flags, subcommands,
//! a flag's enumerated choices, and declared positionals — under the caps
//! and the never-invent rules this module's parent documents.

use super::*;

pub(super) fn emit_flags(
    group: Option<String>,
    entries: Vec<FlagRowEntry>,
    out: &mut ParsedHelp,
) -> (usize, usize) {
    let mut seen = 0usize;
    let mut clean = 0usize;
    for (spec_text, desc_text, choice_names) in entries {
        if out.flags.len() >= MAX_RECOVERED_ENTRIES {
            break;
        }
        seen += 1;
        let spec = parse_flag_spec(&spec_text);
        if spec.fully_consumed {
            clean += 1;
        }
        if spec.spellings.is_empty() {
            // Nothing recognizable as a flag at all; skip rather than
            // emit a garbage entry.
            continue;
        }
        let mut flag = Entity::new(EntityKind::Flag, Provenance::single(Source::HelpText));
        flag.spellings = spec.spellings;
        flag.value_name = spec.value_name;
        flag.value_kind = spec.value_kind;
        flag.group = group.clone();
        flag.description = non_empty_text(&desc_text);
        // Sub-rows nested directly under this flag's own row (llvm-ar's
        // bare `=value` shape and ffmpeg/ffplay's described AVOption shape,
        // see `choices_sub_row_value`/`choice_description_sub_row`) share
        // this same `choices` field with clap's `[possible values: …]`.
        flag.choices = choice_names
            .into_iter()
            .map(|(name, desc)| Choice {
                name,
                description: desc.map(|d| Text::sanitize(&d)),
            })
            .collect();
        out.flags.push(flag);
    }
    (seen, clean)
}

/// Emit the argfile sigil flag [`super::flag_rows::argfile_row_value_name`]
/// recovered (spec §4.5), built directly through
/// [`mandible_core::Entity::argfile_sigil`] rather than
/// [`parse_flag_spec`] — that grammar requires a leading `-`, which an
/// `@`-led row never has. See docs/shapes.md S-021.
pub(super) fn emit_argfile_flag(group: Option<String>, entry: FlagRowEntry, out: &mut ParsedHelp) {
    if out.flags.len() >= MAX_RECOVERED_ENTRIES {
        return;
    }
    let (value_name, desc_text, _choices) = entry;
    let mut flag = Entity::argfile_sigil(value_name, Provenance::single(Source::HelpText));
    flag.group = group;
    flag.description = non_empty_text(&desc_text);
    out.flags.push(flag);
}

/// Emit a [`block_is_packed_flag_rows`]-shaped block's entries directly,
/// never through [`parse_flag_spec`]/[`emit_flags`]: that grammar reads
/// `-wholename` as short flag `-w` plus a required value `"holename"`,
/// which is the wrong reading here — the second element of each entry is
/// the flag's own operand, never a description. The operand text is kept
/// verbatim rather than decomposed further. See docs/shapes.md S-047.
pub(super) fn emit_packed_flags(
    group: Option<String>,
    entries: Vec<(String, String)>,
    out: &mut ParsedHelp,
) {
    // Scoped to this one block: GNU find's `-exec`/`-execdir` document two
    // invocation forms (`COMMAND ;` and `COMMAND {} +`) as two separate
    // packed entries sharing one spelling. One `Flag` per spelling; the
    // second form's operand text is appended to the first's, verbatim.
    // See docs/shapes.md S-047.
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
        let mut flag = Entity::flag_spelled(
            short,
            long,
            single_dash,
            false,
            Provenance::single(Source::HelpText),
        );
        flag.value_name = (!operand.is_empty()).then_some(operand);
        flag.value_kind = value_kind;
        flag.group = group.clone();
        out.flags.push(flag);
    }
}

/// Emit a modifier table's rows as [`mandible_core::EntityKind::Modifier`]
/// entities (spec §7 Tier B, "Modifier tables"). Every row
/// [`scan_modifier_table`] returns is already valid, so unlike
/// [`emit_flags`]/[`emit_subcommands`] there is no second shape check here:
/// `seen` and `clean` are always equal.
pub(super) fn emit_modifiers(
    group: Option<String>,
    rows: Vec<ModifierRow>,
    out: &mut ParsedHelp,
) -> (usize, usize) {
    let mut seen = 0usize;
    for row in rows {
        if out.modifiers.len() >= MAX_RECOVERED_ENTRIES {
            break;
        }
        seen += 1;
        let mut modifier = Entity::modifier(row.letter, Provenance::single(Source::HelpText));
        modifier.value_kind = if row.value_name.is_some() {
            ValueKind::Required
        } else {
            ValueKind::None
        };
        modifier.value_name = row.value_name;
        modifier.group = group.clone();
        modifier.description = non_empty_text(&row.description);
        out.modifiers.push(modifier);
    }
    (seen, seen)
}

/// Emit an environment section's rows as
/// [`mandible_core::EntityKind::EnvVar`] entities (spec §7 Tier B,
/// "Environment sections"). Like [`emit_modifiers`], every row is already
/// valid, so `seen` and `clean` are always equal.
pub(super) fn emit_env_vars(
    group: Option<String>,
    rows: Vec<EnvVarRow>,
    out: &mut ParsedHelp,
) -> (usize, usize) {
    let mut seen = 0usize;
    for row in rows {
        if out.env_vars.len() >= MAX_RECOVERED_ENTRIES {
            break;
        }
        seen += 1;
        let mut env_var = Entity::env_var_item(row.name, Provenance::single(Source::HelpText));
        env_var.group = group.clone();
        env_var.description = non_empty_text(&row.description);
        out.env_vars.push(env_var);
    }
    (seen, seen)
}

/// Emit a recognized bare-word block's entries as subcommand stubs (spec
/// §7 Tier B rules 1 and 3). Entries failing the name-shape test are
/// dropped, never fabricated. See docs/shapes.md S-013.
pub(super) fn emit_subcommands(
    heading: &str,
    entries: Vec<(&str, String)>,
    out: &mut ParsedHelp,
) -> (usize, usize) {
    let mut seen = 0usize;
    let mut clean = 0usize;
    for (spec_text, desc_text) in entries {
        // A trailing colon after the name (cobra's own template convention,
        // e.g. `gh --help`'s `"auth:        Authenticate..."`) is
        // punctuation, never part of the name; strip before the shape
        // check. Framework-general, not gated on a specific one.
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
        // Every call site is already gated on positive evidence of a real
        // command list (a recognized heading, a `command_mode` chain, or
        // argparse's `{choice,...}` pseudo-entry), so an entry recovered
        // here is never conjured from layout alone (spec issue #2).
        node.heading_attested = true;
        out.try_push_subcommand(node);
    }
    (seen, clean)
}

/// Emit a headed command table's rows — `wpa_cli`'s ` = `-separated
/// `commands:` block and `apt-ftparchive`'s operand-only `Commands:` table
/// (see [`scan_bare_command_table`] and `split_heading_inline_row`) — as
/// subcommand stubs with `invocation_attested: true, heading_attested:
/// false`, rather than through [`emit_subcommands`] (always
/// `heading_attested: true`).
///
/// # Why the weaker attestation bit
///
/// `heading_attested` is spec §6 rule 0's gate for whether a word is safe
/// to send as `<tool> <word> --help` probe argv. These tables belong to
/// daemon-control clients whose "commands" are runtime control verbs
/// (`wpa_cli terminate`) that can act on a live process rather than argv
/// subcommands, so probing them is unsafe even though each name is still
/// checked for existence against [`command_table_token_index`]. See
/// docs/shapes.md S-017.
// Named alias, not a plain tuple-of-tuple, because that spelling trips
// clippy's `type_complexity` lint at each of this shape's call sites.
pub(super) type CommandTableEntries<'a> = Vec<(&'a str, Option<String>)>;

pub(super) fn emit_headed_command_table(
    entries: CommandTableEntries<'_>,
    raw_tokens: &std::collections::HashSet<&str>,
    out: &mut ParsedHelp,
) -> (usize, usize) {
    let mut seen = 0usize;
    let mut clean = 0usize;
    for (name, desc) in entries {
        seen += 1;
        // `is_command_name_shaped` is true by construction here (produced
        // by `leading_command_name`), but spec [M-10] is to check
        // explicitly rather than trust construction.
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
/// [`is_command_name_shaped`]'s own character class — the existence index
/// [`emit_headed_command_table`] checks each recovered name against, built
/// once per block rather than rescanning per candidate the way
/// [`token_occurs_literally`] does.
pub(super) fn command_table_token_index(raw: &str) -> std::collections::HashSet<&str> {
    raw.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')))
        .filter(|w| !w.is_empty())
        .collect()
}

/// Strip trailing bracketed optional-modifier groups from a command entry's
/// name token: `m[ab]` names the command `m`, `r[ab][f][u]` names `r` —
/// binutils `ar`'s own operation-table convention. See docs/shapes.md
/// S-018.
///
/// Returns the input untouched unless the suffix is entirely well-formed
/// `[...]` groups, so a token merely containing a bracket keeps failing
/// [`is_command_name_shaped`] as before.
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
/// row's first whitespace token, never a run of further name-shaped
/// tokens: `apt-ftparchive`'s `sources srcpath [overridefile [pathprefix]]`
/// names one command, `sources`, with `srcpath` as its operand. See
/// docs/shapes.md S-018.
///
/// Strips a trailing `:` and any `[...]` optional-modifier suffix first,
/// same as [`emit_subcommands`].
pub(super) fn leading_command_name(field: &str) -> Option<&str> {
    let first = field.split_whitespace().next()?;
    let name = first.trim_end_matches(':');
    let name = strip_optional_modifier_suffix(name);
    is_command_name_shaped(name).then_some(name)
}

/// Split a heading line that carries its section table's first row on the
/// heading's own physical line (`apt-ftparchive`'s `Commands: packages
/// binarypath [overridefile [pathprefix]]`) into the heading label
/// (without its trailing colon) and the trailing row text. See
/// docs/shapes.md S-018.
///
/// Unlike [`split_shared_heading_row`] (a flag row, requiring a real
/// [`MIN_COLUMN_GAP_SPACES`]-wide gap), this table is single-spaced, so
/// this asks only for some non-empty text after the colon and leaves it to
/// the caller's own heading/name checks to decide it's a real row.
/// [`is_section_heading_line`] still keeps this off a colon buried in
/// ordinary prose. Returns `None` when no colon exists, the label doesn't
/// read as a heading, or nothing follows.
pub(super) fn split_heading_inline_row(line: &str) -> Option<(&str, &str)> {
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
/// plausible owning flag exists — an unattributable block is discarded
/// rather than guessed at. See docs/shapes.md S-014.
///
/// Per-value descriptions are kept, not dropped: `tar`'s own `FORMAT is one
/// of the following:` enum documents each value. A row with no separate
/// description still produces a bare [`Choice`] with `description: None`.
pub(super) fn emit_choices(
    heading: &str,
    entries: Vec<(&str, String)>,
    out: &mut ParsedHelp,
) -> (usize, usize) {
    let mut seen = 0usize;
    let mut clean = 0usize;
    let mut candidates: Vec<(String, Option<String>)> = Vec::new();
    for (spec_text, desc_text) in &entries {
        if candidates.len() >= MAX_RECOVERED_ENTRIES {
            break;
        }
        let desc = non_empty_string(desc_text);
        // Real listings sometimes alias several values on one line
        // (`"none, off       never make backups"`); each comma-separated
        // fragment is its own candidate choice, sharing the row's one
        // description.
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
            candidates.push((name.to_string(), desc.clone()));
        }
    }
    if candidates.is_empty() {
        return (seen, clean);
    }
    match find_owning_flag_index(heading, &out.flags) {
        Some(idx) => {
            // Proven ownership (a literal `--name` match or a value_name
            // word match) — full names and descriptions, both trustworthy.
            for (name, desc) in candidates {
                if out.flags[idx].choices.iter().any(|c| c.name == name) {
                    continue;
                }
                out.flags[idx].choices.push(Choice {
                    name,
                    description: desc.map(|d| Text::sanitize(&d)),
                });
            }
        }
        None if !out.flags.is_empty() => {
            // Unproven owner: base's byte-for-byte fallback still attaches
            // bare names to the last-emitted flag, but never a description
            // — an unproven owner gets no new information riding along
            // with it. See docs/shapes.md S-014.
            let idx = out.flags.len() - 1;
            out.saw_unattributable_content = true;
            for (name, _desc) in candidates {
                if out.flags[idx].choices.iter().any(|c| c.name == name) {
                    continue;
                }
                out.flags[idx].choices.push(Choice::bare(name));
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

/// `desc_text.trim()`, as `None` when empty — the shared "is there really a
/// description here" check [`emit_choices`] uses before sanitizing one.
fn non_empty_string(desc_text: &str) -> Option<String> {
    let trimmed = desc_text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The bare name a positional-block row or a usage-synopsis token carries,
/// with the notation stripped: `[interval]` -> `interval`, `<destination>`
/// -> `destination`. `None` for anything that is not a single
/// notation-wrapped word — a multi-word first column is prose, and prose
/// promoted to structure is [M-10].
///
/// The `<...>` rule matches [`extract_positionals`]'s own (nearest `>`, not
/// outermost), so a name found in a declared block and the same name found
/// in the synopsis normalize identically.
pub(super) fn operand_name(token: &str) -> Option<String> {
    let token = token.trim();
    if token.is_empty() || token.split_whitespace().count() != 1 {
        return None;
    }
    let cleaned = token.trim_matches(|c| c == '[' || c == ']' || c == '.');
    let name = match cleaned.strip_prefix('<') {
        Some(stripped) => stripped.get(..stripped.find('>')?)?.to_string(),
        None => cleaned.to_string(),
    };
    // Never a flag, and never something with no word content at all.
    if name.starts_with('-') || !name.chars().any(char::is_alphanumeric) {
        return None;
    }
    Some(name)
}

/// The `(required, variadic)` shape the usage synopsis states for the
/// operand called `name`, or `None` if the synopsis never mentions it.
///
/// The declaring block says which tokens are operands but not whether each
/// is optional or repeatable; the synopsis states exactly those two bits,
/// read with the identical expressions [`extract_positionals`] uses (`[x]`
/// optional, trailing `...` variadic).
pub(super) fn usage_operand_shape(usage_lines: &[String], name: &str) -> Option<(bool, bool)> {
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
/// block is a different kind of evidence from a synopsis guess). See
/// docs/shapes.md S-078.
///
/// Merges rather than appends: an operand written `<file>` in the synopsis
/// and also listed in the block is one positional that gains a
/// description, not two. Order follows the block. Returns the `(seen,
/// clean)` pair every `emit_*` returns, so a refused row lowers the node's
/// confidence instead of vanishing silently.
pub(super) fn emit_declared_positionals(
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
            // Counted above, dropped here, and flagged.
            out.saw_unattributable_content = true;
            continue;
        };
        clean += 1;
        let description = non_empty_text(&desc_text);
        if let Some(existing) = out
            .positionals
            .iter_mut()
            .find(|p| p.primary_name() == name)
        {
            // The synopsis found this one first and has no description to
            // offer; the block does. `required`/`variadic` stay the
            // synopsis's, since only it carries that notation.
            if existing.description.is_none() {
                existing.description = description;
            }
            continue;
        }
        let (required, variadic) = usage_operand_shape(usage_lines, &name)
            // Not in the synopsis at all: required unless the block's own
            // row says otherwise.
            .unwrap_or_else(|| {
                (
                    !spec_text.contains('['),
                    spec_text.trim_end().ends_with("..."),
                )
            });
        let mut positional = Entity::positional(name, Provenance::single(Source::HelpText));
        positional.required = required;
        positional.repeatable = variadic;
        positional.description = description;
        out.positionals.push(positional);
    }
    (seen, clean)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--format=FORMAT`'s enum values are documented under a pseudo-
    /// heading that never says "command" — spec §7 Tier B rule 4: these are
    /// the flag's `choices`, not subcommands. See docs/shapes.md S-014 and
    /// corpus/tar.
    #[test]
    fn tar_format_enum_values_become_flag_choices_not_subcommands() {
        let parsed = parse(TAR_HELP);
        let format = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("format"))
            .expect("--format flag recovered");
        let choice_strs: Vec<&str> = format.choices.iter().map(|c| c.name.as_str()).collect();
        for want in ["gnu", "oldgnu", "pax", "posix", "ustar", "v7"] {
            assert!(choice_strs.contains(&want), "{choice_strs:?}");
        }
        assert!(!parsed.subcommands.iter().any(|c| c.name == "gnu"));
        // The enum list ends, and the options table beneath it resumes: six
        // values are documented, anything past `v7` is a flag row.
        assert_eq!(
            choice_strs,
            ["gnu", "oldgnu", "pax", "posix", "ustar", "v7"],
            "the enum swallowed the flag rows beneath it"
        );
    }

    /// The three GNU tar flags the `FORMAT is one of the following:` enum
    /// used to eat: they sit at indent 6 while the enum's own values sit at
    /// indent 4, so the block never dedented past them. `--portability` is
    /// not asserted: it's a second long alias on `--old-archive`'s row, and
    /// `Flag` has one `long` slot.
    #[test]
    fn tar_options_table_resumes_after_the_format_enum() {
        let parsed = parse(TAR_HELP);
        for want in ["old-archive", "pax-option", "posix"] {
            let flag = parsed
                .flags
                .iter()
                .find(|f| f.long() == Some(want))
                .unwrap_or_else(|| panic!("--{want} consumed by the FORMAT enum"));
            assert!(
                !flag
                    .description
                    .as_ref()
                    .is_none_or(|d| d.as_str().is_empty()),
                "--{want} recovered without its description"
            );
        }
        // The row directly beneath the recovered ones must survive intact.
        assert!(
            parsed
                .flags
                .iter()
                .any(|f| f.long() == Some("label") && f.short() == Some('V')),
            "-V, --label lost"
        );
    }

    /// `--quoting-style`'s valid arguments are introduced by a heading that
    /// names the flag directly — the literal-name-match half of rule 4,
    /// distinct from the pure-adjacency case above.
    #[test]
    fn tar_quoting_style_values_become_flag_choices() {
        let parsed = parse(TAR_HELP);
        let quoting_style = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("quoting-style"))
            .expect("--quoting-style flag recovered");
        let choice_strs: Vec<&str> = quoting_style
            .choices
            .iter()
            .map(|c| c.name.as_str())
            .collect();
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

    /// Every one of git's group headings recovers its commands, seeded by
    /// the leading blurb — must hold across all five groups, not just the
    /// first. See docs/shapes.md S-013.
    #[test]
    fn git_all_command_groups_recovered() {
        let parsed = parse(GIT_HELP);
        let names: Vec<&str> = parsed.subcommands.iter().map(|c| c.name.as_str()).collect();
        for want in ["clone", "add", "bisect", "branch", "fetch"] {
            assert!(names.contains(&want), "{names:?}");
        }
    }

    /// The recall half: argparse declares its operands in a block, and the
    /// synopsis supplies only the notation. `uobjnew`'s real shape — `pid`
    /// required (bare in synopsis), `interval` optional (bracketed), both
    /// described. See docs/shapes.md S-078.
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
                    p.primary_name(),
                    p.required,
                    p.repeatable,
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
        // this is a declaration being read, never a bare-lowercase-word
        // rule.
        assert!(parse(raw).positionals.is_empty());
    }

    /// The declared block must never cost the subparser scan its first
    /// refusal: argparse writes subcommands under the same heading, and
    /// those stay subcommands, with no positional invented from the
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
                .map(|p| p.primary_name())
                .collect::<Vec<_>>()
        );
    }

    /// A declared block whose first column is prose recovers nothing and
    /// says so (`saw_unattributable_content`) — the [M-10] refusal.
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
                .map(|p| p.primary_name())
                .collect::<Vec<_>>()
        );
        assert!(parsed.saw_unattributable_content);
    }

    /// `--[no-]name`, GNU getopt_long's negatable-boolean convention. The
    /// fix must recover the base name, with `negatable` set and no `[`/`]`
    /// ever appearing in `long`. See docs/shapes.md S-077.
    #[test]
    fn negatable_boolean_flags_are_recovered_with_base_names() {
        let raw = "Usage: restore [<options>]\n\nOptions:\n  -S, --[no-]staged     restore the index\n  --[no-]ignore-unmerged\n                        ignore unmerged entries\n  -2, --ours            checkout our version for unmerged files\n";
        let parsed = parse(raw);

        let staged = parsed
            .flags
            .iter()
            .find(|f| f.short() == Some('S'))
            .expect("short-spelled negatable flag must not be dropped");
        assert_eq!(staged.long(), Some("staged"));
        assert!(staged.negatable());

        let ignore_unmerged = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("ignore-unmerged"))
            .expect("long-only negatable flag must not be dropped entirely");
        assert!(ignore_unmerged.short().is_none());
        assert!(ignore_unmerged.negatable());
        assert_eq!(
            ignore_unmerged.description.as_ref().map(|d| d.as_str()),
            Some("ignore unmerged entries"),
            "the description on the following line must still attach"
        );

        // Control case: no `[no-]`, must be unaffected.
        let ours = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("ours"))
            .expect("non-negatable flag must still parse");
        assert!(!ours.negatable());

        for f in &parsed.flags {
            if let Some(long) = f.long() {
                assert!(
                    !long.contains('[') && !long.contains(']'),
                    "long name must never contain brackets: {long:?}"
                );
            }
        }
    }

    /// `apt-ftparchive`'s real defect: `Commands:` carries its first row on
    /// its own physical line, and the remaining rows are pure `name
    /// operand...` with no description. See docs/shapes.md S-018.
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

    /// `--format`'s own value_name is `FORMAT`, and the enum heading
    /// contains that exact word — `find_owning_flag_index`'s second proof.
    /// The proven pin: full names and descriptions attach, byte-exact.
    #[test]
    fn tar_format_choices_carry_descriptions_via_the_value_name_proof() {
        let parsed = parse(TAR_HELP);
        let format = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("format"))
            .expect("--format flag recovered");
        let got: Vec<(&str, &str)> = format
            .choices
            .iter()
            .map(|c| {
                (
                    c.name.as_str(),
                    c.description.as_ref().map_or("", |d| d.as_str()),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                ("gnu", "GNU tar 1.13.x format"),
                ("oldgnu", "GNU format as per tar <= 1.12"),
                ("pax", "POSIX 1003.1-2001 (pax) format"),
                ("posix", "same as pax"),
                ("ustar", "POSIX 1003.1-1988 (ustar) format"),
                ("v7", "old V7 tar format"),
            ],
            "proven ownership must carry every description, verbatim: {got:?}"
        );
    }

    /// `automake --help`'s real shape: `"Warning categories include:"`
    /// documents `-W, --warnings=CATEGORY` several rows earlier, but the
    /// heading names no flag literally and "categories" is not an exact
    /// word match for `CATEGORY` — ownership is unproven, so `-f,
    /// --force-missing` must never receive a description for text that is
    /// not its own. Bare names still attach (base's byte-for-byte
    /// behavior), but never a description. See docs/shapes.md S-014.
    #[test]
    fn automake_style_unproven_block_never_attaches_a_description() {
        let raw = "Usage: widget [OPTION]... [FILE]...\n\nOperation modes:\n  \
                   -W, --warnings=CATEGORY  report the warnings falling in CATEGORY\n  \
                   -f, --force-missing    force update of standard files\n\n\
                   Warning categories include:\n  \
                   cross                  cross compilation issues\n  \
                   gnu                    GNU coding standards\n  \
                   obsolete               obsolete features or constructions\n";
        let parsed = parse(raw);
        let warnings = flag_named(&parsed, "warnings");
        assert!(
            warnings.choices.is_empty(),
            "the true owner must never receive a fuzzy/stem-matched attachment: {:?}",
            warnings.choices
        );
        let force_missing = flag_named(&parsed, "force-missing");
        let names: Vec<&str> = force_missing
            .choices
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["cross", "gnu", "obsolete"],
            "base's own byte-for-byte behavior: bare names still attach to the fallback flag"
        );
        assert!(
            force_missing
                .choices
                .iter()
                .all(|c| c.description.is_none()),
            "an unproven owner must never carry a description, right or wrong: {:?}",
            force_missing.choices
        );
    }

    /// `cp --help`'s real shape: the trailing `VERSION_CONTROL` enum
    /// documents `--backup`, but unrelated prose sits between the flags
    /// table and this block, so `--version` is not provably the owner
    /// either — same unproven-fallback pin as the automake case, different
    /// failure shape (distance rather than a competing named flag).
    #[test]
    fn cp_style_trailing_prose_block_never_attaches_a_description() {
        let raw = "Usage: widget [OPTION]... SOURCE DEST\n\nOptions:\n  \
                   --backup[=CONTROL]     make a backup of each existing destination file\n  \
                   --version              output version information and exit\n\n\
                   Some unrelated prose paragraph about attributes that has nothing to\n\
                   do with any flag above it, spanning a couple of sentences so it reads\n\
                   like real documentation rather than a table.\n\n\
                   The version control method may be selected via the VERSION_CONTROL\n\
                   environment variable.  Here are the values:\n\n  \
                   none, off       never make backups\n  \
                   numbered, t     make numbered backups\n";
        let parsed = parse(raw);
        let backup = flag_named(&parsed, "backup");
        assert!(
            backup.choices.is_empty(),
            "the true owner must never receive an unproven attachment: {:?}",
            backup.choices
        );
        let version = flag_named(&parsed, "version");
        let names: Vec<&str> = version.choices.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["none", "off", "numbered", "t"],
            "base's own byte-for-byte behavior: bare names still attach to the fallback flag"
        );
        assert!(
            version.choices.iter().all(|c| c.description.is_none()),
            "an unproven owner must never carry a description: {:?}",
            version.choices
        );
    }

    /// ffplay's own AVOption sub-table (`-flags`) never reaches
    /// `find_owning_flag_index`/`emit_choices` — recognized entirely inside
    /// `scan_flags_block`'s continuation handling. Proven by construction:
    /// strip every heading from the input and the descriptions still
    /// attach. See docs/shapes.md S-015.
    #[test]
    fn ffplay_choices_never_route_through_the_heading_block_matcher() {
        let raw = "-flags             <flags>      ED.VAS..... (default 0)\n\
                   \x20    unaligned                    .D.V....... allow decoders to produce unaligned output\n\
                   \x20    gray                         ED.V....... only decode/encode grayscale\n";
        let parsed = parse(raw);
        let flags = flag_named(&parsed, "flags");
        let got: Vec<(&str, &str)> = flags
            .choices
            .iter()
            .map(|c| {
                (
                    c.name.as_str(),
                    c.description.as_ref().map_or("", |d| d.as_str()),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                (
                    "unaligned",
                    ".D.V....... allow decoders to produce unaligned output"
                ),
                ("gray", "ED.V....... only decode/encode grayscale"),
            ],
            "no heading governs this input at all, so this can only have come from \
             scan_flags_block's own continuation handling: {got:?}"
        );
    }
}
