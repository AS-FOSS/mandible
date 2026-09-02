//! The `wrapped-prose-row-boundary` detector (atlas S-027): a description
//! wraps onto a line beginning with a dash-led word, and the grammar reads
//! that continuation as a new flag row, fabricating a flag out of prose.
//!
//! Fixtures: `corpus/zgrep/1.12/`, `corpus/resolvconf/255.4/`.
//! The discriminator and its cost are in `docs/shapes.md` S-027.

use mandible_core::{CommandNode, Entity};

/// One physical line whose leading spelling reached the tree as a
/// fabricated flag.
pub struct Finding {
    /// The fabricated spelling, e.g. `"--exclude"`.
    pub flag: String,
    /// The physical line it was lifted from, verbatim.
    pub line: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn finding_count(&self) -> usize {
        self.findings.len()
    }
}

/// Widest run of consecutive ASCII spaces anywhere in `line`.
fn widest_space_run(line: &str) -> usize {
    let mut widest = 0usize;
    let mut run = 0usize;
    for c in line.chars() {
        if c == ' ' {
            run += 1;
            widest = widest.max(run);
        } else {
            run = 0;
        }
    }
    widest
}

/// `node`'s own flag spellings (`-x`, `--long`), recursively — a
/// fabrication from this shape can in principle land anywhere the raw text
/// reaches, not only at the root.
fn tree_has_spelling(node: &CommandNode, spelling: &str) -> bool {
    node.flags().any(|f| entity_matches(f, spelling))
        || node
            .subcommands
            .iter()
            .any(|c| tree_has_spelling(c, spelling))
}

fn entity_matches(entity: &Entity, spelling: &str) -> bool {
    if let Some(long) = entity.long() {
        if spelling == format!("--{long}") {
            return true;
        }
    }
    if let Some(short) = entity.short() {
        if spelling == format!("-{short}") {
            return true;
        }
    }
    false
}

/// The candidate line's own leading spelling, stripped of trailing
/// punctuation a comma-separated prose list glues on (`"--exclude,"` ->
/// `"--exclude"`), or `None` when the first token isn't dash-led at all.
fn leading_spelling(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let token = trimmed.split_whitespace().next()?;
    let spelling = token.trim_end_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '-'));
    if spelling.starts_with('-') && spelling.len() > 1 {
        Some(spelling)
    } else {
        None
    }
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let mut findings = Vec::new();
    let lines: Vec<&str> = raw.lines().collect();
    for i in 1..lines.len() {
        let cur = lines[i];
        let prev = lines[i - 1];
        if prev.trim().is_empty() || cur.trim().is_empty() {
            // A blank separator means `cur` opens a new paragraph, not a
            // continuation of `prev` — out of this shape entirely.
            continue;
        }
        let Some(spelling) = leading_spelling(cur) else {
            continue;
        };
        if prev.trim_start().starts_with('-') {
            // Gate 2 (see module doc comment): the previous line is itself
            // a dash-led row, so `cur` is just the next entry of an
            // ordinary table, not prose wrapping into one.
            continue;
        }
        let cur_indent = cur.len() - cur.trim_start().len();
        let prev_indent = prev.len() - prev.trim_start().len();
        if cur_indent != prev_indent {
            continue;
        }
        if prev.trim_end().ends_with('.') {
            continue;
        }
        if widest_space_run(cur) >= mandible_extract::help_text::MIN_COLUMN_GAP_SPACES {
            continue;
        }
        if tree_has_spelling(root, spelling) {
            findings.push(Finding {
                flag: spelling.to_string(),
                line: cur.to_string(),
            });
        }
    }
    Report { findings }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use crate::detector::{Expect, SelfCheck};
use mandible_core::{Provenance, Source};

/// `zgrep --help`'s real bytes, byte-exact
/// (`corpus/zgrep/1.12/help.txt`).
pub(crate) const ZGREP_HELP: &str = "\
Usage: /usr/bin/zgrep [OPTION]... [-e] PATTERN [FILE]...
Look for instances of PATTERN in the input FILEs, using their
uncompressed contents if they are compressed.

