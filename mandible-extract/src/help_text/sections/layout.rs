//! Line and column primitives shared by every reader: indentation measured
//! in terminal columns, whitespace-separated cells and fields, and the
//! recurrence thresholds that decide a block is genuinely multi-column.

/// The column a tab in leading whitespace advances to, from whatever
/// column it started at — the ordinary terminal tab-stop convention.
pub(super) const TAB_STOP: usize = 8;

/// A line's leading indentation, as a **visual column**, not a raw
/// character count. Agrees with a raw count for pure-space indentation,
/// but a tab expands to the next multiple of [`TAB_STOP`] rather than
/// counting as one column — LVM's emitters indent flag rows with one
/// tab under a two-space heading, and a raw count would read that as
/// *less* indented than the heading, hiding the whole flags table. See
/// docs/shapes.md S-005.
pub(super) fn leading_whitespace(line: &str) -> usize {
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

/// Split `line` on runs of two or more spaces, discarding empty fields.
/// Fields keep any internal single spaces, so a prose fragment comes back
/// as one field containing whitespace — which `is_name_shaped_token`
/// then rejects.
pub(super) fn split_columns(line: &str) -> Vec<&str> {
    line.trim()
        .split("  ")
        .map(|f| f.trim())
        .filter(|f| !f.is_empty())
        .collect()
}

/// Minimum number of distinct entry lines a secondary column offset must
/// recur at before a block is trusted as genuinely multi-column. Same
/// figure as `xtask::misattribution::MIN_COLUMN_RECURRENCE`: real column
/// bleed (`lsof`) recurs 9 times; the worst accidental coincidence
/// measured (`tar`'s `-T` cross-reference) recurs twice. `3` sits
/// strictly between.
pub const MIN_COLUMN_RECURRENCE: usize = 3;

/// Minimum number of entry rows whose *second* spelling cell begins at
/// the same character offset before [`scan_flags_block`] reads that
/// cell as an aligned column of alternate spellings rather than the
/// row's description.
///
/// Two, lower than [`MIN_COLUMN_RECURRENCE`]'s three, since the shape
/// test alone ([`is_spelling_only_cell`]) already excludes prose here —
/// recurrence only rules out coincidental alignment. Three would exclude
/// the shape's own reference case, `jdeprscan --help`'s exactly two
/// rows. The one measured false positive (`lto-dump`'s default-value
/// column) is excluded by alignment, not count, so lowering the count
/// doesn't readmit it.
pub(super) const MIN_SPELLING_COLUMN_RECURRENCE: usize = 2;

/// True if `token` is shaped like a flag spelling: `-x`, `--word`, `+x`,
/// or `+|-x` — lsof spells some flags with a `+` prefix (`+d`, `+m`).
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

/// Character class allowed immediately after a short flag's leading
/// `-`/`+`: alphanumerics plus a small punctuation set (`lsof -?`).
pub(super) fn is_flag_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '?' | '#' | '@')
}

/// First whitespace-delimited word of `s`, or `""` for an all-whitespace
/// string.
pub fn first_word(s: &str) -> &str {
    s.split_whitespace().next().unwrap_or("")
}

/// Split `line` into cells at a column gap — a run of two or more
/// spaces, or any tab — character-indexed, never byte-indexed (AGENTS.md's
/// rule against slicing tool output at a raw byte offset). Returns
/// `(char offset, cell text)` pairs. A single tab is a boundary on its
/// own: `debconf --help`'s table is tab-separated, and requiring 2+
/// spaces alone would glue the alias and description into one cell.
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

