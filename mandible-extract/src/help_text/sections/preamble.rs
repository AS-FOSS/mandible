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
/// lines — see the description-collection block in
/// [`parse_with_profile`]) reads as a version/author/homepage banner rather
/// than descriptive prose.
///
/// Two independent signals, either sufficient on its own, both purely
/// structural — neither ever compares against the probed tool's own name,
/// which the hard constraint on this fix (spec §7 Tier B, generalized from
/// `zoxide`) requires:
///
/// 1. The paragraph's first line is *exactly* two tokens, `<name>
///    <version>` (clap's own template: `"zoxide 0.9.9"`). A longer first
///    line — even one that happens to contain a version-shaped word,
///    e.g. "Build v2 is faster than v1." — does not qualify: the two-token
///    shape is what a version banner actually looks like, and requiring it
///    exactly is what keeps ordinary prose from matching by accident.
/// 2. Any line in the paragraph carries a URL or an email address —
///    `zoxide`'s own author/homepage lines, and the general shape any
///    framework's templated banner uses for contact info.
///
/// Only ever consulted when a *later* paragraph exists to fall back to
/// (see the call site) — a lone paragraph that happens to match this shape
/// is kept rather than discarded, because degrading to "no description" is
/// worse than keeping a paragraph that looks unusual but is all there is.
pub(super) fn is_banner_paragraph(paragraph: &[&str]) -> bool {
    match paragraph.first() {
        Some(first) if looks_like_name_version_line(first) => return true,
        _ => {}
    }
    paragraph.iter().any(|line| line_has_contact_info(line))
}

/// True if `line` is exactly two whitespace-separated tokens, a
/// name-shaped one followed by a version-shaped one — `"zoxide 0.9.9"`,
/// `"cargo 1.75.0"`. Exactly two tokens and no more: a sentence that merely
/// mentions a version number partway through does not qualify.
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
/// `v`, then a run of digits/letters/`-`/`_`/`.` containing at least one
/// digit and at least one `.` — `0.9.9`, `v1.75.0`, `2.4.0-beta`. Digit and
/// dot are both required so a bare word (`x`) or a bare number with no dot
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

/// True if `line` contains a URL (`http://`/`https://`) or an
/// email-shaped token, as a whitespace-delimited word (common surrounding
/// punctuation — `<...>`, trailing `,`/`.` — stripped first, so
/// `"<98ajeet@gmail.com>"` and `"https://example.com,"` both match).
pub(super) fn line_has_contact_info(line: &str) -> bool {
    line.split_whitespace().any(|word| {
        let trimmed = word.trim_matches(|c: char| matches!(c, '<' | '>' | ',' | '.' | '(' | ')'));
        trimmed.starts_with("http://")
            || trimmed.starts_with("https://")
            || looks_like_email(trimmed)
    })
}

/// True if `word` is shaped like an email address: a non-empty local part,
/// an `@`, and a domain part containing a `.` that doesn't start or end
/// with one, with both sides restricted to characters real addresses and
/// domains actually use. Deliberately simple — this only has to
/// distinguish "an address is present" from "one isn't," not validate one.
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

