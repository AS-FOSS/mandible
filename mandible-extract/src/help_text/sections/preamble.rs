//! The leading paragraphs of a `--help` document: where prose ends and
//! structure begins, which paragraphs are banners rather than a
//! description, and which are the tool's own option-error complaint.

use super::*;

/// Bound the leading-prose scan to before the first blank-line-preceded
/// section when there's no usage line at all (avoids treating the whole
/// output as "description" for tools with no `Usage:` line).
pub(super) fn leading_prose_bound(lines: &[&str]) -> usize {
    for (idx, l) in lines.iter().enumerate() {
        if l.trim().is_empty() {
            return idx;
        }
    }
    lines.len()
}

/// True if `paragraph` (a blank-line-delimited run of leading, column-0
/// lines) reads as a version/author/homepage banner rather than
/// descriptive prose. Two independent structural signals — never
/// compared against the tool's own name: an exact two-token `<name>
/// <version>` first line (clap's `"zoxide 0.9.9"`), or any line
/// carrying a URL/email address. Only consulted when a later paragraph
/// exists to fall back to — a lone paragraph matching this shape is
/// kept, since no description is worse than an unusual one. See
/// docs/shapes.md S-068.
pub(super) fn is_banner_paragraph(paragraph: &[&str]) -> bool {
    match paragraph.first() {
        Some(first) if looks_like_name_version_line(first) => return true,
        _ => {}
    }
    paragraph.iter().any(|line| line_has_contact_info(line))
}

/// True if `line` is exactly two whitespace-separated tokens, a
/// name-shaped one followed by a version-shaped one (`"zoxide 0.9.9"`).
/// Exactly two and no more, so a sentence merely mentioning a version
/// number doesn't qualify.
pub(super) fn looks_like_name_version_line(line: &str) -> bool {
    let mut words = line.split_whitespace();
    let (Some(name), Some(version)) = (words.next(), words.next()) else {
        return false;
    };
    if words.next().is_some() {
        return false;
    }
    is_name_shaped_token(name) && looks_like_version_token(version)
}

/// True if `token` is shaped like a version number: an optional leading
/// `v`, then digits/letters/`-`/`_`/`.` with at least one digit and one
/// `.` (`0.9.9`, `v1.75.0`). Both required so a bare number with no dot
/// (`2020`, a copyright year) doesn't qualify.
pub(super) fn looks_like_version_token(token: &str) -> bool {
    let rest = token.strip_prefix('v').unwrap_or(token);
    if rest.is_empty() {
        return false;
    }
    let mut has_digit = false;
    let mut has_dot = false;
    for c in rest.chars() {
        match c {
            '0'..='9' => has_digit = true,
            '.' => has_dot = true,
            c if c.is_ascii_alphabetic() || c == '-' || c == '_' => {}
            _ => return false,
        }
    }
    has_digit && has_dot
}

/// True if `line` contains a URL or an email-shaped token, as a
/// whitespace-delimited word (surrounding `<...>`/trailing `,`/`.`
/// stripped first, so `"<98ajeet@gmail.com>"` matches).
pub(super) fn line_has_contact_info(line: &str) -> bool {
    line.split_whitespace().any(|word| {
        let trimmed = word.trim_matches(|c: char| matches!(c, '<' | '>' | ',' | '.' | '(' | ')'));
        trimmed.starts_with("http://")
            || trimmed.starts_with("https://")
            || looks_like_email(trimmed)
    })
}

/// True if `word` is shaped like an email address: non-empty local part,
/// `@`, and a domain containing a `.` that doesn't start or end with
/// one. Deliberately simple: distinguishes "present" from "absent," not
/// full validation.
pub(super) fn looks_like_email(word: &str) -> bool {
    let Some((local, domain)) = word.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && local
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
        && domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
}

/// True if `paragraph` is the tool's own getopt complaint about the
/// probe's `--help` argument, not descriptive prose, and should be
/// dropped rather than shown as the description — a getopt-family
/// parser with no `--help` flag prints its own complaint
/// (`ssh-keygen`'s `unknown option -- -`, `c_rehash`'s one-line `Usage
/// error; try -h.`), which [`is_banner_paragraph`]'s two signals never
/// catch since it's neither a version banner nor contact info.
///
/// Every line must match [`is_option_error_line`] — required so this
/// fires on `myisamlog`'s four consecutive complaints but refuses a
/// paragraph the moment any line is something else (`crontab`'s
/// differently-worded second line, `vite`'s unrelated error).
///
/// Unlike [`is_banner_paragraph`], checked independently of whether a
/// later paragraph exists to fall back to: a complaint is never a real
/// description, so dropping the tool's only paragraph (`c_rehash`) is
/// still the honest outcome. See docs/shapes.md S-039.
pub(super) fn is_option_error_paragraph(paragraph: &[&str]) -> bool {
    !paragraph.is_empty() && paragraph.iter().all(|line| is_option_error_line(line))
}

