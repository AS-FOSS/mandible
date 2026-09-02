//! The existence detector: checks that every help-text-sourced subcommand
//! name, flag spelling and positional operand occurs literally in the
//! tool's own raw output, at a position a real entry could occupy.
//! Complements [`crate::misattribution`] (description-to-flag attachment).
//!
//! Flags match as a word-boundary substring anywhere in the raw text
//! ([`spelling_occurs`]); candidates are reconstructed for forms the IR
//! normalizes away (value-spec glued on, `--[no-]` negation, GCC-style
//! single-dash multi-character flags — see [`short_candidates`] and
//! [`long_candidates`]). Subcommand names additionally require the
//! occurrence to sit at a real command-list position: line-start
//! ([`line_start_words`]) or a list-row item ([`list_row_words`]).
//! Operand positions use [`option_list_slot`] to reject a placeholder
//! word (`OPTION`) mistaken for a real operand.
//!
//! Only nodes/flags sourced from `HelpText`/`HelpTextSynopsis` are checked;
//! other sources (Cobra `__complete`, native probes, vendored catalogs)
//! are structural and legitimately absent from help text.
//!
//! Fixture: `corpus/tar/1.35/help.txt` ([M-10] replay, see [`detect`]'s
//! tests). Spec: §13.1, §13.1b.

use mandible_core::{is_command_name_shaped, CommandNode, Entity, Provenance, Source};
use std::collections::HashSet;

/// Whether `flag_char` may not immediately follow (or precede) a candidate
/// spelling for it to count as a genuine, isolated occurrence — the same
/// "not embedded in something longer" guard on both sides. Deliberately
/// narrower than `misattribution::is_flag_char`: this only needs to reject
/// the case of one spelling being a strict prefix of a different, longer
/// one (`--foo` inside `--foobar`), not to recognize every legal short-flag
/// character shape.
pub(crate) fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '-' || c == '_'
}

/// True when `candidate` occurs in `raw` as an isolated token: nothing
/// word-shaped ([`is_word_char`]) immediately precedes or follows the
/// match. Char-indexed throughout (AGENTS.md: never slice captured tool
/// output at a raw byte offset).
///
/// Prefix-tolerant, not exact-token: the character after the candidate may
/// be anything non-word-shaped, so `--gpg-sign` matches `--gpg-sign[=KID]`.
///
/// `pub(crate)` for [`crate::bundling`], which needs the identical check.
pub(crate) fn spelling_occurs(raw: &str, candidate: &str) -> bool {
    let hay: Vec<char> = raw.chars().collect();
    let needle: Vec<char> = candidate.chars().collect();
    if needle.is_empty() || hay.len() < needle.len() {
        return false;
    }
    for start in 0..=(hay.len() - needle.len()) {
        if hay[start..start + needle.len()] != needle[..] {
            continue;
        }
        let before_ok = start == 0 || !is_word_char(hay[start - 1]);
        let end = start + needle.len();
        let after_ok = end == hay.len() || !is_word_char(hay[end]);
        if before_ok && after_ok {
            return true;
        }
    }
    false
}

/// The set of every physical line's first whitespace-delimited word
/// (leading whitespace trimmed only), for the "subcommand name sits where
/// a real command-list entry sits" half of the rule.
///
/// Trailing `:`/`,`/`;` is stripped before the token enters the set: a
/// command-list row commonly glues punctuation onto the name with no space
/// (`gh --help`'s `  auth:        Authenticate...`). Safe to strip — none
/// of the three are legal command-name characters
/// (`mandible_core::is_command_name_shaped`).
///
/// Does not by itself cover a name in column 2+ of a multi-column table
/// (`busybox`, `openssl`); [`list_row_words`] does.
///
/// Fixture: `corpus/gh/*/help.txt`.
fn line_start_words(raw: &str) -> HashSet<&str> {
    let mut out = HashSet::new();
    for line in raw.lines() {
        let Some(word) = line.split_whitespace().next() else {
            continue;
        };
        let word = word.trim_end_matches([':', ',', ';']);
        out.insert(word);
        // binutils `ar` glues optional modifier groups onto a command
        // (`m[ab]`, `r[ab][f][u]`); reuses the parser's own stripper rather
        // than a second copy. Additive/prefix-only: returns input unchanged
        // unless the suffix is well-formed `[...]` groups.
        let bare = mandible_extract::help_text::strip_optional_modifier_suffix(word);
        if bare.len() < word.len() && !bare.is_empty() {
            out.insert(bare);
        }
    }
    out
}

/// Minimum items a line must break into before it reads as a **list row**.
/// Three, not two: at two, any two-column description table (a single-word
/// right-hand cell) would misread as a list row, hiding a real fabrication
/// that collided with a description word. `openssl`'s command grid carries
/// 4 items/line; `busybox`'s applet list 9-11.
const MIN_LIST_ROW_ITEMS: usize = 3;

/// Split one physical line into list-row items: tabs and column gaps (2+
/// spaces) first, then commas, trimmed, empties dropped. Both separators
/// are needed: `openssl`'s command index is space-aligned, `busybox`'s is
/// comma-joined.
///
/// `&str` borrows only (`split`/`trim`) — no byte-offset arithmetic
/// (AGENTS.md: never slice captured tool output at a raw byte offset).
fn list_row_items(line: &str) -> Vec<&str> {
    line.split(['\t'])
        .flat_map(|cell| cell.split("  "))
        .flat_map(|cell| cell.split(','))
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .collect()
}

/// The set of every word that sits at an item position of a **list row** —
/// the second half of "subcommand name sits where a real command-list
/// entry sits", covering entries [`line_start_words`] cannot see (a
/// multi-column command grid: `openssl`'s space-aligned index,
/// `busybox`'s comma-joined applet list).
///
/// A line is a list row when all three hold:
///
/// 1. It breaks into at least [`MIN_LIST_ROW_ITEMS`] items at a tab,
///    column gap (2+ spaces), or comma — never a single space, so prose
///    stays one item regardless of word count.
/// 2. Every item is a single word (a multi-word item marks the row as a
///    description row, not an index row).
/// 3. No item is flag-shaped (leading `-`/`+`), so an option table's
///    one-word description cell can't attest a fabricated subcommand.
///
/// Cannot distinguish a genuine two-column index from a two-item row that
/// happens to carry two bare words, e.g. `  fast    quick`.
///
/// Fixture: `corpus/openssl/*/help.txt`, `corpus/busybox/*/help.txt`,
/// `corpus/tar/1.35/help.txt` ([M-10] replay).
fn list_row_words(raw: &str) -> HashSet<&str> {
    let mut out = HashSet::new();
    for line in raw.lines() {
        let items = list_row_items(line);
        if items.len() < MIN_LIST_ROW_ITEMS {
            continue;
        }
        let is_list_row = items
            .iter()
            .all(|item| item.split_whitespace().count() == 1 && !item.starts_with(['-', '+']));
        if is_list_row {
            out.extend(items);
        }
    }
    out
}

/// Every position in `raw` at which a genuine command-list entry is
/// attested: a line's first token ([`line_start_words`]), an item of a
/// list row ([`list_row_words`]), or a name-shaped token immediately
/// following the tool's own name at the start of a line
/// ([`tool_name_prefixed_row_words`]). A subcommand name occurring at none
/// of these is what this module calls fabricated.
fn attested_name_positions<'a>(raw: &'a str, root_name: &str) -> HashSet<&'a str> {
    let mut set = line_start_words(raw);
    set.extend(list_row_words(raw));
    set.extend(tool_name_prefixed_row_words(raw, root_name));
    set
}

/// The set of every name-shaped token attested by spec §7 Tier B's
/// **headingless invocation table** recognizer
/// (`sections::scan_headingless_invocation_table`): on a line whose first
/// token is `root_name`, every token in the leading run of
/// [`mandible_core::is_command_name_shaped`] tokens after it, up to the
/// recognizer's own two-level cap (`btrfs device add` attests both
/// `device` and `add`).
///
/// Needed because [`line_start_words`] only attests a line's first token,
/// which in `btrfs balance start [options] <path>` is the tool's own name.
/// Reuses the parser's own shape rule rather than a second heuristic.
/// Widening-only: can only reduce reports, never hide a real fabrication.
///
/// Fixture: `corpus/btrfs/*/help.txt`. Spec §7 Tier B.
fn tool_name_prefixed_row_words<'a>(raw: &'a str, root_name: &str) -> HashSet<&'a str> {
    let mut out = HashSet::new();
    if root_name.is_empty() {
        return out;
    }
    for line in raw.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix(root_name) else {
            continue;
        };
        if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
            continue;
        }
        for token in rest.split_whitespace().take(2) {
            let bare = token.trim_end_matches([':', ',', ';']);
            let bare = mandible_extract::help_text::strip_optional_modifier_suffix(bare);
            if is_command_name_shaped(bare) {
                out.insert(bare);
            } else {
                break;
            }
        }
    }
    out
}

// ----------------------------------------------------------------------
// Positional operands: where a real one sits, and the one shape that
// stands in the same slot while meaning the opposite
// ----------------------------------------------------------------------

/// Strip a usage token's notation down to the bare word inside it: only
/// the docopt-style delimiters spec §7 Tier B names, trimmed from the ends
/// only (`[FILE]...` -> `FILE`, `<url>` -> `url`). Leading `-` is kept —
/// [`usage_operands`] tests flag-shape next. `+` is trimmed like `...`,
/// for `sg3-utils`'s one-or-more marker glued onto `>` (`<device>+`).
///
/// Fixture: `corpus/scsi_ready/*/help.txt`.
fn clean_usage_token(token: &str) -> &str {
    token.trim_matches(|c| {
        matches!(
            c,
            '[' | ']' | '<' | '>' | '{' | '}' | '(' | ')' | '|' | '.' | '+'
        )
    })
}

/// True when a usage token is written in *notation* — opened with one of
/// the synopsis grammar's group delimiters — rather than as a bare word.
/// `[OPTION...]`, `<options>` and `[<option>` are notation; `MENU_ENTRY`,
/// `pid` and `STRACE_LOG` are bare.
fn is_bracketed(token: &str) -> bool {
    matches!(
        token.as_bytes().first(),
        Some(b'[') | Some(b'<') | Some(b'{')
    )
}

/// One physical line of a synopsis, and whether it opens with the program's
/// own name.
struct SynopsisLine<'a> {
    text: &'a str,
    /// True for a marker line and a bare-`Usage:`-header continuation;
    /// false for a wrapped remainder (first token is already an operand
    /// or flag). Wrong in the `false` direction silently eats a wrapped
    /// line's first token — `git`'s `<command>` sits one token into its
    /// last line.
    opens_with_program_name: bool,
}

