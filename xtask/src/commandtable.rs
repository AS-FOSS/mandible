//! `unparsed-command-table`: a dash-separated command table under a
//! `commands:` heading that reaches the tree as nothing at all.
//!
//! # Why this is not the whole `unparsed-subcommand` family
//!
//! The seed-2 audit labelled 8 tools `unparsed-subcommand`, which made it
//! the largest family in the set. Reading all 8 notes against the tools'
//! own captures shows the label is covering **four unrelated grammars**,
//! not one:
//!
//! | shape | tools | how the subcommand list is written |
//! |---|---|---|
//! | **A — dash-separated command table** | `ar`, `gcc-ar`, `gcc-ar-13`, `aarch64-linux-gnu-ar`, `aarch64-linux-gnu-gcc-ar` | ` commands:` heading, then `  d            - delete file(s) from the archive` |
//! | B — inline label + continuation | `apt-ftparchive` | `Commands: packages binarypath [overridefile [pathprefix]]`, the first entry on the heading's own line |
//! | C — repeated-prefix usage catalogue | `btrfs` | `    btrfs balance start [options] <path>`, no heading anywhere, the name recoverable only by stripping the repeated program name |
//! | D — metavariable alternation set | `ip` | `where  OBJECT := { address \| addrlabel \| ... }`, a brace-delimited pipe-separated set bound to a name used in the usage line |
//!
//! Only shape A is read here, and that is deliberate: the four have no
//! common structure to generalize over, and one detector spanning them
//! would have to be loose enough in each to be worthless in all. B, C and
//! D are named in this detector's [`Scope::known_exclusions`] with a
//! [`Ground::UnreadableEntryShape`] carrying a real line from each tool's
//! own help text, so the misses stay counted and reasoned rather than
//! quietly dropped.
//!
//! Shape A is 5 of the 8, and every one of the 5 is binutils `ar` reached
//! under a different name — an honest caveat this module states rather
//! than hides. The *shape* is nonetheless general (a two-column
//! `name  - description` table under an indented heading is ordinary
//! layout), and the fleet sweep is what settles how general.
//!
//! # What goes wrong
//!
//! `ar`'s help opens with a usage line and then indents everything after
//! it. The usage-block scanner in `mandible-extract`'s
//! `help_text::sections` treats every more-indented line as a synopsis
//! continuation, so ` commands:` and all 8 of its entries — and the two
//! modifier sections after it — are joined into a single `usage` string.
//! The heading is never seen as a heading, so no subcommand is ever
//! recovered: the tree has zero.
//!
//! # What this detector will not do
//!
//! It fires only when the table's names are **all** absent from the tree.
//! A partially-recovered table is not this defect (it is some other one),
//! and firing on it would put the detector in the business of grading
//! recall rather than reporting a section that was ignored wholesale —
//! which is what every one of the 5 audited captures actually shows.

use mandible_core::{is_command_name_shaped, CommandNode, Provenance, Source};
use std::collections::BTreeSet;

/// How many distinct command names a table must offer before a silent
/// tree is read as a defect rather than as noise.
///
/// Two, not one: a single `name  - description` row under a heading that
/// merely *mentions* the word "command" is the shape of an ordinary prose
/// aside, and the fleet is full of them. Two rows sharing one heading and
/// one column layout is a table.
pub const MIN_TABLE_ENTRIES: usize = 2;

/// Ceiling on names collected from one table, so a pathological input
/// (the repeated-banner case `sections.rs` pins at 20,000 lines) cannot
/// turn this scan superlinear. Far above any real command list.
const MAX_TABLE_NAMES: usize = 256;

/// The grammar sentence printed by an exclusion that cites this module —
/// one place, so the report and the check cannot drift apart.
pub const ENTRY_GRAMMAR: &str =
    "an indented `<name>  - <description>` command-table row (two or more spaces, a hyphen, \
     then the description)";

/// One command table whose every name is missing from the tree.
pub struct MissingTable {
    /// The heading line, trimmed, as it appears in the tool's own text.
    pub heading: String,
    /// The command names the table offers, in source order.
    pub names: Vec<String>,
}

/// What [`detect`] found.
pub struct TableReport {
    pub missing: Vec<MissingTable>,
}