/// True if `line` (trimmed) is, on its own, one of the conventional
/// getopt-family "bad option" complaints. Shape: an optional single-token
/// `<name>: ` prefix (the invoking program's name or path), then one of
/// four phrases — `unknown`/`invalid`/`illegal`/`unrecognized`
/// `option`(s) — as the first thing on the (post-prefix) line, with at
/// most a short flag-shaped trailer; or verbatim busybox's `Usage
/// error; try -h.`.
///
/// The prefix strips only when the text before the first `": "` has no
/// whitespace of its own (a bare name/path never does), which is what
/// lets `"Unknown option: help"` (no real prefix) still match on its
/// whole line. The trailer bound (≤24 chars, ≤3 words, ASCII
/// alphanumerics plus light punctuation) is the guard against a real
/// sentence merely mentioning one of these phrases mid-clause, and
/// against a longer continuation clause (`socat`'s `unknown option
/// "--help"; use option "-h" for help`). See docs/shapes.md S-039.
pub(super) fn is_option_error_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    let core = strip_option_error_progname_prefix(trimmed).unwrap_or(trimmed);
    let lower = core.to_ascii_lowercase();
    if lower == "usage error; try -h." || lower == "usage error; try -h" {
        return true;
    }
    const KEYWORDS: [&str; 4] = [
        "unknown option",
        "invalid option",
        "illegal option",
        "unrecognized option",
    ];
    for kw in KEYWORDS {
        let Some(mut tail) = lower.strip_prefix(kw) else {
            continue;
        };
        // Accept the plural ("options") too, without a separate keyword list.
        tail = tail.strip_prefix('s').unwrap_or(tail);
        return option_error_tail_is_shapely(tail);
    }
    false
}

/// Strips a leading `<token>: ` prefix from `line` only when `<token>`
/// itself contains no whitespace — what tells a genuine `<progname>: `
/// prefix apart from a message that merely contains a colon.
pub(super) fn strip_option_error_progname_prefix(line: &str) -> Option<&str> {
    let (prefix, rest) = line.split_once(": ")?;
    if prefix.is_empty() || prefix.chars().count() > 64 || prefix.contains(char::is_whitespace) {
        return None;
    }
    Some(rest)
}