/// True when some physical line of `raw` opens a **labelled** usage block —
/// an ordinary `usage:` marker, or the C `"%s: Usage: ..."` idiom
/// (`starts_with_name_prefixed_usage`) — anywhere in the document.
/// Mirrors `sections::parse_with_profile`'s `labelled_usage_start`
/// (`starts_with_or_marker`/`or:` excluded, same reason: a continuation,
/// not new evidence). Gates whether [`synopsis_lines`] may fall back to
/// an unlabelled synopsis line.
fn has_labelled_usage_start(raw: &str, root_name: &str) -> bool {
    use mandible_extract::help_text::{starts_with_name_prefixed_usage, starts_with_usage_prefix};
    raw.lines().any(|l| {
        let t = l.trim_start();
        starts_with_usage_prefix(t) || starts_with_name_prefixed_usage(t, root_name)
    })
}

/// The physical lines of `raw` that are a synopsis. Five shapes:
///
/// * marker and synopsis on one line (`Usage: tar [OPTION...] [FILE]...`);
/// * that line **wrapped**, remainder on indented lines below, recognized
///   by opening with a group delimiter ([`is_bracketed`]) so ordinary
///   indented prose isn't misread as more synopsis (`git --help`'s
///   `<command>`/`[<args>]` are on its last wrapped line);
/// * a bare `Usage:` header with the synopsis indented beneath (util-linux
///   house style: `setsid`, `wall`, `fsck`, `zoxide`) — every consecutive
///   indented non-empty line is taken, delimiter or not;
/// * the C `"%s: Usage: ..."` idiom (`starts_with_name_prefixed_usage`,
///   `nfsidmap: Usage: nfsidmap [-vh] ...`) and an **unlabelled** synopsis
///   opening with the tool's own name (`looks_like_unlabeled_synopsis_line`,
///   `gh`'s `USAGE` / `gh <command> <subcommand> [flags]`), tried only
///   when [`has_labelled_usage_start`] finds nothing;
/// * a bare own-name line whose notation sits on the **next** line instead
///   (LVM's `vg*`/`lv*`/`pv*` family: `vgextend VG PV ...` then
///   `\t[ -A|--autobackup y|n ]`), recognized via `starts_with_tool_name`
///   plus [`looks_like_bracket_flag_row`] on the next line.
///
/// Marker tests are `mandible_extract::help_text`'s own, re-exported
/// rather than duplicated. The continuation rule is deliberately wider
/// than the tier's: widening can only add to the attested set
/// ([`attested_operand_positions`]), never hide a real fabrication.
///
/// Fixture: `corpus/git/*/help.txt`, `corpus/nfsidmap/*/help.txt`,
/// `corpus/gh/*/help.txt`, `corpus/vgextend/*/help.txt`.
fn synopsis_lines<'a>(raw: &'a str, root_name: &str) -> Vec<SynopsisLine<'a>> {
    use mandible_extract::help_text::{
        looks_like_bracket_flag_row, looks_like_unlabeled_synopsis_line,
        starts_with_name_prefixed_usage, starts_with_or_marker, starts_with_tool_name,
        starts_with_usage_prefix,
    };

    /// What an indented line under an open usage block has to look like
    /// before it counts as more synopsis.
    enum Continuation {
        /// Nothing is open: the last line was neither a marker nor a
        /// continuation.
        None,
        /// A bare `Usage:` header is open — any indented, non-empty line.
        Anything,
        /// A marker line carrying its own synopsis is open — only a wrapped
        /// remainder, i.e. a line opening with a group delimiter.
        NotationOnly,
    }

    let try_unlabelled = !has_labelled_usage_start(raw, root_name);
    let lines: Vec<&str> = raw.lines().collect();
    let mut out = Vec::new();
    let mut open = Continuation::None;
    for (idx, &line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        // LVM's `vg*`/`lv*`/`pv*` family writes a bare invocation line with
        // no notation (`vgextend VG PV ...`); accepted only when the next
        // physical line is unambiguous flag-row evidence.
        let is_bare_own_name_with_flag_row_next = try_unlabelled
            && starts_with_tool_name(trimmed, root_name)
            && lines
                .get(idx + 1)
                .is_some_and(|next| looks_like_bracket_flag_row(next.trim_start()));
        let opens_block = starts_with_usage_prefix(trimmed)
            || starts_with_or_marker(trimmed)
            || starts_with_name_prefixed_usage(trimmed, root_name)
            || (try_unlabelled && looks_like_unlabeled_synopsis_line(trimmed, root_name))
            || is_bare_own_name_with_flag_row_next;
        if opens_block {
            out.push(SynopsisLine {
                text: line,
                opens_with_program_name: true,
            });
            open = match trimmed
                .split_once(':')
                .is_some_and(|(_, rest)| rest.trim().is_empty())
            {
                true => Continuation::Anything,
                false => Continuation::NotationOnly,
            };
            continue;
        }
        let indented = line.starts_with([' ', '\t']) && !trimmed.is_empty();
        let continues = indented
            && match open {
                Continuation::None => false,
                Continuation::Anything => true,
                Continuation::NotationOnly => is_bracketed(trimmed),
            };
        if continues {
            out.push(SynopsisLine {
                text: line,
                opens_with_program_name: matches!(open, Continuation::Anything),
            });
        } else {
            open = Continuation::None;
        }
    }
    out
}

/// One synopsis line read as a sequence of operand slots, plus whether the
/// line writes any literal flag of its own. The program name is dropped
/// when the line opens with it; every remaining non-flag-shaped token is a
/// slot, in source order, paired with whether it was written in notation.
///
/// Unlike the tier's `extract_positionals`, a flag's value token (`-C
/// <path>`) is kept as a slot — attesting an extra token can only reduce
/// reports, and dropping it would false-alarm on `lzgrep`'s `PATTERN`,
/// which sits right after a bracketed `[-e]`.
///
/// `root_name` strips the C `"%s: Usage: ..."` idiom's doubled name
/// (`nfsidmap: Usage: nfsidmap ...`) before the rest of the line is read.
///
/// Fixture: `corpus/lzgrep/*/help.txt`, `corpus/nfsidmap/*/help.txt`.
fn usage_operands<'a>(line: &SynopsisLine<'a>, root_name: &str) -> (Vec<(&'a str, bool)>, bool) {
    use mandible_extract::help_text::{
        starts_with_name_prefixed_usage, starts_with_or_marker, starts_with_usage_prefix,
    };
    let trimmed_full = line.text.trim_start();
    let trimmed = if starts_with_name_prefixed_usage(trimmed_full, root_name) {
        trimmed_full
            .strip_prefix(root_name)
            .and_then(|rest| rest.strip_prefix(": "))
            .unwrap_or(trimmed_full)
    } else {
        trimmed_full
    };
    let mut tokens = trimmed.split_whitespace().peekable();
    if starts_with_usage_prefix(trimmed) || starts_with_or_marker(trimmed) {
        // The marker may be glued to the program name (`usage:git`) or
        // stand alone (`Usage: git`); only consume it when it stands alone.
        if tokens.peek().is_some_and(|t| t.ends_with(':')) {
            tokens.next();
        }
    }
    if line.opens_with_program_name {
        tokens.next();
    }
    let mut slots = Vec::new();
    let mut has_literal_flag = false;
    for token in tokens {
        let cleaned = clean_usage_token(token);
        if cleaned.is_empty() {
            continue;
        }
        if cleaned.starts_with('-') {
            has_literal_flag = true;
            continue;
        }
        slots.push((cleaned, is_bracketed(token)));
    }
    (slots, has_literal_flag)
}

/// The shape rule: which slot of a synopsis line is the tool's **option
/// list** rather than an operand the user supplies (`tar [OPTION...]
/// [FILE]...`, `vim [arguments] [file ..]`).
///
/// Two tests, in fixed order, not a blind union:
///
/// 1. **Vocabulary**: [`mandible_extract::help_text::is_option_list_placeholder`]
///    (`option`/`options`/`flag`/`flags`/`arguments`), scoped to a
///    bracketed slot.
/// 2. **Positional fallback**, only when (1) found nothing on the line: the
///    first slot is the option list when the line writes no literal flag of
///    its own, that slot is bracketed, and at least one further slot
///    follows it.
///
/// Rule 2 must be gated on rule 1, not run alongside it — `gh`'s unlabelled
/// `<command> <subcommand> [flags]` has its placeholder *last*; running
/// both would also flag the genuine first operand `command` as invented.
///
/// Fixture: `corpus/tar/1.35/help.txt`, `corpus/gh/*/help.txt`.
fn option_list_slot<'a>(slots: &[(&'a str, bool)], has_literal_flag: bool) -> HashSet<&'a str> {
    let mut placeholders = HashSet::new();
    for (word, bracketed) in slots {
        if *bracketed && mandible_extract::help_text::is_option_list_placeholder(word) {
            placeholders.insert(*word);
        }
    }
    // Fallback only: gated on the vocabulary loop above finding nothing, so
    // the two rules never both flag the same line (see doc comment).
    if placeholders.is_empty() && !has_literal_flag && slots.len() >= 2 {
        let (first, bracketed) = slots[0];
        if bracketed {
            placeholders.insert(first);
        }
    }
    placeholders
}

/// Every position in `raw` at which a genuine positional operand is
/// attested — the operand half of what [`attested_name_positions`] is for
/// subcommand names.
///
/// Two sources: an operand slot of a synopsis line, minus the option-list
/// slot ([`option_list_slot`]); and a line's first token
/// ([`line_start_words`]), an entry in a declared operand block (argparse's
/// `positional arguments:`). The option-list subtraction applies only to
/// the synopsis set, not to `line_start_words`, to avoid an unmeasured
/// false-alarm class.
fn attested_operand_positions<'a>(raw: &'a str, root_name: &str) -> HashSet<&'a str> {
    let mut out = HashSet::new();
    for line in synopsis_lines(raw, root_name) {
        let (slots, has_literal_flag) = usage_operands(&line, root_name);
        let placeholders = option_list_slot(&slots, has_literal_flag);
        for (word, _) in &slots {
            if !placeholders.contains(word) {
                out.insert(*word);
            }
        }
    }
    out.extend(line_start_words(raw));
    out
}

/// Candidate raw-text spellings for `flag`'s short spelling: the bare `-x`
/// form, plus, when `flag.value_name` is set, the reconstructed
/// single-dash forms `-x<value_name>` and `-x=<value_name>` (GCC/Clang
/// multi-character single-dash flags like `-fdump-scos` are parsed as one
/// `short` char plus the rest glued onto `value_name`, so the bare form
/// never occurs standalone in real output). Fallback only, tried after the
/// bare form fails; cannot introduce a false negative.
///
/// Fixture: `corpus/clang/*/help.txt`, `corpus/lto-dump/*/help.txt`.
fn short_candidates(flag: &Entity, short: char) -> Vec<String> {
    let mut candidates = vec![format!("-{short}")];
    if let Some(value) = &flag.value_name {
        candidates.push(format!("-{short}{value}"));
        candidates.push(format!("-{short}={value}"));
    }
    candidates
}