/// True if `s` is nothing but a single value-placeholder token —
/// bracket-wrapped (`<dir>`), fully upper-case (`NUM`), or an upper-case
/// name with bracketed decoration (`BLOCKSIZE[bskK...]`). Deliberately
/// narrow: a lower-case placeholder (`arptables`'s `-A chain`) isn't
/// recognized here since prose isn't reliably distinguishable from one
/// cell alone — [`fields_in_line`]'s fold-while-bare rule protects that
/// case instead.
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
pub(super) struct Field {
    /// Character offset of the field's first flag-shaped cell — the
    /// position [`block_is_multi_column`] buckets recurrence counts by.
    /// Never updated once created, even as later cells fold in.
    offset: usize,
    /// Every flag-shaped spelling folded into this field — usually one,
    /// more when a row spells short and long forms as adjacent cells
    /// sharing one description (`nano`'s `-A --smarthome`).
    pub(super) tokens: Vec<String>,
    /// Accumulated non-flag-shaped text following this field's
    /// token(s). Empty (or a bare placeholder) means "not yet
    /// described" — see [`Field::is_bare`].
    pub(super) trailing: String,
}

impl Field {
    /// True when this field carries no real descriptive text yet. The
    /// discriminator [`fields_in_line`] uses to decide whether the next
    /// flag-shaped cell opens a new column or is just another spelling
    /// of the option still open.
    fn is_bare(&self) -> bool {
        let trailing = self.trailing.trim();
        trailing.is_empty() || is_value_placeholder_only(trailing)
    }
}

