//! Post-hoc repairs applied to a recovered flag list: the repeated-character
//! shape (`-v`/`-vv`) and the single-dash long-option shape (`-help`,
//! `-number=N`), plus the glued-token index both consult for evidence.

use super::*;

/// Re-read every `-vv`-shaped flag in `flags` as the multi-character
/// single-dash option it is, instead of its first character carrying a
/// required value. `bpftrace`'s `-v`/`-vv` pair (two different
/// descriptions) otherwise lands as `-v` twice, each holding one copy
/// of its own letter. See docs/shapes.md S-035.
///
/// Rewritten when all hold: short spelling, no long name, `Required`
/// value; the value is the short character repeated
/// ([`value_repeats_short`]); another flag in the same node is the bare
/// boolean spelling of the same character
/// ([`documents_bare_boolean`]) — the whole safety argument, and why
/// this is a post-pass rather than a [`parse_flag_spec`] change, since
/// nothing about the token alone tells `-vv` apart from `lessecho`'s
/// genuine `-nn` flag; and the reconstructed token occurs glued and
/// delimited in the raw text ([`token_occurs_glued`]).
///
/// Deliberate false negative: a repeated-character flag whose bare form
/// the tool never writes on its own row (`strace`'s `[-DDD]`) stays
/// split, since the only evidence that would admit it is the shape
/// `lessecho`'s `-nn` also has.
pub(super) fn repair_repeated_character_flags(
    flags: &mut [Entity],
    glued_tokens: &GluedTokenIndex<'_>,
) {
    let booleans: Vec<char> = flags
        .iter()
        .filter(|f| f.value_kind == ValueKind::None)
        .filter_map(|f| f.short())
        .collect();
    for flag in flags.iter_mut() {
        let Some(short) = flag.short() else { continue };
        if flag.long().is_some() || flag.value_kind != ValueKind::Required {
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
        // Whole run becomes one single-dash long spelling; name held
        // bare, `Dashes::Single` puts the dash on at display time.
        flag.spellings = vec![Spelling::single_dash(&token[1..])];
        flag.value_name = None;
        flag.value_kind = ValueKind::None;
    }
}

/// True when `value` is one or more copies of `short` and nothing else
/// (`-vv` stores `"v"`, `strace`'s `[-DDD]` stores `"DD"`). The
/// emptiness guard matters: an empty `Required` value would otherwise
/// pass `chars().all(..)` vacuously. Case-sensitive: `-vV` is two flags
/// glued, not one repeated.
pub(super) fn value_repeats_short(short: char, value: &str) -> bool {
    !value.is_empty() && value.chars().all(|c| c == short)
}

/// What "word-shaped" means on either side of a glued token, for both
/// [`token_occurs_glued`] and [`GluedTokenIndex`] — one definition, so the
/// index and the scan cannot drift apart.
pub(super) fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '-' || c == '_'
}

/// True when `candidate` occurs in `raw` as an isolated token: nothing
/// word-shaped immediately before or after it. The twin of
/// `xtask::existence::spelling_occurs`: `value_name` alone can't tell
/// `-vv` from `-v v`, only the raw text can. Char-indexed, never a
/// byte-offset slice (AGENTS.md).
///
/// The definition, not the hot path: [`GluedTokenIndex`] answers the
/// same question in one pass and is what both repairs call; this form
/// stays as the readable statement and as the fallback for the one
/// candidate shape the index cannot key.
pub(super) fn token_occurs_glued(raw: &str, candidate: &str) -> bool {
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

/// One document's answer to every [`token_occurs_glued`] question the
/// two flag repairs will ask of it, built in one pass. Exists because
/// scanning the whole document per candidate (`O(candidates x
/// document)`) measured ~1.4s to 3.2s on `ffplay`'s 752 KB help text
/// once single-dash long-option work widened the candidate set.
///
/// Indexes every maximal run of word characters ([`is_word_char`]),
/// keyed by the run's text; a lookup hashes the candidate's leading
/// run. Works because a candidate always opens on a word character, so
/// a match's leading run is exactly a run in the index — for an
/// all-word candidate (`-help`, `-vv`) a maximal run *is* both boundary
/// conditions, so the hash lookup alone answers it. A candidate with a
/// trailing non-word character (`-foffload=<targets>`) needs the
/// remainder checked against text after each run occurrence, hence
/// offsets rather than a set.
pub(super) struct GluedTokenIndex<'a> {
    /// The document this was built from, for the fallback in
    /// [`GluedTokenIndex::contains`].
    raw: &'a str,
    /// Every maximal run of word characters in `raw`, keyed by the run's
    /// text and valued by the byte offset just past each occurrence of it.
    runs: std::collections::HashMap<&'a str, Vec<usize>>,
}

