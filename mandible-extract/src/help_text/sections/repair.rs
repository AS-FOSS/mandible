//! Post-hoc repairs applied to a recovered flag list: the repeated-character
//! shape (`-v`/`-vv`) and the single-dash long-option shape (`-help`,
//! `-number=N`), plus the glued-token index both consult for evidence.

use super::*;

/// Re-read every `-vv`-shaped flag in `flags` as the multi-character
/// single-dash option it is, instead of as its own first character carrying
/// a required value.
///
/// # The defect
///
/// `bpftrace`'s option table writes six rows and this parser produced four
/// flags from them:
///
/// ```text
///     -k             emit a warning when a bpf helper returns an error
///     -kk            check all bpf helper functions
///     -v                      verbose messages
///     -vv                     more verbose messages (max 2)
///     -d                      (dry run) debug info
///     -dd                     (dry run) verbose debug info
/// ```
///
/// [`parse_flag_spec`] has no way to read `-vv` as one name: `try_short`
/// takes the `v` and `try_value` glues the second one on as a required
/// value. So `-k`, `-v` and `-d` land correctly as booleans and `-kk`,
/// `-vv` and `-dd` land as *the same three letters again*, each carrying one
/// copy of its own letter — three real, separately-described switches that
/// are not in the tree under any spelling a user could type. Six of the
/// seed-2 audit's 94 verdicts are this defect (all five `.bt` wrappers
/// around `bpftrace`, plus `ntfsfallocate`, whose help text has the identical
/// `-v`/`-vv` pair).
///
/// # The rule, and why it needs the whole list
///
/// A flag is rewritten when **all** of these hold — the same four conditions
/// `xtask`'s `repeated_char` oracle counts the defect with, deliberately and
/// character for character, because that detector is meant to read zero once
/// this lands and it can only do that if the fix and the measurement agree
/// on what the defect is:
///
/// 1. it has a short spelling, no long name, and a `Required` value;
/// 2. the value is that short character repeated
///    ([`value_repeats_short`]);
/// 3. **another flag in the same node is the bare boolean spelling of the
///    same character** ([`documents_bare_boolean`]);
/// 4. the reconstructed token occurs glued and delimited in the tool's own
///    raw text ([`token_occurs_glued`]).
///
/// **Condition 3 is the whole safety argument, and it is why this is a
/// post-pass rather than a change to [`parse_flag_spec`].** Conditions 1, 2
/// and 4 alone are satisfied by `lessecho`'s real `[-nn]`, which is its
/// genuine "-n followed by a number" flag and a correct parse. Nothing about
/// the *token* separates the two: same length, same shape, same glued
/// spelling. What separates them is the document — `bpftrace` writes a row
/// for `-v` and a row for `-vv` with two different descriptions, while
/// `lessecho` writes `[-nn]` and never mentions a bare `-n` at all. A tool
/// that documents `-v` as taking no value has said, in its own words, that
/// `-vv` cannot be `-v` carrying a value. One fragment cannot see that;
/// the assembled list can.
///
/// The knowing false negative, measured on the fleet and left alone under
/// the no-false-positives rule: a repeated-character flag whose bare form the
/// tool never writes on its own row (`strace`'s `[-DDD]`,
/// `wpa_supplicant`'s `[-BddhKLqqstuvW]`) stays split, because the only
/// evidence that would admit it is the token's shape and `lessecho`'s `-nn`
/// has exactly that shape.
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
        // The whole run becomes one single-dash long spelling, replacing
        // the short-plus-glued-value pair the grammar produced: the name
        // is held bare and `Dashes::Single` is what puts one dash in front
        // of it at display time.
        flag.spellings = vec![Spelling::single_dash(&token[1..])];
        flag.value_name = None;
        flag.value_kind = ValueKind::None;
    }
}

/// True when `value` is one or more copies of `short` and nothing else.
///
/// `-vv` stores `"v"`, `-vvv` stores `"vv"`, `strace`'s `[-DDD]` stores
/// `"DD"`. The emptiness guard matters: an empty value is `Required` with
/// nothing in it, which `chars().all(..)` would call vacuously true.
/// Case-sensitive, like every other spelling comparison here — `-v` and `-V`
/// are different flags, so `-vV` is two flags glued, not one repeated.
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
/// word-shaped immediately before or after it.
///
/// The twin of `xtask::existence::spelling_occurs`, and deliberately the
/// same rule: `value_name` alone cannot tell `-vv` from `-v v`, since
/// [`parse_flag_spec`] reads both into the identical fields, and only the
/// raw text says which one the tool wrote. Char-indexed throughout, never a
/// byte-offset `&str` slice — AGENTS.md's rule against slicing captured tool
/// output at a raw byte offset.
///
/// **This is the definition, not the hot path.** It scans the whole
/// document once per candidate, which is fine for one question and
/// quadratic for a document's worth of them; [`GluedTokenIndex`] answers
/// the same question from one pass over the document and is what the two
/// repairs call. This form stays because it is the readable statement of
/// the predicate, because the index falls back to it for the one candidate
/// shape the index cannot key (see [`GluedTokenIndex::contains`]), and
/// because `indexed_form_agrees_with_scanning_form` pins the two together.
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

/// One document's answer to every [`token_occurs_glued`] question the two
/// flag repairs will ask of it, built in one pass over the document.
///
/// # Why it exists
///
/// [`token_occurs_glued`] scans the whole document per candidate, and both
/// repairs ask it once per surviving flag. That was affordable while the
/// conditions in front of it admitted a handful of candidates per node;
/// widening them (v0.4.0's single-dash long-option work) put ~679
/// candidates in front of it for one tool, against `ffplay`'s 752 KB of
/// help text, and `mandible --doctor ffplay` went from ~1.4 s to 3.2 s.
/// The cost is `O(candidates x document)` and the document is the part
/// nobody controls, so the fix is to stop re-reading it.
///
/// # The structure, and why this one
///
/// Every maximal run of word characters ([`is_word_char`]) in the document,
/// keyed by the run's own text. That is the whole index; a lookup is a hash
/// of the candidate's leading run.
///
/// It works because of what the predicate's two boundary conditions say
/// about a match. A candidate always opens on a word character (`-`), so at
/// any position where it matches, the document's run of word characters
/// starting there is *exactly* the candidate's own leading run: the
/// left boundary makes that position a run start, and the candidate's first
/// non-word character — or, if it has none, the right boundary — is where
/// that run ends. So "does this candidate occur glued and delimited" is
/// "is the candidate's leading run a run of this document, and does the
/// document continue with the candidate's remainder". For the common
/// candidate, all word characters (`-help`, `-vv`), the remainder is empty
/// and a run being maximal *is* both boundary conditions holding, so the
/// hash lookup alone is the answer.
///
/// A candidate carrying a non-word character (`-foffload=<targets>`, the
/// glued-value shape [`split_glued_value`] admits) needs the remainder
/// checked against the text after each occurrence of its leading run —
/// hence the offsets, and hence a map rather than a set. That list is as
/// long as that one run's occurrence count, not as long as the document.
pub(super) struct GluedTokenIndex<'a> {
    /// The document this was built from, for the fallback in
    /// [`GluedTokenIndex::contains`].
    raw: &'a str,
    /// Every maximal run of word characters in `raw`, keyed by the run's
    /// text and valued by the byte offset just past each occurrence of it.
    runs: std::collections::HashMap<&'a str, Vec<usize>>,
}