OPTIONs are the same as for 'grep', except that the following 'grep'
options are not supported: --dereference-recursive (-R), --directories (-d),
--exclude, --exclude-from, --exclude-dir, --include, --null (-Z),
--null-data (-z), and --recursive (-r).

Report bugs to <bug-gzip@gnu.org>.
";

/// `resolvconf --help`'s real bytes, byte-exact
/// (`corpus/resolvconf/255.4/help.txt`).
pub(crate) const RESOLVCONF_HELP: &str = "\
resolvconf -a INTERFACE < FILE
resolvconf -d INTERFACE

Register DNS server and domain configuration with systemd-resolved.

  -h --help     Show this help
     --version  Show package version
  -a            Register per-interface DNS server and domain data
  -d            Unregister per-interface DNS server and domain data
  -f            Ignore if specified interface does not exist
  -x            Send DNS traffic preferably over this interface

This is a compatibility alias for the resolvectl(1) tool, providing native
command line compatibility with the resolvconf(8) tool of various Linux
distributions and BSD systems. Some options supported by other implementations
are not supported and are ignored: -m, -p, -u. Various options supported by other
implementations are not supported and will cause the invocation to fail:
-I, -i, -l, -R, -r, -v, -V, --enable-updates, --disable-updates,
--updates-are-enabled.

See the resolvectl(1) man page for details.
";

fn flag_node(long: Option<&str>, short: Option<char>) -> Entity {
    Entity::flag_spelled(
        short,
        long.map(|s| s.to_string()),
        false,
        false,
        Provenance::single(Source::HelpText),
    )
}

