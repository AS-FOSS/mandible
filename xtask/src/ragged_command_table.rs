//! The `ragged-command-table` detector (`unparsed-subcommand` shape E,
//! atlas S-104): a command table whose rows carry an optional short-alias
//! prefix (`i, install` beside `add`) ragged-indents its own rows, and a
//! parser keyed on one fixed indent baseline drops the shallower rows and
//! the run of siblings after them.
//!
//! Fixture: `corpus/pnpm/11.22.0/`. Independently reimplemented from
//! `mandible-extract`'s own `scan_ragged_command_row` — agreeing with the
//! thing under test is exactly what a detector must not do.

use mandible_core::{is_command_name_shaped, CommandNode};

/// See `mandible-extract/src/help_text/sections/emit.rs`'s
/// `MAX_ALIAS_CHARS` — the same bound, independently applied.
const MAX_ALIAS_CHARS: usize = 3;

/// One row this detector's own grammar recognizes as a command-table
/// entry, whose primary name is missing from the tree.
pub struct Finding {
    pub name: String,
    pub alias: Option<String>,
    pub row: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn finding_count(&self) -> usize {
        self.findings.len()
    }
}

fn leading_whitespace(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The byte offset of the first run of 2+ spaces in `line`, after some
/// non-whitespace content — this detector's own column-gap finder,
/// independent of the parser's `find_multi_space_gap`.
fn find_gap(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut run = 0usize;
    let mut seen_content = false;
    for (i, b) in bytes.iter().enumerate() {
        if *b == b' ' {
            run += 1;
            if run >= 2 && seen_content {
                return Some(i - run + 1);
            }
        } else {
            if *b != b'\t' {
                seen_content = true;
            }
            run = 0;
        }
    }
    None
}

/// `(primary, alias)` when `name_field` is a bare command name or a short-
/// alias-prefixed pair, mirroring the parser's own `split_ragged_alias_prefix`.
fn split_alias(name_field: &str) -> Option<(String, Option<String>)> {
    if let Some((left, right)) = name_field.split_once(',') {
        let (left, right) = (left.trim(), right.trim());
        if is_command_name_shaped(left)
            && is_command_name_shaped(right)
            && left.chars().count() <= MAX_ALIAS_CHARS
            && left.chars().count() < right.chars().count()
        {
            return Some((right.to_string(), Some(left.to_string())));
        }
        return None;
    }
    is_command_name_shaped(name_field).then(|| (name_field.to_string(), None))
}

/// One ragged command-table row, this detector's own grammar: a gap, a
/// non-empty description with no further gap of its own (excludes packed
/// reference tables like `less --help`'s key-binding summary), and a name
/// field that is a bare name or a short-alias pair.
fn row_entry(line: &str) -> Option<(String, Option<String>)> {
    let gap = find_gap(line)?;
    let name_field = line.get(..gap)?.trim();
    let desc = line.get(gap..)?.trim();
    if desc.is_empty() || find_gap(desc).is_some() {
        return None;
    }
    split_alias(name_field)
}

fn tree_has_name(node: &CommandNode, name: &str) -> bool {
    node.subcommands.iter().any(|c| c.name == name)
        || node.subcommands.iter().any(|c| tree_has_name(c, name))
}

/// Runs of 2+ adjacent [`row_entry`] matches, each row's own name checked
/// against `root`. Adjacency and the run-length floor mirror the parser's
/// own safety net against a lone false positive (`less`'s `v` row).
pub fn detect(raw: &str, root: &CommandNode) -> Report {
    const MIN_RUN: usize = 2;
    let lines: Vec<&str> = raw.lines().collect();
    let mut findings = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some((name, alias)) = row_entry(lines[i]) else {
            i += 1;
            continue;
        };
        let row_indent = leading_whitespace(lines[i]);
        let mut run: Vec<(usize, String, Option<String>)> = vec![(i, name, alias)];
        let mut j = i + 1;
        // Fold this row's own continuation lines exactly the way the
        // parser does, so a wrapped description doesn't end the run early.
        while j < lines.len() {
            let l = lines[j];
            if l.trim().is_empty() || leading_whitespace(l) <= row_indent || find_gap(l).is_some() {
                break;
            }
            j += 1;
        }
        while j < lines.len() {
            let Some((name, alias)) = row_entry(lines[j]) else {
                break;
            };
            let this_indent = leading_whitespace(lines[j]);
            run.push((j, name, alias));
            let mut k = j + 1;
            while k < lines.len() {
                let l = lines[k];
                if l.trim().is_empty()
                    || leading_whitespace(l) <= this_indent
                    || find_gap(l).is_some()
                {
                    break;
                }
                k += 1;
            }
            j = k;
        }
        if run.len() >= MIN_RUN {
            for (idx, name, alias) in &run {
                if !tree_has_name(root, name) {
                    findings.push(Finding {
                        name: name.clone(),
                        alias: alias.clone(),
                        row: lines[*idx].to_string(),
                    });
                }
            }
        }
        i = j;
    }
    Report { findings }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use crate::detector::{Expect, SelfCheck};
use mandible_core::{Provenance, Source};

/// pnpm's own bytes, byte-exact (`corpus/pnpm/11.22.0/help.txt`), trimmed
/// to the two aliased sections that exhibit the shape.
pub(crate) const PNPM_RAGGED_HELP: &str = "\
Manage your dependencies:
      add                  Installs a package and any packages that it depends
                           on. By default, any new package is installed as a
                           prod dependency
   i, install              Install all dependencies for a project
  ln, link                 Connect the local project to another one
  rm, remove               Removes packages from node_modules and from the
                           project's package.json
      unlink               Unlinks a package. Like yarn unlink but pnpm
                           re-installs the dependency after removing the
                           external link
  up, update               Updates packages to their latest version based on the
                           specified range

Other:
   c, config               Manage the pnpm configuration files
      init                 Create a package.json file
      publish              Publishes a package to the registry
      stage                Stage packages for publishing
";

fn node(name: &str) -> CommandNode {
    CommandNode::new(name, Provenance::single(Source::HelpText))
}

fn tree_with(names: &[&str]) -> CommandNode {
    let mut root = node("pnpm");
    root.subcommands = names.iter().map(|n| node(n)).collect();
    root
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "pnpm's own bytes, every ragged row missing from an empty tree",
            why: "the defect itself: 10 rows across two aliased sections (`i, install`, `ln, \
                  link`, `rm, remove`, `unlink`, `up, update`, `c, config`, `init`, `publish`, \
                  `stage`, plus `add` counted once) never reach a tree that has none of them",
            expect: Expect::Fires(10),
            raw: PNPM_RAGGED_HELP.to_string(),
            root: tree_with(&[]),
        },
        SelfCheck {
            name: "pnpm's own bytes, a correctly repaired tree",
            why: "once every row's primary name is a real subcommand, the detector has nothing \
                  left to report — the repaired-family case",
            expect: Expect::Silent,
            raw: PNPM_RAGGED_HELP.to_string(),
            root: tree_with(&[
                "add", "install", "link", "remove", "unlink", "update", "config", "init",
                "publish", "stage",
            ]),
        },
        SelfCheck {
            name: "less's key-binding summary, never mistaken for a command table",
            why: "the false-positive calibration this shape's own guard exists for: each row's \
                  first 2+-space gap lands right after a single letter exactly the way a real \
                  row's name field would, but the text after it is itself column-aligned \
                  (`^E  j  ^N  CR  *  Forward...`), which the detector's own no-further-gap gate \
                  refuses",
            expect: Expect::Silent,
            raw: "\
  h  H                 Display this help.
  q  :q  Q  :Q  ZZ     Exit.

  e  ^E  j  ^N  CR  *  Forward  one line   (or _N lines).
  y  ^Y  k  ^K  ^P  *  Backward one line   (or _N lines).
"
            .to_string(),
            root: tree_with(&[]),
        },
        SelfCheck {
            name: "a lone ragged row with no sibling",
            why: "the run-length floor: one matching row, cheap to produce by accident, must \
                  not be read as a table on its own",
            expect: Expect::Silent,
            raw: "  v                    Edit the current file with $VISUAL or $EDITOR.\n"
                .to_string(),
            root: tree_with(&[]),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fires_on_every_missing_ragged_row_in_pnpms_own_bytes() {
        let report = detect(PNPM_RAGGED_HELP, &tree_with(&[]));
        assert_eq!(
            report.finding_count(),
            10,
            "{:?}",
            report.findings.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn stays_silent_once_every_row_is_present() {
        let root = tree_with(&[
            "add", "install", "link", "remove", "unlink", "update", "config", "init", "publish",
            "stage",
        ]);
        assert_eq!(detect(PNPM_RAGGED_HELP, &root).finding_count(), 0);
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
                "{}: expected {} finding(s), got {:?}",
                case.name,
                expected,
                report.findings.iter().map(|f| &f.name).collect::<Vec<_>>()
            );
        }
    }
}