impl<'a> GluedTokenIndex<'a> {
    /// One pass over `raw`, cutting it at every boundary between a word
    /// character and a non-word one.
    ///
    /// The offsets come from `char_indices`, so every slice taken here is
    /// taken at a character boundary by construction — and is taken through
    /// `get`, which returns `None` rather than panicking, so AGENTS.md's
    /// rule about byte-offset slicing of captured tool output holds by
    /// construction *and* by API even if that reasoning is ever wrong.
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
        // A candidate that opens on a non-word character has no leading run
        // to key on. Both callers ask about a `-`-led token so nothing
        // reaches this in practice, but the predicate is defined for every
        // string and the scanning form answers those correctly; keeping the
        // fallback is cheaper than narrowing the type.
        if head == 0 {
            return token_occurs_glued(self.raw, candidate);
        }
        let (run, rest) = candidate.split_at(head);
        let Some(ends) = self.runs.get(run) else {
            return false;
        };
        if rest.is_empty() {
            // The key matched a *maximal* run, so there is a non-word
            // character (or the end of the document) on both sides of it
            // already — which is the whole predicate.
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

/// The fewest characters a swallowed tail must carry before it is read as
/// the rest of a single-dash long option's name.
///
/// Two, and the same two `xtask::single_dash_long::MIN_SWALLOWED_CHARS`
/// counts the defect with — the two numbers are one number, and the
/// duplication is the same deliberate one `MIN_CLUSTER_MEMBERS` carries
/// against `bundling::MIN_BUNDLED_MEMBERS`. At one swallowed character the
/// shape is genuinely ambiguous: `rpcgen`'s `-Ss`, `xxd`'s `-ps`, `sg_map`'s
/// `-st`, `mandoc`'s `-ac`, `which`'s `-as` are all two-character
/// single-dash tokens and roughly half of that population is a correct
/// parse of a real character-argument flag. Deliberate lost recall.
pub(super) const MIN_SWALLOWED_NAME_CHARS: usize = 2;

/// Re-read every `-help`-shaped option-table row as the single-dash long
/// option it is, instead of as its own first character carrying a required
/// value.
///
/// # The defect
///
/// `qemu-arm64-static`'s option table writes its long options and its
/// genuine value-taking short flags on adjacent rows, separated by nothing
/// but a space:
///
/// ```text
/// -h                                        print this help
/// -help
/// -g port              QEMU_GDB             wait gdb connection to 'port'
/// -cpu model           QEMU_CPU             select CPU (-cpu help for list)
/// -one-insn-per-tb     QEMU_ONE_INSN_PER_TB run with one guest instruction per emulated TB
/// -version             QEMU_VERSION         display version information and exit
/// ```
///
/// [`parse_flag_spec`] has no way to read `-help` as one name: `try_short`
/// takes the `h` and `try_value` glues the rest on as a required value, so
/// the tree gains a second `-h` carrying the value `"elp"` and loses
/// `-help` under any spelling a user could type. Eleven of that one tool's
/// rows go the same way — `-cpu` → `-c` + `"pu"`, `-version` → `-v` +
/// `"ersion"` — while `-g port`, `-L path` and `-R size` on the same rows
/// are entirely correct. A fleet sweep of this machine's `PATH` measured
/// the family at **132 tools and 8,784 flags**, 17.6% of every flag
/// extracted.
///
/// # The rule
///
/// A flag is rewritten when **all** of these hold — the same seven
/// conditions `xtask`'s `single_dash_long` oracle counts the defect with,
/// deliberately and character for character, because that detector is meant
/// to read zero once this lands and it can only do that if the fix and the
/// measurement agree on what the defect *is*:
///
/// 1. it is **option-table-sourced** ([`Source::HelpText`], never
///    [`Source::HelpTextSynopsis`]);
/// 2. it has a short spelling, no long name, and a `Required` value;
/// 3. the swallowed text's **name half** — everything before the first `=`,
///    or the whole tail when there is no `=` ([`split_glued_value`]) — is
///    **option-name-shaped** ([`is_option_name_tail`]);
/// 4. that name half is at least [`MIN_SWALLOWED_NAME_CHARS`] characters;
/// 5. the reconstructed **name token** is **uniformly lowercase**
///    ([`token_is_uniformly_lowercase`]);
/// 6. the tail is not the flag's own character repeated — the
///    [`repair_repeated_character_flags`] family, handed off rather than
///    claimed twice;
/// 7. the reconstructed token — name **and** glued value — occurs glued and
///    delimited in the tool's own raw text ([`token_occurs_glued`]).
///
/// # The glued `=value` half, and why the first version missed it
///
/// `dbiprof` writes one option table and this parser used to repair half of
/// it:
///
/// ```text
///     -number=N        show top N, defaults to 10
///     -sort=S          sort by S, defaults to total
///     -reverse         reverse the sort
///     -match=K=V       for filtering, see docs
/// ```
///
/// `-reverse` came out right and `-number=N` came out as `-n` carrying
/// `"umber=N"`, in the same table, on adjacent rows. The reason is entirely
/// in condition 3: it asked whether the *whole* swallowed run was an option
/// name, and `umber=N` is not one — it is an option name **plus the value
/// spec the tool glued onto it**. `=` is the one character that says where
/// the name stops, so the fix is to read the two halves separately rather
/// than to admit `=` into [`is_option_name_tail`], which would also admit
/// `-E var=value` and every other value spec that carries one.
///
/// **Condition 5 is unchanged in substance and is still the whole safety
/// argument.** It is measured over the name token (`-number`) instead of
/// the full token (`-number=N`) because the value half is now known to be a
/// value half — and a value spec shouts (`-foffload=<targets>`,
/// `-print-file-name=<lib>`) without saying anything about the flag. The
/// population it must stay silent on is unmoved by that: Ghostscript's real
/// `-sDEVICE=png16m` is a genuine glued short whose *name* token is
/// `-sDEVICE`, `cpp`'s `-DMACRO=value` is `-DMACRO`, `-Wl,-rpath=…` is
/// `-Wl,…` (rejected by condition 3 before case is even consulted) — every
/// one of them shouts on the left of the `=`, which is exactly where the
/// convention puts the argument, and every one is still rejected. What the
/// change buys is the mirror-image population, whose name half is a
/// lowercase *word*: `dbiprof`'s `-number`/`-sort`/`-match`/`-exclude`,
/// `gcc`'s `-foffload`, `-print-file-name`, `-print-prog-name`, `-specs`,
/// `-std` and `-save-temps=<arg>`.
///
/// Unlike the spaced-value case below, the value spec here is **kept**: the
/// document wrote it on the same token, so `-foffload` stays a
/// value-taking flag named `<targets>` rather than becoming a boolean.
///
/// **Conditions 1 and 5 are the whole safety argument, and 5 is why this
/// cannot be a change to [`parse_flag_spec`].** Conditions 2, 3, 4, 6 and 7
/// are satisfied character for character by the GCC/Clang glued-value
/// convention — `cargo -Zscript`, `rpcgen -Dname`, `makewhatis -Tutf8`,
/// `perl -Idirectory`, `find -Olevel`, `cc -oOUTFILE`, `gcc -DMACRO` —
/// thousands of **correct** parses fleet-wide, every one of which this must
/// leave alone. What separates them is case, and only case: the convention
/// is an uppercase flag letter with its argument glued on, while a long
/// option is a *word* and words in `--help` output are lowercase.
/// Condition 5 is measured over the whole token rather than the tail alone,
/// deliberately, and the difference is `-oOUTFILE`: its flag letter is
/// lowercase and only the argument shouts, so a tail-only test would admit
/// it and destroy a correct parse. Condition 1 is what keeps the entire
/// bundled-short population out (`rpcbind`'s `[-adhilswfr]` is
/// all-lowercase, unsorted and indistinguishable from a long option on
/// every other condition) — `parse_bundled_shorts` owns that shape from the
/// synopsis, and the identical shape from an option table is this family.
///
/// # Why `_` is a name character
///
/// Condition 3 used to reject `_`, on the theory that it "also appears in
/// glued value placeholders". It does — and so does every letter of the
/// alphabet. `_` is a **word separator inside a name**, the same job `-`
/// does, and none of the conditions above is measured on which separator
/// a name spells its word breaks with: `-DFOO_BAR` is still rejected by
/// condition 5, `-oOUT_FILE` still by condition 5 read over the whole
/// token, `-o out_file` still by condition 7 (it never occurs glued), and
/// `-d item_a[,...]` still by condition 3's own punctuation test.
///
/// Measured on a full-`PATH` sweep of this machine (2,254 tools, aarch64
/// Ubuntu 24.04), admitting `_` moves **17 tools and 604 flag spellings**,
/// and moves nothing else: no tool appeared or disappeared, no tool
/// changed status or tier, and no flag was lost — the field-level
/// `sweep-diff` reports `0 lost across 0 tool(s)`. Every one of the 604
/// recovered names was then checked against its own tool's raw capture,
/// and **all 604 occur as the leading token of a row the tool itself
/// writes** — `clang -fchar8_t`/`-fno-char8_t`, `llvm-install-name-tool
/// -add_rpath`/`-delete_all_rpaths`, `llvm-lipo -verify_arch`,
/// `llvm-otool -chained_fixups`, `ffmpeg -pix_fmts`/`-filter_script`,
/// `dbiprof -case_sensitive`. There were no counter-examples.
///
/// **ffplay and ffprobe are 97% of it**, and they are the case worth
/// stating explicitly because their rows carry a value spec in a
/// *space-separated* column plus a capability column:
///
/// ```text
///   -is_avc            <boolean>    .D.V..X.... is avc (default false)
///   -grab_x            <int>        .D......... Initial x coordinate. (from 0 to INT_MAX)
/// ```
///
/// Neither column is at risk, because neither was ever in `value_name` —
/// the grammar stored the swallowed name half there (`-i` + `"s_avc"`)
/// and both columns went into the *description*, which this repair does
/// not touch. `ffplay`'s tree keeps the same 1,136 flags and the same
/// 1,135 descriptions, byte for byte, before and after; 679 of them stop
/// being fabricated shorts. The rows that were already recovered on the
/// unmodified parser — `-idct`, `-threads`, `-debug`, whose names carry
/// no underscore — have always read exactly this way, so this is the
/// underscore rows joining them rather than a new behaviour.
///
/// # A rejected alternative: "is the candidate short documented?"
///
/// Recorded because it is the obvious next idea and it is **wrong**:
/// allow the long reading only when the tool's help documents no bare row
/// for the candidate short — `dbiprof` documents no `-c`, so `-c` is
/// fabricated there and `-case_sensitive` wins.
///
/// It does not discriminate. Measured over the 604 spellings above it
/// refuses 111 of them, and every single one of the 111 is a documented
/// row token — a 100% false-refusal rate, buying nothing. `ffplay`
/// documents `-f fmt` **and** `-filter_threads`; `-i input_file` **and**
/// `-is_avc`. A tool documenting both a short and a long option that
/// starts with the same letter is the ordinary case, not a suspicious
/// one. Worse, as a general rule it would revert work already shipped:
/// across those same 17 tools it refuses **632 of the 8,260** single-dash
/// long options the parser already recovers, `ffplay -help` among them —
/// the exact `-h` beside `-help` coexistence
/// `xtask::single_dash_long`'s own doc comment opens with. What the idea
/// is reaching for is already supplied, and supplied better, by
/// conditions 5 and 7 together.
///
/// # What this deliberately does not claim
///
/// Named here rather than discovered later, and each one is a place the
/// oracle is silent too — this fix claims **nothing** the detector does
/// not:
///
/// - **Uppercase-led single-dash long options** (`-Wall`, `-Xlint`).
///   Excluded by condition 5, which cannot tell them from `-Zscript`.
/// - **`ip`'s bracketed abbreviations** (`-h[uman-readable]`, `-b[atch]`,
///   `-rc[vbuf]`). The raw text writes brackets, so the grammar records
///   `ValueKind::Optional` — a value spec a human deliberately typed — and
///   condition 2 never admits it.
/// - **Tails carrying layout punctuation.** `sg_emc_trespass` writes
///   `-hr: Set Honor Reservation bit`, so the tail is `"r:"` and condition
///   3 rejects it. No tail-shape rule can claim that without also admitting
///   every value spec that leaks punctuation.
/// - **Tails whose *name* half carries brackets or other value-spec
///   punctuation.** Condition 3 still rejects `[`, `<`, `,`, `.` and `/`
///   in the name half, for the same reason the oracle does — `-d
///   item[,...]` and `-b{blocksize}` are value specs, not names. Only `=`
///   is read structurally, and only as the boundary between the two halves.
/// - **A tail that ends at the `=` with nothing after it** — refused
///   outright by [`split_glued_value`], which has no evidence for either
///   reading of it.
/// - **One-character tails** ([`MIN_SWALLOWED_NAME_CHARS`]).
///
/// The value a rewritten row's *real* spaced argument named (`-cpu model`
/// documents a `model`) is not recovered: by the time the fragment reached
/// here the grammar had already stored `"pu"` and dropped `"model"` on the
/// floor. The flag becomes the boolean `-cpu` rather than `-c` taking
/// `"pu"` — the correct **name** under a missing value spec, which is
/// strictly better than a fabricated name under a fabricated value spec,
/// and is exactly what `repair_repeated_character_flags` does with `-vv`.
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
        // 3a. Split the swallowed text at the first `=` — see
        //     [`split_glued_value`]. Without a `=` the name half is the
        //     whole tail and every condition below reads exactly as it did
        //     before this split existed.
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
        // 7. The whole token — name *and* glued value — occurs, glued and
        //    delimited, in the raw text. Last because it is the only
        //    condition that reads the document at all — one hash lookup
        //    now, against an index built once for the whole document
        //    ([`GluedTokenIndex`]), rather than a scan per candidate.
        if !glued_tokens.contains(&format!("-{short}{tail}")) {
            continue;
        }
        // The run up to the `=` becomes one single-dash long spelling,
        // replacing the short-plus-glued-name pair the grammar produced:
        // the name is held bare and `Dashes::Single` is what puts one dash
        // in front of it at display time.
        flag.spellings = vec![Spelling::single_dash(&name_token[1..])];
        match glued_value {
            // `-foffload=<targets>`: the document wrote the value spec
            // itself, so it survives the repair on the flag it belongs to.
            Some(value) => flag.value_name = Some(value.to_string()),
            // `-cpu model`: the value was dropped on the floor by the
            // grammar long before this ran, so the flag becomes the
            // boolean it is correctly *named* rather than keeping a
            // fabricated one. See this function's doc comment.
            None => {
                flag.value_name = None;
                flag.value_kind = ValueKind::None;
            }
        }
    }
}