/// Leading-whitespace width, in characters.
fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// True if `s` mentions "command(s)"/"subcommand(s)" as a whole word.
///
/// Deliberately a local copy of the rule `help_text::sections` applies,
/// not a shared import: a detector that called the parser's own helper
/// would agree with the parser by construction, and agreeing with the
/// thing under test is exactly what an oracle must not do.
fn mentions_commands_word(s: &str) -> bool {
    s.split(|c: char| !c.is_alphanumeric())
        .any(|w| {
            matches!(
                w.to_lowercase().as_str(),
                "command" | "commands" | "subcommand" | "subcommands"
            )
        })
}

/// True if `line` introduces a command block: a short, colon-terminated
/// label of plain words that names commands.
///
/// The plain-words test is what keeps usage grammar out. A synopsis
/// fragment carries `[`, `<`, `{` or `|`; a heading does not.
pub fn is_command_heading(line: &str) -> bool {
    let trimmed = line.trim();
    let Some(label) = trimmed.strip_suffix(':') else {
        return false;
    };
    if label.is_empty() || label.chars().count() > 60 {
        return false;
    }
    if !label
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_')
    {
        return false;
    }
    mentions_commands_word(label)
}

/// One parsed table row.
pub struct Entry {
    /// The bare command name: the row's token with any trailing bracketed
    /// modifier groups removed (`m[ab]` names the command `m`).
    pub name: String,
}

/// Byte index of the `  - ` column separator in an already-left-trimmed
/// row: a hyphen with two or more spaces before it and whitespace after.
fn dash_separator(trimmed: &str) -> Option<usize> {
    let bytes = trimmed.as_bytes();
    for (idx, b) in bytes.iter().enumerate() {
        if *b != b'-' || idx < 2 {
            continue;
        }
        if bytes[idx - 1] != b' ' || bytes[idx - 2] != b' ' {
            continue;
        }
        match bytes.get(idx + 1) {
            Some(b' ') | Some(b'\t') => return Some(idx),
            _ => continue,
        }
    }
    None
}

/// The bare command name a row's left-column token names, or `None` when
/// the token is not a command name at all.
///
/// The token must open with an ASCII lowercase letter and everything after
/// the leading name must be bracketed modifier groups. This is the whole
/// discrimination between `ar`'s commands and the two *modifier* sections
/// printed in the identical two-column layout directly beneath them:
/// `m[ab]` yields `m`, while `[a]`, `[l <text> ]`, `@<file>`, `--thin` and
/// `--target=BFDNAME` all yield nothing.
fn bare_name(token: &str) -> Option<String> {
    let name: String = token
        .chars()
        .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .collect();
    if name.is_empty() || !is_command_name_shaped(&name) {
        return None;
    }
    let rest = &token[name.len()..];
    if !rest.is_empty() && !is_bracket_groups(rest) {
        return None;
    }
    Some(name)
}

/// True if `s` is one or more `[...]` groups and nothing else.
fn is_bracket_groups(s: &str) -> bool {
    let mut rest = s;
    while let Some(open) = rest.strip_prefix('[') {
        match open.find(']') {
            Some(close) => rest = &open[close + 1..],
            None => return false,
        }
    }
    rest.is_empty() && s.starts_with('[')
}

/// Parse one indented `<name>  - <description>` row.
///
/// Public because [`crate::detector::Ground::UnreadableEntryShape`] checks
/// its own witness against exactly this predicate: an exclusion claiming a
/// tool's entry shape is unreadable is refused if the line it cites parses
/// here.
pub fn parse_entry(line: &str) -> Option<Entry> {
    if line.is_empty() || !line.starts_with([' ', '\t']) {
        return None;
    }
    let trimmed = line.trim_start();
    let idx = dash_separator(trimmed)?;
    let token = trimmed[..idx].trim_end();
    let description = trimmed[idx + 1..].trim();
    if token.is_empty() || description.is_empty() {
        return None;
    }
    Some(Entry {
        name: bare_name(token)?,
    })
}

/// Every command table in `raw` whose names are all absent from `root`.
pub fn detect(raw: &str, root: &CommandNode) -> TableReport {
    let lines: Vec<&str> = raw.lines().collect();
    let present: BTreeSet<String> = root
        .subcommands
        .iter()
        .map(|c| c.name.to_lowercase())
        .collect();

    let mut missing = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !is_command_heading(lines[i]) {
            i += 1;
            continue;
        }
        let heading_indent = indent_of(lines[i]);
        let heading = lines[i].trim().to_string();
        let mut names: Vec<String> = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut j = i + 1;
        while j < lines.len() {
            let l = lines[j];
            if l.trim().is_empty() || indent_of(l) <= heading_indent {
                break;
            }
            if names.len() < MAX_TABLE_NAMES {
                if let Some(e) = parse_entry(l) {
                    if seen.insert(e.name.clone()) {
                        names.push(e.name);
                    }
                }
            }
            j += 1;
        }
        if names.len() >= MIN_TABLE_ENTRIES && names.iter().all(|n| !present.contains(n)) {
            missing.push(MissingTable { heading, names });
        }
        i = j.max(i + 1);
    }
    TableReport { missing }
}

