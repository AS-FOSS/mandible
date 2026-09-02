//! Structural detectors: verbatim fallback and the argparse unparsed-positional check.
use super::*;

// ----------------------------------------------------------------------
// The detectors themselves
// ----------------------------------------------------------------------

/// `verbatim-fallback`: help text was captured, and the grammar produced no
/// structure from it whatsoever.
///
/// The check is the tree's own shape rather than anything about the text:
/// the root carries unparsed lines and has no flags, no subcommands and no
/// positionals anywhere. That is exactly the state the verbatim tier leaves
/// behind, and it is why this detector is the harness's proving case — the
/// condition is unambiguous, so a disagreement with a human label is a fact
/// about the label rather than about a heuristic's threshold.
pub(crate) struct VerbatimFallback;

impl Detector for VerbatimFallback {
    fn name(&self) -> &'static str {
        "verbatim-fallback"
    }
    fn family(&self) -> Option<&'static str> {
        Some("verbatim-fallback")
    }
    fn describes(&self) -> &'static str {
        "the root has unparsed lines and no flags, subcommands or positionals anywhere — help \
         text came back and the grammar made nothing of it"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        let root = evidence.root;
        if root.unparsed.is_empty() {
            return Vec::new();
        }
        if !tree_is_structureless(root) {
            return Vec::new();
        }
        vec![format!(
            "{} unparsed line(s) and no extracted structure at all",
            root.unparsed.len()
        )]
    }
}

/// True when `node` and everything below it carries no flag, no positional
/// and no child.
fn tree_is_structureless(node: &CommandNode) -> bool {
    node.flags().next().is_none()
        && node.positionals().next().is_none()
        && node.subcommands.iter().all(tree_is_structureless)
        && node.subcommands.is_empty()
}

/// `unparsed-positional`, narrowed to the one shape that can be asserted
/// without a threshold: argparse prints a literal `positional arguments:`
/// heading and lists its operands under it, so a tool whose raw text has
/// that heading and whose root has zero positionals has demonstrably lost
/// them.
///
/// **Deliberately narrower than the family.** `ping4`'s
/// `<destination DNS name or IP address>` and `vim.basic`'s operands are
/// the same family and this detector cannot see them; a broader rule over
/// arbitrary usage lines is exactly where a false-positive rate would come
/// from. Calibration reports the misses as misses rather than letting a
/// narrow rule look like a complete one.
pub(crate) struct UnparsedArgparsePositional;

impl Detector for UnparsedArgparsePositional {
    fn name(&self) -> &'static str {
        "unparsed-argparse-positional"
    }
    fn family(&self) -> Option<&'static str> {
        Some("unparsed-positional")
    }
    fn describes(&self) -> &'static str {
        "raw help has an argparse `positional arguments:` heading with at least one entry under \
         it, and the extracted root has no positionals"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        if evidence.root.positionals().next().is_some() {
            return Vec::new();
        }
        let listed = argparse_positional_names(evidence.raw);
        if listed.is_empty() {
            return Vec::new();
        }
        vec![format!(
            "raw help lists positional argument(s) {} under an argparse heading; the tree has none",
            listed.join(", ")
        )]
    }

    fn self_checks(&self) -> Vec<SelfCheck> {
        argparse_positional_self_checks()
    }
}

/// `uobjnew`'s real bytes (`corpus/rubyobjnew-bpfcc/audit-seed2/help.txt`),
/// the anchor case for both directions of this detector's self-evidence.
pub(crate) const UOBJNEW_HELP: &str = "\
usage: uobjnew [-h] [-l {c,java,ruby,tcl}] [-C TOP_COUNT] [-S TOP_SIZE] [-v] pid [interval]

Summarize object allocations in high-level languages.

positional arguments:
  pid                   process id to attach to
  interval              print every specified number of seconds

options:
  -h, --help            show this help message and exit
  -v, --verbose         verbose mode: print the BPF program (for debugging purposes)
";

/// The same heading carrying an `add_subparsers()` group instead of plain
/// operands — argparse's own rendering, and the nearest false positive this
/// detector has.
pub(crate) const ARGPARSE_SUBPARSER_HELP: &str = "\
usage: widget [-h] {init,build,run} ...

positional arguments:
  {init,build,run}
    init            Initialize a new widget
    build           Build the widget
    run             Run the widget

options:
  -h, --help      show this help message and exit
";

/// An argparse tool with no dashless argument at all: the heading never
/// appears, and zero root positionals is simply the truth about it.
pub(crate) const ARGPARSE_FLAGS_ONLY_HELP: &str = "\
usage: widget [-h] [-v]

options:
  -h, --help     show this help message and exit
  -v, --verbose  chatty
";

/// A block holding both kinds at once — a real operand *and* a subparser
/// group — which is what stops the subparser rule below from being written
/// as "any block containing a `{...}` entry is not about positionals".
pub(crate) const ARGPARSE_MIXED_HELP: &str = "\
usage: widget [-h] path {init,build} ...