/// True if `tail` (after one of [`is_option_error_line`]'s four keyword
/// phrases) is short and flag-shaped, not the start of a longer
/// sentence.
pub(super) fn option_error_tail_is_shapely(tail: &str) -> bool {
    let tail = tail.trim();
    if tail.is_empty() {
        return true;
    }
    if tail.chars().count() > 24 || tail.split_whitespace().count() > 3 {
        return false;
    }
    tail.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_' | ':' | '\'' | '"' | '.' | ';')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Preamble bleeding into the root description (corpus/zoxide/0.9.9) ---

    /// The regression `corpus/zoxide/0.9.9` guards: clap's
    /// name/author/homepage banner paragraph must not bleed into the
    /// real description. See docs/shapes.md S-068.
    #[test]
    fn zoxide_banner_is_dropped_and_real_description_kept() {
        let parsed = parse_named(ZOXIDE_HELP, "zoxide");
        assert_eq!(
            parsed.description.as_deref(),
            Some("A smarter cd command for your terminal")
        );
    }

    /// A tool with no banner (`tar`'s single leading paragraph) is
    /// unaffected: `paragraphs.len() > 1` never holds.
    #[test]
    fn a_single_paragraph_description_is_never_dropped_as_a_banner() {
        let parsed = parse(TAR_HELP);
        let desc = parsed.description.as_deref().unwrap_or_default();
        assert!(desc.contains("GNU 'tar' saves many files together"));
    }

    /// A lone banner-shaped paragraph with nothing to fall back to is
    /// kept rather than discarded.
    #[test]
    fn a_banner_shaped_paragraph_with_no_fallback_is_kept() {
        let raw = "mytool 1.2.3\nDoes a thing.\n\nUsage: mytool [OPTIONS]\n";
        let parsed = parse(raw);
        // The parser keeps source breaks intact; the sanitizer decides
        // which are structural (spec §4.1).
        assert_eq!(
            parsed.description.as_deref(),
            Some("mytool 1.2.3\nDoes a thing.")
        );
        // Neither line is structural, so both reflow into one paragraph.
        assert_eq!(
            mandible_core::Text::sanitize(parsed.description.as_deref().unwrap()).as_str(),
            "mytool 1.2.3 Does a thing."
        );
    }

    /// A banner detected purely by contact info (no name-version first
    /// line) is dropped without ever comparing against the tool's name.
    #[test]
    fn a_contact_info_only_banner_is_dropped() {
        let raw = "Homepage: https://example.com/mytool\nSupport: help@example.com\n\n\
                    Does a thing well.\n\nUsage: mytool [OPTIONS]\n";
        let parsed = parse_named(raw, "mytool");
        assert_eq!(parsed.description.as_deref(), Some("Does a thing well."));
    }

    /// A multi-sentence banner-shaped first line (more than two tokens)
    /// must not be mistaken for a `<name> <version>` banner just because it
    /// contains a version-looking word partway through.
    #[test]
    fn a_sentence_merely_mentioning_a_version_number_is_not_a_banner() {
        let raw = "Build v2 is faster than v1.\n\nSee the changelog for details.\n\n\
                    Usage: mytool [OPTIONS]\n";
        let parsed = parse(raw);
        // Two paragraphs exist (real fallback content follows), so the
        // banner check genuinely runs and must say no: the line is a
        // whole sentence, not the bare `<name> <version>` shape.
        // The blank line between them survives as a paragraph break
        // (spec §4.1, §9.3).
        assert_eq!(
            parsed.description.as_deref(),
            Some("Build v2 is faster than v1.\n\nSee the changelog for details.")
        );
        assert_eq!(
            mandible_core::Text::sanitize(parsed.description.as_deref().unwrap()).as_str(),
            "Build v2 is faster than v1.\n\nSee the changelog for details."
        );
    }

    // --- leading option-error line is not a description -------------------

    /// `ssh-keygen --help`'s exact defect: its own getopt complaint as
    /// the only leading paragraph must still be dropped, unlike the
    /// banner check above. See docs/shapes.md S-039.
    #[test]
    fn a_leading_option_error_line_is_dropped_even_as_the_only_paragraph() {
        let raw = "unknown option -- -\nusage: ssh-keygen [-q] [-a rounds]\n";
        let parsed = parse_named(raw, "ssh-keygen");
        assert_eq!(parsed.description, None);
        assert!(!parsed.usage.is_empty(), "usage block must survive");
    }

    /// `c_rehash --help`'s degenerate case: one line, busybox-style
    /// `Usage error; try -h.`, no usage block. Still dropped.
    #[test]
    fn a_lone_usage_error_line_is_dropped_with_nothing_left() {
        let parsed = parse_named("Usage error; try -h.\n", "c_rehash");
        assert_eq!(parsed.description, None);
    }

    /// A `<progname>: ` prefix is recognized and stripped before
    /// matching — `ping`'s real shape.
    #[test]
    fn a_progname_prefixed_option_error_line_is_dropped() {
        let raw = "/usr/bin/ping: invalid option -- '-'\n\nUsage: ping [options] <destination>\n";
        let parsed = parse_named(raw, "ping");
        assert_eq!(parsed.description, None);
    }

    /// `myisamlog`'s shape: several consecutive complaints, one
    /// paragraph, all lines matching, so the paragraph is dropped.
    #[test]
    fn a_paragraph_of_several_option_error_lines_is_dropped() {
        let raw = "illegal option: \"--\"\nillegal option: \"-h\"\nillegal option: \"-e\"\n\nUsage: myisamlog\n";
        let parsed = parse_named(raw, "myisamlog");
        assert_eq!(parsed.description, None);
    }

    /// A real description merely containing one of the four phrases
    /// mid-sentence must survive: the phrase has to open the line.
    #[test]
    fn a_sentence_mentioning_invalid_option_mid_clause_survives() {
        let raw = "An invalid option combination here raises an error, so check twice.\n\n\
                    Usage: mytool [OPTIONS]\n";
        let parsed = parse(raw);
        assert_eq!(
            parsed.description.as_deref(),
            Some("An invalid option combination here raises an error, so check twice.")
        );
    }

    /// A leading complaint followed by unrelated content in the same
    /// paragraph must not be dropped — `sshd`'s real shape: its version
    /// banner sits directly under the complaint.
    #[test]
    fn a_mixed_paragraph_with_real_content_is_kept_whole() {
        let raw = "unknown option -- -\nOpenSSH_9.6p1 Ubuntu, OpenSSL 3.0.13\n\n\
                    usage: sshd [-46DdeGiqTtV]\n";
        let parsed = parse_named(raw, "sshd");
        assert_eq!(
            parsed.description.as_deref(),
            Some("unknown option -- -\nOpenSSH_9.6p1 Ubuntu, OpenSSL 3.0.13")
        );
        // Neither line is structural, so the pair reflows once sanitized.
        assert_eq!(
            mandible_core::Text::sanitize(parsed.description.as_deref().unwrap()).as_str(),
            "unknown option -- - OpenSSH_9.6p1 Ubuntu, OpenSSL 3.0.13"
        );
    }

    /// A trailing continuation clause past the terse-flag bound must not
    /// qualify — `socat`'s shape, isolating the trailer bound.
    #[test]
    fn a_trailing_continuation_clause_is_not_a_shapely_trailer() {
        assert!(!is_option_error_line(
            "unknown option \"--help\"; use option \"-h\" for help"
        ));
    }

    /// The busybox `Usage error; try -h.` shape matches verbatim only;
    /// nothing merely resembling it with extra words does.
    #[test]
    fn only_the_exact_usage_error_shape_matches() {
        assert!(is_option_error_line("Usage error; try -h."));
        assert!(!is_option_error_line(
            "Usage error occurred while parsing; try -h."
        ));
    }
}