// --- self-check material, byte-exact from the tools' own captures -------

/// `ar`'s real command table, byte-exact from `corpus/ar/audit-seed2/
/// help.txt` — the usage line that starts the swallowing, the heading, all
/// 8 command rows, and the *modifier* section beneath them whose rows use
/// the identical two-column layout and must not be read as commands.
const AR_HELP: &str = "Usage: /usr/bin/ar [emulation options] [-]{dmpqrstx}[abcDfilMNoOPsSTuvV] [--plugin <name>] [member-name] [count] archive-file file...\n       /usr/bin/ar -M [<mri-script]\n commands:\n  d            - delete file(s) from the archive\n  m[ab]        - move file(s) in the archive\n  p            - print file(s) found in the archive\n  q[f]         - quick append file(s) to the archive\n  r[ab][f][u]  - replace existing or insert new file(s) into the archive\n  s            - act as ranlib\n  t[O][v]      - display contents of the archive\n  x[o]         - extract file(s) from the archive\n command specific modifiers:\n  [a]          - put file(s) after [member-name]\n  [b]          - put file(s) before [member-name] (same as [i])\n  [D]          - use zero for timestamps and uids/gids (default)\n";

/// `ar`'s modifier sections *alone*, with the command section removed. The
/// heading still contains the word "command", and every row is in the same
/// column layout — the closest thing in the audited set to a false
/// positive, and the detector must stay silent on it.
const AR_MODIFIERS_ONLY: &str = " command specific modifiers:\n  [a]          - put file(s) after [member-name]\n  [b]          - put file(s) before [member-name] (same as [i])\n  [D]          - use zero for timestamps and uids/gids (default)\n  [N]          - use instance [count] of name\n";

/// `btrfs`'s real subcommand catalogue, byte-exact from
/// `corpus/btrfs/audit-seed2/help.txt` — shape C, the declared exclusion's
/// witness. No heading and no dash column, so nothing here parses as a row.
pub const BTRFS_ENTRY: &str = "    btrfs balance start [options] <path>";

/// `apt-ftparchive`'s real command block opener, byte-exact from its own
/// capture — shape B's witness: the label and the first entry share a line.
pub const APT_FTPARCHIVE_ENTRY: &str = "Commands: packages binarypath [overridefile [pathprefix]]";

/// `ip`'s real object set, byte-exact from
/// `corpus/ip/audit-seed2/help.stderr.txt` — shape D's witness.
pub const IP_ENTRY: &str =
    "where  OBJECT := { address | addrlabel | amt | fou | help | ila | ioam | l2tp |";

/// A correctly-parsed command table: `git`-style rows under a `Commands:`
/// heading, with the tree already carrying every name. The must-stay-silent
/// case that keeps a fleet count of zero meaningful after the fix lands.
const PARSED_TABLE: &str = "Usage: demo <command>\n Commands:\n  clone        - Clone a repository\n  commit       - Record changes\n  push         - Update remote refs\n";

fn node(name: &str, subcommands: &[&str]) -> CommandNode {
    let help = || Provenance::single(Source::HelpText);
    let mut root = CommandNode::new(name, help());
    root.subcommands = subcommands
        .iter()
        .map(|s| CommandNode::new(*s, help()))
        .collect();
    root
}