/// True if `paragraph` (the same leading, blank-line-delimited unit
/// [`is_banner_paragraph`] is consulted on — see the description-collection
/// block in [`parse_with_profile`]) is the tool's own complaint about the
/// probe's argument, not descriptive prose, and should be dropped rather
/// than shown as the DESCRIPTION.
///
/// # The defect
///
/// A program built on a getopt-family option parser that has no `--help`
/// flag answers the probe by treating `--help` as an unrecognized option:
/// it prints its own one-line complaint, then (often, not always) still
/// manages to print a usage line. `ssh-keygen --help` writes exactly two
/// lines to stderr — `unknown option -- -` then its usage block — and
/// `c_rehash --help` writes *one* line and stops there: `Usage error; try
/// -h.`. Neither line describes what the tool does; both describe what the
/// probe did wrong. But step 2 above (leading prose becomes the
/// description) has no way to tell that apart from real descriptive
/// prose — `wpa_cli`'s root description was exactly this kind of banner
/// before that fix landed (see step 2's own comment) — and this shape
/// slipped through the same gap: it is the tool's *own* self-referential
/// complaint, not a version/author banner, so [`is_banner_paragraph`]'s
/// two signals (name-version line, contact info) never fire on it either.
///
/// # The rule
///
/// Every line in the paragraph must match [`is_option_error_line`] below.
/// Requiring *all* lines (not just the first) is what lets this fire on
/// `myisamlog`, whose probe apparently retried against several rejected
/// characters and printed four consecutive complaints
/// (`illegal option: "--"` / `"-h"` / `"-e"` / `"-l"`) with no blank line
/// between them — one paragraph, four lines, every one of them this exact
/// shape — while refusing a paragraph the moment any line in it is
/// something else, e.g. `crontab`'s second line (`crontab: usage error:
/// unrecognized option`, a *different* self-referential message this
/// predicate does not recognize as its own shape) or `vite`'s second line
/// (an unrelated Qt platform-plugin error). Both are still probably junk,
/// but this fix does not claim to know that; it only removes the lines it
/// can name with confidence, per this file's standing rule of narrow
/// predicates over broad ones.
///
/// # Why this can drop the *only* paragraph
///
/// [`is_banner_paragraph`] is only ever consulted when a later paragraph
/// exists to fall back to (see the call site's comment) — dropping a
/// tool's only leading paragraph there would trade a merely-unusual
/// description for no description at all, a worse outcome when the
/// paragraph might still be real prose that happens to look bannerish.
/// This predicate is not exposed to that risk: it recognizes a *complaint*,
/// which is never a description regardless of what else is or isn't
/// available, so it is checked before, and independently of, the
/// banner check — see the call site. `c_rehash`'s entire captured output
/// is its one-line complaint; dropping it leaves the node with no
/// description at all, which is the honest outcome — mandible does not
/// know what `c_rehash` does, and showing the probe's own error about
/// `--help` in the description pane is a worse answer than showing none.
pub(super) fn is_option_error_paragraph(paragraph: &[&str]) -> bool {
    !paragraph.is_empty() && paragraph.iter().all(|line| is_option_error_line(line))
}

