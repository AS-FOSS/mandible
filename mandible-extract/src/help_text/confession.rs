//! Truncation-confession detection (spec §6 rule 2b): recognizing when a
//! tool's own `--help` text tells the reader, in its own words, that a
//! plain `--help` is not the complete document.
//!
//! The specimen this module was built from is `curl --help`, which ends:
//!
//! ```text
//! This is not the full help, this menu is stripped into categories.
//! Use "--help category" to get an overview of all categories.
//! For all options use the manual or "--help all".
//! ```
//!
//! It is a convention, not a curl quirk — `ffmpeg -h long`/`-h full`,
//! `git help -a`, and `gcc --help=<class>` are the same genus (spec's WS5
//! brief). Measured on this machine: `curl --help` recovers 12 flags;
//! `curl --help all` recovers 258.
//!
//! **The grammar is closed and content-keyed, never keyed on `argv[0]`**
//! (AGENTS.md §1's invariant): it matches one specific quoted shape —
//! `"--help <word>"` or `"-h <word>"`, the word bare and the quote
//! immediately closing right after it — never a tool name, so it fires
//! identically for any tool that happens to print the same convention and
//! never fires just because the probed tool happens to be named `curl`.
//! Deliberately **not** matching an unquoted form (`Use --help all to see
//! everything`, no quotes): every real specimen this module was built
//! against (curl) quotes the directive, and an unquoted match risks firing
//! on ordinary prose that merely mentions the flag in passing ("See --help
//! for more options") — the exact false positive this module's own tests
//! (`mentioning_help_in_passing_is_not_a_directive`) prove it avoids.
//! Broadening to unquoted forms, if a real specimen ever needs it, is a
//! new, separately-reviewable shape — not a default this module reaches
//! for.
//!
//! **Two more specimens, detected but never followed (spec §6 rule 2b's
//! detection-only extension).** curl was not the only tool this genus
//! named; two of the others measured "confidently wrong" — `ok` at full
//! confidence over a document their own text says is incomplete — because
//! neither confesses in curl's *quoted* shape:
//!
//! - **ffmpeg** confesses unquoted, inside a flag-table row, not prose:
//!   ```text
//!   Getting help:
//!       -h      -- print basic options
//!       -h long -- print more options
//!       -h full -- print all options (including all format and codec specific options, very long)
//!   ```
//!   [`match_unquoted_table_row`] recognizes `<flag> <word> -- <description>`,
//!   anchored to the trimmed line's start exactly like the quoted form is
//!   anchored to the character right after an opening quote — a bare `--`
//!   token has to sit directly between the captured word and a non-empty
//!   description, so nothing short of that exact three-part shape matches.
//!   That anchoring is what keeps an ordinary flag row
//!   (`-h, --help  show this help message and exit`) safe: the comma sits
//!   where this grammar requires a space, so the row never even reaches the
//!   word-capture step, and a real *description* almost never has a bare
//!   `--` as its first token (descriptions are prose, not `-- more prose`).
//! - **gcc** confesses as a flag definition, not an invocation example: its
//!   own `--help` output lists `--help` itself as taking a value —
//!   ```text
//!     --help={common|optimizers|params|target|warnings|[^]{joined|separate|undocumented}}[,...].
//!                              Display specific types of command line options.
//!   ```
//!   [`match_flag_value_row`] recognizes `<flag>=<opener>...`, requiring the
//!   character right after `=` to be one of `{`, `[`, `<`, `(` — the
//!   punctuation a class/placeholder enumeration opens with — never a bare
//!   word, so a hypothetical literal-valued row (`--help=yes`) or an
//!   optional-value row (`--help[=FMT]`, `=` not touching the flag) is
//!   never mistaken for this shape.
//!
//! Both are **detection only**: neither shape's word is added to
//! [`FOLLOWABLE_WORDS`], and no new [`crate::exec::InertArgv`]
//! variant exists to *follow* `-h <word>` (no leading `--help`, so it isn't
//! `HelpExpand`'s shape) or `--help=<class>` (a different argv token
//! entirely — `--help=common`, one token, not `--help` `all`, two). Each
//! would need its own §6 deliberation before this crate could construct
//! that argv (deferred: WS5b). Detecting them without following them is
//! still strictly better than the status quo: an undetected confession is
//! a false `ok`; a detected-but-unfollowed one is an honest `incomplete`
//! (spec §6 rule 2b's status ladder) — which is the entire point of this
//! extension.