/// Split a swallowed tail into the option-name half and the glued value
/// half: `"umber=N"` → `("umber", Some("N"))`, `"elp"` → `("elp", None)`.
///
/// `None` — refuse the row entirely — when the tail ends at the `=` with
/// nothing after it (`"oo="`). A `Required` value whose spec is the empty
/// string is a shape nothing in the fleet was measured on, and inventing
/// either reading of it (boolean, or a value named `""`) would be a claim
/// this repair has no evidence for.
///
/// **Splitting at the *first* `=` is what makes `dbiprof`'s `-match=K=V`
/// come out right**: the name ends at the first one and everything after it
/// is the value spec the tool wrote, `=` included.
///
/// The twin of `xtask::single_dash_long::split_glued_value`, character for
/// character, for the reason [`repair_single_dash_long_options`]'s doc
/// comment gives.
pub(super) fn split_glued_value(tail: &str) -> Option<(&str, Option<&str>)> {
    match tail.split_once('=') {
        Some((_, "")) => None,
        Some((name, value)) => Some((name, Some(value))),
        None => Some((tail, None)),
    }
}

/// True when `tail` could be the rest of a single-dash long option's name:
/// ASCII alphanumerics, `-` and `_`, with at least one ASCII letter in it.
///
/// The twin of `xtask::single_dash_long::is_option_name_tail`, character
/// for character. The letter requirement is what stops a glued *numeric*
/// argument (`-b4096`, `-j8`) from riding in on a run that is technically
/// alphanumeric. Everything else is rejected because a long option's name
/// does not contain it: `:` (`sg_emc_trespass`'s layout-mangled `-hr:`),
/// `[`/`{`/`<`/`,` (`-d item[,...]`, `-b{blocksize}`), `.` and `/` (paths).
///
/// `_` is admitted on the same footing as `-`, for the reason given in
/// [`repair_single_dash_long_options`]'s "Why `_` is a name character"
/// section: it separates words inside a name, and every condition that
/// makes this repair safe is measured over the token, not over which
/// separator the name happens to spell its word breaks with.
///
/// `=` never reaches here: [`split_glued_value`] has already consumed it as
/// the boundary between the name and its glued value spec, so what this
/// sees is only ever the name half.
pub(super) fn is_option_name_tail(tail: &str) -> bool {
    tail.chars().any(|c| c.is_ascii_alphabetic())
        && tail
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// True when `token` carries no ASCII uppercase letter at all — the
/// discriminator against the GCC/Clang glued-value convention, whose whole
/// population is an uppercase flag letter with its argument glued on
/// (`-Zscript`, `-Dname`, `-Tutf8`, `-Idirectory`, `-Olevel`, `-DMACRO`,
/// `-oOUTFILE`, `-Wall`).
///
/// The twin of `xtask::single_dash_long::token_is_uniformly_lowercase`,
/// and measured over the *whole* token rather than only the tail for the
/// reason recorded there: `-oOUTFILE`'s flag letter is lowercase and only
/// its argument shouts, so a tail-only test would admit it.
pub(super) fn token_is_uniformly_lowercase(token: &str) -> bool {
    !token.chars().any(|c| c.is_ascii_uppercase())
}

/// Fold a run of *adjacent* option-table rows into one multi-spelling
/// entity when each row names exactly one spelling and the rows document,
/// word for word, the same thing under different names — spec §7's row
/// grammar gains this as the adjacency fold.
///
/// # The defect
///
/// `ffplay --help`'s `Main options:` table writes help itself under four
/// separate physical rows:
///
/// ```text
/// -h topic            show help
/// -? topic            show help
/// -help topic         show help
/// --help topic        show help
/// ```
///
/// Before [`mandible_core::Entity::spellings`] could hold more than two
/// spellings, nothing could recover all four under one entity; even now,
/// nothing folds them at all — [`fields_in_line`]'s alias fold only
/// widens *one physical line* carrying several bare cells under a shared
/// description, and [`crate::merge::pair_aliases`] (a different module,
/// a different job — cross-*source* short/long pairing, spec §4.4)
/// refuses a single-dash long option as either half of its two-way pair on
/// purpose (the `lto-dump` incident its own doc comment records). None of
/// that machinery is wrong to refuse this shape; nothing existed to *do*
/// it.
///
/// # The rule
///
/// Two adjacent rows fold into one entity only when **all** of these hold:
///
/// 1. both are option-table rows (never a synopsis-mined flag,
///    [`Source::HelpTextSynopsis`] never [`Source::HelpText`] alone);
/// 2. both name **exactly one** spelling apiece — an entity a row already
///    spelled with several aliases (`-A, --catenate`) is complete on its
///    own and never a fold *input*;
/// 3. both carry the same [`mandible_core::Entity::group`] (so a table
///    boundary — ffplay's own `AVCodecContext AVOptions:` block starting
///    right after `Main options:` ends — can never be crossed even if two
///    unrelated rows happen to repeat the same description); and
/// 4. description **and** `value_name` **and** `value_kind` are identical,
///    word for word; and
/// 5. a run may claim **at most one distinct long-like name** —
///    [`long_like_name`]'s own doc comment has the regression this
///    condition exists to refuse (`dbiprof`'s `-match`/`-exclude`,
///    `dpkg`'s `--configure`/`--triggers-only`: two different long
///    options, one boilerplate description, no relation to each other).
///
/// Condition 4 is deliberately strict and *never* relaxed to "close
/// enough" — `xxd`'s two `-r` rows and `du`'s bare `--time` beside its
/// valued `--time=WORD` share a description each but disagree on shape,
/// and folding either pair would be exactly the false merge spec §7's row
/// grammar exists to refuse (see the block comment above
/// `extract_usage_flags`'s caller in this module's parent for `du`'s and
/// `ex`'s own regression history).
///
/// # Restoring `-help`'s own value first
///
/// Condition 4 would never admit `-help` on its own: [`try_short`] reads
/// it as `-h` plus the glued value `"elp"`, [`repair_single_dash_long_options`]
/// (correctly) rewrites that into the single-dash spelling `-help`, and —
/// documented there as a deliberate, measured limitation — clears the
/// value entirely rather than inventing one, because by the time that
/// repair runs the row's real, *separately spaced* value (`"topic"`) was
/// already dropped by the grammar. That limitation is correct and stays:
/// changing it would reopen a decision measured across 132 tools and
/// 8,784 flags (`qemu-arm64-static`'s own `-cpu model` is the same shape,
/// and *its* `"model"` truly has no anchor — nothing else in that table
/// documents `"model"` as a value).
///
/// `-help`'s row is different only in that this run **already contains
/// the answer**: `-h`, `-?` and `--help` all recovered `value_name:
/// "topic"`, `value_kind: Required` cleanly, from the *same* description.
/// So before the strict fold runs, each single-dash entity in the run
/// still missing a value is checked against its run-mates' shared value:
/// if the raw document literally contains `<this row's own spelling> "
/// "<that value>` as a delimited phrase — the exact column shape `-h
/// topic`/`-? topic`/`--help topic` already establish — the value is
/// restored from that anchor, never invented from whitespace alone. A row
/// with no such anchor (or whose run-mates disagree on the value) is left
/// exactly as [`repair_single_dash_long_options`] made it, and condition 4
/// then correctly refuses to fold it.
pub(super) fn fold_adjacent_alias_rows(flags: Vec<Entity>, raw: &str) -> Vec<Entity> {
    fn eligible(f: &Entity) -> bool {
        f.spellings.len() == 1
            && f.description.is_some()
            && f.provenance.sources.contains(&Source::HelpText)
            && !f.provenance.sources.contains(&Source::HelpTextSynopsis)
    }

    // Step 1: chain adjacent, eligible rows that share a description and a
    // table (`group`) into runs. Every run is non-empty; a row that broke
    // the chain (ineligible, or a new description/group) always starts a
    // fresh one, so no row is ever dropped here — only regrouped.
    let mut runs: Vec<Vec<Entity>> = Vec::new();
    for flag in flags {
        let joins_previous = eligible(&flag)
            && runs.last().and_then(|run| run.last()).is_some_and(|last| {
                eligible(last) && last.description == flag.description && last.group == flag.group
            });
        if joins_previous {
            runs.last_mut().expect("just checked non-empty").push(flag);
        } else {
            runs.push(vec![flag]);
        }
    }

    // Step 2: fold each run (restoring an anchored value first), and
    // flatten the result back into one list in document order.
    let mut result = Vec::new();
    for run in runs {
        result.extend(fold_run(run, raw));
    }
    result
}

/// One [`fold_adjacent_alias_rows`] run: same description, same table,
/// each row one spelling. Recovers an anchored value (see that function's
/// doc comment), then folds the maximal adjacent sub-runs that now truly
/// agree on `value_name` and `value_kind` — never the whole run blindly,
/// so a row the recovery step could not anchor stays its own entity
/// exactly as it already was.
fn fold_run(mut run: Vec<Entity>, raw: &str) -> Vec<Entity> {
    if run.len() < 2 {
        return run;
    }

    // The value this run's own well-parsed rows already agree the shared
    // description takes, if any — the anchor every recovery below is
    // checked against, never a value invented from the candidate row
    // alone.
    let anchor = run
        .iter()
        .find(|f| f.value_kind != ValueKind::None && f.value_name.is_some())
        .map(|f| {
            (
                f.value_name.clone().expect("checked Some above"),
                f.value_kind,
            )
        });

    if let Some((value, kind)) = anchor {
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

    let mut out = Vec::with_capacity(run.len());
    let mut i = 0;
    while i < run.len() {
        let mut j = i + 1;
        // The one long-like name (if any) already claimed by the subrun
        // growing from `i` — see `long_like_name`'s doc comment for why a
        // second, *different* long-like name ends the subrun even when
        // `value_name`/`value_kind` still agree.
        let mut long_name = long_like_name(&run[i]);
        // Every spelling already claimed by the subrun — `ffplay --help`'s
        // own AVOptions dump repeats several options (`-raw_packet_size`,
        // `-gateway`, `-keep_ass_markup`) verbatim across more than one
        // demuxer/decoder's identical block, so an *exact* duplicate row
        // can otherwise satisfy every condition above. That is not a
        // second spelling of the option — it is the same spelling
        // documented twice — and folding it in would produce an entity
        // whose own `spellings` repeats one name, which is worse than the
        // pre-existing duplicate rows this fold must leave untouched
        // (out of scope for this fix; not an information loss either way).
        let mut seen: Vec<(&str, Dashes)> = vec![(
            run[i].spellings[0].name.as_str(),
            run[i].spellings[0].dashes,
        )];
        while j < run.len()
            && run[j].value_name == run[i].value_name
            && run[j].value_kind == run[i].value_kind
        {
            let candidate_spelling = (
                run[j].spellings[0].name.as_str(),
                run[j].spellings[0].dashes,
            );
            if seen.contains(&candidate_spelling) {
                break;
            }
            match (long_name, long_like_name(&run[j])) {
                (Some(a), Some(b)) if a != b => break,
                (None, Some(b)) => long_name = Some(b),
                _ => {}
            }
            seen.push(candidate_spelling);
            j += 1;
        }
        if j - i >= 2 {
            out.push(merge_alias_run(&run[i..j]));
        } else {
            out.push(run[i].clone());
        }
        i = j;
    }
    out
}

/// The bare name of `e`'s one spelling, but only when that spelling is
/// long-*like* (two dashes, or one dash with more than one character —
/// the same shape [`mandible_core::Entity::long`] recognizes): `Some("help")`
/// for both `-help` and `--help`, `None` for `-h` or `-?`.
///
/// # Why [`fold_run`] refuses two different long-like names in one run
///
/// `description`/`value_name`/`value_kind` agreeing is not, on its own,
/// evidence that two rows spell the *same* option — a short letter is
/// cheap enough that tools genuinely offer more than one mnemonic for one
/// flag (`-h`/`-?` both meaning "help" is the specimen this fold exists
/// for), but a long *word* is a real name, and two different words never
/// name the same option merely because their rows happen to share
/// boilerplate. `dbiprof --help`'s own `-match=K=V`/`-exclude=K=V`
/// (`corpus/dbiprof/1.643`) share the description `"for filtering, see
/// docs"` and the identical `K=V`/`Required` value shape and are two
/// completely different flags; `dpkg --help`'s own `--configure`/
/// `--triggers-only` (`corpus/dpkg/1.22.6`) share `"<package>... |
/// -a|--pending"` the same way. Both regressed this fold on first
/// implementation before this check existed. Requiring at most one
/// distinct long-like name per fold is exactly [`crate::merge::
/// pair_aliases`]'s own `complementary` rule read the other way round:
/// that function refuses to pair two long-like spellings *at all*; this
/// one admits it, but only when they are the same word under a different
/// dash count (`-help`/`--help`) — never two different words.
fn long_like_name(e: &Entity) -> Option<&str> {
    e.spellings.first().and_then(|s| {
        (matches!(s.dashes, Dashes::Double)
            || (matches!(s.dashes, Dashes::Single) && s.name.chars().count() > 1))
            .then_some(s.name.as_str())
    })
}

/// Merge a run already proven to satisfy [`fold_adjacent_alias_rows`]'s
/// strict gate into one entity: every spelling, in document order, on the
/// first row's other fields — mirroring [`crate::merge::pair_aliases`]'s
/// own `absorb_pair` field-by-field policy (union where a field can
/// legitimately carry more than one value, first-wins where it can't).
fn merge_alias_run(run: &[Entity]) -> Entity {
    let mut merged = run[0].clone();
    merged.spellings = run
        .iter()
        .flat_map(|e| e.spellings.iter().cloned())
        .collect();
    for other in &run[1..] {
        for choice in &other.choices {
            if !merged.choices.contains(choice) {
                merged.choices.push(choice.clone());
            }
        }
        merged.repeatable |= other.repeatable;
        merged.required |= other.required;
        merged.hidden &= other.hidden;
        merged.inherited |= other.inherited;
        merged.default = merged.default.clone().or_else(|| other.default.clone());
        merged.env_var = merged.env_var.clone().or_else(|| other.env_var.clone());
        for reference in &other.see_also {
            if !merged.see_also.contains(reference) {
                merged.see_also.push(reference.clone());
            }
        }
        merged.provenance.absorb(&other.provenance);
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- the repeated-character flag repair -----------------------------

    /// `bpftrace`'s real troubleshooting block, byte-exact from
    /// `corpus/killsnoop.bt/audit-seed2/help.stderr.txt`. Four rows, four
    /// real flags; before the repair the tree had two.
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
        // ...and the booleans the repair reads as its evidence are still
        // there, untouched. A repair that consumed them would satisfy the
        // must_contain_flags contract and destroy the tool.
        for short in ['v', 'd'] {
            let flag = parsed
                .flags
                .iter()
                .find(|f| f.short() == Some(short))
                .unwrap_or_else(|| panic!("-{short} must survive"));
            assert_eq!(flag.value_kind, ValueKind::None);
        }
    }

    /// The false positive the whole design turns on: `lessecho`'s `[-nn]`
    /// is character-for-character this shape and is a correct parse of a
    /// real flag taking a number. It survives only because `lessecho` never
    /// writes a bare `-n`.
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
        // ...and the identical token *is* repaired the moment a document
        // declares the bare spelling a boolean, confirming that condition
        // is what was doing the work rather than some other one failing.
        let parsed = parse("  -n         never overwrite\n  -nn        never ever overwrite\n");
        assert!(flag_named(&parsed, "nn").single_dash());
    }

    /// A spaced value is indistinguishable from a glued one once
    /// [`parse_flag_spec`] has stored it, so the raw text is what decides.
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

    /// The other two families sharing the `short && !long && value_name`
    /// fingerprint must come through untouched, even when the document
    /// offers the bare boolean the repair looks for.
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

    /// The index is an optimization and nothing else, so the thing worth
    /// pinning is not any one answer but the *agreement*: for every case
    /// below, [`GluedTokenIndex::contains`] and [`token_occurs_glued`] must
    /// return the same thing, and that thing must be the documented one.
    ///
    /// The cases are the ones where an index built out of maximal word
    /// runs could plausibly disagree with a scan — a glued neighbour on
    /// either side, a token flush against the start or the end of the
    /// document with no delimiter there at all, a match that is a real
    /// substring but not a delimited one, the same token written more than
    /// once, a candidate carrying a non-word character (the
    /// [`split_glued_value`] shape, which is what makes the index a map of
    /// offsets rather than a set), one whose leading run occurs repeatedly
    /// but with the right remainder behind only one of them, a candidate
    /// that opens on a non-word character (the fallback path), and
    /// multi-byte delimiters, which is where a byte-offset index would
    /// panic or silently miss.
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

    /// `qemu-arm64-static`'s real option table, byte-exact from
    /// `corpus/qemu-arm64-static/audit-seed2/help.txt` — the long options
    /// and the genuine value-taking short flags on adjacent rows, which is
    /// the whole false-positive problem in six lines.
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

    /// The false-positive case that matters most, and the reason the
    /// `qemu` table is carried whole rather than as the `-help` row alone:
    /// `-g port` stores a `value_name` exactly as `-help` stores `"elp"`,
    /// and only the space in the raw text tells them apart.
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
        // ...and the bare `-h` boolean the document also writes is still a
        // short flag in its own right. `-h` and `-help` are two different
        // flags of this tool and the repair must produce both.
        assert!(parsed
            .flags
            .iter()
            .any(|f| f.short() == Some('h') && f.value_kind == ValueKind::None));
    }

    /// The whole safety argument in one test: the GCC/Clang glued-value
    /// convention satisfies every condition but the case one, and every
    /// member of it is a **correct** parse that must survive untouched.
    /// Each token here is a real flag of a real tool, and `-oOUTFILE` is
    /// the one that forces the case test to read the whole token rather
    /// than only the tail.
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

    /// `dbiprof`'s real option table, byte-exact from
    /// `corpus/dbiprof/1.643/help.txt` — the glued-`=value` rows and the
    /// value-less rows in one table, which is the whole `=`-split problem
    /// in five lines.
    const DBIPROF_TABLE: &str = concat!(
        "    -number=N        show top N, defaults to 10\n",
        "    -sort=S          sort by S, defaults to total\n",
        "    -reverse         reverse the sort\n",
        "    -match=K=V       for filtering, see docs\n",
        "    -exclude=K=V     for filtering, see docs\n",
        "    -case_sensitive  for -match and -exclude\n",
        "    -version         print version number and exit\n",
    );

    /// The defect the `=` split exists for: a single-dash long option
    /// carrying a glued value came out as its own first character plus a
    /// mangled value (`-number=N` → `-n` + `"umber=N"`), while the
    /// value-less rows of the *same table* came out right.
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
            // `Flag::spelling` writes a required value with a space, the
            // same repo-wide display convention that renders `--output=FILE`
            // as `--output FILE`; what matters here is that the *name* is
            // whole and the value is the tool's own.
            assert_eq!(flag.spelling(), format!("-{name} {value}"));
            assert_eq!(flag.short(), None);
            // The document wrote the value spec on the token, so unlike the
            // spaced case it survives the repair. `-match=K=V` splits at the
            // *first* `=` and keeps the rest verbatim.
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

    /// `gcc`'s `-foffload=<targets>`, stored as short `f` with `value_name`
    /// `offload=<targets>` — a real parser bug the human audit confirmed on
    /// `corpus/gcc/13.3.0`, and the same family as `dbiprof`'s. Carried as
    /// gcc's own rows so the uppercase value spec is exercised: the case
    /// test now reads the name half, and `<targets>` shouts on the other
    /// side of the `=`.
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

    /// The inverse direction, and the reason condition 5 may look at the
    /// name half alone: the glued-value convention puts its shout to the
    /// **left** of the `=`, so every genuine glued short with a `key=value`
    /// argument is still rejected on exactly the signal it always was.
    /// Ghostscript's `-sDEVICE=` is the type specimen — a lowercase flag
    /// letter, which is what makes it the hard case.
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

    /// The separator is still the whole difference: a **spaced** `key=value`
    /// argument stores byte-for-byte what `dbiprof`'s glued `-number=N`
    /// stores, and only condition 7's scan of the raw text tells them
    /// apart.
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

    /// `_` separates words inside an option name exactly as `-` does, and
    /// `dbiprof` proves it in one table: `-case_sensitive` sits between
    /// `-exclude=K=V` and `-version`, both of which this repair already
    /// recovered, and came out as `-c` carrying `"ase_sensitive"` — a
    /// short flag `dbiprof` does not document at all.
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
        // The fabricated short must not survive under any other flag.
        assert!(
            !parsed
                .flags
                .iter()
                .any(|f| f.short() == Some('c') && f.long().is_none()),
            "the invented -c is not left behind"
        );
    }

    /// The ffmpeg `AVOption` table is 97% of this widening's population,
    /// and the thing that has to survive it is the **value spec**: these
    /// rows write `<int>`/`<flags>`/`<string>` in a space-separated column
    /// of their own, followed by a `.D.V..X....` capability column. Both
    /// already live in the *description* — the grammar never stored them
    /// in `value_name`, which held the swallowed name half instead — so
    /// the repair must move the name and leave the description untouched.
    ///
    /// Rows quoted byte-for-byte from `ffplay --help` (6.1.1-3ubuntu5).
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
            // The control: no underscore, so this row is recovered on the
            // parser as it stands. Its description is what the two above
            // must now look like.
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

    /// The inverse, in the direction that matters: an underscore in the
    /// *swallowed* text is not on its own a licence to read a long option.
    /// Every one of these is a correct parse the widening must leave
    /// standing, and each is refused by a different condition.
    #[test]
    fn an_underscore_alone_never_buys_the_long_reading() {
        for (row, refused) in [
            // Condition 5: the GCC/Clang glued-value convention shouts,
            // and an underscored macro name shouts with it.
            ("  -DFOO_BAR         define a macro\n", "DFOO_BAR"),
            ("  -DMAX_PATH=4096   define a macro\n", "DMAX_PATH"),
            // Condition 5 again, via the whole token: only the argument
            // shouts, and that is exactly the `-oOUTFILE` shape.
            ("  -oOUT_FILE        write output here\n", "oOUT_FILE"),
            // Condition 7: a *spaced* underscored value stores the same
            // bytes a glued one would, and the raw text is what tells
            // them apart — `-o out_file` never occurs glued.
            ("  -o out_file       write output here\n", "out_file"),
            // Condition 3: the name half still may not carry value-spec
            // punctuation just because it also carries an underscore.
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

    /// The declared out-of-scope misses, asserted rather than described —
    /// a miss that is only written down in prose stops being checked the
    /// day the prose goes stale.
    ///
    /// `ip`'s bracketed abbreviation (`-V[ersion]`, `-h[uman-readable]`,
    /// `-j[son]`) used to be a third miss here: this repair pass's
    /// Required-only fingerprint could never see it, and nothing else
    /// resolved it either, so the bracket was silently discarded and the
    /// row lost its long name. It is not a miss any more — the grammar's
    /// abbreviation model (`grammar::try_short`) now reads the bracket
    /// directly and produces `long: "human-readable"` on its own, before
    /// this repair pass ever runs. See
    /// `grammar::tests::short_flag_abbreviation_bracket_is_not_an_invented_value`
    /// for that positive case.
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

    /// A synopsis-sourced cluster is all-lowercase, unsorted, and
    /// indistinguishable from a long option on every condition but its
    /// source. Condition 1 is the only thing keeping the entire bundled-
    /// short population out of this repair.
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
    /// [`parse_flag_spec`] has stored it, so the raw text is what decides
    /// — the same condition 7 the repeated-character repair leans on.
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
        // A one-character tail is the ambiguous population both repairs
        // decline: `rpcgen -Ss` and friends are half correct parses.
        let parsed = parse("  -ps        postscript\n");
        assert!(parsed.flags.iter().all(|f| f.long().is_none()));
    }

    #[test]
    fn is_option_name_tail_rejects_every_value_spec_shape() {
        assert!(is_option_name_tail("elp"));
        assert!(is_option_name_tail("one-insn-per-tb"));
        assert!(is_option_name_tail("utf8"));
        // `_` is a word separator inside a name, on the same footing as
        // `-`: `dbiprof`'s `-case_sensitive`, ffmpeg's `-pix_fmts`.
        assert!(is_option_name_tail("ase_sensitive"));
        assert!(is_option_name_tail("ix_fmts"));
        // Leading, trailing and doubled separators are still names — the
        // shape test is about the character set, and every other
        // condition is what makes the repair safe.
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
        // The case the whole-token rule exists for: a lowercase flag
        // letter with a shouting argument glued on.
        assert!(!token_is_uniformly_lowercase("-oOUTFILE"));
    }

    // --- the adjacency fold ----------------------------------------------

    /// `ffplay --help`'s own `Main options:` table, byte-exact from
    /// `corpus/ffplay/6.1.1/help.txt` — issue #30's primary example. The
    /// regression this pins: all four rows must land on one entity, in
    /// document order, with `-help`'s own `topic` value restored (it is
    /// swallowed by `try_short` and then cleared by
    /// `repair_single_dash_long_options`, exactly as that repair's own doc
    /// comment records — see `fold_run`'s anchored-recovery step).
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
    fn ffplays_four_help_rows_fold_into_one_entity_in_document_order() {
        let parsed = parse(FFPLAY_MAIN_OPTIONS);
        let help = flag_named(&parsed, "help");
        assert_eq!(
            help.spellings
                .iter()
                .map(Spelling::render)
                .collect::<Vec<_>>(),
            vec!["-h", "-?", "-help", "--help"],
            "all four spellings, in the document's own order"
        );
        assert_eq!(help.value_name.as_deref(), Some("topic"));
        assert_eq!(help.value_kind, ValueKind::Required);
        assert_eq!(
            help.description.as_ref().map(Text::as_str),
            Some("show help")
        );
        // `-L` and `-version` (different descriptions) must never be
        // swept into the fold — the whole node still reads as three
        // flags, not one.
        assert_eq!(parsed.flags.len(), 3);
    }

    /// `dbiprof`'s own `-match=K=V`/`-exclude=K=V` (`corpus/dbiprof/
    /// 1.643/help.txt`): two different long options sharing a boilerplate
    /// description and an identical value shape. The strict gate's
    /// description/value match alone would fold them; `long_like_name`'s
    /// distinct-long-name check is what refuses to.
    #[test]
    fn two_different_long_options_sharing_a_description_never_fold() {
        let raw = concat!(
            "options:\n",
            "    -match=K=V       for filtering, see docs\n",
            "    -exclude=K=V     for filtering, see docs\n",
        );
        let parsed = parse(raw);
        assert_eq!(parsed.flags.len(), 2, "-match and -exclude stay separate");
        assert!(flag_named(&parsed, "match").spellings.len() == 1);
        assert!(flag_named(&parsed, "exclude").spellings.len() == 1);
    }

    /// `dpkg`'s own `--configure`/`--triggers-only` (`corpus/dpkg/
    /// 1.22.6/help.txt`): the same false-merge shape as `dbiprof`, but
    /// with both dashes and a bare (no-value) row, pinned separately
    /// because it is the specimen that first caught the regression.
    #[test]
    fn dpkgs_configure_and_triggers_only_never_fold() {
        let raw = concat!(
            "Commands:\n",
            "  --configure       <package>... | -a|--pending\n",
            "  --triggers-only   <package>... | -a|--pending\n",
        );
        let parsed = parse(raw);
        assert_eq!(parsed.flags.len(), 2);
    }

    /// A run may repeat one spelling verbatim (`ffplay --help`'s AVOptions
    /// dump documents `-raw_packet_size` under more than one demuxer with
    /// byte-identical text) without the fold gluing the duplicate onto an
    /// entity's own `spellings` list — see `fold_run`'s `seen` guard.
    #[test]
    fn an_exact_duplicate_row_is_never_folded_into_a_repeated_spelling() {
        let raw = concat!(
            "options:\n",
            "  -raw_packet_size  (from 1 to INT_MAX) (default 1024)\n",
            "  -raw_packet_size  (from 1 to INT_MAX) (default 1024)\n",
        );
        let parsed = parse(raw);
        assert_eq!(parsed.flags.len(), 2, "the duplicate rows stay separate");
        for flag in &parsed.flags {
            assert_eq!(flag.spellings.len(), 1);
        }
    }
}
