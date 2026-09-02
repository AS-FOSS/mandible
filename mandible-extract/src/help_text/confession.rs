//! Truncation-confession detection (spec §6 rule 2b): a tool's own `--help`
//! text can say, in its own words, that plain `--help` is not the complete
//! document. Three shapes are matched — curl's quoted directive, ffmpeg's
//! unquoted table row, gcc's flag-value row — content-keyed only, never on
//! `argv[0]` (AGENTS.md §1). See docs/shapes.md S-080; fixtures at
//! corpus/curl/8.5.0(-all)/, corpus/ffmpeg/6.1.1/, corpus/gcc/13.3.0/.

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
const FLAGS: [&str; 2] = ["--help", "-h"];

/// Quote characters real specimens use for this shape. See docs/shapes.md
/// S-080.
const QUOTES: [char; 3] = ['"', '\'', '`'];

/// Defensive upper bound on a directive word's length so a pathological
/// line can't produce an unbounded match; no real specimen comes close.
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
                // `pos` is from `match_indices` on a single-byte ASCII
                // quote, so `pos + quote.len_utf8()` is always a valid
                // UTF-8 boundary (never an unverified byte offset).
                let after = &line[pos + quote.len_utf8()..];
                if let Some(directive) = match_quoted(after, quote) {
                    push_unique(&mut out, directive);
                }
            }
        }
        // Both unquoted shapes are anchored to line start, not scanned
        // mid-line, so a sentence merely mentioning `--help` never matches.
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

/// Append `directive` unless an equal one is already present.
fn push_unique(out: &mut Vec<Directive>, directive: Directive) {
    if !out.contains(&directive) {
        out.push(directive);
    }
}

