//! `trailing-bracket-group-multiword-operand` (atlas S-132): the primary
//! usage line's own trailing bracket group holds two or more real words
//! and yields no positional (`caffeinate`'s `[command arguments...]`).
//! Distinct from `usage_bracket_group_multiword_value` (S-131, a flag's
//! own value) and from `crate::multi_operand_usage_tail` (S-109, a *run*
//! of single-word operands): this recovers *one* operand of two or more
//! words. No seed-2/4/5/6 labelled tool carries this shape, so
//! [`Detector::family`] returns `None`. Fixture: corpus/caffeinate/26.6.2,
//! issue #135.

use crate::detector::{Detector, Expect, Scope, SelfCheck, ToolEvidence};
use mandible_core::CommandNode;

pub struct Finding {
    pub name: String,
    pub line: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

/// The usage line, when it is the *whole* usage entry: no more-indented
/// line follows it. `mandible_extract`'s own scan folds a following,
/// more-indented, non-prose line into the same entry
/// (`primary_synopsis_lines` then spans several physical lines, and every
/// trailing-operand recovery function requires exactly one). `bdftopcf`'s
/// own two-line annotation (`where # for -p is 1, 2, 4, or 8`) is exactly
/// this shape: the parser correctly declines it, and this detector must
/// decline it too rather than disagree with the parser it measures.
fn first_usage_line(raw: &str) -> Option<&str> {
    let mut lines = raw.lines().peekable();
    while let Some(l) = lines.next() {
        if !l.trim_start().to_ascii_lowercase().starts_with("usage:") {
            continue;
        }
        let base_indent = l.len() - l.trim_start().len();
        if let Some(next) = lines.peek() {
            let next_trim = next.trim_start();
            let next_indent = next.len() - next_trim.len();
            if !next_trim.is_empty() && next_indent > base_indent {
                return None;
            }
        }
        return Some(l);
    }
    None
}

/// `s` cut at the first run of two or more consecutive spaces — the
/// description-column boundary a usage line's own inline trailing prose
/// sits behind (`mandible_extract::help_text::MIN_COLUMN_GAP_SPACES`).
fn cut_before_wide_gap(s: &str) -> &str {
    const GAP: usize = 2;
    let mut run = 0usize;
    let mut run_start = None;
    for (i, c) in s.char_indices() {
        if c == ' ' {
            if run == 0 {
                run_start = Some(i);
            }
            run += 1;
            if run >= GAP {
                return &s[..run_start.unwrap()];
            }
        } else {
            run = 0;
        }
    }
    s
}

/// Whitespace-delimited groups, a `[...]` span kept as one group even with
/// internal spaces.
fn group_tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '[' => {
                depth += 1;
                cur.push(c);
            }
            ']' => {
                depth = (depth - 1).max(0);
                cur.push(c);
            }
            c if c.is_whitespace() && depth == 0 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn plain_word(w: &str) -> bool {
    !w.is_empty()
        && w.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && w.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// True when a bracket group's own stripped content is one flag spelling
/// followed by two or more alphabetic-led words — the S-131 shape
/// (`caffeinate`'s `-w Process ID`). Looser than [`plain_word`] (which
/// requires a lowercase lead and would refuse `ID`).
fn looks_like_multiword_value_group(stripped: &str) -> bool {
    let mut words = stripped.split_whitespace();
    let Some(flag) = words.next() else {
        return false;
    };
    if !flag.starts_with('-') || flag.len() < 2 {
        return false;
    }
    let rest: Vec<&str> = words.collect();
    rest.len() >= 2
        && rest
            .iter()
            .all(|w| !w.is_empty() && w.chars().next().is_some_and(|c| c.is_ascii_alphabetic()))
}

/// The combined name of the usage line's own trailing bracket group, when
/// it holds two or more real words (an ellipsis marker, glued or its own
/// token, is never counted as one), something else — flag or not —
/// stands between the program name and it, and another group on the same
/// line already shows the identical multi-word shape as a flag's own
/// value. That last requirement is load-bearing: without it, "two prose
/// words in a bracket" cannot be told apart from a generic "pass this
/// program's own extra options through" aside (`luksformat`'s `[ mkfs
/// options ]`, `xauth`'s `[command arg ...]`, neither a real fixed
/// operand at all).
/// `None` for `true`'s own `Usage: true [ignored command line
/// arguments]`, which has nothing at all before the group.
fn trailing_multiword_operand_name(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let idx = lower.find("usage:")?;
    let after = &line[idx + "usage:".len()..];
    let before_desc = cut_before_wide_gap(after);
    let groups = group_tokens(before_desc.trim());
    // The program name, at least one group ahead of the trailing one, and
    // the trailing group itself.
    if groups.len() < 3 {
        return None;
    }
    let has_sibling_multiword_value = groups[1..groups.len() - 1]
        .iter()
        .any(|g| looks_like_multiword_value_group(g.trim_matches(|c| c == '[' || c == ']')));
    if !has_sibling_multiword_value {
        return None;
    }
    let last = groups.last()?;
    let stripped = last.trim_matches(|c| c == '[' || c == ']');
    let mut words = stripped.split_whitespace();
    let raw_first = words.next()?;
    let first = raw_first.trim_end_matches('.');
    if !plain_word(first) {
        return None;
    }
    let mut name = first.to_string();
    let mut real_word_count = 1usize;
    for w in words {
        if w.chars().all(|c| c == '.') {
            continue;
        }
        let w_clean = w.trim_end_matches('.');
        if !plain_word(w_clean) {
            return None;
        }
        name.push(' ');
        name.push_str(w_clean);
        real_word_count += 1;
    }
    (real_word_count >= 2).then_some(name)
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let Some(line) = first_usage_line(raw) else {
        return Report {
            findings: Vec::new(),
        };
    };
    let Some(name) = trailing_multiword_operand_name(line) else {
        return Report {
            findings: Vec::new(),
        };
    };
    let findings = if root.positionals().any(|p| p.primary_name() == name) {
        Vec::new()
    } else {
        vec![Finding {
            name,
            line: line.to_string(),
        }]
    };
    Report { findings }
}

pub struct TrailingBracketGroupMultiwordOperand;

impl Detector for TrailingBracketGroupMultiwordOperand {
    fn name(&self) -> &'static str {
        "trailing-bracket-group-multiword-operand"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "the primary usage line's own trailing bracket group holds two or more real words, with \
         or without an ellipsis marker, and the tree carries no positional for it at all"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        detect(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| format!("{:?} never became a positional, from {:?}", f.name, f.line))
            .collect()
    }

    fn scope(&self) -> Scope {
        Scope::full()
    }

    fn self_checks(&self) -> Vec<SelfCheck> {
        self_checks()
    }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use mandible_core::{Entity, Provenance, Source};

/// `caffeinate`'s real usage line, byte-exact (corpus/caffeinate/26.6.2's
/// `help.stderr.txt`).
pub(crate) const CAFFEINATE_USAGE: &str =
    "usage: caffeinate [-disu] [-t timeout] [-w Process ID] [command arguments...]\n";

fn node_with_positionals(name: &str, names: &[&str]) -> CommandNode {
    let mut root = CommandNode::new(name, Provenance::single(Source::HelpText));
    let entities = names
        .iter()
        .map(|n| Entity::positional(*n, Provenance::single(Source::HelpText)))
        .collect();
    root.set_entities_of(mandible_core::EntityKind::Positional, entities);
    root
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "caffeinate's own bytes, the trailing operand dropped entirely",
            why: "the defect itself: `[command arguments...]` documents the tool's only real \
                  positional and the root carries none",
            expect: Expect::Fires(1),
            raw: CAFFEINATE_USAGE.to_string(),
            root: node_with_positionals("caffeinate", &[]),
        },
        SelfCheck {
            name: "caffeinate's own bytes, the operand already recovered",
            why: "once the tree carries `command arguments`, the same usage line must go silent",
            expect: Expect::Silent,
            raw: CAFFEINATE_USAGE.to_string(),
            root: node_with_positionals("caffeinate", &["command arguments"]),
        },
        SelfCheck {
            name: "true's own bytes, nothing between the program name and the group",
            why: "`Usage: true [ignored command line arguments]` is prose about forgiving \
                  argument handling, not an operand list — nothing stands between the program \
                  name and the group, the same guard `recover_primary_tail_operands` needs for \
                  this identical counter-example",
            expect: Expect::Silent,
            raw: "Usage: /usr/bin/true [ignored command line arguments]\n".to_string(),
            root: node_with_positionals("true", &[]),
        },
        SelfCheck {
            name: "a single-word trailing group, a different shape entirely",
            why: "one real word is `crate::tail_operand`'s own `unparsed-positional` family, not \
                  this one — the group must hold two or more real words before this detector \
                  claims anything",
            expect: Expect::Silent,
            raw: "usage: prog [-a] file\n".to_string(),
            root: node_with_positionals("prog", &[]),
        },
        SelfCheck {
            name: "bdftopcf's own bytes, the same split caffeinate has",
            why: "`-o pcf file` is itself the S-131 shape beside the trailing `[bdf file]`, the \
                  sibling evidence this detector requires — a real fleet specimen, not just \
                  caffeinate's own",
            expect: Expect::Fires(1),
            raw: "usage: /usr/bin/bdftopcf [-p#] [-o pcf file] [bdf file]\n".to_string(),
            root: node_with_positionals("bdftopcf", &[]),
        },
        SelfCheck {
            name: "bdftopcf's own real bytes, its own two-line annotation follows",
            why: "a full-`PATH` sweep's own false alarm: `where # for -p is 1, 2, 4, or 8` folds \
                  into the same usage entry as a more-indented continuation, which the parser's \
                  own trailing-operand recovery already declines to touch (it requires exactly \
                  one physical line) — this detector must decline it on the same evidence \
                  instead of disagreeing with the parser it measures",
            expect: Expect::Silent,
            raw: "/usr/bin/bdftopcf: invalid option '--help'\n\
                  usage: /usr/bin/bdftopcf [-p#] [-o pcf file] [bdf file]\n\
                  \x20\x20\x20\x20\x20\x20\x20where # for -p is 1, 2, 4, or 8\n\
                  \x20\x20\x20\x20\x20\x20\x20and   # for -u is 1, 2, or 4\n"
                .to_string(),
            root: node_with_positionals("bdftopcf", &[]),
        },
        SelfCheck {
            name: "luksformat's own bytes, `mkfs options` is a generic aside, not an operand",
            why: "a full-`PATH` sweep's own false alarm: `[ mkfs options ]` means \"pass mkfs's \
                  own options through\", not a fixed two-word operand name, and no earlier group \
                  on the line shows the sibling multi-word-value shape this detector requires",
            expect: Expect::Silent,
            raw: "Usage: luksformat [-t <file system>] <device> [ mkfs options ]\n".to_string(),
            root: node_with_positionals("luksformat", &[]),
        },
        SelfCheck {
            name: "xauth's own bytes, `command arg` is two operands, not one",
            why: "the other false alarm the same sweep found: `[-options ...]` is a flag \
                  placeholder holding no sibling multi-word value, so `command` and `arg` stay \
                  unclaimed by this detector rather than fused into one guessed name",
            expect: Expect::Silent,
            raw: "usage:  /usr/bin/xauth [-options ...] [command arg ...]\n".to_string(),
            root: node_with_positionals("xauth", &[]),
        },
    ]
}
