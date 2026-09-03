//! `usage-only-value-name` (atlas S-100): a usage block states a flag's
//! value name (`-q [errorfile]`, `-t tag`) that the flag's own row never
//! carries, so the entity reaches the tree with `value_name: None`.
//!
//! Fixtures: `corpus/nvim/0.9.5/` (`-q [errorfile]`), also present on
//! `corpus/vim.basic/audit-seed4/` (`-q [errorfile]`, `-t tag`).

use mandible_core::CommandNode;

pub struct Finding {
    pub flag: char,
    pub value_name: String,
    pub usage_line: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn finding_count(&self) -> usize {
        self.findings.len()
    }
}

/// The contiguous "Usage:" block: the heading line plus every following
/// non-blank line, the same shape both `vim.basic` (`or:` continuations
/// on the heading's own indent) and `nvim` (bare continuation lines, no
/// repeated heading) use.
fn usage_block(raw: &str) -> Vec<&str> {
    let lines: Vec<&str> = raw.lines().collect();
    let Some(start) = lines
        .iter()
        .position(|l| l.trim_start().to_ascii_lowercase().starts_with("usage:"))
    else {
        return Vec::new();
    };
    let mut block = vec![lines[start]];
    for line in &lines[start + 1..] {
        if line.trim().is_empty() {
            break;
        }
        block.push(line);
    }
    block
}

fn strip_brackets(tok: &str) -> &str {
    tok.trim_start_matches('[').trim_end_matches(']')
}

/// True when `tok` (brackets stripped) reads as a real, lower-case-led
/// value name rather than another flag or a bare word fragment.
fn is_value_like(tok: &str) -> bool {
    let inner = strip_brackets(tok);
    let mut chars = inner.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        }
        _ => false,
    }
}

/// Every `(short flag, value name)` pair a usage line's own tokens state,
/// in order: a bare single-letter short flag immediately followed by a
/// value-like token.
fn usage_value_pairs(line: &str) -> Vec<(char, String)> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let mut pairs = Vec::new();
    for pair in tokens.windows(2) {
        let [flag_tok, value_tok] = pair else {
            continue;
        };
        if flag_tok.len() != 2 || !flag_tok.starts_with('-') {
            continue;
        }
        let letter = flag_tok.chars().nth(1).unwrap();
        if !letter.is_ascii_alphabetic() || !is_value_like(value_tok) {
            continue;
        }
        pairs.push((letter, strip_brackets(value_tok).to_string()));
    }
    pairs
}

fn entity_value_name(root: &CommandNode, short: char) -> Option<String> {
    root.flags()
        .find(|e| e.short() == Some(short))
        .and_then(|e| e.value_name.clone())
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let mut findings = Vec::new();
    for line in usage_block(raw) {
        for (letter, value_name) in usage_value_pairs(line) {
            if entity_value_name(root, letter).is_none() {
                findings.push(Finding {
                    flag: letter,
                    value_name,
                    usage_line: line.to_string(),
                });
            }
        }
    }
    Report { findings }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use crate::detector::{Expect, SelfCheck};
use mandible_core::{Entity, Provenance, Source};

/// nvim's real usage block, byte-exact (`corpus/nvim/0.9.5/help.txt`).
pub(crate) const NVIM_USAGE: &str = concat!(
    "Usage:\n",
    "  nvim [options] [file ...]      Edit file(s)\n",
    "  nvim [options] -t <tag>        Edit file where tag is defined\n",
    "  nvim [options] -q [errorfile]  Edit file with first error\n",
);

/// vim.basic's real usage block, byte-exact
/// (`corpus/vim.basic/audit-seed4/help.txt`).
pub(crate) const VIM_USAGE: &str = concat!(
    "Usage: vim [arguments] [file ..]       edit specified file(s)\n",
    "   or: vim [arguments] -               read text from stdin\n",
    "   or: vim [arguments] -t tag          edit file where tag is defined\n",
    "   or: vim [arguments] -q [errorfile]  edit file with first error\n",
);

fn flag(short: char) -> Entity {
    Entity::flag_spelled(
        Some(short),
        None,
        false,
        false,
        Provenance::single(Source::HelpText),
    )
}

fn flag_with_value(short: char, value_name: &str) -> Entity {
    let mut e = flag(short);
    e.value_name = Some(value_name.to_string());
    e
}

fn node_with_flags(name: &str, flags: Vec<Entity>) -> CommandNode {
    let mut root = CommandNode::new(name, Provenance::single(Source::HelpText));
    root.set_entities_of(mandible_core::EntityKind::Flag, flags);
    root
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "nvim's own bytes, `-q`'s value name dropped",
            why: "the defect itself: `[errorfile]` is stated only in the usage line",
            expect: Expect::Fires(1),
            raw: NVIM_USAGE.to_string(),
            root: node_with_flags("nvim", vec![flag('t'), flag('q')]),
        },
        SelfCheck {
            name: "vim.basic's own bytes, `-t` and `-q` both dropped",
            why: "the same shape, two flags on one tool: a bare value word and a bracketed one",
            expect: Expect::Fires(2),
            raw: VIM_USAGE.to_string(),
            root: node_with_flags("vim.basic", vec![flag('t'), flag('q')]),
        },
        SelfCheck {
            name: "`-q`'s value name already recovered",
            why: "once the entity carries a value name, the usage line must go silent for it",
            expect: Expect::Silent,
            raw: NVIM_USAGE.to_string(),
            root: node_with_flags("nvim", vec![flag('t'), flag_with_value('q', "errorfile")]),
        },
        SelfCheck {
            name: "a usage line with no short-flag-plus-value pair at all",
            why: "an ordinary usage line naming only the program and a bracketed operand must \
                  not be misread as a flag's value",
            expect: Expect::Silent,
            raw: "Usage: prog [file ..]\n".to_string(),
            root: node_with_flags("prog", vec![]),
        },
    ]
}