positional arguments:
  path            the file to process
  {init,build}
    init          Initialize a new widget
    build         Build the widget

options:
  -h, --help      show this help message and exit
";

fn argparse_positional_node(name: &str, positionals: &[&str], subcommands: &[&str]) -> CommandNode {
    let mut root = CommandNode::new(name, Provenance::single(Source::HelpText));
    root.set_positionals(
        positionals
            .iter()
            .map(|p| {
                let mut positional = Entity::positional(*p, Provenance::single(Source::HelpText));
                positional.required = true;
                positional
            })
            .collect(),
    );
    root.subcommands = subcommands
        .iter()
        .map(|s| CommandNode::new(*s, Provenance::single(Source::HelpText)))
        .collect();
    root
}

/// The hand-built cases this detector is judged on now that the labelled
/// set has nothing left to confirm (spec §13.1e).
///
/// The two `Silent` cases carrying the heading are the ones that do the
/// work. Without them, "fires on a block of names when the tree has no
/// positionals" is satisfied by a rule that fires on *every* argparse tool
/// with a subcommand group, which is the false-alarm class this project
/// refuses — and the fleet-wide zero this detector will now read would be
/// unfalsifiable either way.
fn argparse_positional_self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "uobjnew's two operands, dropped",
            why: "the defect itself: `pid` and `interval` are listed under argparse's own \
                  heading and the tree has no positionals at all",
            expect: Expect::Fires(1),
            raw: UOBJNEW_HELP.to_string(),
            root: argparse_positional_node("rubyobjnew-bpfcc", &[], &[]),
        },
        SelfCheck {
            name: "uobjnew's two operands, recovered",
            why: "the other half of the fleet-count-of-zero question: after the fix the same \
                  bytes must go silent because the operands are in the tree, not because the \
                  rule stopped working",
            expect: Expect::Silent,
            raw: UOBJNEW_HELP.to_string(),
            root: argparse_positional_node("rubyobjnew-bpfcc", &["pid", "interval"], &[]),
        },
        SelfCheck {
            name: "an add_subparsers() group under the same heading",
            why: "the nearest real false positive: argparse prints a subcommand list under \
                  `positional arguments:` too, and a correctly parsed one legitimately leaves \
                  the root with zero positionals — firing here would report a complete parse as \
                  a lost operand",
            expect: Expect::Silent,
            raw: ARGPARSE_SUBPARSER_HELP.to_string(),
            root: argparse_positional_node("widget", &[], &["init", "build", "run"]),
        },
        SelfCheck {
            name: "an argparse tool with no operands at all",
            why: "zero root positionals is the truth for most argparse tools; the heading's \
                  absence, not the count, is what has to keep this silent",
            expect: Expect::Silent,
            raw: ARGPARSE_FLAGS_ONLY_HELP.to_string(),
            root: argparse_positional_node("widget", &[], &[]),
        },
        SelfCheck {
            name: "a real operand sharing the block with a subparser group",
            why: "the subparser rule must skip the `{...}` entry and its deeper rows, not the \
                  whole block — `path` is still a lost operand here, and a rule that discarded \
                  the block wholesale would report this tool as clean",
            expect: Expect::Fires(1),
            raw: ARGPARSE_MIXED_HELP.to_string(),
            root: argparse_positional_node("widget", &[], &["init", "build"]),
        },
    ]
}

/// Names listed under an argparse `positional arguments:` heading: the
/// first token of each row at the block's own entry indent, until a blank
/// or non-indented line ends the block.
///
/// Only rows at the entry indent: argparse writes two other things one
/// level deeper — a wrapped description continuation, and an
/// `add_subparsers()` group's real subcommands under a `{a,b,c}`
/// pseudo-entry. Counting those would turn every correctly parsed argparse
/// subcommand tree into a claimed lost positional. The pseudo-entry itself
/// is dropped by name; a plain operand sharing the block with it still
/// counts (exclusion is per row, not per block).
pub(crate) fn argparse_positional_names(raw: &str) -> Vec<String> {
    let mut lines = raw.lines();
    for line in lines.by_ref() {
        if line
            .trim_end()
            .eq_ignore_ascii_case("positional arguments:")
        {
            break;
        }
    }
    let mut out = Vec::new();
    let mut entry_indent: Option<usize> = None;
    for line in lines {
        if line.trim().is_empty() {
            break;
        }
        if !line.starts_with(' ') && !line.starts_with('\t') {
            break;
        }
        let Some(word) = line.split_whitespace().next() else {
            break;
        };
        // An option row under this heading means the block already ended and
        // the layout is not the one this rule understands; stop rather than
        // report a flag as a positional.
        if word.starts_with('-') {
            break;
        }
        let indent = line.len() - line.trim_start().len();
        let base = *entry_indent.get_or_insert(indent);
        if indent > base {
            continue;
        }
        if word.starts_with('{') {
            continue;
        }
        out.push(word.to_string());
    }
    out
}