/// True if `line` (trimmed) is, on its own, one of the handful of
/// conventional getopt-family "you gave me a bad option" complaints —
/// see [`is_option_error_paragraph`] for why this exists and how it's used.
///
/// # The shape
///
/// An optional single-token `<name>: ` prefix (the invoking program's own
/// name or full path — `nginx: ...`, `/usr/sbin/rpcbind: ...`), then one of
/// four conventional complaints — `unknown`/`invalid`/`illegal`/
/// `unrecognized` `option`(s) — as the very first thing on the (post-prefix)
/// line, with at most a short, flag-shaped trailer (`-- '-'`, `: --help`,
/// `"--help"`); or, verbatim, busybox's `Usage error; try -h.`.
///
/// The prefix is stripped only when the text before the first `": "` has no
/// whitespace of its own — a bare name or path never contains a space, so
/// this is what tells a real `<progname>: ` prefix (`ping`'s
/// `/usr/bin/ping: invalid option -- '-'`) apart from a message that merely
/// *contains* a colon (`debconf-copydb`'s `Unknown option: help`, whose
/// pre-colon text, `"Unknown option"`, has a space and is therefore never
/// mistaken for a program name). Both shapes are handled by the same code
/// path: when the candidate prefix fails the no-whitespace test, stripping
/// is simply skipped and the *whole* line is checked against the four
/// complaints instead, which is exactly what `"Unknown option: help"`
/// needs (the message itself contains the `": "` that a real prefix would
/// have used).
///
/// The trailer bound (at most 24 characters, at most 3 whitespace-separated
/// words, and drawn only from ASCII letters/digits plus a small punctuation
/// set: space, `-`, `_`, `:`, `'`, `"`, `.`, `;`) is the safety argument
/// against the sentence reading: a real description that merely *mentions*
/// one of these phrases mid-clause (GNU tar's `--occurrence[=NUMBER]`
/// entry — hypothetical prose like "an invalid option combination here
/// raises an error" — never has the phrase open the line to begin with, so
/// it never reaches the trailer check at all; a line that *does* open with
/// the phrase but keeps going past a terse flag-shaped trailer (`socat`'s
/// `unknown option "--help"; use option "-h" for help`, whose trailer runs
/// well past both the length and word-count bound) is rejected there
/// instead.
///
/// # Measured
///
/// Over the 2,301 frozen captures in `audit/queue-captures/` (spec
/// §13.1d's frozen queue), measured the honest way — not by re-deriving
/// paragraph collection by hand (which drifts from the real usage-block
/// detection: an early attempt at this measurement undercounted because it
/// didn't recognize `nfsidmap: Usage: ...`'s name-prefixed usage line the
/// way the real scanner does), but by diffing [`parse_with_profile`]'s
/// actual `description` output with and without this predicate wired in,
/// over the same real call path: **116
/// tools** have their DESCRIPTION changed by this fix, among them `ssh`,
/// `ssh-keygen`, `ssh-keyscan`, `ssh-agent`, `sftp`, `slogin`,
/// `ssh-copy-id`, `c_rehash`, `nginx`, `myisamlog`, `ping`, `ping4`,
/// `ping6`, `reset`, `tput`, `tic`, `infocmp`, all fifteen probed `xfs_*`
/// tools, all four `fsck.ext{2,3,4}`/`mke2fs` variants, and the four
/// `debconf-*` tools (full list in this fix's PR description).
///
/// A **broader** shape — the same four keywords or "usage error" occurring
/// anywhere in the tool's raw leading text — additionally matches **52
/// tools** whose description this fix deliberately leaves untouched, each
/// excluded for one of three checked reasons rather than rounded into the
/// total:
///
/// 1. **The line never opens with a recognized phrase, even after
///    prefix-stripping** (9 tools): a multi-token prefix — a timestamp
///    and/or pid, not a bare name — on `filan`'s `2026/08/14 19:31:25
///    filan[18942] E unknown option --help`, `procan`, `socat`, `socat1`
///    (whose `; use option "-h" for help` continuation would also fail the
///    trailer bound on its own); an extra field between the prefix and the
///    message on `dash`/`sh`'s `/bin/dash: 0: Illegal option --` (the
///    shell's own `argv[0]: lineno: message` convention) and `ftp`/`tnftp`'s
///    `ftp: --: unknown option`; and a leading `*** ` marker on
///    `nslookup`'s `*** Invalid option: -help`.
/// 2. **A real banner or unrelated error precedes the complaint as the
///    paragraph's own first line** (7 tools): `debugfs`, `dumpe2fs`,
///    `e2image`, `resize2fs` (`e2image 1.47.0 (5-Feb-2023)`, a
///    three-token version line that also isn't quite
///    [`is_banner_paragraph`]'s two-token shape — a pre-existing,
///    separate gap, not one this fix claims to close), `ntfstruncate`
///    (version plus copyright), and `byobu-quiet`/`byobu-silent` (a `sed:
///    couldn't readlink ...` line ahead of the real `tmux: unknown option
///    -- X` complaint).
/// 3. **The first line matches but a later line in the same paragraph
///    carries real, distinct content** (36 tools) — `is_option_error_paragraph`'s
///    all-lines requirement (above) correctly refuses the whole paragraph
///    rather than guess which lines to drop: `crontab`'s second line
///    (`crontab: usage error: unrecognized option`, a different
///    self-referential message this predicate does not claim to
///    recognize), `sshd`'s second line (its own version banner), `lsof`'s
///    second/third lines (a different diagnostic, then its version
///    banner), `mkfs.xfs`'s second line (`unknown option -\0 `, a literal
///    embedded NUL that correctly fails the trailer's character-class
///    check), and 32 more of the same shape: `Xvfb`, `arptables-nft-save`,
///    `arptables-save`, `cgi-fcgi`, `cpgr`, `cppw`, `delv`, `devlink`,
///    `ebtables-nft-save`, `ebtables-save`, `fuser`, `ip6tables-legacy-save`,
///    `ip6tables-nft-save`, `ip6tables-save`, `iptables-legacy-save`,
///    `iptables-nft-save`, `iptables-save`, `lvmdump`, `mytop`, `nfsconf`,
///    `nfsidmap`, `nsupdate`, `pppoe-discovery`, `pptp`, `prtstat`,
///    `rsyslogd`, `socat-broker.sh`, `socat-chain.sh`, `socat-mux.sh`,
///    `vite`, `xfs_rtcp`, `zipdetails`.
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