/// Whether `raw` spells `short` as a member of a **bundled short-flag
/// cluster** — `-C` inside `tmux`'s `[-2CDlNuVv]`, `-#` inside `tcpdump`'s
/// `[-AbdDefhHIJKlLnNOpqStuUvxX#]`. Fallback only, after literal forms
/// fail; reuses `help_text::grammar::parse_bundled_shorts` (the same
/// function the parser splits clusters with), so it cannot manufacture a
/// false negative — it attests exactly what the parser derived.
///
/// Splits on brackets/braces/parens/pipes/commas as well as whitespace,
/// since a synopsis token can glue two bracket groups together with no
/// space (`rpcgen`'s `[-abkCLNTM][-Dname[=value]]`).
///
/// Fixture: `corpus/tmux/*/help.txt`, `corpus/rpcgen/*/help.txt`. K2, §13.1b.
fn occurs_as_a_bundle_member(raw: &str, short: char) -> bool {
    raw.split(|c: char| {
        c.is_whitespace() || matches!(c, '[' | ']' | '{' | '}' | '(' | ')' | '|' | ',')
    })
    .any(|token| {
        mandible_extract::help_text::parse_bundled_shorts(token)
            .is_some_and(|members| members.contains(&short))
    })
}

/// Candidate raw-text spellings for `flag`'s long name: the negatable-
/// boolean bracket convention (this module's doc comment), each also in the
/// value-reconstructed form [`short_candidates`] builds for the short half
/// (`--<long><value>`, `--<long>=<value>`) — needed because
/// [`spelling_occurs`]'s word boundary treats `_` as word-shaped, so a
/// value glued directly onto `long` with no separator (`--perf-
/// no_read_workqueue` stored as `long: "perf-no"`, `value_name:
/// "_read_workqueue"`) would otherwise reject the flag's own raw token.
/// Fallback only, reached after the plain form fails.
///
/// Fixture: `corpus/cryptsetup/*/help.txt`.
fn long_candidates(flag: &Entity, long: &str) -> Vec<String> {
    // Single-dash long options (`qemu -help`, `lto-dump -CC`) hold their
    // bare name in `long` exactly as a `--` option does.
    let dashes = if flag.single_dash() { "-" } else { "--" };
    let mut bases = if flag.negatable() {
        vec![
            format!("{dashes}[no-]{long}"),
            format!("{dashes}[no]{long}"),
            format!("{dashes}{long}"),
        ]
    } else {
        vec![format!("{dashes}{long}")]
    };
    // An abbreviation-bracket spelling (`-r[esolve]`, `--br[ief]`) never
    // occurs as its full name in raw text, only the bracket form does.
    // `Spelling::render` reproduces that form verbatim.
    if let Some(spelling) = flag.long_spelling() {
        if spelling.abbrev.is_some() {
            bases.push(spelling.render());
        }
    }
    let Some(value) = &flag.value_name else {
        return bases;
    };
    let mut candidates = Vec::with_capacity(bases.len() * 3);
    for base in bases {
        candidates.push(format!("{base}{value}"));
        candidates.push(format!("{base}={value}"));
        candidates.push(base);
    }
    candidates
}

/// The display spelling for a fabrication report — `--[no-]foo` for a
/// negatable long flag, `--foo` otherwise, matching
/// `mandible_core::Entity::spelling`'s own convention for the long half.
fn display_long(flag: &Entity, long: &str) -> String {
    let dashes = if flag.single_dash() { "-" } else { "--" };
    if flag.negatable() {
        format!("{dashes}[no-]{long}")
    } else {
        format!("{dashes}{long}")
    }
}

/// Whether `provenance` credits the help-text tier at all — see this
/// module's doc comment on why that's the right scope: `HelpText` and
/// `HelpTextSynopsis` are the only two sources whose spellings are ever
/// expected to occur in captured `--help`/`-h` prose; every other source
/// is structural and legitimately silent there.
fn is_help_text_sourced(provenance: &Provenance) -> bool {
    provenance
        .sources
        .iter()
        .any(|s| matches!(s, Source::HelpText | Source::HelpTextSynopsis))
}