/// Reads `<flag><spaces><word><quote>` from the start of `after` (text
/// right after an opening `quote`). Matches only when the same quote
/// closes immediately after the word — curl's `"--help all"`, not
/// `"--help all the options"`. See docs/shapes.md S-080.
fn match_quoted(after: &str, quote: char) -> Option<Directive> {
    for flag in FLAGS {
        let Some(rest) = after.strip_prefix(flag) else {
            continue;
        };
        let trimmed = rest.trim_start_matches(' ');
        if trimmed.len() == rest.len() {
            // No space after the flag: not `<flag> <word>`.
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

/// Reads `<flag> <word> -- <description>` from the start of `trimmed` —
/// ffmpeg's shape (`-h long -- print more options`). Every joint (space,
/// word, space, bare `--`, space, description) must hold, which is what
/// rejects an ordinary flag row, a longer `--help`-prefixed flag name, and
/// a row whose description merely starts with prose. See docs/shapes.md
/// S-080 and corpus/ffmpeg/6.1.1/.
fn match_unquoted_table_row(trimmed: &str) -> Option<Directive> {
    for flag in FLAGS {
        let Some(rest) = trimmed.strip_prefix(flag) else {
            continue;
        };
        let after_flag = rest.trim_start_matches(' ');
        if after_flag.len() == rest.len() {
            // No space right after the flag: not `<flag> <word>`.
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
            // No space after the word: not `<word> --`.
            continue;
        }
        let Some(after_dashes) = after_space.strip_prefix("--") else {
            continue;
        };
        if !after_dashes.starts_with(' ') {
            // `--` must stand alone, not start a longer token or glue to
            // the description.
            continue;
        }
        let description = after_dashes.trim_start_matches(' ');
        if description.is_empty() {
            // Nothing after `--`: a truncated line, not a confession.
            continue;
        }
        return Some(Directive { flag, word });
    }
    None
}

/// Punctuation a class/placeholder enumeration opens with, right after
/// `=`. Distinguishes gcc's `--help={common|...}` from a literal-valued
/// `--help=yes` or an optional-value `--help[=FMT]` (`[` before `=`, so
/// `strip_prefix('=')` below never reaches it).
const VALUE_LIST_OPENERS: [char; 4] = ['{', '[', '<', '('];

/// Reads `<flag>=<opener>...` from the start of `trimmed` — gcc's flag
/// row listing `--help` as taking a value. Word recorded is the first
/// class name, verbatim. Detected but never followed (would need
/// enumerating every class as its own probe). See docs/shapes.md S-080
/// and corpus/gcc/13.3.0/.
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

/// Closed vocabulary of words this tier will actually re-probe with. A
/// directive whose word isn't here is still detected (spec §6 rule 2b's
/// `incomplete` status) but not followed.
const FOLLOWABLE_WORDS: &[&str] = &["all"];

/// The first followable directive, in detection order, from a set
/// [`detect_directives`] already found. `None` when nothing qualifies.
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

    /// Only `"all"` is followable; `"category"` is detected but deferred.
    #[test]
    fn only_the_all_directive_is_expandable() {
        let directives = detect_directives(CURL_TAIL);
        let chosen = expandable(&directives).expect("`all` must be followable");
        assert_eq!(chosen.word, "all");
    }

    /// Prose merely mentioning `--help` in passing must detect nothing.
    #[test]
    fn mentioning_help_in_passing_is_not_a_directive() {
        let text =
            "Run with --help for more information.\nSee -h, --help  show this help and exit\n";
        assert!(detect_directives(text).is_empty(), "{text:?}");
    }

    /// A quote merely surrounding an invocation, not opening right at the
    /// flag, must not match.
    #[test]
    fn a_quoted_sentence_around_the_flag_is_not_a_directive() {
        let text = "Run 'tool --help' to see options.\n";
        assert!(detect_directives(text).is_empty(), "{text:?}");
    }

    /// An ordinary flag row must never be read as a directive.
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

    /// A directive word outside the followable vocabulary is still
    /// detected, just not expandable.
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

    /// ffmpeg's "Getting help:" block, byte-for-byte. See
    /// corpus/ffmpeg/6.1.1/.
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

    /// Neither `long` nor `full` is followable: detected (`incomplete`
    /// status) but not followed — following needs a new argv shape,
    /// deferred (spec §6 rule 2b, WS5b).
    #[test]
    fn ffmpeg_directives_are_detected_but_not_expandable() {
        let directives = detect_directives(FFMPEG_GETTING_HELP);
        assert!(!directives.is_empty());
        assert!(expandable(&directives).is_none());
    }

    /// Real ffmpeg rows next to the confession that must not be misread
    /// as one: a word after the flag but no bare `--` token following.
    #[test]
    fn ffmpeg_topic_rows_are_not_directives() {
        let text = "-h topic            show help\n--help topic        show help\n";
        assert!(detect_directives(text).is_empty(), "{text:?}");
    }

    /// gcc's `--help=<class-list>` row, byte-for-byte. See
    /// corpus/gcc/13.3.0/.
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

    /// gcc's plain `--help` row (adjacent in real output) must not match.
    #[test]
    fn gcc_plain_help_row_is_not_a_directive() {
        let text = "  --help                   Display this information.\n";
        assert!(detect_directives(text).is_empty(), "{text:?}");
    }

    /// A longer flag name merely starting with `--help` (e.g.
    /// `--help-all`) must never match.
    #[test]
    fn a_longer_help_prefixed_flag_is_not_a_directive() {
        let text = "  --help-all             Show all help options\n";
        assert!(detect_directives(text).is_empty(), "{text:?}");
    }

    /// A literal-valued `--help=yes` row must not be read as gcc's shape.
    #[test]
    fn a_literal_valued_help_flag_is_not_a_directive() {
        let text = "  --help=yes             enable extra help\n";
        assert!(detect_directives(text).is_empty(), "{text:?}");
    }

    /// A GNU optional-value row (`--help[=FMT]`) must not match either.
    #[test]
    fn an_optional_value_help_flag_is_not_a_directive() {
        let text = "  --help[=FMT]           show help, formatted per FMT\n";
        assert!(detect_directives(text).is_empty(), "{text:?}");
    }
}
