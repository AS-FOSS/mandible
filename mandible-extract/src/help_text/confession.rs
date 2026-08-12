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
                    if !out.contains(&directive) {
                        out.push(directive);
                    }
                }
            }
        }
    }
    out
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
}