/// One thing the help-text tier emitted that does not occur in the tool's
/// own raw captured text.
pub struct Fabrication {
    /// Space-separated path to the node that carries this fabrication —
    /// the *parent* node for a subcommand-name fabrication (the name
    /// itself isn't part of the tree's own path since it wasn't a real
    /// node), the owning node for a flag-spelling fabrication.
    pub path: String,
    pub kind: FabricationKind,
    /// The specific spelling or name that failed to attest, for display.
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FabricationKind {
    /// A `CommandNode::name` with no line-start-ish occurrence anywhere in
    /// the raw text — [M-10]'s exact shape.
    Subcommand,
    /// An `Entity` short or long spelling with no boundary-respecting
    /// occurrence anywhere in the raw text.
    Flag,
    /// A `Positional` name that occurs at no position a real operand
    /// occupies — nowhere in a declared operand block, and nowhere in a
    /// synopsis except in the slot that names the tool's own option list
    /// ([`option_list_slot`]).
    Positional,
}

/// The result of analyzing one tool.
pub struct ExistenceReport {
    pub fabrications: Vec<Fabrication>,
}

impl ExistenceReport {
    pub fn fabrication_count(&self) -> usize {
        self.fabrications.len()
    }
}

fn check_flags(node: &CommandNode, path: &str, raw: &str, out: &mut Vec<Fabrication>) {
    for flag in node.flags() {
        if !is_help_text_sourced(&flag.provenance) {
            continue;
        }
        if let Some(short) = flag.short() {
            let spelling = format!("-{short}");
            let candidates = short_candidates(flag, short);
            let attested = candidates.iter().any(|c| spelling_occurs(raw, c))
                || occurs_as_a_bundle_member(raw, short);
            if !attested {
                out.push(Fabrication {
                    path: path.to_string(),
                    kind: FabricationKind::Flag,
                    name: spelling,
                });
            }
        }
        if let Some(long) = flag.long() {
            let candidates = long_candidates(flag, long);
            if !candidates.iter().any(|c| spelling_occurs(raw, c)) {
                out.push(Fabrication {
                    path: path.to_string(),
                    kind: FabricationKind::Flag,
                    name: display_long(flag, long),
                });
            }
        }
    }
}

/// The positional half of the rule: every help-text-sourced operand this
/// node records must sit at a position a real operand occupies
/// ([`attested_operand_positions`]).
///
/// Scoped by provenance exactly as [`check_flags`] is, and for the same
/// reason — a completion script or a `__complete` protocol reply names
/// operands the tool's prose never prints.
fn check_positionals(
    node: &CommandNode,
    path: &str,
    operands: &HashSet<&str>,
    out: &mut Vec<Fabrication>,
) {
    for positional in node.positionals() {
        if !is_help_text_sourced(&positional.provenance) {
            continue;
        }
        if !operands.contains(positional.primary_name()) {
            out.push(Fabrication {
                path: path.to_string(),
                kind: FabricationKind::Positional,
                name: positional.primary_name().to_string(),
            });
        }
    }
}

fn walk(
    node: &CommandNode,
    path: &str,
    raw: &str,
    attested: &HashSet<&str>,
    operands: &HashSet<&str>,
    out: &mut Vec<Fabrication>,
) {
    check_flags(node, path, raw, out);
    check_positionals(node, path, operands, out);
    for child in &node.subcommands {
        if is_help_text_sourced(&child.provenance) && !attested.contains(child.name.as_str()) {
            out.push(Fabrication {
                path: path.to_string(),
                kind: FabricationKind::Subcommand,
                name: child.name.clone(),
            });
        }
        let child_path = format!("{path} {}", child.name);
        walk(child, &child_path, raw, attested, operands, out);
    }
}

/// Analyze `root`'s help-text-sourced subcommand names and flag spellings
/// against `raw` (the same raw `--help`/`-h` text
/// [`crate::misattribution::RecordingProbe::root_help_text`] hands back)
/// for existence: does each one occur literally in the tool's own output,
/// or was it invented?
///
/// The root node's own name is never checked — it is the literal argv0 a
/// user typed, structurally attested by construction
/// (`mandible_extract::runner::Runner::extract_full`'s own `NodeHints`),
/// never a candidate a parser could have fabricated. Its *flags* are
/// checked like any other node's.
pub fn detect(raw: &str, root: &CommandNode) -> ExistenceReport {
    let attested = attested_name_positions(raw, &root.name);
    let operands = attested_operand_positions(raw, &root.name);
    let mut fabrications = Vec::new();
    walk(
        root,
        &root.name,
        raw,
        &attested,
        &operands,
        &mut fabrications,
    );
    ExistenceReport { fabrications }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::misattribution::RecordingProbe;
    use mandible_core::{Provenance, Source};

    fn help_text_flag(short: Option<char>, long: Option<&str>, negatable: bool) -> Entity {
        Entity::flag_spelled(
            short,
            long.map(str::to_string),
            false,
            negatable,
            Provenance::single(Source::HelpText),
        )
    }

    fn help_text_node(name: &str) -> CommandNode {
        CommandNode::new(name, Provenance::single(Source::HelpText))
    }

    // --- spelling_occurs -----------------------------------------------

    #[test]
    fn spelling_occurs_matches_a_bare_flag() {
        assert!(spelling_occurs(
            "  -v, --verbose  be verbose\n",
            "--verbose"
        ));
        assert!(spelling_occurs("  -v, --verbose  be verbose\n", "-v"));
    }

    #[test]
    fn spelling_occurs_matches_a_value_spec_glued_on_with_no_space() {
        // git's own real shape: `--gpg-sign[=<keyid>]`.
        assert!(spelling_occurs(
            "  -S, --gpg-sign[=<keyid>]\n              GPG-sign commits\n",
            "--gpg-sign"
        ));
    }

    #[test]
    fn spelling_occurs_rejects_a_strict_prefix_of_a_longer_flag() {
        // `--foo` must not match inside the unrelated, longer `--foobar`.
        assert!(!spelling_occurs("  --foobar   does a thing\n", "--foo"));
    }

    #[test]
    fn spelling_occurs_rejects_a_short_flag_embedded_in_a_long_ones_dashes() {
        // `-g` must not match the `-g` substring sitting inside `--gpg-sign`.
        assert!(!spelling_occurs("  --gpg-sign  GPG-sign commits\n", "-g"));
    }

    #[test]
    fn spelling_occurs_false_when_nothing_matches() {
        assert!(!spelling_occurs("  -v, --verbose  be verbose\n", "--quiet"));
    }

    // --- negatable / value-stripping candidates -------------------------

    #[test]
    fn negatable_long_matches_its_real_bracketed_raw_spelling() {
        // `--[no-]source <tree-ish>`, git's own real convention.
        let raw = "  -s, --[no-]source <tree-ish>\n         use tree-ish as source\n";
        let flag = help_text_flag(Some('s'), Some("source"), true);
        let candidates = long_candidates(&flag, "source");
        assert!(candidates.iter().any(|c| spelling_occurs(raw, c)));
    }

    #[test]
    fn non_negatable_long_does_not_get_a_bracketed_candidate_falsely_matching() {
        // A flag stored as non-negatable must not be satisfied merely
        // because *some* unrelated negatable flag's brackets happen to
        // appear elsewhere in the same raw text.
        let raw = "  --[no-]other   toggles other\n";
        let flag = help_text_flag(None, Some("source"), false);
        let candidates = long_candidates(&flag, "source");
        assert!(!candidates.iter().any(|c| spelling_occurs(raw, c)));
    }

    // --- abbreviation-bracket candidates ---------------------------------

    /// `ip --help`'s real `-r[esolve]` row: the entity's `long()` is
    /// `"resolve"`, the full word — which never occurs in the raw text on
    /// its own, only the bracket form `-r[esolve]` does. Before
    /// `long_candidates` learned to reconstruct it, this exact shape
    /// reported every abbreviation-bracket spelling in the fleet as
    /// fabricated (measured: `existence_fabrication_tools` jumped from 52
    /// to 75 on a full sweep the moment multi-spelling emission started
    /// producing `abbrev` spellings).
    #[test]
    fn long_candidates_reconstructs_an_abbreviation_bracket_spelling() {
        use mandible_core::{Dashes, Spelling};
        let mut flag = Entity::new(mandible_core::EntityKind::Flag, Provenance::default());
        flag.spellings = vec![Spelling {
            name: "resolve".to_string(),
            dashes: Dashes::Single,
            negatable: false,
            abbrev: Some(1),
        }];
        let raw = "OPTIONS := { -V[ersion] | -s[tatistics] | -d[etails] | -r[esolve] |\n";
        let candidates = long_candidates(&flag, "resolve");
        assert!(
            candidates.iter().any(|c| spelling_occurs(raw, c)),
            "{candidates:?}"
        );
    }

    // --- short-flag reconstruction (GCC/Clang single-dash flags) ---------

    /// `gcc`'s (and `lto-dump`'s, a GCC LTO plugin sharing the same
    /// front-end option grammar) real, byte-exact line for its `-fdump-scos`
    /// flag (`corpus`'s own real-tool capture policy: exact strings, not
    /// paraphrased ones). Before [`short_candidates`] existed, this exact
    /// shape drove this task's own real regression: comparing only the bare
    /// `-f` (which never occurs standalone anywhere in `lto-dump --help`'s
    /// real output — every one of its hundreds of `-f...` options glues
    /// more identifier characters directly on) reported `lto-dump` at 848
    /// fabrications and `clang` at 710, both entirely false — see this
    /// module's doc comment.
    const GCC_SINGLE_DASH_LINE: &str = "  -fdump-scos                 \t\t[available in Ada]\n";

    #[test]
    fn short_candidates_reconstructs_a_glued_single_dash_multi_char_flag() {
        let flag = {
            let mut f = help_text_flag(Some('f'), None, false);
            f.value_name = Some("dump-scos".to_string());
            f
        };
        let candidates = short_candidates(&flag, 'f');
        assert!(candidates.contains(&"-fdump-scos".to_string()));
        assert!(candidates
            .iter()
            .any(|c| spelling_occurs(GCC_SINGLE_DASH_LINE, c)));
    }

    // --- bundled short-flag cluster members ------------------------------

    /// `tmux`'s real usage line, byte-exact from
    /// `corpus/tmux/audit-seed2/help.stderr.txt`.
    const TMUX_USAGE: &str = "usage: tmux [-2CDlNuVv] [-c shell-command] [-f file] [-L socket-name]\n            [-S socket-path] [-T features] [command [flags]]\n";

    /// The half that motivates [`occurs_as_a_bundle_member`]: seven of
    /// `tmux`'s eight cluster members occur *nowhere* on their own, so the
    /// literal check alone would report all seven as invented the moment
    /// the grammar started emitting them as flags.
    #[test]
    fn a_cluster_members_bare_spelling_does_not_occur_on_its_own() {
        for member in ['C', 'D', 'l', 'N', 'u', 'V'] {
            assert!(
                !spelling_occurs(TMUX_USAGE, &format!("-{member}")),
                "-{member} must not occur standalone in tmux's real usage line"
            );
        }
    }

    /// ...and the cluster check attests every one of them, because the raw
    /// text really does write them — as one token.
    #[test]
    fn every_cluster_member_is_attested_by_its_own_cluster() {
        for member in "2CDlNuVv".chars() {
            assert!(
                occurs_as_a_bundle_member(TMUX_USAGE, member),
                "-{member} must be attested by tmux's own [-2CDlNuVv]"
            );
        }
        // tcpdump's real cluster, including its non-alphanumeric member.
        let raw = "Usage: tcpdump [-AbdDefhHIJKlLnNOpqStuUvxX#] [ -B size ]\n";
        for member in "AbdDefhHIJKlLnNOpqStuUvxX#".chars() {
            assert!(occurs_as_a_bundle_member(raw, member), "-{member}");
        }
    }

    /// End to end: the whole split tree over the real capture reports zero
    /// fabrications. Without the cluster check this is seven.
    #[test]
    fn detect_does_not_flag_the_members_of_a_real_cluster() {
        let mut root = help_text_node("tmux");
        for member in "2CDlNuVv".chars() {
            root.entities
                .push(help_text_flag(Some(member), None, false));
        }
        for (short, value) in [('c', "shell-command"), ('f', "file"), ('T', "features")] {
            let mut flag = help_text_flag(Some(short), None, false);
            flag.value_name = Some(value.to_string());
            root.entities.push(flag);
        }
        let report = detect(TMUX_USAGE, &root);
        assert_eq!(
            report.fabrication_count(),
            0,
            "tmux's real flags must not be flagged: {:?}",
            report
                .fabrications
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
    }

    /// `rpcgen`'s real usage line, byte-exact: the cluster and the next
    /// option group are one whitespace token, which is why the tokenizer
    /// splits on brackets. Found on the fleet sweep, not by reasoning —
    /// this was the one tool the first version of this check still reported,
    /// for `-k`, a spelling that appears nowhere else in `rpcgen --help`.
    const RPCGEN_USAGE: &str =
        "\trpcgen [-abkCLNTM][-Dname[=value]] [-i size] [-I [-K seconds]] [-Y path] infile\n";

    #[test]
    fn a_cluster_glued_to_the_next_bracket_group_still_attests_its_members() {
        for member in "abkCLNTM".chars() {
            assert!(
                occurs_as_a_bundle_member(RPCGEN_USAGE, member),
                "-{member} must be attested by rpcgen's own [-abkCLNTM]"
            );
        }
        // The half that made this a real finding: `-k` occurs nowhere in
        // rpcgen's output except inside that cluster.
        assert!(!spelling_occurs(RPCGEN_USAGE, "-k"));
        // ...and the neighbouring group is still not a cluster, so it
        // vouches for nothing.
        for member in ['D', 'n', 'a', 'm', 'e', 'v'] {
            assert!(!mandible_extract::help_text::parse_bundled_shorts("-Dname")
                .is_some_and(|m| m.contains(&member)));
        }
    }

    /// The check must not become a blanket amnesty for short flags. A
    /// genuinely invented spelling is still reported when the tool's text
    /// carries a cluster that does not contain it, and a token that is not
    /// a cluster by the grammar's own rule attests nothing at all.
    #[test]
    fn a_genuinely_invented_short_flag_is_still_reported_beside_a_cluster() {
        let mut root = help_text_node("tmux");
        root.entities.push(help_text_flag(Some('Q'), None, false));
        let report = detect(TMUX_USAGE, &root);
        assert_eq!(report.fabrication_count(), 1);
        assert_eq!(report.fabrications[0].name, "-Q");
        // A word-shaped token is not a cluster, so it vouches for nothing:
        // `-pass-exit-codes` must not attest `-a`, `-s`, `-e`...
        for member in ['a', 's', 'e', 'x', 'i', 't'] {
            assert!(!occurs_as_a_bundle_member(
                "  -pass-exit-codes   Exit with highest error code\n",
                member
            ));
        }
    }

    #[test]
    fn bare_short_alone_does_not_occur_in_gccs_real_single_dash_line() {
        // The other half of the regression: confirms *why* the bare form
        // alone was failing, not just that the reconstructed form passes.
        assert!(!spelling_occurs(GCC_SINGLE_DASH_LINE, "-f"));
    }

    #[test]
    fn detect_does_not_flag_gccs_real_single_dash_multi_char_flag() {
        let mut root = help_text_node("lto-dump");
        let mut flag = help_text_flag(Some('f'), None, false);
        flag.value_name = Some("dump-scos".to_string());
        root.entities.push(flag);
        let report = detect(GCC_SINGLE_DASH_LINE, &root);
        assert_eq!(
            report.fabrication_count(),
            0,
            "gcc's own real -fdump-scos must not be flagged: {:?}",
            report
                .fabrications
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
    }

    /// `lto-dump --help`'s real `--param=` table, byte-exact
    /// (`  --param=lazy-modules=       \t\t[available in C++]`): the long-
    /// flag half of the same real capture, confirming value stripping
    /// (already covered generically above) holds for this exact shape too
    /// — this is the line the maintainer's own first hypothesis named
    /// specifically.
    #[test]
    fn detect_does_not_flag_lto_dumps_real_param_shape() {
        let raw = "  --param=lazy-modules=       \t\t[available in C++]\n";
        let mut root = help_text_node("lto-dump");
        let mut flag = help_text_flag(None, Some("param"), false);
        flag.value_name = Some("lazy-modules=".to_string());
        root.entities.push(flag);
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 0);
    }

    #[test]
    fn short_candidates_also_covers_the_equals_separated_reconstruction() {
        // A synthetic shape (not yet measured against a real tool, unlike
        // the two above) covering `try_value`'s other branch: if the
        // grammar's `=VALUE` arm ever fires for a single-dash multi-char
        // flag (consuming and discarding a leading `=` before storing the
        // rest as `value_name`), the reconstructed spelling needs the `=`
        // put back — `short_candidates`'s second fallback candidate.
        let raw = "  -c=foo   does a thing\n";
        let flag = {
            let mut f = help_text_flag(Some('c'), None, false);
            f.value_name = Some("foo".to_string());
            f
        };
        let candidates = short_candidates(&flag, 'c');
        assert!(candidates.contains(&"-c=foo".to_string()));
        assert!(candidates.iter().any(|c| spelling_occurs(raw, c)));
    }

    #[test]
    fn detect_still_flags_a_genuinely_fabricated_short_flag_with_a_value_name() {
        // The reconstruction fallback must not blanket-suppress every
        // short flag that happens to carry a `value_name` — only one whose
        // *reconstructed* spelling genuinely occurs. `-z` with a value_name
        // that appears nowhere in the raw text must still be caught.
        let raw = "  -fdump-scos                 \t\t[available in Ada]\n";
        let mut root = help_text_node("t");
        let mut flag = help_text_flag(Some('z'), None, false);
        flag.value_name = Some("totally-invented".to_string());
        root.entities.push(flag);
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 1);
        assert_eq!(report.fabrications[0].name, "-z");
    }

    // --- long-flag value reconstruction ----------------------------------

    /// `cryptsetup --help`'s real line, byte-exact. The parser stores this
    /// as `long: "perf-no"`, `value_name: "_read_workqueue"`, so the bare
    /// `--perf-no` is followed in the raw text by `_` — word-shaped, and
    /// therefore rejected by [`spelling_occurs`]'s boundary until
    /// [`long_candidates`] learned to put the value back.
    const CRYPTSETUP_UNDERSCORE_LINE: &str =
        "      --perf-no_read_workqueue          Bypass dm-crypt workqueue and process\n";

    #[test]
    fn long_candidates_reconstructs_a_glued_underscore_value() {
        let mut flag = help_text_flag(None, Some("perf-no"), false);
        flag.value_name = Some("_read_workqueue".to_string());
        let candidates = long_candidates(&flag, "perf-no");
        assert!(candidates.contains(&"--perf-no_read_workqueue".to_string()));
        assert!(candidates
            .iter()
            .any(|c| spelling_occurs(CRYPTSETUP_UNDERSCORE_LINE, c)));
    }

    #[test]
    fn bare_long_alone_does_not_occur_in_cryptsetups_real_line() {
        // The other half of the regression, stated the same way the GCC
        // short-flag pair is: confirms *why* the bare form was failing.
        assert!(!spelling_occurs(CRYPTSETUP_UNDERSCORE_LINE, "--perf-no"));
    }

    #[test]
    fn detect_does_not_flag_cryptsetups_real_underscore_flag() {
        let mut root = help_text_node("cryptsetup");
        let mut flag = help_text_flag(None, Some("perf-no"), false);
        flag.value_name = Some("_read_workqueue".to_string());
        root.entities.push(flag);
        let report = detect(CRYPTSETUP_UNDERSCORE_LINE, &root);
        assert_eq!(report.fabrication_count(), 0);
    }

    #[test]
    fn detect_still_flags_a_genuinely_fabricated_long_flag_with_a_value_name() {
        // The mirror of `detect_still_flags_a_genuinely_fabricated_short_
        // flag_with_a_value_name`: the reconstruction fallback must not
        // blanket-suppress every long flag that carries a `value_name`,
        // only one whose reconstructed spelling genuinely occurs.
        let mut root = help_text_node("t");
        let mut flag = help_text_flag(None, Some("perf-no"), false);
        flag.value_name = Some("_totally_invented".to_string());
        root.entities.push(flag);
        let report = detect(CRYPTSETUP_UNDERSCORE_LINE, &root);
        assert_eq!(report.fabrication_count(), 1);
        assert_eq!(report.fabrications[0].name, "--perf-no");
    }

    /// A single-dash long option is searched for with one dash, because
    /// that is how the tool spells it. Left at two, the repeated-character
    /// repair (`help_text::sections::repair_repeated_character_flags`)
    /// would have this oracle report `lto-dump`'s perfectly real `-CC` and
    /// `-MM` as inventions — a correctly-parsed flag counted as a
    /// fabrication, which is the exact false positive this oracle's zero is
    /// meant to be trustworthy about. Measured before the fix: `lto-dump`
    /// went from 10 fabrications to 12, and both were this.
    #[test]
    fn a_single_dash_long_option_is_searched_for_with_one_dash() {
        let raw = "    -v      verbose messages\n    -vv     more verbose messages\n";
        let mut root = help_text_node("t");
        let mut flag = help_text_flag(None, Some("vv"), false);
        flag.spellings = vec![mandible_core::Spelling::single_dash("vv")];
        root.entities.push(flag);
        assert_eq!(detect(raw, &root).fabrication_count(), 0);
        // ...and a genuinely invented one is still reported, with the
        // single-dash spelling it claims to have.
        let mut root = help_text_node("t");
        let mut flag = help_text_flag(None, Some("qq"), false);
        flag.spellings = vec![mandible_core::Spelling::single_dash("qq")];
        root.entities.push(flag);
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 1);
        assert_eq!(report.fabrications[0].name, "-qq");
    }

