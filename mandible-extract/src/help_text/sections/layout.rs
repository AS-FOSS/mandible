//! Line and column primitives shared by every reader: indentation measured
//! in terminal columns, whitespace-separated cells and fields, and the
//! recurrence thresholds that decide a block is genuinely multi-column.

/// The column a tab in leading whitespace advances to, from whatever
/// column it started at — the ordinary terminal tab-stop convention.
pub(super) const TAB_STOP: usize = 8;

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
pub(super) const MIN_SPELLING_COLUMN_RECURRENCE: usize = 2;

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
pub(super) fn is_flag_char(c: char) -> bool {
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
pub(super) struct Field {
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
    pub(super) tokens: Vec<String>,
    /// Accumulated non-flag-shaped text following this field's token(s).
    /// Empty (or a bare value placeholder) means "not yet described" —
    /// see [`Field::is_bare`].
    pub(super) trailing: String,
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

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;

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
            .filter(|f| f.short() == Some('f'))
            .collect();
        assert_eq!(f_flags.len(), 1, "{:#?}", parsed.flags);
        assert_eq!(f_flags[0].long(), Some("follow"));
        assert_eq!(f_flags[0].value_name, None);

        // `-o, --output`'s description is now whole, not truncated at the
        // point the continuation line used to be misread as a new entry.
        let output = flag_named(&parsed, "output");
        assert_eq!(
            output.description.as_ref().map(|d| d.to_string()).as_deref(),
            Some("Write output to FILENAME (or FILENAME. in case -f is also used) instead of standard error")
        );
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
            .find(|f| f.long() == Some("all"))
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
                    .filter(|f| f.long() == Some(long))
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
            !parsed.flags.iter().any(|f| f.long().is_none()),
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
