//! `lowdown-bullet-command-row` (atlas S-143): lowdown's man-page-like
//! `--help` rendering (nix/Lix, issue #138) writes every subcommand as a
//! `·`-led bullet row, `"nix help - show help..."`, rather than a plain
//! indented row. Independent re-implementation (no shared code with
//! `mandible_extract`, this module's own convention): a bullet row shaped
//! `word1 word2 - description` names a candidate subcommand (`word2`);
//! this detector fires when three or more such candidates appear and none
//! reached the tree. No seed-2/4/5/6/7 labelled tool carries this shape,
//! so [`Detector::family`] returns `None`.

use crate::detector::{Detector, Expect, SelfCheck, ToolEvidence};
use mandible_core::CommandNode;

/// Fewest candidate rows before this detector claims anything — one or
/// two rows matching `word1 word2 - text` is coincidence (ordinary prose
/// reads this way too often); three is the fewest this shape's own
/// specimen (nix) ever shows in one document.
const MIN_CANDIDATE_ROWS: usize = 3;

fn is_lowercase_word(w: &str) -> bool {
    !w.is_empty() && w.chars().all(|c| c.is_ascii_lowercase())
}

/// `line`'s candidate subcommand name, if it is a `·`-marked bullet row
/// shaped `word1 word2 - description`: the middle dot (U+00B7) plus one
/// space, then two lowercase bare words, then a literal ` - `.
fn bullet_command_candidate(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let text = trimmed.strip_prefix('\u{b7}')?.strip_prefix(' ')?;
    let mut words = text.split_whitespace();
    let w1 = words.next()?;
    let w2 = words.next()?;
    if !is_lowercase_word(w1) || !is_lowercase_word(w2) {
        return None;
    }
    let prefix = format!("{w1} {w2} - ");
    text.starts_with(&prefix).then(|| w2.to_string())
}

fn node_has_subcommand(root: &CommandNode, name: &str) -> bool {
    root.subcommands.iter().any(|c| c.name == name)
}

pub struct LowdownBulletCommandRow;

impl Detector for LowdownBulletCommandRow {
    fn name(&self) -> &'static str {
        "lowdown-bullet-command-row"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "a `·`-marked bullet row shaped `word1 word2 - description` never became a subcommand, \
         with three or more such rows in the same document"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        let candidates: Vec<(String, &str)> = evidence
            .raw
            .lines()
            .filter_map(|l| bullet_command_candidate(l).map(|name| (name, l)))
            .collect();
        if candidates.len() < MIN_CANDIDATE_ROWS {
            return Vec::new();
        }
        candidates
            .into_iter()
            .filter(|(name, _)| !node_has_subcommand(evidence.root, name))
            .map(|(name, line)| format!("{name:?} never became a subcommand, from {line:?}"))
            .collect()
    }

    fn self_checks(&self) -> Vec<SelfCheck> {
        self_checks()
    }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use mandible_core::{Provenance, Source};

const THREE_BULLET_ROWS: &str = concat!(
    "  · nix help - show help about nix\n",
    "  · nix build - build a derivation\n",
    "  · nix run - run a Nix application\n",
);

fn node(name: &str, subcommands: &[&str]) -> CommandNode {
    let mut root = CommandNode::new(name, Provenance::single(Source::HelpText));
    root.subcommands = subcommands
        .iter()
        .map(|n| CommandNode::new(*n, Provenance::single(Source::HelpText)))
        .collect();
    root
}

fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "nix's own shape, all three bullet commands unrecovered",
            why: "the defect itself: every bullet row names a real subcommand and the tree \
                  carries none of them",
            expect: Expect::Fires(3),
            raw: THREE_BULLET_ROWS.to_string(),
            root: node("nix", &[]),
        },
        SelfCheck {
            name: "nix's own shape, all three already recovered",
            why: "once every bullet name is a real subcommand, the same rows must go silent",
            expect: Expect::Silent,
            raw: THREE_BULLET_ROWS.to_string(),
            root: node("nix", &["help", "build", "run"]),
        },
        SelfCheck {
            name: "only two candidate rows in the whole document",
            why: "two rows matching the shape is not enough evidence on its own; ordinary prose \
                  reads this way by coincidence too often below the three-row floor",
            expect: Expect::Silent,
            raw: "  · nix help - show help about nix\n  · nix build - build a derivation\n"
                .to_string(),
            root: node("nix", &[]),
        },
        SelfCheck {
            name: "a bullet row that is ordinary prose, not a command entry",
            why: "`Store path` and `This is the default` (nix's own Installables section) must \
                  never be mistaken for a command row: no lowercase-word-word-dash shape, no \
                  claim",
            expect: Expect::Silent,
            raw: "  · Store path \n  · This is the default \n  · Fileish, optionally qualified\n"
                .to_string(),
            root: node("nix", &[]),
        },
    ]
}
