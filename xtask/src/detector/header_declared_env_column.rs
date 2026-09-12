//! `header-declared-env-column` (atlas S-166): a header-declared three-
//! column option table (`Argument`/`Env-variable`/`Description`, the
//! whole `qemu-*-static` fleet) whose own header row names its middle
//! column as an environment variable. The unfixed parser glues that
//! column onto the matching flag's description instead of reading it as
//! the flag's own [`Entity::env_var`] cross-reference (spec §4.5, §7
//! Tier B rule 16).
//!
//! Independent re-implementation of the header/row shape — no shared
//! code with `mandible_extract::help_text::sections`, so the detector
//! cannot agree with the parser by construction.
//!
//! The seed-7 audit labels `qemu-riscv64-static` "incomplete" and
//! describes this exact shape ("a triple column help text ... flags
//! being an alias of the env vars"), but no `DEFECT_FAMILIES` entry
//! names it yet and the entry carries no derived family, so
//! [`Detector::family`] returns `None` (spec §13.1e rule 6) rather than
//! forcing it onto an unrelated family.

use crate::detector::{Detector, Expect, Scope, SelfCheck, ToolEvidence};
use mandible_core::CommandNode;

pub struct Finding {
    pub argument: String,
    pub env_var: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

fn is_argument_label(cell: &str) -> bool {
    matches!(
        cell.trim().to_lowercase().as_str(),
        "argument" | "arguments" | "option" | "options" | "flag" | "flags"
    )
}

fn is_env_var_label(cell: &str) -> bool {
    let compact: String = cell
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| !matches!(c, '-' | ' '))
        .collect();
    matches!(
        compact.as_str(),
        "envvariable" | "environmentvariable" | "envvar"
    )
}

fn is_description_label(cell: &str) -> bool {
    cell.trim().eq_ignore_ascii_case("description")
}

/// Column offsets a header row declares for itself, an independent copy
/// of `mandible_extract`'s own reading.
fn header_offsets(line: &str) -> Option<(usize, usize)> {
    let cols: Vec<&str> = line
        .trim()
        .split("  ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let [c0, c1, c2]: [&str; 3] = cols.try_into().ok()?;
    if !is_argument_label(c0) || !is_env_var_label(c1) || !is_description_label(c2) {
        return None;
    }
    let env_col = line.find(c1)?;
    let desc_col = line[env_col..].find(c2)? + env_col;
    Some((env_col, desc_col))
}

/// True when `root` carries the row's own env var as a cross-reference on
/// the matching flag, and that flag's description does not also repeat
/// it — the fixed shape. False for both directions of the defect: the
/// contaminated description, and a flag that never gained `env_var` at
/// all.
fn row_is_clean(root: &CommandNode, arg_name: &str, env_var: &str) -> bool {
    let Some(flag) = root
        .flags()
        .find(|f| f.spellings.iter().any(|s| s.name == arg_name))
    else {
        return false;
    };
    let carries_reference = flag.env_var.as_deref() == Some(env_var);
    let description_clean = !flag
        .description
        .as_ref()
        .is_some_and(|d| d.as_str().contains(env_var));
    carries_reference && description_clean
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let mut findings = Vec::new();
    let lines: Vec<&str> = raw.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let Some((env_col, desc_col)) = header_offsets(lines[i]) else {
            i += 1;
            continue;
        };
        let mut j = i + 1;
        while j < lines.len() && !lines[j].trim().is_empty() {
            let line = lines[j];
            let argument = line.get(..env_col).unwrap_or(line).trim();
            if !argument.starts_with('-') {
                break;
            }
            let env_var = line
                .get(env_col..desc_col)
                .map(str::trim)
                .unwrap_or_default();
            if !env_var.is_empty() {
                let name = argument
                    .split_whitespace()
                    .next()
                    .unwrap_or(argument)
                    .trim_start_matches('-');
                if !row_is_clean(root, name, env_var) {
                    findings.push(Finding {
                        argument: argument.to_string(),
                        env_var: env_var.to_string(),
                    });
                }
            }
            j += 1;
        }
        i = j;
    }
    Report { findings }
}

pub struct HeaderDeclaredEnvColumn;

impl Detector for HeaderDeclaredEnvColumn {
    fn name(&self) -> &'static str {
        "header-declared-env-column"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "a header-declared three-column option table (Argument/Env-variable/Description) whose \
         middle column never reached the matching flag's own env_var cross-reference"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        detect(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| {
                format!(
                    "{:?} never carried {:?} as its own env_var",
                    f.argument, f.env_var
                )
            })
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

use mandible_core::{Entity, Provenance, Spelling, Text};

const QEMU_HEADER_AND_ROW: &str = "Argument             Env-variable         Description\n\
     -cpu model           QEMU_CPU             select CPU (-cpu help for list)\n";

fn flag_with(short: char, description: Option<&str>, env_var: Option<&str>) -> Entity {
    let mut e = Entity::flag_spelled(Some(short), None, false, false, Provenance::default());
    e.spellings = vec![Spelling::single_dash("cpu")];
    e.description = description.map(Text::sanitize);
    e.env_var = env_var.map(str::to_string);
    e
}

fn node_with(flags: Vec<Entity>) -> CommandNode {
    let mut root = CommandNode::new("qemu-riscv64-static", Provenance::default());
    root.set_entities_of(mandible_core::EntityKind::Flag, flags);
    root
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "qemu's own contaminated row, env var glued onto the description",
            why: "the defect itself: QEMU_CPU never reached its own env_var and still sits in \
                  the description",
            expect: Expect::Fires(1),
            raw: QEMU_HEADER_AND_ROW.to_string(),
            root: node_with(vec![flag_with(
                'c',
                Some("QEMU_CPU select CPU (-cpu help for list)"),
                None,
            )]),
        },
        SelfCheck {
            name: "qemu's own row, already read at the header's own offsets",
            why: "once env_var carries the cross-reference and the description is clean, the \
                  same row must go silent",
            expect: Expect::Silent,
            raw: QEMU_HEADER_AND_ROW.to_string(),
            root: node_with(vec![flag_with(
                'c',
                Some("select CPU (-cpu help for list)"),
                Some("QEMU_CPU"),
            )]),
        },
        SelfCheck {
            name: "an ordinary two-column table, no env-variable header at all",
            why: "a table whose header never names an environment-variable column carries no \
                  finding to report",
            expect: Expect::Silent,
            raw: "Option               Description\n-cpu model           select CPU\n".to_string(),
            root: node_with(vec![flag_with('c', Some("select CPU"), None)]),
        },
    ]
}