fn node_with_flags(name: &str, flags: Vec<Entity>) -> CommandNode {
    let mut root = CommandNode::new(name, Provenance::single(Source::HelpText));
    root.set_entities_of(mandible_core::EntityKind::Flag, flags);
    root
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "zgrep's own bytes, the paragraph's first misread line",
            why: "the defect itself: the sentence listing unsupported grep options wraps onto a \
                  dash-led line and that line's own leading spelling (`--exclude`) reached the \
                  tree. The sentence actually wraps a second time (`--null-data`), but that \
                  second wrap's own predecessor is itself dash-led — gate 2 silences it along \
                  with every real table row, a deliberate lower bound (see module doc comment)",
            expect: Expect::Fires(1),
            raw: ZGREP_HELP.to_string(),
            root: node_with_flags(
                "zgrep",
                vec![
                    flag_node(Some("exclude"), None),
                    flag_node(Some("null-data"), None),
                ],
            ),
        },
        SelfCheck {
            name: "resolvconf's own bytes, one prose fabrication amid a real table",
            why: "the same shape beside a genuine, correctly-parsed option table: the six real \
                  flags must not be mistaken for more of the same defect, and only the wrapped \
                  sentence's own leading spelling (`-I`) is in this detector's declared reach — \
                  `--enable-updates`, pulled from mid-line on the same wrap, is a real second \
                  fabrication this detector does not claim (see module doc comment)",
            expect: Expect::Fires(1),
            raw: RESOLVCONF_HELP.to_string(),
            root: node_with_flags(
                "resolvconf",
                vec![
                    flag_node(Some("help"), Some('h')),
                    flag_node(Some("version"), None),
                    flag_node(None, Some('a')),
                    flag_node(None, Some('d')),
                    flag_node(None, Some('f')),
                    flag_node(None, Some('x')),
                    flag_node(None, Some('I')),
                ],
            ),
        },
        SelfCheck {
            name: "a genuine aligned two-row option table",
            why: "the nearest real false positive: two dash-led rows at the same indent, each \
                  with a wide description-column gap — the column-gap gate is what keeps this \
                  silent, since the indent-equality and open-sentence gates alone would not",
            expect: Expect::Silent,
            raw: "\
Register DNS server and domain configuration with systemd-resolved.

  -a            Register per-interface DNS server and domain data
  -d            Unregister per-interface DNS server and domain data
"
            .to_string(),
            root: node_with_flags(
                "resolvconf",
                vec![flag_node(None, Some('a')), flag_node(None, Some('d'))],
            ),
        },
        SelfCheck {
            name: "a deeper-indented continuation (C3's own territory)",
            why: "a continuation line indented past its entry is the ordinary whitespace- \
                  continuation shape C3 already reads correctly — this detector's indent- \
                  equality gate must leave it alone even when the row's own flag is real and in \
                  the tree",
            expect: Expect::Silent,
            raw: "\
  -x            Send DNS traffic preferably over
                -this interface, deliberately dash-led
"
            .to_string(),
            root: node_with_flags("prog", vec![flag_node(None, Some('x'))]),
        },
        SelfCheck {
            name: "a new sentence after a completed one",
            why: "the previous physical line ends with a period, so the next line is a fresh \
                  sentence rather than a wrap-in-progress — must stay silent even though its \
                  leading token happens to spell a flag genuinely in the tree",
            expect: Expect::Silent,
            raw: "\
This tool exits zero on success.
-1 is returned on every other outcome.
"
            .to_string(),
            root: node_with_flags("prog", vec![flag_node(None, Some('1'))]),
        },
        SelfCheck {
            name: "wall's real table, a ragged description column mid-list",
            why: "the false alarm calibration actually found: `--timeout <timeout>` is one \
                  character longer than its neighbours, so its own row's gap narrows to a single \
                  space — gate 5 alone would misread it as prose. Gate 2 is what actually saves \
                  it: the row directly above is itself dash-led, so this can never be a wrap out \
                  of prose",
            expect: Expect::Silent,
            raw: "\
Options:
 -g, --group <group>     only send message to group
 -n, --nobanner          do not print banner, works only for root
 -t, --timeout <timeout> write timeout in seconds
"
            .to_string(),
            root: node_with_flags(
                "wall",
                vec![
                    flag_node(Some("group"), Some('g')),
                    flag_node(Some("nobanner"), Some('n')),
                    flag_node(Some("timeout"), Some('t')),
                ],
            ),
        },
        SelfCheck {
            name: "a tab-indented option list with no wide gap at all",
            why: "the other false alarm calibration found: every row's own gap (one tab) is \
                  under the column-gap threshold, and the heading above the list is real prose \
                  (`General arguments:`) — the same surface shape as a genuine wrapped-prose \
                  paragraph. Gate 2 is again what saves it: the second and later rows are each \
                  preceded by another dash-led row",
            expect: Expect::Silent,
            raw: "\
General arguments:

-i <input file>
-l List all supported pastebins
"
            .to_string(),
            root: node_with_flags(
                "pastebinit",
                vec![flag_node(None, Some('i')), flag_node(None, Some('l'))],
            ),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_zgreps_first_wrapped_prose_fabrication() {
        let root = node_with_flags(
            "zgrep",
            vec![
                flag_node(Some("exclude"), None),
                flag_node(Some("null-data"), None),
            ],
        );
        let report = detect(ZGREP_HELP, &root);
        assert_eq!(report.finding_count(), 1);
        assert_eq!(report.findings[0].flag, "--exclude");
    }

    #[test]
    fn stays_silent_on_walls_ragged_description_column() {
        let raw = " -g, --group <group>     only send message to group\n \
                    -n, --nobanner          do not print banner, works only for root\n \
                    -t, --timeout <timeout> write timeout in seconds\n";
        let root = node_with_flags(
            "wall",
            vec![
                flag_node(Some("group"), Some('g')),
                flag_node(Some("nobanner"), Some('n')),
                flag_node(Some("timeout"), Some('t')),
            ],
        );
        assert_eq!(detect(raw, &root).finding_count(), 0);
    }

    #[test]
    fn stays_silent_on_a_genuine_aligned_option_table() {
        let raw = "  -a            Register\n  -d            Unregister\n";
        let root = node_with_flags(
            "prog",
            vec![flag_node(None, Some('a')), flag_node(None, Some('d'))],
        );
        assert_eq!(detect(raw, &root).finding_count(), 0);
    }

    #[test]
    fn every_self_check_holds() {
        for case in self_checks() {
            let expected = match case.expect {
                Expect::Fires(n) => n,
                Expect::Silent => 0,
            };
            let report = detect(&case.raw, &case.root);
            assert_eq!(
                report.finding_count(),
                expected,
                "{}: expected {} finding(s)",
                case.name,
                expected
            );
        }
    }
}