    #[test]
    fn a_negatable_long_still_gets_its_bracketed_forms_when_it_has_a_value() {
        let mut flag = help_text_flag(None, Some("source"), true);
        flag.value_name = Some("<tree-ish>".to_string());
        let candidates = long_candidates(&flag, "source");
        assert!(candidates.contains(&"--[no-]source".to_string()));
        assert!(candidates.contains(&"--[no-]source<tree-ish>".to_string()));
        assert!(candidates.contains(&"--source".to_string()));
    }

    // --- line-start-ish subcommand rule ----------------------------------

    #[test]
    fn line_start_words_finds_gits_real_indented_command_list() {
        let raw = "start a working area (see also: git help tutorial)\n   clone     Clone a repository into a new directory\n   init      Create an empty Git repository or reinitialize an existing one\n";
        let words = line_start_words(raw);
        assert!(words.contains("clone"));
        assert!(words.contains("init"));
        assert!(!words.contains("area"));
    }

    /// binutils `ar`'s real command table, byte-exact. Each row glues the
    /// command's optional modifier groups onto its name, so the row's first
    /// token is `m[ab]` while the tree stores `m`. Both must be attested,
    /// or five real `ar` commands get reported as invented — measured: the
    /// fleet sweep went from 0 to 5 fabrications on exactly this.
    #[test]
    fn line_start_words_attests_a_command_carrying_its_modifier_groups() {
        let raw = " commands:\n  d            - delete file(s) from the archive\n  m[ab]        - move file(s) in the archive\n  r[ab][f][u]  - replace existing or insert new file(s) into the archive\n";
        let words = line_start_words(raw);
        assert!(words.contains("d"));
        assert!(words.contains("m"), "m[ab] must attest the command m");
        assert!(words.contains("r"), "r[ab][f][u] must attest the command r");
        // The whole token stays attested too — this is additive.
        assert!(words.contains("m[ab]"));
        // A bracket-led token names no command and must attest nothing new.
        let modifiers = line_start_words("  [a]          - put file(s) after [member-name]\n");
        assert!(!modifiers.contains(""));
        assert!(modifiers.contains("[a]"));
    }

    // --- tool_name_prefixed_row_words (headingless invocation tables) ---

    /// btrfs's real shape: on a line starting with the tool's own name,
    /// both the direct-child word and its grandchild word must be
    /// attested — this is what keeps every node the new headingless-
    /// invocation-table recognizer produces from being reported as
    /// invented by this detector.
    #[test]
    fn tool_name_prefixed_row_words_attests_both_levels_of_a_btrfs_row() {
        let raw = "    btrfs balance start [options] <path>\n        Balance chunks\n";
        let words = tool_name_prefixed_row_words(raw, "btrfs");
        assert!(words.contains("balance"));
        assert!(words.contains("start"));
        // Placeholder/bracket tokens past the run are never attested.
        assert!(!words.contains("options"));
        assert!(!words.contains("path"));
    }

    /// A line that does not start with the tool's own name contributes
    /// nothing — this rule is scoped exactly to the recognizer's own
    /// evidence, never "any line naming this word anywhere".
    #[test]
    fn tool_name_prefixed_row_words_ignores_unrelated_lines() {
        let raw = "  some other prog balance start\n";
        let words = tool_name_prefixed_row_words(raw, "btrfs");
        assert!(words.is_empty());
    }

