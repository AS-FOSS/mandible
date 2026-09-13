//! Recognizing a usage label and the program name beside it: the label
//! vocabulary, the tool's own name at the head of a synopsis, and the
//! `<program>: ` diagnostic prefix in front of a label. Split out of
//! `usage.rs` to keep that file under the size ceiling
//! (`scripts/shape_guard.sh`).

use super::*;

/// True if `t` starts with `"usage:"`, case-insensitively.
///
/// Compares raw bytes via `[u8]::get` rather than slicing the `str`, which
/// can panic when a multi-byte character (e.g. a box-drawing glyph) lands
/// off a UTF-8 boundary at the slice point.
pub fn starts_with_usage_prefix(t: &str) -> bool {
    t.as_bytes()
        .get(..6)
        .map(|b| b.eq_ignore_ascii_case(b"usage:"))
        .unwrap_or(false)
}

/// True if `t` starts with `"or:"`, case-insensitively — GNU coreutils'
/// marker for a genuine *alternative* invocation form, distinct from a
/// wrapped continuation of the form above it. Without it, joining every
/// more-indented usage line onto its predecessor would swallow `or:`'s
/// alternative form too. See S-037 and corpus/du/9.4/help.txt.
pub fn starts_with_or_marker(t: &str) -> bool {
    t.as_bytes()
        .get(..3)
        .map(|b| b.eq_ignore_ascii_case(b"or:"))
        .unwrap_or(false)
}

/// True if `t`'s only content, once trimmed, is the word `or` — any case —
/// with an optional trailing colon: `sg_luns`' bare second-form separator
/// (`corpus/sg_luns/1.45`), one whole physical line with nothing else on
/// it. Distinct from [`starts_with_or_marker`], which matches an `or:`
/// *prefix* even when real form content follows the colon on the same
/// line (`ip`'s `or: ip link ...`); a line this predicate matches carries
/// no such content and must contribute none to either usage form.
pub fn is_bare_or_form_separator(t: &str) -> bool {
    t.trim().trim_end_matches(':').eq_ignore_ascii_case("or")
}

/// True if `t`, trimmed, is a short label ending in the word `usage`
/// (case-insensitive), a colon, and nothing else — `perlthanks`'s
/// `Advanced usage:`, distinct from the literal `usage:`
/// [`starts_with_usage_prefix`] alone matches. At most three words, every
/// one plain ASCII alphabetic, so a sentence that merely ends near the
/// word `usage` never qualifies. See S-151, `corpus/perlthanks`.
pub fn starts_with_extended_usage_label(t: &str) -> bool {
    let Some(head) = t.trim().strip_suffix(':') else {
        return false;
    };
    let words: Vec<&str> = head.split_whitespace().collect();
    if words.is_empty() || words.len() > 3 {
        return false;
    }
    if !words
        .last()
        .expect("checked non-empty above")
        .eq_ignore_ascii_case("usage")
    {
        return false;
    }
    words
        .iter()
        .all(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_alphabetic()))
}

/// True if `t`, trimmed, is only a usage label with nothing after it on
/// the same line: the literal `usage:`/`or:` markers with an empty
/// remainder, or [`starts_with_extended_usage_label`]'s generalized form
/// (which by construction carries no remainder either). `fdisk`'s bare
/// `Usage:` line is this shape; the two real forms sit on the lines below
/// it. See S-150.
pub fn is_bare_usage_label(t: &str) -> bool {
    let trimmed = t.trim();
    if starts_with_usage_prefix(trimmed) {
        return trimmed
            .get(6..)
            .map(|rest| rest.trim().is_empty())
            .unwrap_or(true);
    }
    if starts_with_or_marker(trimmed) {
        return trimmed
            .get(3..)
            .map(|rest| rest.trim().is_empty())
            .unwrap_or(true);
    }
    starts_with_extended_usage_label(trimmed)
}

/// True when `t`, trimmed, opens with an alphabetic label of 2 to 20
/// characters, a colon, and immediately (no space) the tool's own `name`
/// at a word boundary — `mksquashfs`'s `SYNTAX:mksquashfs source1 ...`.
/// Distinct from the two already-recognized markers (`usage:`, `or:`),
/// which are matched regardless of what follows; a label glued straight
/// to unrelated text, or to the name with a space, does not qualify.
/// Mirrors `xtask`'s `usage_label_glued_to_program_name` detector, kept as
/// an independent copy per this crate's convention of never depending on
/// `xtask`. Returns the byte offset in `t` where the tool's own name
/// begins, so the caller can drop the label. See S-142, issue #143.
pub fn label_glued_to_tool_name(t: &str, name: &str) -> Option<usize> {
    if name.is_empty() {
        return None;
    }
    let colon_idx = t.find(':')?;
    let label = &t[..colon_idx];
    if label.len() < 2 || label.len() > 20 || !label.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    if label.eq_ignore_ascii_case("usage") || label.eq_ignore_ascii_case("or") {
        return None;
    }
    let after_idx = colon_idx + 1;
    let after = t.get(after_idx..)?;
    if after.is_empty() || after.starts_with(char::is_whitespace) {
        return None;
    }
    // A URL scheme (`https://github.com/ajeetdsouza/zoxide`) glues its own
    // colon straight to a `//` authority, and its path's own basename
    // routinely coincides with the tool's own name (a homepage on the
    // tool's own domain) — the false positive S-142's own atlas entry
    // names. Never a usage label.
    if after.starts_with("//") {
        return None;
    }
    let first_token = after.split_whitespace().next().unwrap_or(after);
    let basename = first_token.rsplit('/').next().unwrap_or(first_token);
    let rest = basename.strip_prefix(name)?;
    let boundary_ok = rest.is_empty()
        || !rest
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
    boundary_ok.then_some(after_idx)
}