/// Strips a leading `<token>: ` prefix from `line` when, and only when,
/// `<token>` itself contains no whitespace — see [`is_option_error_line`]
/// for why that single condition is what tells a genuine `<progname>: `
/// prefix apart from a message that merely contains a colon.
pub(super) fn strip_option_error_progname_prefix(line: &str) -> Option<&str> {
    let (prefix, rest) = line.split_once(": ")?;
    if prefix.is_empty() || prefix.chars().count() > 64 || prefix.contains(char::is_whitespace) {
        return None;
    }
    Some(rest)
}

/// True if `tail` (everything after one of [`is_option_error_line`]'s four
/// keyword phrases) is short and flag-shaped rather than the start of a
/// longer sentence — see that function's doc comment for the false
/// positive (`socat`'s continuation clause) this bound exists to refuse.
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

    /// The regression `corpus/zoxide/0.9.9` guards: clap's own `--help`
    /// template renders `<name> <version>` / author / homepage as one
    /// paragraph, a blank line, then the real description. Before the
    /// preamble fix, every leading column-0 line was concatenated
    /// regardless of that blank line, so the root description read "zoxide
    /// 0.9.9 Ajeet D'Souza <98ajeet@gmail.com> https://... A smarter cd
    /// command for your terminal". Nothing was fabricated or missing, so no
    /// existing gate caught it — this is a direct assertion on the text
    /// itself.
    #[test]
    fn zoxide_banner_is_dropped_and_real_description_kept() {
        let parsed = parse_named(ZOXIDE_HELP, "zoxide");
        assert_eq!(
            parsed.description.as_deref(),
            Some("A smarter cd command for your terminal")
        );
    }

    /// A tool with no banner at all — `tar`'s leading prose is a single
    /// paragraph, no blank line before `Usage:` — must be completely
    /// unaffected by the banner-drop logic: `paragraphs.len() > 1` never
    /// holds, so nothing is ever dropped.
    #[test]
    fn a_single_paragraph_description_is_never_dropped_as_a_banner() {
        let parsed = parse(TAR_HELP);
        let desc = parsed.description.as_deref().unwrap_or_default();
        assert!(desc.contains("GNU 'tar' saves many files together"));
    }

    /// A lone paragraph that *happens* to open with a version-shaped first
    /// line, with nothing after it to fall back to, must be kept rather
    /// than discarded — degrading to "no description" is worse than
    /// keeping a paragraph that merely looks unusual.
    #[test]
    fn a_banner_shaped_paragraph_with_no_fallback_is_kept() {
        let raw = "mytool 1.2.3\nDoes a thing.\n\nUsage: mytool [OPTIONS]\n";
        let parsed = parse(raw);
        // The parser hands the paragraph over with its source breaks
        // intact (spec §4.1: which break is hard-wrapping and which is
        // structure is the sanitizer's call, not the parser's) …
        assert_eq!(
            parsed.description.as_deref(),
            Some("mytool 1.2.3\nDoes a thing.")
        );
        // … and neither line is structural, so both still reflow into one
        // paragraph exactly as they did when the parser flattened them
        // itself.
        assert_eq!(
            mandible_core::Text::sanitize(parsed.description.as_deref().unwrap()).as_str(),
            "mytool 1.2.3 Does a thing."
        );
    }

    /// A banner detected purely by contact info (no name-version first
    /// line) is dropped the same way, and — the general-rule requirement —
    /// this must work without ever comparing against the tool's own name.
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
        // Two *paragraphs* exist here (real fallback content follows), so
        // the banner check genuinely runs — and must say no, because the
        // first paragraph's line is a whole sentence (more than the two
        // bare tokens `<name> <version>` a real banner is), not merely
        // because there's nothing to fall back to.
        // The blank line between them survives as a paragraph break: the
        // detail pane renders one as a blank row (spec §4.1, §9.3), and it
        // only ever could once the parser stopped flattening the two
        // paragraphs into a single space-joined line here.
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

    /// `ssh-keygen --help`'s exact defect, byte-for-byte: its own getopt
    /// complaint about the unrecognized `--help` probe, then its usage
    /// block, and nothing else. The complaint is the *only* leading
    /// paragraph — unlike the banner check above, this must still drop it,
    /// leaving no description at all rather than showing the tool's own
    /// error about the probe.
    #[test]
    fn a_leading_option_error_line_is_dropped_even_as_the_only_paragraph() {
        let raw = "unknown option -- -\nusage: ssh-keygen [-q] [-a rounds]\n";
        let parsed = parse_named(raw, "ssh-keygen");
        assert_eq!(parsed.description, None);
        assert!(!parsed.usage.is_empty(), "usage block must survive");
    }

    /// `c_rehash --help`'s degenerate case: the entire captured output is
    /// one line, busybox-style `Usage error; try -h.`, with no usage block
    /// to recover at all. Still dropped, for the same reason.
    #[test]
    fn a_lone_usage_error_line_is_dropped_with_nothing_left() {
        let parsed = parse_named("Usage error; try -h.\n", "c_rehash");
        assert_eq!(parsed.description, None);
    }

    /// A `<progname>: ` prefix (bare name or full path) is recognized and
    /// stripped before matching the four conventional complaints —
    /// `ping`'s real shape.
    #[test]
    fn a_progname_prefixed_option_error_line_is_dropped() {
        let raw = "/usr/bin/ping: invalid option -- '-'\n\nUsage: ping [options] <destination>\n";
        let parsed = parse_named(raw, "ping");
        assert_eq!(parsed.description, None);
    }

    /// `myisamlog`'s shape: several consecutive complaints, one per
    /// rejected character, with no blank line between them — one
    /// paragraph, several lines, every one of them this exact shape. All
    /// must match for the paragraph to be dropped.
    #[test]
    fn a_paragraph_of_several_option_error_lines_is_dropped() {
        let raw = "illegal option: \"--\"\nillegal option: \"-h\"\nillegal option: \"-e\"\n\nUsage: myisamlog\n";
        let parsed = parse_named(raw, "myisamlog");
        assert_eq!(parsed.description, None);
    }

    /// A real description that merely *contains* one of the four keyword
    /// phrases mid-sentence must never be dropped — the phrase has to open
    /// the (post-prefix) line, not merely occur in it. This is the
    /// `--occurrence`-style false positive the hard constraint on this fix
    /// calls out by name.
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

    /// A leading complaint followed by a *second, unrelated* line in the
    /// same paragraph (no blank line between them) must not be dropped —
    /// `is_option_error_paragraph` requires every line in the paragraph to
    /// match, and refuses to guess which lines to keep. `sshd`'s real
    /// shape: its own version banner sits directly under the complaint.
    #[test]
    fn a_mixed_paragraph_with_real_content_is_kept_whole() {
        let raw = "unknown option -- -\nOpenSSH_9.6p1 Ubuntu, OpenSSL 3.0.13\n\n\
                    usage: sshd [-46DdeGiqTtV]\n";
        let parsed = parse_named(raw, "sshd");
        assert_eq!(
            parsed.description.as_deref(),
            Some("unknown option -- -\nOpenSSH_9.6p1 Ubuntu, OpenSSL 3.0.13")
        );
        // Neither line is structural, so the pair still reflows into one
        // paragraph once sanitized — the parser keeps the breaks, the
        // sanitizer decides about them (spec §4.1).
        assert_eq!(
            mandible_core::Text::sanitize(parsed.description.as_deref().unwrap()).as_str(),
            "unknown option -- - OpenSSH_9.6p1 Ubuntu, OpenSSL 3.0.13"
        );
    }

    /// A trailing continuation clause past the terse-flag bound must not
    /// qualify — `socat`'s real shape (minus its log-format prefix, which
    /// independently also disqualifies it; this isolates the trailer
    /// bound specifically).
    #[test]
    fn a_trailing_continuation_clause_is_not_a_shapely_trailer() {
        assert!(!is_option_error_line(
            "unknown option \"--help\"; use option \"-h\" for help"
        ));
    }

    /// The busybox `Usage error; try -h.` shape matches verbatim, but nothing
    /// that merely resembles it with extra words does.
    #[test]
    fn only_the_exact_usage_error_shape_matches() {
        assert!(is_option_error_line("Usage error; try -h."));
        assert!(!is_option_error_line(
            "Usage error occurred while parsing; try -h."
        ));
    }
}
