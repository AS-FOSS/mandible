//! Backfilling a flag's description from a prose paragraph that names it,
//! for documents whose option table carries no description column.

use super::*;

/// The largest indent at which a line may still be read as flush-left
/// document prose rather than as part of some block's own body.
///
/// Prose paragraphs that document an option are written at the document's
/// own margin; the same sentence *indented under an option row* is that
/// option's continuation text, and already belongs to whichever flag the
/// row named. Keeping the two apart is the whole reason this is a bound
/// and not simply "any line": `java`, `jdeps` and `rg` all write
/// "The --x option …" sentences deep inside another flag's description
/// column, and reading those as standalone paragraphs would attach one
/// flag's prose to a different flag.
pub(super) const MAX_PROSE_PARAGRAPH_INDENT: usize = 3;

/// Fill in descriptions a document wrote as **prose paragraphs keyed by
/// option name**, rather than as text in the option table's own
/// description column.
///
/// `jdeprscan --help` is the specimen. Its `options:` block is a bare list
/// of spellings with no description column at all, and every option's
/// prose lives further down the document in its own flush-left paragraph:
///
/// ```text
/// options:
///         --for-removal
///   -l    --list
/// …
/// The --for-removal option limits scanning or listing to APIs that are
/// deprecated for removal. Cannot be used with a release value of 6, 7, or 8.
///
/// The --list (-l) option prints out the set of deprecated APIs. No scanning is done,
/// so no directory, jar, or class arguments should be provided.
/// ```
///
/// The table parses correctly — the spellings are all recovered — and then
/// every description is dropped on the floor, because nothing in the
/// grammar ever revisits a flag with text found later in the document
/// (measured: 8 flags, 0.0% with text). This is that revisit, and it is a
/// pass over the assembled flag list for the same reason
/// [`repair_repeated_character_flags`] and
/// [`repair_single_dash_long_options`] are: the question it answers needs
/// the whole node's flags, so it cannot be answered at the row that
/// produced any one of them.
///
/// Shape-keyed, never tool-keyed (spec §1). A paragraph qualifies when:
///
/// 1. Every one of its lines sits at indent ≤ [`MAX_PROSE_PARAGRAPH_INDENT`]
///    and none of them starts with `-`, so an option table's own rows and
///    an option's indented continuation text can never be read as one.
/// 2. Its first line opens `The <spelling> option …` — an article, one
///    option spelling, an optional parenthesised alias list, then the word
///    `option`, `flag` or `switch`. That is a *reference* to an option, the
///    one form in which running prose names one unambiguously.
///
/// Two invariants bound what this can cost, and both matter more than the
/// recall it gives up:
///
/// - **It never creates a flag.** A spelling that names nothing already in
///   `flags` is ignored, so a paragraph mentioning an option the tool did
///   not table (`apt-ftparchive`'s `--source-override`) cannot fabricate
///   one — the invention class spec §7 Tier B forbids.
/// - **It never overwrites a description.** Only a flag whose description
///   is `None` can be filled, so a table that already said something keeps
///   saying it (`apropos`'s `--regex` is described in its own table *and*
///   mentioned in a trailing paragraph; the table wins, untouched).
///
/// Matching is by *any* spelling the reference names, primary or
/// parenthesised, which is what makes it independent of how well the table
/// row parsed: jdeprscan's `-l    --list` row yields a flag with
/// `short: 'l'` and no long name at all, and `The --list (-l) option …`
/// still finds it through the `-l` in the parenthetical.
pub(super) fn backfill_prose_paragraph_descriptions(flags: &mut [Entity], lines: &[&str]) {
    if flags.is_empty() {
        return;
    }
    let mut i = 0usize;
    while i < lines.len() {
        if lines[i].trim().is_empty() {
            i += 1;
            continue;
        }
        let start = i;
        while i < lines.len() && !lines[i].trim().is_empty() {
            i += 1;
        }
        let paragraph = &lines[start..i];
        if !paragraph.iter().all(|l| {
            leading_whitespace(l) <= MAX_PROSE_PARAGRAPH_INDENT && !l.trim_start().starts_with('-')
        }) {
            continue;
        }
        let Some(spellings) = prose_option_reference(paragraph[0]) else {
            continue;
        };
        let text = paragraph
            .iter()
            .map(|l| l.trim())
            .collect::<Vec<_>>()
            .join(" ");
        let Some(description) = non_empty_text(&text) else {
            continue;
        };
        for flag in flags.iter_mut() {
            if flag.description.is_some() {
                continue;
            }
            if spellings.iter().any(|s| flag_answers_to_spelling(flag, s)) {
                flag.description = Some(description.clone());
                break;
            }
        }
    }
}