impl<'a> GluedTokenIndex<'a> {
    /// One pass over `raw`, cutting at every word/non-word boundary.
    /// Offsets come from `char_indices`, so every slice is at a
    /// character boundary by construction, taken through `get` (never
    /// panics) per AGENTS.md's rule against byte-offset slicing.
    pub(super) fn new(raw: &'a str) -> Self {
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
        // A non-word-opening candidate has no leading run to key on;
        // both callers ask about `-`-led tokens so this is unreached in
        // practice, but the fallback is cheaper than narrowing the type.
        if head == 0 {
            return token_occurs_glued(self.raw, candidate);
        }
        let (run, rest) = candidate.split_at(head);
        let Some(ends) = self.runs.get(run) else {
            return false;
        };
        if rest.is_empty() {
            // The key matched a maximal run, so both boundary conditions
            // already hold.
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

/// Fewest characters a swallowed tail must carry before it's read as the
/// rest of a single-dash long option's name. Two: at one character the
/// shape is genuinely ambiguous (`rpcgen`'s `-Ss`, `xxd`'s `-ps`), and
/// roughly half that population is a correct parse of a real
/// character-argument flag instead. Deliberate lost recall. Same figure
/// as `xtask::single_dash_long::MIN_SWALLOWED_CHARS`.
pub(super) const MIN_SWALLOWED_NAME_CHARS: usize = 2;

/// Re-read every `-help`-shaped option-table row as the single-dash long
/// option it is, instead of its own first character carrying a required
/// value. `qemu-arm64-static`'s `-help` otherwise becomes `-h` plus the
/// value `"elp"`, alongside genuinely correct spaced-value rows such as
/// `-g port` on adjacent lines. See docs/shapes.md S-035.
///
/// Rewritten when all seven hold: option-table-sourced
/// ([`Source::HelpText`], never `HelpTextSynopsis`); short spelling, no
/// long name, `Required` value; the swallowed text's name half (before
/// the first `=`, via [`split_glued_value`]) is option-name-shaped
/// ([`is_option_name_tail`]); that name half is at least
/// [`MIN_SWALLOWED_NAME_CHARS`] characters; the reconstructed name
/// token is uniformly lowercase ([`token_is_uniformly_lowercase`]) —
/// the whole safety argument, since the GCC/Clang glued-value
/// convention (`gcc -DMACRO`, `cc -oOUTFILE`) is otherwise
/// indistinguishable by shape, and is separated only by case (an
/// uppercase flag letter vs. a lowercase word), measured over the
/// *whole* token so `-oOUTFILE`'s lowercase flag letter doesn't slip
/// through; the tail is not the repeated-character family (condition 6,
/// handed off to [`repair_repeated_character_flags`]); and the
/// reconstructed token (name and glued value) occurs glued and
/// delimited in the raw text ([`token_occurs_glued`]).
///
/// A glued `=value` half (`dbiprof`'s `-number=N`) is split at the `=`
/// and, when present, kept on the resulting flag (`-foffload` stays
/// value-taking, named `<targets>`) rather than folded into the name
/// test — admitting `=` into [`is_option_name_tail`] would also admit
/// `-E var=value`. `_` counts as a name character (a word separator,
/// the same job `-` does): `ffplay`/`ffprobe` are 97% of the underscore
/// population, and every recovered name occurs as the leading token of
/// a row the tool itself writes.
///
/// A spaced value that was already swallowed before this repair runs
/// (`-cpu model`'s `model`) is not recovered — the flag becomes the
/// correct boolean `-cpu` rather than keeping a fabricated value, the
/// same trade [`repair_repeated_character_flags`] makes for `-vv`.
///
/// Deliberately unclaimed, same as the `xtask` oracle: uppercase-led
/// long options (`-Wall`), `ip`'s bracketed abbreviations
/// (`-h[uman-readable]`), tails carrying layout punctuation
/// (`sg_emc_trespass`'s `-hr:`), and one-character tails.
pub(super) fn repair_single_dash_long_options(
    flags: &mut [Entity],
    glued_tokens: &GluedTokenIndex<'_>,
) {
    for flag in flags.iter_mut() {
        // 1. Option-table-sourced, never synopsis.
        if !flag.provenance.sources.contains(&Source::HelpText)
            || flag.provenance.sources.contains(&Source::HelpTextSynopsis)
        {
            continue;
        }
        // 2. A bare short flag carrying a required value.
        let Some(short) = flag.short() else { continue };
        if flag.long().is_some() || flag.value_kind != ValueKind::Required {
            continue;
        }
        let Some(tail) = flag.value_name.as_deref() else {
            continue;
        };
        // 3a. Split at the first `=` ([`split_glued_value`]); without one
        //     the name half is the whole tail.
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
        // 7. Whole token occurs glued and delimited in the raw text. Last
        //    since it's the only condition reading the document.
        if !glued_tokens.contains(&format!("-{short}{tail}")) {
            continue;
        }
        // Run up to the `=` becomes one single-dash long spelling; name
        // held bare, `Dashes::Single` adds the dash at display time.
        flag.spellings = vec![Spelling::single_dash(&name_token[1..])];
        match glued_value {
            // The document wrote the value spec, so it survives.
            Some(value) => flag.value_name = Some(value.to_string()),
            // Dropped by the grammar before this ran; becomes the
            // correctly-named boolean rather than a fabricated value.
            None => {
                flag.value_name = None;
                flag.value_kind = ValueKind::None;
            }
        }
    }
}

/// Split a swallowed tail into the option-name half and the glued value
/// half: `"umber=N"` → `("umber", Some("N"))`, `"elp"` → `("elp", None)`.
/// `None` when the tail ends at the `=` with nothing after it (`"oo="`)
/// — no evidence for either reading. Splits at the *first* `=`, which is
/// what makes `dbiprof`'s `-match=K=V` come out right. Twin of
/// `xtask::single_dash_long::split_glued_value`.
pub(super) fn split_glued_value(tail: &str) -> Option<(&str, Option<&str>)> {
    match tail.split_once('=') {
        Some((_, "")) => None,
        Some((name, value)) => Some((name, Some(value))),
        None => Some((tail, None)),
    }
}

/// True when `tail` could be the rest of a single-dash long option's
/// name: ASCII alphanumerics, `-` and `_`, with at least one letter.
/// The letter requirement stops a glued numeric argument (`-b4096`)
/// from riding in on a technically-alphanumeric run. `_` is admitted on
/// the same footing as `-` (a word separator, not value-spec
/// punctuation). Twin of `xtask::single_dash_long::is_option_name_tail`.
/// See docs/shapes.md S-035.
pub(super) fn is_option_name_tail(tail: &str) -> bool {
    tail.chars().any(|c| c.is_ascii_alphabetic())
        && tail
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// True when `token` carries no ASCII uppercase letter at all — the
/// discriminator against the GCC/Clang glued-value convention
/// (`-DMACRO`, `-oOUTFILE`). Measured over the whole token, not just
/// the tail, since `-oOUTFILE`'s flag letter is lowercase.
pub(super) fn token_is_uniformly_lowercase(token: &str) -> bool {
    !token.chars().any(|c| c.is_ascii_uppercase())
}

/// Restore a value the single-dash long-option repair cleared, anchored
/// against a run-mate's already-correct value and the raw document's
/// own literal phrase — never invented, never merging spellings.
/// `ffplay --help` documents `-h`/`-?`/`-help`/`--help topic` as four
/// rows; [`repair_single_dash_long_options`] rewrites `-help` correctly
/// but clears its value (already dropped by the grammar by the time it
/// runs, a deliberate limitation left untouched). This function
/// restores `-help`'s value from its run-mates' evidence only. See
/// docs/shapes.md S-007.
///
/// Corrected only when: option-table-sourced, single spelling; an
/// adjacent run sharing description and `group` contains another entity
/// already carrying a real value (the anchor); and the raw document
/// literally contains `<this row's spelling> <that value>` as a
/// delimited phrase.
///
/// Deliberately does not merge run-mates into one multi-spelling
/// entity, even when their values match — an earlier version did, and
/// a fleet sweep found six real, unrelated pairs sharing boilerplate
/// commentary (`as`'s `-w`/`-X`, both "ignored"; `lto-dump`'s
/// `-C`/`-CC`, both "[disabled]") that description equality alone
/// cannot tell from a genuine alias. A repeat-count guard doesn't
/// discriminate either: the false positives recur exactly as often as
/// genuine aliases like `gold`'s `-R`/`-rpath`.
pub(super) fn recover_anchored_values(mut flags: Vec<Entity>, raw: &str) -> Vec<Entity> {
    fn eligible(f: &Entity) -> bool {
        f.spellings.len() == 1
            && f.description.is_some()
            && f.provenance.sources.contains(&Source::HelpText)
            && !f.provenance.sources.contains(&Source::HelpTextSynopsis)
    }

    // Chain adjacent, eligible rows that share a description and a table
    // (`group`) into runs — the same evidence boundary a genuine alias run
    // must respect, so a table boundary (ffplay's own `AVCodecContext
    // AVOptions:` block starting right after `Main options:` ends) can
    // never be crossed even if two unrelated rows happen to repeat the
    // same description.
    let mut run_start = 0;
    while run_start < flags.len() {
        let mut run_end = run_start + 1;
        while run_end < flags.len()
            && eligible(&flags[run_end])
            && eligible(&flags[run_end - 1])
            && flags[run_end].description == flags[run_start].description
            && flags[run_end].group == flags[run_start].group
        {
            run_end += 1;
        }
        if run_end - run_start >= 2 {
            recover_run(&mut flags[run_start..run_end], raw);
        }
        run_start = run_end;
    }
    flags
}

/// One [`recover_anchored_values`] run: same description, same table, each
/// row one spelling. Finds the value this run's own well-parsed rows
/// already agree the shared description takes (if any), then restores it
/// — in place, never reordering or merging — onto any run-mate whose own
/// row literally documents that exact value glued to its own spelling.
fn recover_run(run: &mut [Entity], raw: &str) {
    let anchor = run
        .iter()
        .find(|f| f.value_kind != ValueKind::None && f.value_name.is_some())
        .map(|f| {
            (
                f.value_name.clone().expect("checked Some above"),
                f.value_kind,
            )
        });
    let Some((value, kind)) = anchor else {
        return;
    };
    for f in run.iter_mut() {
        if f.value_kind == ValueKind::None
            && f.value_name.is_none()
            && f.single_dash()
            && f.spellings.len() == 1
        {
            let phrase = format!("{} {value}", f.spellings[0].typed());
            if token_occurs_glued(raw, &phrase) {
                f.value_name = Some(value.clone());
                f.value_kind = kind;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- the repeated-character flag repair -----------------------------

    /// `bpftrace`'s real troubleshooting block, byte-exact. See
    /// docs/shapes.md S-035 and
    /// corpus/killsnoop.bt/audit-seed2/help.stderr.txt.
    const BPFTRACE_TROUBLESHOOTING: &str = concat!(
        "TROUBLESHOOTING OPTIONS:\n",
        "    -v                      verbose messages\n",
        "    -vv                     more verbose messages (max 2)\n",
        "    -d                      (dry run) debug info\n",
        "    -dd                     (dry run) verbose debug info\n",
    );

    #[test]
    fn bpftraces_repeated_character_flags_become_single_dash_long_options() {
        let parsed = parse(BPFTRACE_TROUBLESHOOTING);
        for (name, description) in [
            ("vv", "more verbose messages (max 2)"),
            ("dd", "(dry run) verbose debug info"),
        ] {
            let flag = flag_named(&parsed, name);
            assert!(flag.single_dash(), "-{name} is spelled with one dash");
            assert_eq!(flag.spelling(), format!("-{name}"));
            assert_eq!(flag.short(), None);
            assert_eq!(flag.value_name, None);
            assert_eq!(flag.value_kind, ValueKind::None);
            assert_eq!(
                flag.description.as_ref().map(|t| t.as_str()),
                Some(description),
                "the row's own description must survive the repair"
            );
        }
        // The booleans the repair reads as evidence stay untouched.
        for short in ['v', 'd'] {
            let flag = parsed
                .flags
                .iter()
                .find(|f| f.short() == Some(short))
                .unwrap_or_else(|| panic!("-{short} must survive"));
            assert_eq!(flag.value_kind, ValueKind::None);
        }
    }

    /// `lessecho`'s `[-nn]` is character-for-character this shape but a
    /// correct parse of a real number-taking flag; survives only
    /// because `lessecho` never writes a bare `-n`. See docs/shapes.md
    /// S-035.
    #[test]
    fn lessechos_real_glued_character_arguments_are_left_alone() {
        let raw = "usage: lessecho [-ox] [-cx] [-pn] [-dn] [-mx] [-nn] [-ex] [-a] file ...\n";
        let parsed = parse(raw);
        assert!(
            parsed.flags.iter().all(|f| f.long().is_none()),
            "no lessecho flag may be rewritten: {:?}",
            parsed
                .flags
                .iter()
                .map(|f| f.spelling())
                .collect::<Vec<_>>()
        );
        // The identical token is repaired once the bare spelling is
        // declared a boolean, confirming that condition does the work.
        let parsed = parse("  -n         never overwrite\n  -nn        never ever overwrite\n");
        assert!(flag_named(&parsed, "nn").single_dash());
    }

    /// A spaced value is indistinguishable from a glued one once stored;
    /// the raw text is what decides.
    #[test]
    fn a_spaced_value_is_never_repaired() {
        let parsed = parse("  -v         verbose\n  -v v       take a v\n");
        assert!(
            parsed.flags.iter().all(|f| f.long().is_none()),
            "only a glued token may be repaired: {:?}",
            parsed
                .flags
                .iter()
                .map(|f| f.spelling())
                .collect::<Vec<_>>()
        );
    }

    /// The bundle and long-option families sharing the same fingerprint
    /// must come through untouched.
    #[test]
    fn the_bundle_and_long_option_families_are_not_repaired_as_repeats() {
        let parsed = parse("  -2         two\n  -2CDlNuVv  a cluster\n  -Z         z\n  -Zscript   an unstable flag\n");
        assert!(
            parsed.flags.iter().all(|f| f.long().is_none()),
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

    /// The index is an optimization only, so what's worth pinning is
    /// agreement: [`GluedTokenIndex::contains`] and [`token_occurs_glued`]
    /// must return the same answer for every case where they could
    /// plausibly disagree — glued neighbours, document-boundary tokens,
    /// a real but non-delimited substring, a repeated token, a
    /// non-word-character candidate, and multi-byte delimiters (where a
    /// byte-offset index would panic or miss).
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

    /// `qemu-arm64-static`'s real option table, byte-exact. See
    /// docs/shapes.md S-035 and corpus/qemu-arm64-static/audit-seed2/help.txt.
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
            assert!(flag.single_dash(), "-{name} is spelled with one dash");
            assert_eq!(flag.spelling(), format!("-{name}"));
            assert_eq!(flag.short(), None);
            assert_eq!(flag.value_name, None);
            assert_eq!(flag.value_kind, ValueKind::None);
        }
    }

    /// `-g port` stores a `value_name` exactly as `-help` stores
    /// `"elp"`; only the space in the raw text tells them apart.
    #[test]
    fn qemus_genuine_valued_short_flags_on_adjacent_rows_are_left_alone() {
        let parsed = parse(QEMU_TABLE);
        let g = parsed
            .flags
            .iter()
            .find(|f| f.short() == Some('g'))
            .expect("-g must survive as a short flag");
        assert_eq!(
            g.long(),
            None,
            "-g port is a correct parse, not a long option"
        );
        assert_eq!(g.value_name.as_deref(), Some("port"));
        assert_eq!(g.value_kind, ValueKind::Required);
        // `-h` and `-help` are two different flags; both must survive.
        assert!(parsed
            .flags
            .iter()
            .any(|f| f.short() == Some('h') && f.value_kind == ValueKind::None));
    }

    /// The GCC/Clang glued-value convention satisfies every condition
    /// but case, and must survive untouched. `-oOUTFILE` is what forces
    /// the case test to read the whole token, not just the tail.
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
                parsed.flags.iter().all(|f| f.long().is_none()),
                "a correct glued-value parse was destroyed by {row:?}: {:?}",
                parsed
                    .flags
                    .iter()
                    .map(|f| f.spelling())
                    .collect::<Vec<_>>()
            );
        }
    }

    /// `dbiprof`'s real option table, byte-exact. See docs/shapes.md
    /// S-035 and corpus/dbiprof/1.643/help.txt.
    const DBIPROF_TABLE: &str = concat!(
        "    -number=N        show top N, defaults to 10\n",
        "    -sort=S          sort by S, defaults to total\n",
        "    -reverse         reverse the sort\n",
        "    -match=K=V       for filtering, see docs\n",
        "    -exclude=K=V     for filtering, see docs\n",
        "    -case_sensitive  for -match and -exclude\n",
        "    -version         print version number and exit\n",
    );

    /// The defect the `=` split exists for: `-number=N` came out as
    /// `-n` + `"umber=N"` while the table's value-less rows came out
    /// right.
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
            assert!(flag.single_dash(), "-{name} is spelled with one dash");
            // `Flag::spelling` renders a required value with a space
            // (same convention as `--output=FILE` -> `--output FILE`).
            assert_eq!(flag.spelling(), format!("-{name} {value}"));
            assert_eq!(flag.short(), None);
            // Value spec survives the repair; `-match=K=V` splits at the
            // first `=` and keeps the rest verbatim.
            assert_eq!(flag.value_name.as_deref(), Some(value));
            assert_eq!(flag.value_kind, ValueKind::Required);
        }
        // The value-less rows in the same table are unchanged by the split.
        for name in ["reverse", "version"] {
            let flag = flag_named(&parsed, name);
            assert!(flag.single_dash());
            assert_eq!(flag.value_kind, ValueKind::None);
        }
    }

    /// `gcc`'s `-foffload=<targets>`, same family as `dbiprof`'s,
    /// exercising an uppercase value spec on the far side of the `=`.
    /// See corpus/gcc/13.3.0.
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
            assert!(flag.single_dash(), "-{name} is spelled with one dash");
            assert_eq!(flag.short(), None);
            assert_eq!(flag.value_name.as_deref(), Some(value));
        }
    }