/// The hand-built cases this detector is judged on once the family is
/// repaired and the labelled set has nothing left to confirm.
///
/// Both directions are present because
/// [`crate::detector::Calibration::self_checks_are_conclusive`] requires
/// them: the must-stay-silent half is what stops a detector that fired on
/// everything from scoring perfectly, and it is where this project's
/// no-false-positives-over-recall rule becomes a gate input instead of a
/// comment.
pub(crate) fn self_checks() -> Vec<crate::detector::SelfCheck> {
    use crate::detector::{Expect, SelfCheck};

    vec![
        SelfCheck {
            name: "ar's real 8-command table, swallowed",
            why: "the defect itself: ` commands:` and all 8 rows joined into the usage string, \
                  leaving a tree with zero subcommands",
            expect: Expect::Fires(1),
            raw: AR_HELP.to_string(),
            root: node("ar", &[]),
        },
        SelfCheck {
            name: "ar's table once the parser recovers it",
            why: "the other half of the fleet-count-of-zero question: after the fix the same \
                  bytes must go silent because the names are in the tree, not because the rule \
                  stopped working",
            expect: Expect::Silent,
            raw: AR_HELP.to_string(),
            root: node("ar", &["d", "m", "p", "q", "r", "s", "t", "x"]),
        },
        SelfCheck {
            name: "ar's modifier section in the identical column layout",
            why: "the nearest real false positive: the heading contains the word `command` and \
                  every row is a two-column dash entry, but `[a]`/`[b]`/`[D]` are modifiers — \
                  only the bare-name test separates them from commands",
            expect: Expect::Silent,
            raw: AR_MODIFIERS_ONLY.to_string(),
            root: node("ar", &[]),
        },
        SelfCheck {
            name: "a correctly parsed Commands: table",
            why: "an ordinary well-formed command table with its names already in the tree — a \
                  detector firing here would be reporting recall, not a swallowed section",
            expect: Expect::Silent,
            raw: PARSED_TABLE.to_string(),
            root: node("demo", &["clone", "commit", "push"]),
        },
        SelfCheck {
            name: "btrfs's repeated-prefix catalogue (shape C)",
            why: "the declared exclusion, asserted rather than assumed: this detector must be \
                  silent on a different grammar even with an empty tree, so the miss is a scope \
                  boundary and not a rule that half-fires",
            expect: Expect::Silent,
            raw: format!(
                "usage: btrfs [global] <group> [<group>...] <command> [<args>]\n\n{BTRFS_ENTRY}\n        Balance chunks across the devices\n    btrfs check [options] <device>\n        Check structural integrity of a filesystem (unmounted).\n"
            ),
            root: node("btrfs", &[]),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ars_eight_commands_and_ignores_its_modifiers() {
        let report = detect(AR_HELP, &node("ar", &[]));
        assert_eq!(report.missing.len(), 1);
        assert_eq!(report.missing[0].heading, "commands:");
        assert_eq!(
            report.missing[0].names,
            vec!["d", "m", "p", "q", "r", "s", "t", "x"]
        );
    }

    #[test]
    fn bracketed_modifier_tokens_are_not_command_names() {
        assert!(bare_name("[a]").is_none());
        assert!(bare_name("[l <text> ]").is_none());
        assert!(bare_name("@<file>").is_none());
        assert!(bare_name("--thin").is_none());
        assert!(bare_name("--target=BFDNAME").is_none());
        assert_eq!(bare_name("m[ab]").as_deref(), Some("m"));
        assert_eq!(bare_name("r[ab][f][u]").as_deref(), Some("r"));
        assert_eq!(bare_name("d").as_deref(), Some("d"));
    }

    #[test]
    fn the_three_excluded_shapes_do_not_parse_as_rows() {
        assert!(parse_entry(BTRFS_ENTRY).is_none());
        assert!(parse_entry(APT_FTPARCHIVE_ENTRY).is_none());
        assert!(parse_entry(IP_ENTRY).is_none());
    }

    #[test]
    fn a_table_already_in_the_tree_is_silent() {
        let root = node("demo", &["clone", "commit", "push"]);
        assert!(detect(PARSED_TABLE, &root).missing.is_empty());
    }

    #[test]
    fn a_usage_fragment_is_not_a_command_heading() {
        assert!(is_command_heading(" commands:"));
        assert!(is_command_heading("Available Commands:"));
        assert!(!is_command_heading("Usage: ip [ OPTIONS ] OBJECT { COMMAND | help }"));
        assert!(!is_command_heading("Options:"));
    }

    /// The repeated-banner input `sections.rs` pins: the detector must not
    /// collect an unbounded name list from it.
    #[test]
    fn a_repeated_banner_does_not_grow_an_unbounded_name_list() {
        let banner = " commands:\n  build        - build it\n  test         - test it\n";
        let raw = banner.repeat(20_000);
        let report = detect(&raw, &node("x", &[]));
        for table in &report.missing {
            assert!(table.names.len() <= MAX_TABLE_NAMES);
            assert_eq!(table.names, vec!["build", "test"]);
        }
    }
}
