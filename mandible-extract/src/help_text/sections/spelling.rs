//! Aligned spelling columns: a row whose second column is another spelling
//! of the same option rather than the start of its description.

use super::*;

/// A row's leading run of cells that are **nothing but option spellings**,
/// recovered by [`spelling_run`].
pub(super) struct SpellingRun {
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
pub(super) fn is_spelling_only_cell(cell: &str) -> bool {
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
pub(super) fn value_token(cell: &str) -> Option<String> {
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
pub(super) fn bare_spelling(cell: &str) -> String {
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
pub(super) fn value_suffix(cell: &str) -> Option<String> {
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
pub(super) fn cells_name_the_same_value(a: &str, b: &str) -> bool {
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
/// trailing text, and even if they were not, `mandible_core::Entity` has one
/// `short: Option<char>` and no field to hold the second.
///
/// The caller applies this only to a block that shows the column actually
/// recurring — see [`block_has_aligned_spelling_column`].
pub(super) fn spelling_run(line: &str) -> Option<SpellingRun> {
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
pub(super) fn block_has_aligned_spelling_column(entry_lines: &[&str]) -> bool {
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
pub(super) fn split_aligned_spelling_entry(line: &str) -> (String, String) {
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
pub(super) fn split_single_column_entry(line: &str) -> (String, String) {
    // `find_dash_token_separator_gap` is deliberately *not* part of
    // `find_description_gap`'s own chain (see that function's own doc
    // comment): its `strip_dash_token_separator` must run only when it is
    // the one that supplied the gap, never when `find_multi_space_gap`
    // already found a real column and the description on the far side of
    // it simply begins with a literal `-` as the tool's own text (`ar`'s
    // own `@<file>      - read options from <file>`, where that dash must
    // survive verbatim). So it is tried here, out of band, only once
    // `find_description_gap` itself has already found nothing at all.
    let (gap, from_dash_token) = match find_description_gap(line) {
        Some(col) => (Some(col), false),
        None => (find_dash_token_separator_gap(line), true),
    };
    let (spec, desc) = split_at_column(line, gap);
    // `find_equals_separator_gap`/`find_multi_space_gap` may have cut at or
    // before a lone `=` separator token, leaving it attached to the front
    // of `desc` (`= be verbose`, `= a local filename`) — see
    // `strip_equals_separator`.
    let desc = strip_equals_separator(&desc).to_string();
    // `find_colon_separator_gap` leaves its own separator attached the
    // same way — see `strip_colon_separator`.
    let desc = strip_colon_separator(&desc).to_string();
    // `find_dash_token_separator_gap` leaves its own separator attached
    // the same way — see `strip_dash_token_separator`. Scoped to exactly
    // the row that fallback itself matched, per the comment above.
    let desc = if from_dash_token {
        strip_dash_token_separator(&desc).to_string()
    } else {
        desc
    };
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

#[cfg(test)]
mod tests {
    use super::*;

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
                flag.short().or(flag.long().and_then(|l| l.chars().next()))
            );
        }
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
        assert_eq!(a.short(), Some('A'));
        assert_eq!(
            a.description.as_ref().map(|t| t.as_str()),
            Some("Enable smart home key"),
            "the description must be the third column only — before this \
             rule it read `--smarthome Enable smart home key`"
        );
        let c = flag_named(&parsed, "backupdir");
        assert_eq!(c.short(), Some('C'));
        assert_eq!(c.value_name.as_deref(), Some("<dir>"));
        assert_eq!(c.value_kind, ValueKind::Required);
        assert_eq!(
            c.description.as_ref().map(|t| t.as_str()),
            Some("Directory for saving unique backup files")
        );
        // Nothing invented from the table's own header row.
        assert!(
            !parsed.flags.iter().any(|f| f.long() == Some("option")),
            "the `Option  Long option  Meaning` header is not a flag"
        );
    }

    #[test]
    fn jdeprscans_two_column_table_recovers_the_long_form_it_used_to_drop() {
        let parsed = parse(JDEPRSCAN_TABLE);
        for (long, short) in [("list", 'l'), ("verbose", 'v')] {
            let flag = flag_named(&parsed, long);
            assert_eq!(flag.short(), Some(short));
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
        assert_eq!(help.short(), Some('?'));
        assert!(
            !parsed.flags.iter().any(|f| f.short() == Some('h')),
            "`-h` is still dropped — see corpus/jdeprscan/audit-seed2"
        );
    }

    #[test]
    fn awks_tab_aligned_spelling_columns_are_read_as_spellings() {
        let parsed = parse(AWK_TABLE);
        assert_eq!(
            flag_named(&parsed, "characters-as-bytes").short(),
            Some('b')
        );
        assert_eq!(flag_named(&parsed, "traditional").short(), Some('c'));
        assert_eq!(flag_named(&parsed, "copyright").short(), Some('C'));
        let d = flag_named(&parsed, "dump-variables");
        assert_eq!(d.short(), Some('d'));
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
            assert_eq!(flag.short(), Some(short));
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
        assert_eq!(d.short(), Some('d'));
        assert_eq!(d.value_name.as_deref(), Some("file"));
        assert_eq!(d.value_kind, ValueKind::Optional);
        let e = flag_named(&parsed, "source");
        assert_eq!(e.short(), Some('e'));
        assert_eq!(
            e.value_name.as_deref(),
            Some("'program-text'"),
            "a quoted value survives whole — rejoining both cells verbatim \
             used to leave `'program-text',` and lose `--source` entirely"
        );
        assert_eq!(e.value_kind, ValueKind::Required);
        let exec = flag_named(&parsed, "exec");
        assert_eq!(exec.short(), Some('E'));
        assert_eq!(exec.value_name.as_deref(), Some("file"));
        assert_eq!(exec.value_kind, ValueKind::Required);
        let lint = flag_named(&parsed, "lint");
        assert_eq!(lint.short(), Some('L'));
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
        assert_eq!(prompt.short(), Some('P'));
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
        assert_eq!(pattern.short(), Some('p'));
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
        assert_eq!(count.short(), Some('c'));
        assert_eq!(count.value_name.as_deref(), Some("num"));
        assert_eq!(count.value_kind, ValueKind::Required);
        assert_eq!(
            count.description.as_ref().map(|t| t.as_str()),
            Some("Number of times to write(default = 1)")
        );
        let bytes = flag_named(&parsed, "bytes");
        assert_eq!(bytes.short(), Some('b'));
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
                    && f.long() == Some("append")
                    && f.short() == Some('A')),
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
            strip.short(),
            None,
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
            .find(|f| f.short() == Some('x'))
            .expect("-x survives");
        assert_eq!(
            x.description.as_ref().map(|t| t.as_str()),
            Some("--foo is a synonym for --bar"),
            "a description beginning with a spelling is still a description"
        );
        assert!(
            !parsed.flags.iter().any(|f| f.long() == Some("foo")),
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
            !parsed.flags.iter().any(|f| f.short() == Some('1')),
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
                flag.short(),
                None,
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
            !parsed.flags.iter().any(|f| f.long() == Some("beta")),
            "one row is not evidence of a column"
        );
    }
}