    /// End-to-end against btrfs's real, committed `corpus/btrfs/
    /// audit-seed2/help.txt`: a tree shaped exactly as
    /// `scan_headingless_invocation_table` produces it (two levels,
    /// `device` -> `add`) must report zero fabrications — the whole point
    /// of this fix, and the regression this addendum exists to prevent
    /// (before it, every node from this recognizer was a false positive
    /// here).
    #[test]
    fn detect_does_not_flag_a_real_headingless_invocation_table_child() {
        let raw = include_str!("../../corpus/btrfs/audit-seed2/help.txt");
        let mut root = help_text_node("btrfs");
        let mut device = help_text_node("device");
        device.subcommands.push(help_text_node("add"));
        root.subcommands.push(device);
        let report = detect(raw, &root);
        assert_eq!(
            report.fabrication_count(),
            0,
            "a real headingless-invocation-table child must not be reported as invented: {:?}",
            report
                .fabrications
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
    }

    /// The addendum must not blunt the detector: a genuinely fabricated
    /// name that merely happens to sit on a line starting with the tool's
    /// own name-shaped root word must still be caught if it isn't actually
    /// one of the attested run's tokens.
    #[test]
    fn tool_name_prefixed_row_words_does_not_over_attest_past_the_run() {
        let raw = "    btrfs balance start [options] <path>\n        Balance chunks\n";
        let words = tool_name_prefixed_row_words(raw, "btrfs");
        assert!(
            !words.contains("chunks"),
            "a description word must never be attested by this rule"
        );
    }

    #[test]
    fn line_start_words_excludes_a_mid_line_word() {
        let raw = "  -k, --keep-old-files       don't replace existing files when extracting,\n                             treat them as errors\n";
        let words = line_start_words(raw);
        // "errors" is the *last* word of a wrapped continuation line, not
        // its first — must not register as a line-start word.
        assert!(!words.contains("errors"));
        // "treat" is that continuation line's own first word, and *does*
        // register — this module's rule is honestly "first word of a
        // line," not "belongs to a real command-list section"; see the
        // module doc comment on what's left unverified.
        assert!(words.contains("treat"));
    }

    // --- list rows (the multi-column / comma-joined index) ---------------

    /// `openssl --help`'s real command index, byte-exact from the fleet
    /// capture: four commands per line, column-aligned, only the first of
    /// them at a line start. 112 of the 656 fleet-wide fabrications were
    /// this one layout.
    const OPENSSL_GRID: &str = "Standard commands\nasn1parse         ca                ciphers           cmp\ncms               crl               crl2pkcs7         dgst\n";

    /// `busybox --help`'s real applet index, byte-exact: comma-joined and
    /// wrapped, trailing comma included. 247 of the 656.
    const BUSYBOX_LIST: &str = "Currently defined functions:\n    [, [[, acpid, adjtimex, ar, arch, arp, arping, ascii, ash, awk,\n    base64, basename, bc, bunzip2, busybox, bzcat, bzip2, cal, cat,\n";

    #[test]
    fn list_row_words_finds_a_column_aligned_command_grid() {
        let words = list_row_words(OPENSSL_GRID);
        for name in ["asn1parse", "ca", "ciphers", "cmp", "crl2pkcs7", "dgst"] {
            assert!(words.contains(name), "missing {name}: {words:?}");
        }
        // The section heading is prose on its own line — two words joined
        // by a single space, which is not an item separator.
        assert!(!words.contains("Standard"));
        assert!(!words.contains("commands"));
    }

    #[test]
    fn list_row_words_finds_a_comma_joined_applet_list() {
        let words = list_row_words(BUSYBOX_LIST);
        for name in ["acpid", "adjtimex", "arping", "base64", "bunzip2", "cat"] {
            assert!(words.contains(name), "missing {name}: {words:?}");
        }
        assert!(!words.contains("Currently"));
        assert!(!words.contains("defined"));
    }

    #[test]
    fn detect_does_not_flag_opensslfs_real_grid_entries() {
        let mut root = help_text_node("openssl");
        for name in ["ca", "ciphers", "cmp", "crl2pkcs7", "dgst"] {
            root.subcommands.push(help_text_node(name));
        }
        let report = detect(OPENSSL_GRID, &root);
        assert_eq!(
            report.fabrication_count(),
            0,
            "openssl's own real command grid must attest: {:?}",
            report
                .fabrications
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn detect_does_not_flag_busyboxs_real_applet_list() {
        let mut root = help_text_node("busybox");
        for name in ["acpid", "adjtimex", "arping", "bunzip2", "cat"] {
            root.subcommands.push(help_text_node(name));
        }
        let report = detect(BUSYBOX_LIST, &root);
        assert_eq!(report.fabrication_count(), 0);
    }

    // --- list rows: the true positives that must survive the loosening ---

    #[test]
    fn a_prose_line_is_never_a_list_row_however_many_words_it_has() {
        // [M-10]'s own shape. Single spaces are not item separators, so
        // this whole continuation line is one item and the line breaks
        // into fewer than `MIN_LIST_ROW_ITEMS`.
        let raw = "                             treat them as errors\n";
        let words = list_row_words(raw);
        assert!(words.is_empty(), "{words:?}");
    }

    #[test]
    fn detect_still_flags_prose_words_from_a_wrapped_continuation_line() {
        let raw = "  -k, --keep-old-files       don't replace existing files when extracting,\n                             treat them as errors\n";
        let mut root = help_text_node("tar");
        for name in ["them", "as", "errors"] {
            root.subcommands.push(help_text_node(name));
        }
        let report = detect(raw, &root);
        assert_eq!(
            report.fabrication_count(),
            3,
            "every mid-prose word must still be caught: {:?}",
            report
                .fabrications
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
    }

    /// The description column of an *option* table must not attest a
    /// subcommand, even when that description happens to be one word.
    /// Rule 3 (no flag-shaped item) is the one doing the work here: the
    /// row names `-v`, so it is an option row, not an index row.
    #[test]
    fn an_option_row_never_attests_its_own_one_word_description() {
        let raw = "  -v, --verbose      verbose\n  -q, --quiet        silent\n";
        let words = list_row_words(raw);
        assert!(!words.contains("verbose"), "{words:?}");
        assert!(!words.contains("silent"), "{words:?}");

        let mut root = help_text_node("t");
        root.subcommands.push(help_text_node("silent"));
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 1);
        assert_eq!(report.fabrications[0].name, "silent");
    }

    #[test]
    fn a_row_carrying_any_multi_word_item_is_not_a_list_row() {
        // A genuine two-column *description* table. `clone` is still
        // attested — by the first-token rule, which has not changed — but
        // nothing from the right-hand column is.
        let raw = "   clone     Clone a repository into a new directory\n";
        let words = list_row_words(raw);
        assert!(words.is_empty(), "{words:?}");
        assert!(line_start_words(raw).contains("clone"));
    }

    /// A two-column table whose right-hand cell is a single word must not
    /// attest that word. This is the shape `MIN_LIST_ROW_ITEMS = 2` let
    /// through: an `ENVIRONMENT` section and a one-word-description index
    /// are both indistinguishable from a real command grid by every other
    /// rule here, and only their *width* separates them.
    #[test]
    fn a_two_column_table_never_attests_its_right_hand_column() {
        let env = "  TMPDIR    directory\n  EDITOR    editor\n";
        let words = list_row_words(env);
        assert!(!words.contains("directory"), "{words:?}");
        assert!(!words.contains("editor"), "{words:?}");

        let index = "  add       adds\n  remove    removes\n";
        let words = list_row_words(index);
        assert!(!words.contains("adds"), "{words:?}");
        assert!(!words.contains("removes"), "{words:?}");

        // ...and the fabrication that hid behind it is reported again.
        let mut root = help_text_node("t");
        root.subcommands.push(help_text_node("editor"));
        let report = detect(env, &root);
        assert_eq!(report.fabrication_count(), 1);
        assert_eq!(report.fabrications[0].name, "editor");
    }

    /// The left-hand column of such a table is still attested, by the
    /// unchanged first-token rule — tightening the width threshold must
    /// not cost a real index entry that happens to sit in a narrow table.
    #[test]
    fn a_two_column_tables_left_column_is_still_attested() {
        let index = "  add       adds\n  remove    removes\n";
        let starts = line_start_words(index);
        assert!(starts.contains("add"));
        assert!(starts.contains("remove"));
    }

    #[test]
    fn a_single_item_line_is_not_a_list_row() {
        let raw = "        solo\n";
        assert!(list_row_words(raw).is_empty());
    }

    /// The [M-10] replay, re-run against `tar`'s real corpus capture with
    /// the loosened rule in place: the whole point of the loosening is that
    /// it must not reach real prose, and `tar --help` is nothing but
    /// option rows and wrapped prose.
    #[test]
    fn list_rows_admit_nothing_from_tars_real_corpus_text() {
        let raw = include_str!("../../corpus/tar/1.35/help.txt");
        let admitted = list_row_words(raw);
        let already = line_start_words(raw);
        let newly: Vec<&&str> = admitted.iter().filter(|w| !already.contains(*w)).collect();
        assert!(
            newly.is_empty(),
            "the list-row rule must admit no new name position in tar's own text: {newly:?}"
        );
    }

    // --- detect: flags ----------------------------------------------------

    #[test]
    fn detect_flags_a_fabricated_flag_spelling() {
        let raw = "  -v, --verbose  be verbose\n";
        let mut root = help_text_node("t");
        root.entities
            .push(help_text_flag(None, Some("quiet"), false));
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 1);
        assert_eq!(report.fabrications[0].kind, FabricationKind::Flag);
        assert_eq!(report.fabrications[0].name, "--quiet");
    }

    #[test]
    fn detect_does_not_flag_a_real_flag_with_a_stripped_value_spec() {
        let raw = "  -S, --gpg-sign[=<keyid>]  GPG-sign commits\n";
        let mut root = help_text_node("git");
        let mut flag = help_text_flag(Some('S'), Some("gpg-sign"), false);
        flag.value_name = Some("<keyid>".to_string());
        root.entities.push(flag);
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 0);
    }

    #[test]
    fn detect_does_not_flag_a_negatable_flag_against_its_bracketed_raw_form() {
        let raw = "  -s, --[no-]source <tree-ish>\n         use tree-ish as source\n";
        let mut root = help_text_node("git");
        root.entities
            .push(help_text_flag(Some('s'), Some("source"), true));
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 0);
    }