/// True if `t` (already trimmed of leading whitespace) begins with `name`
/// at a word boundary. Lets a tool that repeats its own name across lines
/// with no `or:`/`usage:` marker read as two entries rather than one
/// continuation swallowing the other. Word-boundary checked so `git`
/// doesn't also claim `gitk` or `git-foo`. See S-037.
pub fn starts_with_tool_name(t: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    match t.strip_prefix(name) {
        Some(rest) => rest.is_empty() || rest.starts_with(char::is_whitespace),
        None => false,
    }
}

/// True when `t`'s own first whitespace-delimited token spells the tool
/// under different notation than its resolved `name`: as a full path
/// (`/usr/bin/ar` against `ar`), or as the dotted stem a resolved name
/// itself extends (`vim` against `vim.basic`). Twin of
/// [`starts_with_tool_name`], kept as a separate, narrower predicate so
/// every other caller of that one stays exactly as strict as before — used
/// only at `sections/mod.rs`'s `is_own_name` site. See S-108 and
/// `corpus/ar/audit-seed2/help.txt`'s second usage line.
pub fn starts_with_tool_name_spelled_differently(t: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let Some(first) = t.split_whitespace().next() else {
        return false;
    };
    let basename = first.rsplit('/').next().unwrap_or(first);
    basename == name || basename == name.split('.').next().unwrap_or(name)
}

/// True if `t` (already trimmed) is the C `fprintf(stderr, "%s: Usage:
/// ...", argv[0])` idiom's line: the tool's own name, a literal `": "`,
/// then `usage:` case-insensitively. [`starts_with_usage_prefix`] alone
/// misses this since it only tests the line's start. Kept tight: the
/// `usage:` must be preceded by *only* the name and `": "`, never scanned
/// for elsewhere in the line. See S-001 and corpus/nfsidmap/audit-seed/help.txt.
pub fn starts_with_name_prefixed_usage(t: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    t.strip_prefix(name)
        .and_then(|rest| rest.strip_prefix(": "))
        .is_some_and(starts_with_usage_prefix)
}

/// Drop the `<program>: ` prefix [`starts_with_name_prefixed_usage`]
/// recognizes, wherever it sits in front of a usage label — not only the
/// document's first line. `nfsidmap`'s C `fprintf(stderr, "%s: Usage:
/// ...", argv[0])` idiom keeps that prefix glued to its own usage line
/// rendered; the diagnostic prefix is not the label, and a reader wants the
/// label. Returns `t` trimmed and unchanged when the prefix isn't present.
/// See docs/shapes.md S-162 and S-001.
pub(super) fn strip_name_prefixed_usage_label(t: &str, tool_name: Option<&str>) -> String {
    let trimmed = t.trim();
    if let Some(name) = tool_name {
        if starts_with_name_prefixed_usage(trimmed, name) {
            // `starts_with_name_prefixed_usage` already confirmed `trimmed`
            // opens with exactly `"{name}: "`.
            return trimmed[name.len() + 2..].to_string();
        }
    }
    trimmed.to_string()
}

/// True if `t` opens with `name` at a word boundary and its remainder
/// reads as usage-synopsis grammar rather than prose — the unlabelled
/// synopsis convention (`wpa_cli --help` opens `wpa_cli [-p<path>]
/// [-i<ifname>] ...` with no `Usage:` marker at all). A name match alone
/// is not evidence (`"tar is an archiving program..."` starts with `tar`
/// too), so both must hold: the remainder contains a docopt group
/// delimiter (spec §7 Tier B: `[`, `<`, `{`), and it does not read as an
/// English sentence ([`is_prose_sentence`]). See S-001 and wpa_cli's help.
pub fn looks_like_unlabeled_synopsis_line(t: &str, name: &str) -> bool {
    let Some(rest) = t.strip_prefix(name) else {
        return false;
    };
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        return false;
    }
    let rest = rest.trim_start();
    if rest.is_empty() {
        return false;
    }
    rest.contains(['[', '<', '{']) && !is_prose_sentence(rest)
}