    /// The glued-value convention shouts to the left of the `=`, so a
    /// genuine glued short with a `key=value` argument is still
    /// rejected. Ghostscript's `-sDEVICE=` (lowercase flag letter) is
    /// the hard case.
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
                parsed.flags.iter().all(|f| f.long().is_none()),
                "a correct glued-value parse was destroyed by {row:?}: {:?}",
                parsed
                    .flags
                    .iter()
                    .map(|f| f.spelling())
                    .collect::<Vec<_>>()
            );
        }
    }

    /// A spaced `key=value` argument stores byte-for-byte what
    /// `dbiprof`'s glued `-number=N` stores; only condition 7's raw-text
    /// scan tells them apart.
    #[test]
    fn a_spaced_key_value_argument_is_never_a_long_option() {
        for row in [
            "  -e var=value    set an environment variable\n",
            "  -o key=val      set a mount option\n",
            "  -v var=val      assign an awk variable\n",
        ] {
            let parsed = parse(row);
            assert!(
                parsed.flags.iter().all(|f| f.long().is_none()),
                "a spaced value was glued into a name by {row:?}: {:?}",
                parsed
                    .flags
                    .iter()
                    .map(|f| f.spelling())
                    .collect::<Vec<_>>()
            );
        }
    }

    /// `_` separates words inside a name exactly as `-` does:
    /// `-case_sensitive` used to come out as `-c` carrying
    /// `"ase_sensitive"`, a short flag `dbiprof` never documents.
    #[test]
    fn an_underscored_name_is_recovered_from_the_table_it_shares() {
        let parsed = parse(DBIPROF_TABLE);
        let flag = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("case_sensitive"))
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
        assert!(flag.single_dash(), "it is spelled with one dash");
        assert_eq!(flag.short(), None, "the fabricated -c is gone");
        assert_eq!(flag.value_kind, ValueKind::None);
        assert_eq!(
            flag.description.as_ref().map(|d| d.as_str()),
            Some("for -match and -exclude")
        );
        // Fabricated short must not survive under any other flag.
        assert!(
            !parsed
                .flags
                .iter()
                .any(|f| f.short() == Some('c') && f.long().is_none()),
            "the invented -c is not left behind"
        );
    }

    /// The ffmpeg `AVOption` table is 97% of the underscore population;
    /// its `<int>`/capability-column value spec already lives in the
    /// description, so the repair must move the name and leave the
    /// description untouched. Rows byte-for-byte from `ffplay --help`
    /// (6.1.1-3ubuntu5). See docs/shapes.md S-035.
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
            // Control: no underscore, recovered on the parser as it
            // stands.
            (
                "threads",
                "<int> ED.VA...... set the number of threads (from 0 to INT_MAX) (default 1)",
            ),
        ] {
            let flag = parsed
                .flags
                .iter()
                .find(|f| f.long() == Some(name))
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
            assert!(flag.single_dash());
            assert_eq!(
                flag.description.as_ref().map(|d| d.as_str()),
                Some(spec),
                "-{name} lost its value spec or capability column"
            );
        }
    }

    /// An underscore in the swallowed text is not on its own a licence
    /// to read a long option; each row here is a correct parse, refused
    /// by a different condition.
    #[test]
    fn an_underscore_alone_never_buys_the_long_reading() {
        for (row, refused) in [
            // Condition 5: glued-value convention shouts, and an
            // underscored macro name shouts with it.
            ("  -DFOO_BAR         define a macro\n", "DFOO_BAR"),
            ("  -DMAX_PATH=4096   define a macro\n", "DMAX_PATH"),
            // Condition 5 again, whole token: only the argument shouts.
            ("  -oOUT_FILE        write output here\n", "oOUT_FILE"),
            // Condition 7: a spaced value never occurs glued.
            ("  -o out_file       write output here\n", "out_file"),
            // Condition 3: name half still can't carry value-spec
            // punctuation.
            ("  -d item_a[,...]   a list\n", "item_a"),
            ("  -b some_path/name a path\n", "some_path/name"),
            // Condition 4: one character of name is still not a name.
            ("  -s_               a stray\n", "s_"),
        ] {
            let parsed = parse(row);
            assert!(
                parsed.flags.iter().all(|f| f.long() != Some(refused)),
                "{row:?} was read as the long option -{refused}: {:?}",
                parsed
                    .flags
                    .iter()
                    .map(|f| f.spelling())
                    .collect::<Vec<_>>()
            );
        }
    }

    /// Declared out-of-scope misses, asserted rather than described so
    /// they stay checked. `ip`'s bracketed abbreviation
    /// (`-h[uman-readable]`) is no longer one of them — the grammar's
    /// abbreviation model reads the bracket directly now, before this
    /// repair ever runs.
    #[test]
    fn the_declared_out_of_scope_misses_stay_missed() {
        // A tail that ends at the `=` with nothing after it: refused
        // outright rather than read as either a boolean or an empty value.
        let parsed = parse("  -foo=   an empty value spec\n");
        assert!(
            parsed.flags.iter().all(|f| f.long() != Some("foo")),
            "an empty value spec has no measured reading"
        );
        // `sg_emc_trespass` glues the layout's own colon onto the flag, so
        // the tail is `"r:"` and is not an option name.
        let parsed = parse("    -hr: Set Honor Reservation bit\n");
        assert!(
            parsed.flags.iter().all(|f| f.long() != Some("hr")),
            "a tail carrying punctuation is not a name"
        );
    }

    /// A synopsis-sourced cluster is indistinguishable from a long
    /// option on every condition but source; condition 1 alone keeps
    /// the bundled-short population out.
    #[test]
    fn a_synopsis_sourced_bundle_is_never_read_as_a_long_option() {
        let parsed = parse("usage: rpcbind [-adhilswfr]\n");
        assert!(
            parsed.flags.iter().all(|f| f.long() != Some("adhilswfr")),
            "the bundle belongs to parse_bundled_shorts, not to this repair: {:?}",
            parsed
                .flags
                .iter()
                .map(|f| f.spelling())
                .collect::<Vec<_>>()
        );
    }

    /// A spaced value is indistinguishable from a glued one once
    /// stored; the raw text decides (condition 7).
    #[test]
    fn a_spaced_value_is_never_read_as_a_long_option() {
        let parsed = parse("  -g port    wait gdb connection to 'port'\n");
        assert!(
            parsed.flags.iter().all(|f| f.long().is_none()),
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
            parsed.flags.iter().all(|f| f.long().is_none()),
            "a repeated-character run is the other repair's, and only when it has its boolean"
        );
        // One-character tail is the ambiguous population both repairs
        // decline (`rpcgen -Ss` and friends are half correct parses).
        let parsed = parse("  -ps        postscript\n");
        assert!(parsed.flags.iter().all(|f| f.long().is_none()));
    }

    #[test]
    fn is_option_name_tail_rejects_every_value_spec_shape() {
        assert!(is_option_name_tail("elp"));
        assert!(is_option_name_tail("one-insn-per-tb"));
        assert!(is_option_name_tail("utf8"));
        // `_` is a word separator inside a name, same footing as `-`.
        assert!(is_option_name_tail("ase_sensitive"));
        assert!(is_option_name_tail("ix_fmts"));
        // Leading, trailing and doubled separators are still names.
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
        // Why the whole-token rule exists: lowercase flag letter, a
        // shouting argument glued on.
        assert!(!token_is_uniformly_lowercase("-oOUTFILE"));
    }

    // --- the anchored value recovery --------------------------------------

    /// `ffplay --help`'s `Main options:` table, byte-exact. `-help`'s
    /// `topic` value is restored, anchored against its run-mates and the
    /// literal phrase in the document; all four rows stay their own
    /// entities. See docs/shapes.md S-007 and corpus/ffplay/6.1.1/help.txt.
    const FFPLAY_MAIN_OPTIONS: &str = concat!(
        "Main options:\n",
        "-L                  show license\n",
        "-h topic            show help\n",
        "-? topic            show help\n",
        "-help topic         show help\n",
        "--help topic        show help\n",
        "-version            show version\n",
    );

    #[test]
    fn ffplays_help_row_recovers_its_topic_value_without_merging() {
        let parsed = parse(FFPLAY_MAIN_OPTIONS);
        // Every row is its own entity; this pass only writes
        // `value_name`/`value_kind` in place.
        for (name, dashes) in [
            ("h", Dashes::Single),
            ("?", Dashes::Single),
            ("help", Dashes::Single),
            ("help", Dashes::Double),
        ] {
            let flag = parsed
                .flags
                .iter()
                .find(|f| {
                    f.spellings
                        == [Spelling {
                            name: name.to_string(),
                            dashes,
                            negatable: false,
                            abbrev: None,
                        }]
                })
                .unwrap_or_else(|| {
                    panic!(
                        "no entity spelled exactly {dashes:?}{name} among {:?}",
                        parsed
                            .flags
                            .iter()
                            .map(|f| f.spelling())
                            .collect::<Vec<_>>()
                    )
                });
            assert_eq!(
                flag.value_name.as_deref(),
                Some("topic"),
                "{dashes:?}{name} should carry the recovered value"
            );
            assert_eq!(flag.value_kind, ValueKind::Required);
            assert_eq!(
                flag.description.as_ref().map(Text::as_str),
                Some("show help")
            );
        }
        // `-L` and `-version` must never be touched.
        assert_eq!(parsed.flags.len(), 6);
    }

    /// Recovery requires both halves of the anchor: a run-mate's value
    /// and the literal `<spelling> <value>` phrase in the document. A
    /// shared description alone must never be enough.
    #[test]
    fn a_value_is_never_recovered_without_its_own_literal_phrase_in_the_document() {
        let raw = concat!(
            "options:\n",
            "-h val               show help\n",
            "-help                show help\n",
            "--help val           show help\n",
        );
        let parsed = parse(raw);
        // `-help` never wrote "val" on its own row, so it stays bare.
        let bare_help = parsed
            .flags
            .iter()
            .find(|f| f.spellings.len() == 1 && f.spellings[0].name == "help")
            .unwrap_or_else(|| {
                panic!(
                    "no bare single-spelling -help entity in {:?}",
                    parsed
                        .flags
                        .iter()
                        .map(|f| f.spelling())
                        .collect::<Vec<_>>()
                )
            });
        assert!(bare_help.single_dash());
        assert_eq!(bare_help.value_name, None);
        assert_eq!(bare_help.value_kind, ValueKind::None);
        assert_eq!(
            parsed.flags.len(),
            3,
            "each row is (and stays) its own entity: {:?}",
            parsed
                .flags
                .iter()
                .map(|f| f.spelling())
                .collect::<Vec<_>>()
        );
    }
}