/// One directive a tool's own `--help` text printed, recommending a
/// further probe. `word` is taken verbatim from the tool's own text —
/// never fabricated, never guessed (spec §6 rule 2b).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directive {
    /// The flag the directive printed, exactly as written: `"--help"` or
    /// `"-h"`.
    pub flag: &'static str,
    /// The word that followed it, verbatim from the tool's own text.
    pub word: String,
}

/// The closed set of flags a directive may recommend re-probing with.
/// Checked longest-first has no bearing here (neither is a prefix of the
/// other), but kept in this order since `--help` is the far more common
/// specimen.
const FLAGS: [&str; 2] = ["--help", "-h"];

/// The three quote characters real specimens use. Curl uses `"`; this also
/// accepts `'`/`` ` `` for the same shape from a tool that quotes
/// differently, without loosening *what* has to be quoted.
const QUOTES: [char; 3] = ['"', '\'', '`'];

/// Defensive upper bound on a directive word's length — no real specimen
/// comes close (curl's longest is `category`, 8 characters); this exists
/// only to keep a pathological line from producing an unbounded match.
const MAX_WORD_LEN: usize = 32;

fn is_word_start(c: char) -> bool {
    c.is_ascii_alphabetic()
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

/// Find every truncation-confession directive in `text`, in the order
/// they appear. Empty when the text contains none — the overwhelming
/// common case.
pub fn detect_directives(text: &str) -> Vec<Directive> {
    let mut out: Vec<Directive> = Vec::new();
    for line in text.lines() {
        for quote in QUOTES {
            for (pos, _) in line.match_indices(quote) {
                // `pos` came from `match_indices` on a single-byte ASCII
                // quote character, so `pos + quote.len_utf8()` is always a
                // valid UTF-8 boundary — never a raw numeric offset guess
                // (AGENTS.md: never slice at an unverified byte offset).
                let after = &line[pos + quote.len_utf8()..];
                if let Some(directive) = match_quoted(after, quote) {
                    push_unique(&mut out, directive);
                }
            }
        }
        // The two unquoted shapes (ffmpeg's table row, gcc's flag-value
        // row) are both anchored to the start of the line — never scanned
        // mid-line the way quotes are — which is what keeps them from
        // reading a sentence that merely *mentions* `--help` as a
        // directive. `trim_start` only strips leading whitespace, so a
        // real flag-table row's own indentation is the only thing this
        // removes.
        let trimmed = line.trim_start();
        if let Some(directive) = match_unquoted_table_row(trimmed) {
            push_unique(&mut out, directive);
        }
        if let Some(directive) = match_flag_value_row(trimmed) {
            push_unique(&mut out, directive);
        }
    }
    out
}

/// Append `directive` unless an equal one is already present — the same
/// dedup [`detect_directives`] has always applied to the quoted shape, now
/// shared across all three shapes.
fn push_unique(out: &mut Vec<Directive>, directive: Directive) {
    if !out.contains(&directive) {
        out.push(directive);
    }
}

/// Try to read `<flag><spaces><word><quote>` from the very start of
/// `after` — everything following an opening `quote` character. Matches
/// only when the word is immediately followed by the *same* quote
/// character closing it (no trailing content before the quote), which is
/// exactly curl's own shape: `"--help all"`, not `"--help all the
/// options"`.
fn match_quoted(after: &str, quote: char) -> Option<Directive> {
    for flag in FLAGS {
        let Some(rest) = after.strip_prefix(flag) else {
            continue;
        };
        let trimmed = rest.trim_start_matches(' ');
        if trimmed.len() == rest.len() {
            // No space between the flag and whatever follows — not a
            // `<flag> <word>` shape at all (e.g. `"--helper"` or a
            // trailing `"--help"` with nothing after it).
            continue;
        }
        let word: String = trimmed.chars().take_while(|&c| is_word_char(c)).collect();
        if word.is_empty() || word.chars().count() > MAX_WORD_LEN {
            continue;
        }
        if !word.chars().next().is_some_and(is_word_start) {
            continue;
        }
        let after_word = &trimmed[word.len()..];
        if after_word.starts_with(quote) {
            return Some(Directive { flag, word });
        }
    }
    None
}

/// Try to read `<flag> <word> -- <description>` from the very start of
/// `trimmed` — ffmpeg's own shape:
///
/// ```text
/// -h long -- print more options
/// ```
///
/// Requires, in order: the flag, then at least one space, then a bare
/// word, then at least one more space, then a literal `--` immediately
/// followed by whitespace, then a non-empty description. Every one of
/// those joints has to hold, which is what makes this safe against the
/// two shapes it must never match:
///
/// - An ordinary flag row (`-h, --help  show this help message and
///   exit`): after `-h` comes a comma, not a space, so the very first
///   joint fails and nothing downstream is even attempted.
/// - A distinct, longer flag name (`--help-all  Show all help options`):
///   after `--help` comes `-all` with no space, so the first joint fails
///   here too — this shape can never fire on a flag that merely starts
///   with `--help`.
/// - A row whose description happens to be prose (`--help  show more
///   information`): the first captured word (`show`) is not immediately
///   followed by a bare `--` — it's followed by more words — so the `--`
///   joint fails. A real description essentially never opens with a
///   standalone `--` token.
fn match_unquoted_table_row(trimmed: &str) -> Option<Directive> {
    for flag in FLAGS {
        let Some(rest) = trimmed.strip_prefix(flag) else {
            continue;
        };
        let after_flag = rest.trim_start_matches(' ');
        if after_flag.len() == rest.len() {
            // No space right after the flag: not `<flag> <word>` at all —
            // covers both `<flag>,` (ordinary row) and `<flag>-suffix`
            // (a different, longer flag name).
            continue;
        }
        let word: String = after_flag
            .chars()
            .take_while(|&c| is_word_char(c))
            .collect();
        if word.is_empty()
            || word.chars().count() > MAX_WORD_LEN
            || !word.chars().next().is_some_and(is_word_start)
        {
            continue;
        }
        let after_word = &after_flag[word.len()..];
        let after_space = after_word.trim_start_matches(' ');
        if after_space.len() == after_word.len() {
            // The word runs straight into whatever follows, with no space
            // — not `<word> --`.
            continue;
        }
        let Some(after_dashes) = after_space.strip_prefix("--") else {
            continue;
        };
        if !after_dashes.starts_with(' ') {
            // `--` has to stand alone, separated from the description by
            // whitespace — never the start of a longer token (`---`,
            // `--foo`) and never glued straight to the description.
            continue;
        }
        let description = after_dashes.trim_start_matches(' ');
        if description.is_empty() {
            // Nothing follows `--` at all: `-h long --` alone (never a
            // real specimen) is not a confession, just a truncated line.
            continue;
        }
        return Some(Directive { flag, word });
    }
    None
}

/// The punctuation marks a class/placeholder enumeration opens with, right
/// after `=` — never a bare word. This is what tells gcc's own
/// `--help={common|optimizers|...}` apart from a hypothetical
/// literal-valued flag (`--help=yes`, which starts with none of these) or
/// an optional-value flag (`--help[=FMT]`, where `[` sits *before* `=`,
/// not after it, so `strip_prefix('=')` below never even reaches it).
const VALUE_LIST_OPENERS: [char; 4] = ['{', '[', '<', '('];

/// Try to read `<flag>=<opener>...` from the very start of `trimmed` —
/// gcc's own shape: the flag itself, printed as a flag-table row, listed
/// as taking a value:
///
/// ```text
/// --help={common|optimizers|params|target|warnings|[^]{joined|separate|undocumented}}[,...].
/// ```
///
/// The word recorded is the first class name in the enumeration
/// (`"common"`, here) — taken verbatim from the tool's own text, the same
/// discipline [`match_quoted`] and [`match_unquoted_table_row`] both
/// follow, never fabricated. It plays the same role curl's own `--help
/// category` directive does: detected so `incomplete` fires honestly, but
/// not in [`FOLLOWABLE_WORDS`], because following it would need
/// enumerating every class as its own probe — the same "menu, not a
/// single document" shape that directive already defers.
fn match_flag_value_row(trimmed: &str) -> Option<Directive> {
    for flag in FLAGS {
        let Some(rest) = trimmed.strip_prefix(flag) else {
            continue;
        };
        let Some(value) = rest.strip_prefix('=') else {
            continue;
        };
        let Some(opener) = value.chars().next() else {
            continue;
        };
        if !VALUE_LIST_OPENERS.contains(&opener) {
            continue;
        }
        let word: String = value[opener.len_utf8()..]
            .chars()
            .take_while(|&c| is_word_char(c))
            .collect();
        if word.is_empty()
            || word.chars().count() > MAX_WORD_LEN
            || !word.chars().next().is_some_and(is_word_start)
        {
            continue;
        }
        return Some(Directive { flag, word });
    }
    None
}

/// The closed vocabulary of words this tier will actually follow with a
/// single re-probe (spec's scope discipline: "ship the single `all`-form
/// first"). A directive whose word isn't in this list is still detected
/// (feeding the `incomplete` status, spec §6 rule 2b), just not followed —
/// curl's own `--help category` is exactly this case: following it would
/// return a menu of category *names*, not flags, and turning that into a
/// real recovery needs enumerating each category as its own probe, which
/// is the "category enumeration" this batch explicitly defers building.
const FOLLOWABLE_WORDS: &[&str] = &["all"];

/// The one directive (if any) worth actually re-probing, from a set
/// [`detect_directives`] already found — the first followable match, in
/// detection order. `None` when nothing was detected, or when everything
/// detected names a word outside [`FOLLOWABLE_WORDS`].
pub fn expandable(directives: &[Directive]) -> Option<&Directive> {
    directives
        .iter()
        .find(|d| FOLLOWABLE_WORDS.contains(&d.word.to_ascii_lowercase().as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact specimen this module was built from.
    const CURL_TAIL: &str = "This is not the full help, this menu is stripped into categories.\nUse \"--help category\" to get an overview of all categories.\nFor all options use the manual or \"--help all\".\n";

    #[test]
    fn detects_both_curl_directives() {
        let directives = detect_directives(CURL_TAIL);
        assert_eq!(
            directives,
            vec![
                Directive {
                    flag: "--help",
                    word: "category".to_string(),
                },
                Directive {
                    flag: "--help",
                    word: "all".to_string(),
                },
            ]
        );
    }

    /// Only `"all"` is in the followable vocabulary — `"category"` is
    /// detected but scope discipline defers following it (it names a menu
    /// of further probes, not a single complete document).
    #[test]
    fn only_the_all_directive_is_expandable() {
        let directives = detect_directives(CURL_TAIL);
        let chosen = expandable(&directives).expect("`all` must be followable");
        assert_eq!(chosen.word, "all");
    }

    /// The negative case verification requires: prose that merely mentions
    /// `--help` in passing, with no quoted directive, must detect nothing
    /// at all.
    #[test]
    fn mentioning_help_in_passing_is_not_a_directive() {
        let text =
            "Run with --help for more information.\nSee -h, --help  show this help and exit\n";
        assert!(detect_directives(text).is_empty(), "{text:?}");
    }

    /// A quote that merely *surrounds* an invocation mentioning `--help`,
    /// rather than immediately preceding it, must not match — the quote
    /// has to open right where the flag starts.
    #[test]
    fn a_quoted_sentence_around_the_flag_is_not_a_directive() {
        let text = "Run 'tool --help' to see options.\n";
        assert!(detect_directives(text).is_empty(), "{text:?}");
    }

    /// A flag table row (`-h, --help  show this help message and exit`)
    /// must never be read as a directive: there is no word immediately
    /// after `--help`, just a comma or two spaces before the description.
    #[test]
    fn an_ordinary_help_flag_row_is_not_a_directive() {
        let text = "  -h, --help            show this help message and exit\n";
        assert!(detect_directives(text).is_empty(), "{text:?}");
    }

    /// `-h <word>` is recognized too, not just `--help <word>`.
    #[test]
    fn dash_h_form_is_recognized() {
        let directives = detect_directives("Use \"-h full\" to see everything.\n");
        assert_eq!(
            directives,
            vec![Directive {
                flag: "-h",
                word: "full".to_string(),
            }]
        );
    }

    /// A directive word outside the followable vocabulary (anything other
    /// than "all") is still detected, just not expandable — this is what
    /// makes the `incomplete` status honest for a tool whose only
    /// directive is e.g. `--help category`.
    #[test]
    fn a_non_all_directive_alone_is_detected_but_not_expandable() {
        let directives = detect_directives("Use \"--help category\" to see more.\n");
        assert_eq!(directives.len(), 1);
        assert!(expandable(&directives).is_none());
    }

    #[test]
    fn no_directives_at_all_is_not_expandable() {
        assert!(expandable(&[]).is_none());
    }

    /// ffmpeg's own "Getting help:" block, byte-for-byte (spec §6 rule 2b's
    /// detection-only extension): both `long` and `full` are detected;
    /// the bare `-h` bullet (no word between `-h` and `--`) and the
    /// `type=name` bullet (no space right after the word, so the `--`
    /// joint never lines up) are correctly not.
    const FFMPEG_GETTING_HELP: &str = "Getting help:\n    -h      -- print basic options\n    -h long -- print more options\n    -h full -- print all options (including all format and codec specific options, very long)\n    -h type=name -- print all options for the named decoder/encoder/demuxer/muxer/filter/bsf/protocol\n    See man ffmpeg for detailed description of the options.\n";

    #[test]
    fn detects_ffmpeg_long_and_full() {
        let directives = detect_directives(FFMPEG_GETTING_HELP);
        assert_eq!(
            directives,
            vec![
                Directive {
                    flag: "-h",
                    word: "long".to_string(),
                },
                Directive {
                    flag: "-h",
                    word: "full".to_string(),
                },
            ]
        );
    }

    /// Neither `long` nor `full` is in the followable vocabulary — both
    /// are detected (capping status at `incomplete`) but not followed,
    /// exactly like curl's own `--help category`: following either would
    /// need a new argv shape (`-h long`/`-h full`, no leading `--help`,
    /// so it isn't `HelpExpand`'s shape either), which spec §6 rule 2b's
    /// extension explicitly defers to WS5b.
    #[test]
    fn ffmpeg_directives_are_detected_but_not_expandable() {
        let directives = detect_directives(FFMPEG_GETTING_HELP);
        assert!(!directives.is_empty());
        assert!(expandable(&directives).is_none());
    }

    /// Real rows from ffmpeg's own output that sit right next to the
    /// confession and must not be misread as one: `-h topic` and `--help
    /// topic` both name a word after the flag, but the very next thing is
    /// prose (`show help`), never a bare `--` token.
    #[test]
    fn ffmpeg_topic_rows_are_not_directives() {
        let text = "-h topic            show help\n--help topic        show help\n";
        assert!(detect_directives(text).is_empty(), "{text:?}");
    }

    /// gcc's own `--help=<class-list>` row, byte-for-byte (spec §6 rule
    /// 2b's detection-only extension): the first class name (`common`) is
    /// recorded as the word, taken verbatim from the tool's own text.
    #[test]
    fn detects_gcc_help_equals_row() {
        let text = "  --help={common|optimizers|params|target|warnings|[^]{joined|separate|undocumented}}[,...].\n                           Display specific types of command line options.\n";
        let directives = detect_directives(text);
        assert_eq!(
            directives,
            vec![Directive {
                flag: "--help",
                word: "common".to_string(),
            }]
        );
        assert!(expandable(&directives).is_none());
    }

    /// gcc's own plain `--help` row, right above the `--help=<class>` row
    /// in real output, must not itself be read as a directive: nothing
    /// follows `--help` but whitespace then prose, never `=`.
    #[test]
    fn gcc_plain_help_row_is_not_a_directive() {
        let text = "  --help                   Display this information.\n";
        assert!(detect_directives(text).is_empty(), "{text:?}");
    }

    /// A *longer* flag name that merely starts with `--help` (a real GNU
    /// convention on some tools, e.g. `--help-all`) must never be read as
    /// this tier's `--help` confessing anything: there is no space between
    /// `--help` and `-all`, so the row never reaches the word-capture step
    /// in either unquoted matcher.
    #[test]
    fn a_longer_help_prefixed_flag_is_not_a_directive() {
        let text = "  --help-all             Show all help options\n";
        assert!(detect_directives(text).is_empty(), "{text:?}");
    }

    /// A hypothetical literal-valued `--help=yes` row must not be read as
    /// gcc's shape: the character right after `=` is a bare word
    /// character, not one of the punctuation marks a class/placeholder
    /// enumeration opens with.
    #[test]
    fn a_literal_valued_help_flag_is_not_a_directive() {
        let text = "  --help=yes             enable extra help\n";
        assert!(detect_directives(text).is_empty(), "{text:?}");
    }

    /// A GNU-style optional-value row (`--help[=FMT]`) must not be read as
    /// gcc's shape either: the `[` sits *before* `=`, not immediately
    /// after it, so `strip_prefix('=')` never matches at all.
    #[test]
    fn an_optional_value_help_flag_is_not_a_directive() {
        let text = "  --help[=FMT]           show help, formatted per FMT\n";
        assert!(detect_directives(text).is_empty(), "{text:?}");
    }
}