    #[test]
    fn detect_does_not_flag_a_short_and_long_pair_from_separate_alias_rows() {
        // `mandible_core::merge::pair_aliases`'s own real shape: `-R` and
        // `--repo` arrive as two rows with an identical description and
        // get unified into one `Entity` carrying both spellings. Neither
        // needs to sit next to the other in the raw text.
        let raw = "  -R  Select another repository\n  --repo  Select another repository (long form documented on its own line)\n";
        let mut root = help_text_node("gh");
        root.entities
            .push(help_text_flag(Some('R'), Some("repo"), false));
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 0);
    }

    #[test]
    fn detect_ignores_flags_not_sourced_from_help_text() {
        let raw = "  -v, --verbose  be verbose\n";
        let mut root = help_text_node("t");
        let invented = Entity::flag_long(
            "totally-invented",
            Provenance::single(Source::KnownSpec {
                provider: "carapace".to_string(),
            }),
        );
        root.entities.push(invented);
        let report = detect(raw, &root);
        assert_eq!(
            report.fabrication_count(),
            0,
            "a structurally-sourced flag must never be checked against help text"
        );
    }

    // --- detect: subcommands ----------------------------------------------

    #[test]
    fn detect_does_not_flag_gits_real_subcommands() {
        let raw = "start a working area (see also: git help tutorial)\n   clone     Clone a repository into a new directory\n   init      Create an empty Git repository or reinitialize an existing one\n";
        let mut root = help_text_node("git");
        root.subcommands.push(help_text_node("clone"));
        root.subcommands.push(help_text_node("init"));
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 0);
    }

    #[test]
    fn detect_flags_a_subcommand_name_that_never_occurs_at_all() {
        let raw = "start a working area (see also: git help tutorial)\n   clone     Clone a repository into a new directory\n";
        let mut root = help_text_node("git");
        root.subcommands.push(help_text_node("clone"));
        root.subcommands.push(help_text_node("teleport"));
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 1);
        assert_eq!(report.fabrications[0].kind, FabricationKind::Subcommand);
        assert_eq!(report.fabrications[0].name, "teleport");
    }

    #[test]
    fn detect_flags_a_subcommand_name_present_only_mid_line() {
        // "errors" occurs literally in the raw text (see
        // `line_start_words_excludes_a_mid_line_word` above) but never as
        // a line's first word — the shape a real command-list entry never
        // takes.
        let raw = "  -k, --keep-old-files       don't replace existing files when extracting,\n                             treat them as errors\n";
        let mut root = help_text_node("tar");
        root.subcommands.push(help_text_node("errors"));
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 1);
        assert_eq!(report.fabrications[0].name, "errors");
    }

    #[test]
    fn detect_ignores_subcommands_not_sourced_from_help_text() {
        let raw = "start a working area\n   clone     Clone a repository\n";
        let mut root = help_text_node("git");
        let structural = CommandNode::new(
            "hidden-native-only",
            Provenance::single(Source::NativeDynamic {
                protocol: "cobra-dunder-complete".to_string(),
            }),
        );
        root.subcommands.push(structural);
        let report = detect(raw, &root);
        assert_eq!(
            report.fabrication_count(),
            0,
            "a structurally-sourced subcommand must never be checked against help text"
        );
    }

    #[test]
    fn detect_never_checks_the_root_nodes_own_name() {
        // The root's name is the literal argv0 the user typed — never a
        // candidate this module should second-guess, regardless of
        // whether it happens to appear in its own `--help` text.
        let raw = "Usage: definitely-not-in-this-text [OPTION...]\n";
        let root = help_text_node("some-other-name-entirely");
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 0);
    }

    #[test]
    fn detect_recurses_into_real_subcommands_own_flags() {
        let raw = "  clone     Clone a repository\n  -v, --verbose  be verbose\n";
        let mut root = help_text_node("git");
        let mut clone = help_text_node("clone");
        clone
            .entities
            .push(help_text_flag(None, Some("invented"), false));
        root.subcommands.push(clone);
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 1);
        assert_eq!(report.fabrications[0].path, "git clone");
        assert_eq!(report.fabrications[0].name, "--invented");
    }

    // --- the M-10 replay: tar's own real corpus text ----------------------

    /// [M-10]'s real war story, replayed against `tar`'s own committed
    /// corpus capture: a hand-built synthetic tree carrying a subcommand
    /// this module's author invented (never edited into
    /// `mandible-extract`, per this task's own constraint — no tier
    /// change could reproduce the historical bug directly, since
    /// `is_command_name_shaped` already rejects a multi-word candidate
    /// like the real *"treat them as errors"* today) proves the detector
    /// would have caught the *shape* of [M-10] against real, byte-exact
    /// tool output: a name with no line-start occurrence anywhere in it.
    #[test]
    fn detects_an_invented_subcommand_against_tars_real_corpus_text() {
        let raw = include_str!("../../corpus/tar/1.35/help.txt");
        let mut root = help_text_node("tar");
        // tar has no real subcommands at all — every one of its "commands"
        // is actually a flag (`-c`, `-x`, `-t`, ...). A phantom node here
        // is exactly [M-10]'s shape: a plausible-looking lowercase word
        // that is not a line-start entry anywhere in tar's own text.
        root.subcommands.push(help_text_node("phantomize"));
        let report = detect(raw, &root);
        assert_eq!(
            report.fabrication_count(),
            1,
            "expected the invented subcommand to be caught: {:?}",
            report
                .fabrications
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
        assert_eq!(report.fabrications[0].kind, FabricationKind::Subcommand);
        assert_eq!(report.fabrications[0].name, "phantomize");
    }

    /// Confirms zero fabrications on `tar`'s real, well-formed flags —
    /// every one of tar's genuine flag spellings really does occur in its
    /// own `--help` text, negatable/value-spec forms included (`-H,
    /// --format=FORMAT`, `--sparse-version=MAJOR[.MINOR]`, ...).
    #[test]
    fn no_fabrications_on_tars_own_real_flags() {
        let raw = include_str!("../../corpus/tar/1.35/help.txt");
        let mut root = help_text_node("tar");
        for (short, long) in [
            (Some('c'), Some("create")),
            (Some('x'), Some("extract")),
            (Some('H'), Some("format")),
            (None, Some("sparse-version")),
            (None, Some("occurrence")),
        ] {
            root.entities.push(help_text_flag(short, long, false));
        }
        let report = detect(raw, &root);
        assert_eq!(
            report.fabrication_count(),
            0,
            "unexpected fabrications on tar's own real flags: {:?}",
            report
                .fabrications
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
    }

    // --- positional operands ---------------------------------------------

    fn help_text_positional(name: &str) -> mandible_core::Entity {
        mandible_core::Entity::positional(name, Provenance::single(Source::HelpText))
    }

    /// Every option-list placeholder shape the 15-tool fix removed, each
    /// byte-exact from the tool's own capture, paired with the operand
    /// standing beside it. The rule has to name the first and spare the
    /// second in every row.
    const PLACEHOLDER_PAIRS: &[(&str, &str, &str)] = &[
        ("Usage: tar [OPTION...] [FILE]...\n", "OPTION", "FILE"),
        ("Usage: du [OPTION]... [FILE]...\n", "OPTION", "FILE"),
        (
            "usage: pkgconf [OPTIONS] [LIBRARIES]\n",
            "OPTIONS",
            "LIBRARIES",
        ),
        (
            "Usage: dpkg-statoverride [<option> ...] <command>\n",
            "option",
            "command",
        ),
        ("Usage: rmiregistry <options> <port>\n", "options", "port"),
        ("Usage: vim [arguments] [file ..]\n", "arguments", "file"),
        ("Usage: curl [options...] <url>\n", "options", "url"),
        (
            "Usage: grub-set-default [OPTION] MENU_ENTRY\n",
            "OPTION",
            "MENU_ENTRY",
        ),
    ];

    #[test]
    fn the_option_list_slot_is_never_an_attested_operand() {
        for (raw, placeholder, operand) in PLACEHOLDER_PAIRS {
            let attested = attested_operand_positions(raw, "");
            assert!(
                !attested.contains(placeholder),
                "{placeholder:?} must not be attested by {raw:?}: {attested:?}"
            );
            assert!(
                attested.contains(operand),
                "{operand:?} must be attested by {raw:?}: {attested:?}"
            );
        }
    }

    #[test]
    fn detect_flags_the_option_list_placeholder_and_spares_the_operand_beside_it() {
        for (raw, placeholder, operand) in PLACEHOLDER_PAIRS {
            let mut root = help_text_node("t");
            root.entities.push(help_text_positional(placeholder));
            root.entities.push(help_text_positional(operand));
            let report = detect(raw, &root);
            let names: Vec<&String> = report.fabrications.iter().map(|f| &f.name).collect();
            assert_eq!(names, vec![placeholder], "for {raw:?}");
            assert_eq!(report.fabrications[0].kind, FabricationKind::Positional);
        }
    }

    /// The [M-10]-shaped replay for operands, against `tar`'s own real
    /// corpus capture rather than a hand-typed line: `tar` shipped an
    /// operand called `OPTION`, lifted out of `[OPTION...]`, and this
    /// oracle saw it and said nothing.
    #[test]
    fn detects_tars_own_fabricated_option_operand_against_its_real_corpus_text() {
        let raw = include_str!("../../corpus/tar/1.35/help.txt");
        let mut root = help_text_node("tar");
        root.entities.push(help_text_positional("OPTION"));
        root.entities.push(help_text_positional("FILE"));
        let report = detect(raw, &root);
        assert_eq!(
            report.fabrication_count(),
            1,
            "expected exactly the fabricated operand: {:?}",
            report
                .fabrications
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
        assert_eq!(report.fabrications[0].kind, FabricationKind::Positional);
        assert_eq!(report.fabrications[0].name, "OPTION");
    }

    /// Clause 1 of [`option_list_slot`]: a synopsis that spells its own
    /// options out needs no stand-in for them, so the rule stays out of the
    /// line entirely. `uobjnew`'s real argparse synopsis, byte-exact — its
    /// `pid` and `interval` are the two operands the same commit
    /// *recovered*, and reporting either would undo that.
    #[test]
    fn a_synopsis_that_writes_its_own_flags_has_no_placeholder_slot() {
        let raw = "usage: uobjnew [-h] [-l {c,java,ruby,tcl}] [-C TOP_COUNT] [-S TOP_SIZE] [-v] pid [interval]\n";
        let mut root = help_text_node("uobjnew");
        root.entities.push(help_text_positional("pid"));
        root.entities.push(help_text_positional("interval"));
        assert_eq!(detect(raw, &root).fabrication_count(), 0);
    }

    /// Clause 2: a bare word is a named operand, never a stand-in for a
    /// flag list. Without it, `basename`'s own first operand reads as the
    /// option list.
    #[test]
    fn an_unbracketed_first_slot_is_never_the_option_list() {
        let raw = "Usage: basename NAME [SUFFIX]\n";
        let mut root = help_text_node("basename");
        root.entities.push(help_text_positional("NAME"));
        root.entities.push(help_text_positional("SUFFIX"));
        assert_eq!(detect(raw, &root).fabrication_count(), 0);
    }

    /// Clause 3: a synopsis naming exactly one slot is naming the one
    /// operand the tool takes. `zoxide` and `strace-log-merge` are both
    /// real corpus fixtures, and both would lose a genuine operand without
    /// this.
    #[test]
    fn a_sole_slot_is_the_tools_operand_not_its_option_list() {
        for (raw, name) in [
            ("Usage:\n  zoxide <COMMAND>\n", "COMMAND"),
            ("Usage: strace-log-merge STRACE_LOG\n", "STRACE_LOG"),
        ] {
            let mut root = help_text_node("t");
            root.entities.push(help_text_positional(name));
            let report = detect(raw, &root);
            assert_eq!(report.fabrication_count(), 0, "for {raw:?}");
        }
    }

    /// A bare `Usage:` header with the synopsis indented beneath it —
    /// util-linux's house style, and the shape that carries `wall`'s two
    /// real operands one slot past its `[options]` placeholder.
    #[test]
    fn a_bare_usage_header_opens_a_synopsis_block() {
        let raw = "Usage:\n wall [options] [<file> | <message>]\n\nWrite a message to all users.\n";
        let attested = attested_operand_positions(raw, "wall");
        assert!(attested.contains("file"), "{attested:?}");
        assert!(attested.contains("message"), "{attested:?}");
        assert!(!attested.contains("options"), "{attested:?}");
    }

    /// `git --help`'s real wrapped synopsis: the two operands sit on the
    /// *fifth* physical line. Reading only the marker line reported both
    /// `command` and `args` as invented — measured against the committed
    /// fixture, not imagined.
    #[test]
    fn a_wrapped_synopsis_still_attests_the_operands_on_its_last_line() {
        let raw = include_str!("../../corpus/git/2.43.0/help.txt");
        let mut root = help_text_node("git");
        root.entities.push(help_text_positional("command"));
        root.entities.push(help_text_positional("args"));
        let report = detect(raw, &root);
        assert_eq!(
            report.fabrication_count(),
            0,
            "git's own real operands must not be flagged: {:?}",
            report
                .fabrications
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
    }

    /// Ordinary indented prose under a usage line is not more synopsis.
    /// `du --help` writes exactly that, and admitting it would attest every
    /// word of a sentence as an operand position.
    #[test]
    fn indented_prose_under_a_usage_line_is_not_a_synopsis_continuation() {
        let raw = concat!(
            "Usage: du [OPTION]... [FILE]...\n",
            "  or:  du [OPTION]... --files0-from=F\n",
            "  Summarize device usage of the set of FILEs, recursively for directories.\n",
        );
        let attested = attested_operand_positions(raw, "du");
        assert!(!attested.contains("device"), "{attested:?}");
        assert!(!attested.contains("recursively"), "{attested:?}");
        assert!(attested.contains("FILE"), "{attested:?}");
    }

    /// A real operand that appears only in a declared operand block —
    /// argparse's `positional arguments:` heading, which
    /// `sections::emit_declared_positionals` reads — is attested by its
    /// block entry, not by the synopsis.
    #[test]
    fn a_declared_operand_block_entry_attests_its_operand() {
        let raw = concat!(
            "usage: uobjnew [-h] pid\n",
            "\n",
            "positional arguments:\n",
            "  pid                   process id to attach to\n",
            "  interval              print every specified number of seconds\n",
        );
        let mut root = help_text_node("uobjnew");
        root.entities.push(help_text_positional("interval"));
        assert_eq!(detect(raw, &root).fabrication_count(), 0);
    }

    /// The other direction, and the one that makes the rest worth
    /// anything: an operand named by nothing in the document at all is
    /// still reported.
    #[test]
    fn detect_flags_an_operand_that_occurs_nowhere() {
        let raw = "Usage: tar [OPTION...] [FILE]...\n";
        let mut root = help_text_node("tar");
        root.entities.push(help_text_positional("TELEPORT"));
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 1);
        assert_eq!(report.fabrications[0].kind, FabricationKind::Positional);
        assert_eq!(report.fabrications[0].name, "TELEPORT");
    }

    #[test]
    fn detect_ignores_positionals_not_sourced_from_help_text() {
        let raw = "Usage: tar [OPTION...] [FILE]...\n";
        let mut root = help_text_node("tar");
        let mut structural = help_text_positional("never-printed");
        structural.provenance = Provenance::single(Source::NativeDynamic {
            protocol: "cobra-dunder-complete".to_string(),
        });
        root.entities.push(structural);
        assert_eq!(
            detect(raw, &root).fabrication_count(),
            0,
            "a structurally-sourced operand must never be checked against help text"
        );
    }

    #[test]
    fn detect_recurses_into_a_subcommands_own_positionals() {
        let raw = "Usage: t [OPTION...] <file>\n   sub    do a thing\n";
        let mut root = help_text_node("t");
        let mut sub = help_text_node("sub");
        sub.entities.push(help_text_positional("invented"));
        root.subcommands.push(sub);
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 1);
        assert_eq!(report.fabrications[0].path, "t sub");
        assert_eq!(report.fabrications[0].name, "invented");
    }

    /// A flag's own value token stays an attested operand position on
    /// purpose — see [`usage_operands`]. `lzgrep` writes its genuine
    /// `PATTERN` immediately after a bracketed `[-e]`, and a
    /// value-consuming reader would report a real operand as invented.
    #[test]
    fn an_operand_written_after_a_bracketed_flag_is_still_attested() {
        let raw = "Usage: lzgrep [OPTION]... [-e] PATTERN [FILE]...\n";
        let mut root = help_text_node("lzgrep");
        root.entities.push(help_text_positional("PATTERN"));
        root.entities.push(help_text_positional("FILE"));
        assert_eq!(detect(raw, &root).fabrication_count(), 0);
    }

    // --- RecordingProbe wiring sanity (mirrors misattribution's own) -----

    #[test]
    fn empty_text_and_empty_tree_produce_no_fabrications() {
        let root = help_text_node("nothing");
        let report = detect("", &root);
        assert_eq!(report.fabrication_count(), 0);
    }

    /// A trivial confirmation this module really does read
    /// [`RecordingProbe`] the same way `misattribution::detect` does —
    /// "no new probes" (this module's doc comment) means reusing the exact
    /// same capture, not a parallel one.
    #[test]
    fn recording_probe_text_feeds_detect_directly() {
        let probe = RecordingProbe::new();
        assert!(probe.root_help_text().is_none());
        let root = help_text_node("nothing");
        let report = detect(probe.root_help_text().unwrap_or_default().as_str(), &root);
        assert_eq!(report.fabrication_count(), 0);
    }

    // --- unlabelled and name-prefixed synopses (this task's own fix) -----

    /// `gh --help`'s real shape, byte-exact: a bare `USAGE` heading (no
    /// colon, so `starts_with_usage_prefix` never matches it) followed by an
    /// indented line that simply opens with the tool's own name and reads as
    /// usage grammar. Before this fix, this line was entirely invisible to
    /// `synopsis_lines`, so both of `gh`'s real operands were reported as
    /// invented despite occurring literally in the tool's own output — this
    /// task's own worked example.
    const GH_USAGE: &str = "USAGE\n  gh <command> <subcommand> [flags]\n";

    #[test]
    fn an_unlabelled_synopsis_opening_with_the_tools_own_name_is_recognized() {
        let attested = attested_operand_positions(GH_USAGE, "gh");
        assert!(attested.contains("command"), "{attested:?}");
        assert!(attested.contains("subcommand"), "{attested:?}");
        // `flags` is gh's own flag-list stand-in (vocabulary-matched, see
        // `option_list_slot`) and must not itself be an attested operand.
        assert!(!attested.contains("flags"), "{attested:?}");
    }

    #[test]
    fn detect_does_not_flag_ghs_real_operands_from_its_unlabelled_synopsis() {
        let mut root = help_text_node("gh");
        root.entities.push(help_text_positional("command"));
        root.entities.push(help_text_positional("subcommand"));
        let report = detect(GH_USAGE, &root);
        assert_eq!(
            report.fabrication_count(),
            0,
            "gh's own real operands must not be flagged: {:?}",
            report
                .fabrications
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
    }

    /// A labelled `usage:` line anywhere in the document must still take
    /// precedence: the unlabelled fallback is only ever tried when
    /// [`has_labelled_usage_start`] finds nothing, exactly mirroring the
    /// tier's own `labelled_usage_start`/`unlabelled_synopsis_start`
    /// precedence.
    #[test]
    fn a_real_labelled_usage_line_suppresses_the_unlabelled_fallback() {
        let raw = "Usage: gh [flags]\nEXTRA\n  gh <command> <subcommand> [flags]\n";
        assert!(has_labelled_usage_start(raw, "gh"));
        let attested = attested_operand_positions(raw, "gh");
        // The unlabelled fallback line never opens a block here, so its
        // `command`/`subcommand` are not attested via that path — only
        // whatever the real labelled block itself carries (nothing, in this
        // synthetic example, since its only slot is the option list).
        assert!(!attested.contains("command"), "{attested:?}");
    }

    /// The C `"%s: Usage: ..."` idiom: the tool's own name, a literal
    /// `": "`, then `usage:` — `nfsidmap`'s real shape. The tool's name
    /// occurs *twice* on this one line (the `fprintf` prefix, then again as
    /// the invocation's own program name), and both copies must be
    /// consumed before the remaining tokens are read as operands.
    const NFSIDMAP_USAGE: &str = "nfsidmap: Usage: nfsidmap [-vh] [-c || -d] [-l] path\n";

    #[test]
    fn a_name_prefixed_usage_idiom_line_is_recognized() {
        let attested = attested_operand_positions(NFSIDMAP_USAGE, "nfsidmap");
        assert!(attested.contains("path"), "{attested:?}");
    }

    #[test]
    fn detect_does_not_flag_nfsidmaps_real_operand_from_the_name_prefixed_idiom() {
        let mut root = help_text_node("nfsidmap");
        root.entities.push(help_text_positional("path"));
        let report = detect(NFSIDMAP_USAGE, &root);
        assert_eq!(
            report.fabrication_count(),
            0,
            "nfsidmap's own real operand must not be flagged: {:?}",
            report
                .fabrications
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
    }

    /// [`option_list_slot`]'s own regression: the vocabulary rule and the
    /// positional shape rule must never both fire on the same line. `gh`'s
    /// synopsis puts its real flag stand-in *last* (`flags`, vocabulary-
    /// matched) while a genuine operand (`command`) sits first — exactly
    /// the shape that would trip the position rule into treating `command`
    /// as *a second* excluded placeholder if the two rules ran
    /// unconditionally instead of the vocabulary rule gating the fallback.
    #[test]
    fn a_trailing_vocabulary_placeholder_does_not_also_trigger_the_leading_position_rule() {
        let slots = vec![("command", true), ("subcommand", true), ("flags", true)];
        let placeholders = option_list_slot(&slots, false);
        assert_eq!(placeholders, HashSet::from(["flags"]));
    }

    /// `vgextend --help`'s real shape, byte-exact: LVM's own bare-own-name
    /// convention. The invocation line carries no docopt notation at all
    /// (`vgextend VG PV ...`, no `[`, `<` or `{` anywhere on it), so
    /// `looks_like_unlabeled_synopsis_line` alone can never find it — the
    /// evidence that it is usage grammar is entirely on the *next* physical
    /// line, an unambiguous bracketed flag row.
    const VGEXTEND_USAGE: &str = concat!(
        "  vgextend - Add physical volumes to a volume group\n",
        "\n",
        "  vgextend VG PV ...\n",
        "\t[ -A|--autobackup y|n ]\n",
        "\t[ -f|--force ]\n",
        "\t[ COMMON_OPTIONS ]\n",
    );

    #[test]
    fn a_bare_own_name_line_followed_by_a_bracket_flag_row_is_recognized() {
        let attested = attested_operand_positions(VGEXTEND_USAGE, "vgextend");
        assert!(attested.contains("VG"), "{attested:?}");
        assert!(attested.contains("PV"), "{attested:?}");
    }

    #[test]
    fn detect_does_not_flag_vgextends_real_operands_from_its_bare_own_name_line() {
        let mut root = help_text_node("vgextend");
        root.entities.push(help_text_positional("VG"));
        root.entities.push(help_text_positional("PV"));
        let report = detect(VGEXTEND_USAGE, &root);
        assert_eq!(
            report.fabrication_count(),
            0,
            "vgextend's own real operands must not be flagged: {:?}",
            report
                .fabrications
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
    }

    /// A bare own-name line whose *next* line is ordinary prose, not a
    /// bracketed flag row, must not open a synopsis block — the same
    /// discipline `starts_with_tool_name` alone would otherwise blur: a
    /// sentence that happens to open with the tool's own name is not usage
    /// grammar.
    #[test]
    fn a_bare_own_name_line_without_a_flag_row_next_does_not_open_a_block() {
        let raw = "vgextend is a tool for extending volume groups.\nIt takes no further arguments here.\n";
        let attested = attested_operand_positions(raw, "vgextend");
        assert!(!attested.contains("is"), "{attested:?}");
        assert!(!attested.contains("tool"), "{attested:?}");
    }
}