/// Every option spelling named by a paragraph-opening option *reference* —
/// `The --list (-l) option …` → `["--list", "-l"]` — or `None` when the
/// line does not open with one.
///
/// Grammar, all of it required and in this order: an optional article
/// (`The`/`A`/`An`), one dash-led spelling, an optional parenthesised list
/// of further dash-led spellings, then the literal word `option`, `flag` or
/// `switch`. The trailing noun is what distinguishes a reference from a
/// sentence that merely happens to start with a flag-shaped token, and the
/// leading article keeps it clear of an option *table* row, which starts
/// with the spelling itself.
pub(super) fn prose_option_reference(line: &str) -> Option<Vec<String>> {
    let mut words = line.split_whitespace().peekable();
    let first = words.peek()?;
    if matches!(*first, "The" | "A" | "An" | "the" | "a" | "an") {
        words.next();
    }
    let primary = words.next()?;
    if !primary.starts_with('-') || primary.len() < 2 {
        return None;
    }
    let mut spellings = vec![primary.to_string()];
    // An optional parenthesised alias list: `(-? -h)`, `(-l)`.
    if words.peek().is_some_and(|w| w.starts_with('(')) {
        let mut closed = false;
        for word in words.by_ref() {
            let inner = word.trim_start_matches('(').trim_end_matches(')');
            if inner.starts_with('-') && inner.len() >= 2 {
                spellings.push(inner.to_string());
            }
            if word.ends_with(')') {
                closed = true;
                break;
            }
        }
        if !closed {
            return None;
        }
    }
    let noun = words.next()?;
    if !matches!(noun, "option" | "flag" | "switch") {
        return None;
    }
    Some(spellings)
}

/// True if `flag` is the flag `spelling` names — `--list` against its
/// `long`, `-l` against its `short`, and a single-dash long option
/// (`-print-sysroot`) against its long spelling when the entity says
/// that is how the tool spells it.
pub(super) fn flag_answers_to_spelling(flag: &Entity, spelling: &str) -> bool {
    if let Some(long) = spelling.strip_prefix("--") {
        return !long.is_empty() && flag.long() == Some(long) && !flag.single_dash();
    }
    let Some(rest) = spelling.strip_prefix('-') else {
        return false;
    };
    if rest.is_empty() {
        return false;
    }
    let mut chars = rest.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if flag.short() == Some(c) {
            return true;
        }
    }
    flag.single_dash() && flag.long() == Some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `jdeprscan --help` documents every option in its own flush-left
    /// prose paragraph, and its `options:` table has no description column
    /// at all: 8 flags, 0.0% with text before this. The paragraph names the
    /// option it documents, so the two can be associated.
    ///
    /// `--list` is the load-bearing case: the table row `-l    --list`
    /// loses its long spelling to a separate, still-unfixed bug, so the
    /// only way to reach that flag is the `(-l)` in the paragraph's own
    /// parenthetical.
    #[test]
    fn a_prose_paragraph_naming_an_option_supplies_its_description() {
        let help = "Usage: jdeprscan [options] {dir|jar|class} ...\n\
                    \n\
                    options:\n        \
                    --for-removal\n  \
                    -l    --list\n\
                    \n\
                    Scans each argument for usages of deprecated APIs.\n\
                    \n\
                    The --for-removal option limits scanning or listing to APIs that are\n\
                    deprecated for removal.\n\
                    \n\
                    The --list (-l) option prints out the set of deprecated APIs.\n";
        let parsed = parse(help);
        assert_eq!(
            parsed
                .flags
                .iter()
                .find(|f| f.long() == Some("for-removal"))
                .and_then(|f| f.description.as_ref())
                .map(|d| d.as_str()),
            Some(
                "The --for-removal option limits scanning or listing to APIs that are \
                 deprecated for removal."
            )
        );
        assert_eq!(
            parsed
                .flags
                .iter()
                .find(|f| f.short() == Some('l'))
                .and_then(|f| f.description.as_ref())
                .map(|d| d.as_str()),
            Some("The --list (-l) option prints out the set of deprecated APIs.")
        );
    }

    /// The backfill's two hard limits, which are what bound its cost:
    /// it may never invent a flag, and it may never overwrite a
    /// description the table itself supplied.
    ///
    /// Both cases are real. `apt-ftparchive`'s prose mentions
    /// `--source-override`, an option its table never lists; `apropos`
    /// describes `--regex` in its own table *and* mentions it in a
    /// trailing paragraph.
    #[test]
    fn the_prose_backfill_never_invents_a_flag_or_overwrites_a_description() {
        let help = "Usage: tool [options]\n\
                    \n\
                    Options:\n  \
                    -r, --regex                interpret each keyword as a regex\n\
                    \n\
                    The --regex option is enabled by default.\n\
                    \n\
                    The --source-override option can be used to specify a src override file\n";
        let parsed = parse(help);
        assert!(
            !parsed
                .flags
                .iter()
                .any(|f| f.long() == Some("source-override")),
            "a paragraph must never create a flag: {:?}",
            parsed.flags
        );
        assert_eq!(
            parsed
                .flags
                .iter()
                .find(|f| f.long() == Some("regex"))
                .and_then(|f| f.description.as_ref())
                .map(|d| d.as_str()),
            Some("interpret each keyword as a regex"),
            "the table's own description must win"
        );
    }

    /// A "The --x option ..." sentence *indented under another flag's row*
    /// is that flag's continuation text, not a standalone paragraph — so
    /// it must never be lifted out and attached to `--x`. Real shape:
    /// `java`, `jdeps` and `rg` all write such sentences inside a
    /// description column.
    #[test]
    fn an_indented_sentence_is_continuation_text_not_a_prose_paragraph() {
        let help = "Usage: tool [options]\n\
                    \n\
                    Options:\n      \
                    --dry-run\n      \
                    --validate-modules   Validate all modules.\n                  \
                    The --dry-run option may be useful for validating the\n                  \
                    command line.\n";
        let parsed = parse(help);
        assert_eq!(
            parsed
                .flags
                .iter()
                .find(|f| f.long() == Some("dry-run"))
                .map(|f| f.description.is_none()),
            Some(true),
            "an indented sentence belongs to the row above it: {:?}",
            parsed.flags
        );
    }
}