/// Group `line`'s cells (see [`cells`]) into [`Field`]s: one per logical
/// column entry, not one per raw cell.
///
/// Fold-while-bare rule, stricter than `misattribution::fields_in_line`:
/// while the open field is still bare, any further flag-shaped cell
/// folds into it as another spelling of the same option, regardless of
/// whether that cell's own trailing text looks real. This is what a
/// genuine alias pair looks like (`nano`'s `-A  --smarthome`), and it's
/// also what protects `arptables`'s `--append  -A chain` — `-A chain`
/// has real-looking trailing text, but `--append` is still bare when
/// `-A` arrives, so `chain` folds in as shared trailing text rather
/// than proving a second flag. See docs/shapes.md S-036.
pub(super) fn fields_in_line(line: &str) -> Vec<Field> {
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

/// True if `entry_lines` (a flags block's raw entry rows, never
/// continuation lines) shows real column alignment: a secondary field
/// recurring at the same character offset across at least
/// [`MIN_COLUMN_RECURRENCE`] rows. Mirrors
/// `misattribution::build_definition_index`'s recurrence check, scoped
/// to one block. Only secondary fields count (each row's own first
/// field is skipped), since a row's primary entry can legitimately
/// cross-reference another real flag in its own prose (`du`'s `-H`
/// mentioning `-D`) without that looking like a second column. See
/// docs/shapes.md S-036.
pub(super) fn block_is_multi_column(entry_lines: &[&str]) -> bool {
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

// Packed flag rows: GNU `find --help`'s "Tests"/"Actions" tables pack
// several `-flag [ARG]` entries onto one physical line with single
// spaces and no description column at all. Neither
// `block_is_multi_column` (needs each cell to carry its own
// description) nor the ordinary single-column path applies; reading the
// whole line as one flag's spec would misread `-size N[bcwkMG]`'s
// bracketed suffix as a placeholder-boundary gap and fabricate the next
// entries as that flag's description. See docs/shapes.md S-047.

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;

    // --- the tab-stop leading-indentation fix ---------------------------

    /// `sotruss --help`'s real specimen: a three-tab-indented
    /// continuation line whose trimmed text starts with a dash (`-f is
    /// also used`). Three raw tabs measure as indent 3 (a naive count),
    /// which falls inside `scan_flags_block`'s indent tolerance and
    /// fabricates a phantom `-f` entry; expanded to column 24 it's
    /// correctly read as a continuation. See docs/shapes.md S-005.
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
        // No phantom `-f` carrying value `is` — only real `-f, --follow`.
        let f_flags: Vec<_> = parsed
            .flags
            .iter()
            .filter(|f| f.short() == Some('f'))
            .collect();
        assert_eq!(f_flags.len(), 1, "{:#?}", parsed.flags);
        assert_eq!(f_flags[0].long(), Some("follow"));
        assert_eq!(f_flags[0].value_name, None);

        // `-o, --output`'s description is whole, not truncated.
        let output = flag_named(&parsed, "output");
        assert_eq!(
            output.description.as_ref().map(|d| d.to_string()).as_deref(),
            Some("Write output to FILENAME (or FILENAME. in case -f is also used) instead of standard error")
        );
    }

    // --- Multi-column option tables (corpus/lsof/4.95.0, corpus/unzip/6.00) ---

    /// `corpus/lsof/4.95.0`: lsof's options table packs three
    /// flag+description pairs onto one physical line. Every flag must
    /// be present and carry its own text, not a neighbour's. See
    /// docs/shapes.md S-036.
    #[test]
    fn lsof_three_column_options_table_is_split_per_flag() {
        let parsed = parse_named(LSOF_HELP, "lsof");
        let desc_of = |short: char| -> String {
            parsed
                .flags
                .iter()
                .find(|f| f.short() == Some(short))
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
        // No flag's description contains another flag's spelling.
        assert!(!desc_of('?').contains("-a"));
        assert!(!desc_of('?').contains("-b"));
    }

    /// A block with only one description column parses unaffected —
    /// `block_is_multi_column`'s gate requires real recurring alignment.
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
            .find(|f| f.long() == Some("all"))
            .unwrap();
        assert_eq!(all.description.as_ref().unwrap().as_str(), "do everything");
        assert_eq!(parsed.flags.len(), 3);
    }

    /// `nano`-shaped alias row: short and long spelling of the same
    /// option sharing one description. Folds into one field per line,
    /// so no phantom flag is fabricated out of the long spelling. See
    /// docs/shapes.md S-036.
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
                    .filter(|f| f.short() == Some(short))
                    .count(),
                1,
                "expected exactly one -{short}, got {:?}",
                parsed.flags
            );
        }
        assert!(
            !parsed.flags.iter().any(|f| f.short().is_none()),
            "a spellingless (fabricated) flag was emitted: {:?}",
            parsed.flags
        );
    }

    /// `iptables`/`patch`-shaped row: a short/long alias pair whose
    /// short spelling carries a lowercase value placeholder
    /// (`is_value_placeholder_only` doesn't recognize it). Must fold
    /// into one field, not fabricate a second flag. See docs/shapes.md
    /// S-036.
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
                    .filter(|f| f.long() == Some(long))
                    .count(),
                1,
                "expected exactly one --{long}, got {:?}",
                parsed.flags
            );
        }
        // No phantom `-A`/`-C`/`-D` split out as its own separate flag.
        assert!(
            !parsed.flags.iter().any(|f| f.long().is_none()),
            "a spellingless (fabricated) flag was emitted: {:?}",
            parsed.flags
        );
    }

    /// `awk`-shaped row: two columns of option spellings (POSIX short
    /// beside GNU long), never flag+description. `is_synonym_not_description`'s
    /// single-column check saves this shape, since the row's lowercase
    /// value placeholder keeps its primary field from reading as bare.
    /// See docs/shapes.md S-036.
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
                    .filter(|fl| fl.short() == Some(short))
                    .count(),
                1,
                "expected exactly one -{short}, got {:?}",
                parsed.flags
            );
        }
        for flag in &parsed.flags {
            let desc = flag.description.as_ref().map(|d| d.as_str()).unwrap_or("");
            assert!(!desc.starts_with('-'), "{:?} -> {desc:?}", flag.short());
        }
    }

    /// `corpus/unzip/6.00`: a genuine two-column table, real flag on
    /// both sides of every row. Spot-checks one pair from each of
    /// unzip's two tables. See docs/shapes.md S-036.
    #[test]
    fn unzip_two_column_options_table_is_split_per_flag() {
        let parsed = parse_named(UNZIP_HELP, "unzip");
        let desc_of = |short: char| -> String {
            parsed
                .flags
                .iter()
                .find(|f| f.short() == Some(short))
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
}
