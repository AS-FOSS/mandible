//! The family-detector calibration harness: the seam a fleet-wide defect
//! detector registers itself in, and the confusion matrix that says
//! whether its fleet-wide number may be quoted yet.
//!
//! A family detector generalizes one human finding across every tool on
//! `PATH`. It is not a correctness instrument — only the audit (`xtask
//! audit`, spec §13.1c) touches truth. A detector's fleet-wide count
//! ("814 tools exhibit this defect") means nothing until calibrated: it
//! must fire on the known-bad tools and stay silent on the known-good
//! ones ([`calibrate`]), or it repeats the "measures itself" trap
//! [M-10]/`%flags_text` already fell into (spec §13.1b).
//!
//! The calibration set is 94 human verdicts from the seed-2 audit, sorted
//! into defect families by a machine reading of reviewer prose plus
//! fixture evidence (`Entry::families`/`families_derived`). Three limits,
//! all printed above every rendered matrix: families are derived, not the
//! verdicts themselves; 94 tools (~4% of `PATH`) validates correctness,
//! not the fleet number's magnitude; only tools with a
//! `corpus/<tool>/audit-seed2/` fixture are evaluable — others are
//! counted separately ([`Calibration::not_evaluable`]), never dropped.
//!
//! [`Detector`] mirrors the shape of the two existing fleet oracles
//! (`detect(raw, root)`, no probes of its own). A new detector implements
//! three methods and adds one line to [`registry`].
//! [`Detector::family`] returns `None` when it generalizes no family the
//! audit ever labelled (the existence oracle: no reviewer reported a
//! fabricated name) — not calibratable, and saying so is the honest
//! result.

use crate::corpus;
use mandible_core::audit::{self, AuditFile};
use mandible_core::{CommandNode, Entity, Provenance, Source};
use std::collections::BTreeMap;
use std::path::Path;

mod calibration;
mod commands;
mod detectors_families;
mod detectors_flags;
mod detectors_misc;
mod detectors_structural;
mod render;

// Round-4 family detectors (atlas S-106 to S-111). Nested here, unlike the
// earlier detector modules under `xtask/src/`, because `xtask/src/main.rs`
// is already at its own size ceiling (spec AGENTS.md §2).
pub(crate) mod glued_optional_group_spelling;
pub(crate) mod multi_operand_usage_tail;
pub(crate) mod or_joined_alias_with_values;
pub(crate) mod underscore_in_long_option;
pub(crate) mod usage_alternative_or_prefix;
pub(crate) mod usage_program_word_mismatch;

// Round-5 family detectors (three parser families: same-spelling-fold-loss,
// bare-or-usage-separator, usage-spelling-duplicates-table-row). Each
// implements `Detector` directly rather than the separate
// detect()/Report + wrapper split the round-4 modules above use — no
// wrapper is needed since none of these three shares its detection logic
// with anything else in this file.
pub(crate) mod bare_or_usage_separator;
pub(crate) mod same_spelling_fold_loss;
pub(crate) mod usage_spelling_duplicates_table_row;

pub(crate) use calibration::*;
pub(crate) use commands::*;
pub(crate) use detectors_families::*;
pub(crate) use detectors_flags::*;
pub(crate) use detectors_misc::*;
pub(crate) use detectors_structural::*;
pub(crate) use render::*;

/// Everything a detector is allowed to look at for one tool: the raw help
/// text the pipeline built from, and the tree it produced. Deliberately no
/// tool name is offered to the *check* — spec §1's no-per-tool-logic rule is
/// structural here, not a convention, because a detector that cannot see a
/// name cannot special-case one.
pub struct ToolEvidence<'a> {
    pub raw: &'a str,
    pub root: &'a CommandNode,
}

/// A registered family detector.
pub trait Detector {
    /// Stable identifier, used by `--detector` and printed in reports.
    fn name(&self) -> &'static str;

    /// The `mandible_core::audit` defect family this detector claims to
    /// generalize, or `None` when it generalizes none that the labelled set
    /// contains — see the module doc comment on why that is a first-class
    /// answer rather than a gap to be filled with the nearest match.
    fn family(&self) -> Option<&'static str>;

    /// One line saying what this detector looks for, printed in `list` and
    /// above every calibration matrix so a reader can judge the check
    /// itself, not just its score.
    fn describes(&self) -> &'static str;

    /// Why this detector believes its family is present in `evidence`. An
    /// empty vector means silent. Returning the *reasons* rather than a
    /// bare `bool` is what makes a disagreement checkable by hand: a false
    /// alarm is only useful if you can see what the detector thought it saw.
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String>;

    /// What this detector claims to catch, and by name what it
    /// deliberately does not (mirrors `corpus/README.md`'s `verdict_scope`).
    /// Default is the full family with no declared exclusion.
    ///
    /// A tool in [`Scope::known_exclusions`] moves calibration's
    /// silent-on-labelled-bad cell from a false negative to a named
    /// out-of-scope miss ([`Calibration::out_of_scope_misses`]) only when
    /// actually silent this run — never excuses a false alarm.
    fn scope(&self) -> Scope {
        Scope::full()
    }

    /// Hand-built cases this detector asserts about itself: the defective
    /// shape constructed directly, plus correct parses that look like it,
    /// each with the expected finding count.
    ///
    /// On the trait, not in `#[cfg(test)]`, because once a family is
    /// repaired the labelled set has nothing left to confirm against —
    /// "zero, bug gone" and "zero, detector broken" are indistinguishable
    /// from the fleet number alone (spec §13.1e). [`calibrate`] needs this
    /// before reporting [`Verdict::Repaired`]; [`ratchet_at_zero`] needs it
    /// before accepting a fleet count of zero.
    ///
    /// Default is empty: a detector with no self-evidence can never be
    /// called repaired.
    fn self_checks(&self) -> Vec<SelfCheck> {
        Vec::new()
    }
}

/// A detector's declared scope: see [`Detector::scope`].
pub struct Scope {
    /// One line saying what the detector claims to catch, printed above
    /// every calibration matrix next to [`Detector::describes`].
    pub claim: &'static str,
    /// Tools the detector's own author already knows it will not catch.
    pub known_exclusions: &'static [Exclusion],
}

impl Scope {
    /// No declared exclusion: every labelled member of the family is in
    /// scope. The default for a detector that has not measured a
    /// deliberate gap.
    pub fn full() -> Self {
        Scope {
            claim: "every labelled member of the family (no declared exclusion)",
            known_exclusions: &[],
        }
    }
}

/// One tool a detector declares it will not catch.
///
/// The reason is not free text: declaring an exclusion converts a blocking
/// false negative into a non-blocking named miss, so [`Ground`] enforces
/// the justification mechanically — the exclusion carries a witness token
/// and a constant, and the out-of-scope arithmetic is computed from the
/// witness, never asserted by the author.
pub struct Exclusion {
    /// The tool, as `mandible_core::audit::Entry::tool` spells it.
    pub tool: &'static str,
    /// The structural property that puts it out of scope, checkable.
    pub ground: Ground,
    /// Human context, printed alongside — never *instead of* — the ground.
    /// Prose may add colour here; it may not carry the justification.
    pub note: &'static str,
}

/// The structural property that puts a tool out of a detector's scope.
///
/// A closed set on purpose: a genuinely new exclusion kind means adding a
/// variant here with its own `holds` arm, a reviewable change — unlike
/// typing a new sentence into a `&str`.
pub enum Ground {
    /// The tool's real shape is a member cluster carrying fewer swallowed
    /// members than the detector's declared minimum. `cluster` is the
    /// literal token (e.g. `"-hU"`); the swallowed-member count is derived
    /// from it, not stated, so [`Ground::holds`] refuses a cluster that
    /// isn't actually below threshold. `threshold` should be the constant
    /// itself (`bundling::MIN_BUNDLED_MEMBERS`), not a retyped copy.
    BelowMemberThreshold {
        cluster: &'static str,
        constant: &'static str,
        threshold: usize,
    },
    /// The tool writes its subcommand list in a grammar the detector does
    /// not read. `entry` is a real line from the tool's own help text; the
    /// ground holds only when that line does not parse as the detector's
    /// entry shape (checked by running the detector's own row parser over
    /// it). `grammar` names the shape that was looked for.
    UnreadableEntryShape {
        entry: &'static str,
        grammar: &'static str,
    },

    /// The two spellings the label is about are not on one flag-spec
    /// fragment: a run of spaces at least `column_gap` wide (the layout
    /// splitter's description-column boundary,
    /// `help_text::MIN_COLUMN_GAP_SPACES`) sits between them, so everything
    /// right of it reaches the grammar as a description. `row` is the
    /// literal row; the gap is measured from it, not asserted. `column_gap`
    /// should be the constant itself, not a retyped copy.
    AcrossDescriptionColumn {
        row: &'static str,
        constant: &'static str,
        column_gap: usize,
    },

    /// The alias separator sits inside a brace alternation group, which the
    /// manifest labels as a family of its own; the tool is a genuine member
    /// of both families and this detector claims only one. `token` is the
    /// literal group (e.g. `"{-v | --version}"`); both its shape and
    /// `family` (must be a manifest-declared family) are checked.
    InsideAlternationGroup {
        token: &'static str,
        family: &'static str,
    },
    /// The witness writes its tail in brackets, so the grammar records the
    /// value as `ValueKind::Optional` — outside a Required-only fingerprint
    /// by construction. A property of the token's shape, not of the tool.
    OptionalBracketedTail { token: &'static str },
    /// The witness carries a tail that is not an option name, so no rule
    /// reading a tail as the rest of a name can claim it. Again a property
    /// of the token, checked rather than asserted.
    TailIsNotAnOptionName { token: &'static str },
}

/// Characters an option name may be spelled with — used by
/// [`Ground::TailIsNotAnOptionName`] to recompute its own disqualification
/// rather than take an author's word for it.
fn is_option_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-'
}

/// Everything after a witness token's leading `-` and first flag character.
fn witness_tail(token: &str) -> String {
    token.chars().skip(2).collect()
}

/// The widest run of spaces in `row` that sits between two `-`-initial
/// tokens — the measurement [`Ground::AcrossDescriptionColumn`] is judged
/// on, computed from the witness rather than stated by its author.
fn widest_gap_between_spellings(row: &str) -> usize {
    let mut widest = 0usize;
    let mut seen_spelling = false;
    let mut run = 0usize;
    for word in row.split_inclusive(' ') {
        if word == " " {
            run += 1;
            continue;
        }
        let token = word.trim_end();
        if seen_spelling && token.starts_with('-') {
            widest = widest.max(run);
        }
        seen_spelling |= token.starts_with('-');
        run = if word.ends_with(' ') { 1 } else { 0 };
    }
    widest
}

impl Ground {
    /// Swallowed members implied by the witness: a cluster is one leading
    /// `-`, one surviving flag character, and the rest swallowed.
    ///
    /// Zero for every ground that is not about a cluster — the number is
    /// only meaningful for [`Ground::BelowMemberThreshold`], and
    /// [`Ground::explain`] never prints it for the others.
    pub fn swallowed_members(&self) -> usize {
        match self {
            Ground::BelowMemberThreshold { cluster, .. } => {
                cluster.chars().count().saturating_sub(2)
            }
            // Not cluster grounds: none of these swallows members, and
            // nothing reads this for them. Listed variant by variant rather
            // than caught by `_` so that adding a ground has to come with a
            // decision here instead of silently defaulting to zero.
            Ground::UnreadableEntryShape { .. }
            | Ground::AcrossDescriptionColumn { .. }
            | Ground::InsideAlternationGroup { .. }
            | Ground::OptionalBracketedTail { .. }
            | Ground::TailIsNotAnOptionName { .. } => 0,
        }
    }

    /// Whether the declared ground actually puts this tool out of scope.
    ///
    /// `Err` is a *declaration* bug, not a detector result: it means
    /// someone wrote an exclusion whose own witness does not support it.
    /// [`validate_registry_scopes`] runs this over every registered
    /// detector, so such an entry cannot reach a calibration report.
    pub fn holds(&self) -> Result<(), String> {
        match self {
            Ground::BelowMemberThreshold {
                cluster,
                constant,
                threshold,
            } => {
                if !cluster.starts_with('-') || cluster.chars().count() < 2 {
                    return Err(format!(
                        "witness {cluster:?} is not a short-flag cluster token (it must start \
                         with `-` and name at least one flag character)"
                    ));
                }
                if *threshold == 0 {
                    return Err(format!(
                        "{constant} is 0, so nothing can be below it — a threshold of zero \
                         excludes everything and justifies nothing"
                    ));
                }
                let swallowed = self.swallowed_members();
                if swallowed >= *threshold {
                    return Err(format!(
                        "witness {cluster:?} swallows {swallowed} member(s), which is NOT below \
                         {constant} = {threshold} — this tool is inside the detector's declared \
                         scope and a miss on it is a false negative, not an exclusion"
                    ));
                }
                Ok(())
            }
            Ground::UnreadableEntryShape { entry, grammar } => {
                if entry.trim().is_empty() {
                    return Err(
                        "the witness line is empty, so it evidences nothing about the tool's \
                         entry shape"
                            .to_string(),
                    );
                }
                if crate::commandtable::parse_entry(entry).is_some() {
                    return Err(format!(
                        "witness {entry:?} DOES parse as {grammar} — this tool's entry shape is \
                         one the detector reads, so a miss on it is a false negative, not an \
                         exclusion"
                    ));
                }
                Ok(())
            }
            Ground::AcrossDescriptionColumn {
                row,
                constant,
                column_gap,
            } => {
                if *column_gap == 0 {
                    return Err(format!(
                        "{constant} is 0, so every pair of spellings is 'across the column' and \
                         this ground would excuse every miss there is"
                    ));
                }
                let gap = widest_gap_between_spellings(row);
                if gap == 0 {
                    return Err(format!(
                        "witness row {row:?} does not carry two `-`-initial spellings with a \
                         space run between them, so there is no column gap in it to measure"
                    ));
                }
                if gap < *column_gap {
                    return Err(format!(
                        "witness row {row:?} separates its spellings by {gap} space(s), which is \
                         NOT at least {constant} = {column_gap} — the two reach the grammar as \
                         one fragment and a miss on them is a false negative, not an exclusion"
                    ));
                }
                Ok(())
            }
            Ground::InsideAlternationGroup { token, family } => {
                let inner = token
                    .strip_prefix('{')
                    .and_then(|t| t.strip_suffix('}'))
                    .ok_or_else(|| {
                        format!(
                            "witness {token:?} is not a brace alternation group (it must be \
                             wrapped in `{{`/`}}`)"
                        )
                    })?;
                let spellings = inner
                    .split(['|', ','])
                    .filter(|m| m.trim().starts_with('-'))
                    .count();
                if spellings < 2 {
                    return Err(format!(
                        "witness {token:?} alternates {spellings} flag spelling(s), not the 2 an \
                         alias pair needs — nothing about it is a dropped alias, so it justifies \
                         no exclusion"
                    ));
                }
                if mandible_core::audit::family_meaning(family).is_none() {
                    return Err(format!(
                        "{family:?} is not a defect family this manifest declares, so naming it \
                         moves the miss nowhere"
                    ));
                }
                Ok(())
            }
            Ground::OptionalBracketedTail { token } => {
                if !token.starts_with('-') || token.chars().count() < 2 {
                    return Err(format!(
                        "witness {token:?} is not a flag token (it must start with `-` and name \
                         at least one flag character)"
                    ));
                }
                let tail = witness_tail(token);
                if !tail.contains('[') || !tail.contains(']') {
                    return Err(format!(
                        "witness {token:?} writes no bracketed tail, so its value is not \
                         Optional and this tool is inside the detector's declared scope — a miss \
                         on it is a false negative, not an exclusion"
                    ));
                }
                Ok(())
            }
            Ground::TailIsNotAnOptionName { token } => {
                if !token.starts_with('-') || token.chars().count() < 3 {
                    return Err(format!(
                        "witness {token:?} is not a flag token with a tail (it must start with \
                         `-`, name a flag character, and carry at least one more)"
                    ));
                }
                let tail = witness_tail(token);
                if tail.chars().all(is_option_name_char) {
                    return Err(format!(
                        "witness {token:?} has the option-name-shaped tail {tail:?}, so this tool \
                         is inside the detector's declared scope — a miss on it is a false \
                         negative, not an exclusion"
                    ));
                }
                Ok(())
            }
        }
    }

    /// The structural sentence printed in the report, generated from the
    /// witness rather than written by the exclusion's author.
    pub fn explain(&self) -> String {
        match self {
            Ground::BelowMemberThreshold {
                cluster,
                constant,
                threshold,
            } => format!(
                "the real token {cluster:?} swallows {} member(s), below {constant} = {threshold} \
                 — a property of the token's shape, not of the tool",
                self.swallowed_members()
            ),
            Ground::UnreadableEntryShape { entry, grammar } => format!(
                "the tool's own line {entry:?} is not {grammar} — a property of how this tool \
                 writes its list, not of the tool"
            ),
            Ground::AcrossDescriptionColumn {
                row,
                constant,
                column_gap,
            } => format!(
                "the real row {row:?} separates its two spellings by {} space(s), at or above \
                 {constant} = {column_gap} — so the layout splitter cuts there and the pair never \
                 reaches the flag-spec grammar as one fragment. A property of the row's spacing, \
                 not of the tool",
                widest_gap_between_spellings(row)
            ),
            Ground::InsideAlternationGroup { token, family } => format!(
                "the real token {token:?} alternates {} flag spelling(s) inside a brace group, \
                 which is the separately-labelled {family:?} family ({}). A property of the \
                 token's shape, not of the tool",
                token
                    .trim_matches(['{', '}'])
                    .split(['|', ','])
                    .filter(|m| m.trim().starts_with('-'))
                    .count(),
                mandible_core::audit::family_meaning(family).unwrap_or("(undeclared)"),
            ),
            Ground::OptionalBracketedTail { token } => format!(
                "the real token {token:?} writes its tail in brackets, which the grammar records \
                 as ValueKind::Optional — outside a Required-only fingerprint by construction, \
                 a property of the token's shape, not of the tool"
            ),
            Ground::TailIsNotAnOptionName { token } => {
                let tail = witness_tail(token);
                let offender: String = tail.chars().filter(|c| !is_option_name_char(*c)).collect();
                format!(
                    "the real token {token:?} has the tail {tail:?}, which carries {offender:?} — \
                     not an option-name character, so no rule that reads a tail as the rest of a \
                     name can claim it. A property of the token's shape, not of the tool"
                )
            }
        }
    }
}

/// Every registered detector's declared exclusions, checked against their
/// own witnesses. Any `Err` is a declaration that was written without a
/// structural reason that survives arithmetic.
pub fn validate_registry_scopes() -> Result<(), String> {
    for d in registry() {
        for exclusion in d.scope().known_exclusions {
            exclusion.ground.holds().map_err(|e| {
                format!(
                    "{}: declared exclusion of {:?} does not hold: {e}",
                    d.name(),
                    exclusion.tool
                )
            })?;
        }
    }
    Ok(())
}

// ----------------------------------------------------------------------
// Self-checks: the evidence a fleet count of zero has to be read against
// ----------------------------------------------------------------------

/// What a [`SelfCheck`] case demands of the detector.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Expect {
    /// The defective shape, built by hand: the detector must report exactly
    /// this many findings on it.
    Fires(usize),
    /// A real, correct parse that structurally resembles the defect: the
    /// detector must report nothing at all. **The half that makes the
    /// evidence worth anything** — a detector that fired indiscriminately
    /// would satisfy every `Fires` case there is.
    Silent,
}

impl Expect {
    /// The exact number of findings this case demands.
    pub fn expected_hits(&self) -> usize {
        match self {
            Expect::Fires(n) => *n,
            Expect::Silent => 0,
        }
    }
}

/// One hand-built case a detector offers as evidence about itself — see
/// [`Detector::self_checks`].
pub struct SelfCheck {
    /// Printed in every report; name it after the real tool or shape it was
    /// built from, so a failure says what stopped working.
    pub name: &'static str,
    /// Why this case is worth asserting — which condition it exercises, and
    /// which real counter-example it stands for.
    pub why: &'static str,
    pub expect: Expect,
    pub raw: String,
    pub root: CommandNode,
}

/// One [`SelfCheck`] after the detector was actually run on it.
pub struct SelfCheckOutcome {
    pub name: &'static str,
    pub why: &'static str,
    pub expect: Expect,
    /// Whether the detector reported exactly what the case demanded.
    pub held: bool,
    /// What it reported, so a failure is checkable by hand rather than a
    /// bare boolean.
    pub hits: Vec<String>,
}

/// Run every one of `detector`'s own hand-built cases through it.
///
/// Costs nothing: the cases carry their own frozen bytes and hand-built
/// trees, so this spawns no subprocess and reads no fixture.
pub fn run_self_checks(detector: &dyn Detector) -> Vec<SelfCheckOutcome> {
    detector
        .self_checks()
        .into_iter()
        .map(|case| {
            let hits = detector.hits(&ToolEvidence {
                raw: &case.raw,
                root: &case.root,
            });
            SelfCheckOutcome {
                name: case.name,
                why: case.why,
                expect: case.expect,
                held: hits.len() == case.expect.expected_hits(),
                hits,
            }
        })
        .collect()
}

/// Whether a set of outcomes is strong enough to stand in for a labelled
/// set that has nothing left to confirm. Three load-bearing conditions:
/// non-empty (an empty `all(...)` is vacuously true), every case held, and
/// at least one `Fires` plus one `Silent` case (either alone is
/// satisfiable by a detector useless in the opposite direction).
pub fn self_checks_are_conclusive(outcomes: &[SelfCheckOutcome]) -> bool {
    !outcomes.is_empty()
        && outcomes.iter().all(|o| o.held)
        && outcomes
            .iter()
            .any(|o| matches!(o.expect, Expect::Fires(_)))
        && outcomes.iter().any(|o| o.expect == Expect::Silent)
}

/// Render a self-check block for a report, listing every case by name.
///
/// Printed unconditionally by both consumers, exactly like the declared
/// out-of-scope list: it is the evidence a claim rests on, so it never
/// becomes something a clean run gets to omit.
pub fn render_self_checks(outcomes: &[SelfCheckOutcome]) -> String {
    let mut s = format!(
        "SELF-CHECK EVIDENCE — the detector's own hand-built cases, re-run just now ({} \
         declared):\n",
        outcomes.len()
    );
    if outcomes.is_empty() {
        s.push_str(
            "  (this detector declares NO self-check. It can never be reported as repaired and \
             can never satisfy the ratchet gate: with no case of its own, a fleet count of zero \
             is indistinguishable from the detector having been deleted.)\n",
        );
        return s;
    }
    for o in outcomes {
        let demand = match o.expect {
            Expect::Fires(n) => format!("must fire {n}x"),
            Expect::Silent => "must stay silent".to_string(),
        };
        let mark = if o.held { "held" } else { "FAILED" };
        s.push_str(&format!(
            "  [{mark:<6}] {:<16} {}\n      why: {}\n",
            demand, o.name, o.why
        ));
        if !o.held {
            s.push_str(&format!(
                "      {RED}got {} hit(s): {:?}{RESET}\n",
                o.hits.len(),
                o.hits
            ));
        }
    }
    s
}

/// Every detector this build knows about.
///
/// **Adding a detector is one line here plus one `impl Detector`.** Nothing
/// else in this module is per-detector, which is the whole point of the
/// seam: the next family detector inherits the calibration report, the
/// caveat banner, the evaluability accounting and the CLI for free.
pub fn registry() -> Vec<Box<dyn Detector>> {
    vec![
        Box::new(VerbatimFallback),
        Box::new(UnparsedArgparsePositional),
        Box::new(BundledShortFlag),
        Box::new(BraceAlternationFlag),
        Box::new(UnparsedCommandTable),
        Box::new(DroppedAliasDetector),
        Box::new(SingleDashLong),
        Box::new(RepeatedCharFlag),
        Box::new(ExistenceOracle),
        Box::new(MisattributionOracle),
        Box::new(WrappedProseRowBoundary),
        Box::new(UnparsedTailOperand),
        Box::new(PlusPrefixedOption),
        Box::new(EndOfOptionsMarker),
        Box::new(SingleSpaceDescriptionColumn),
        Box::new(UsageOnlyValueName),
        Box::new(SecondOptionalValueDropped),
        Box::new(ParentheticalQualifierAsValue),
        Box::new(OrJoinedAlias),
        Box::new(UnderscoreInLongOption),
        Box::new(UsageAlternativeOrPrefix),
        Box::new(UsageProgramWordMismatch),
        Box::new(MultiOperandUsageTail),
        Box::new(OrJoinedAliasWithValues),
        Box::new(GluedOptionalGroupSpelling),
        Box::new(same_spelling_fold_loss::SameSpellingFoldLoss),
        Box::new(bare_or_usage_separator::BareOrUsageSeparator),
        Box::new(usage_spelling_duplicates_table_row::UsageSpellingDuplicatesTableRow),
    ]
}

/// Look one detector up by [`Detector::name`].
pub fn find(name: &str) -> anyhow::Result<Box<dyn Detector>> {
    registry()
        .into_iter()
        .find(|d| d.name() == name)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no detector named {name:?} — registered: {}",
                registry()
                    .iter()
                    .map(|d| d.name())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::Text;

    fn node(name: &str) -> CommandNode {
        CommandNode::new(name, Provenance::single(Source::HelpText))
    }

    /// The row-level exclusions in [`argparse_positional_names`], asserted
    /// directly rather than only through the detector that calls it — a
    /// block whose every row is a subparser artefact must yield *no names*,
    /// which is what turns the detector silent, and one real operand
    /// sharing that block must survive.
    #[test]
    fn an_argparse_subparser_group_lists_no_positional_names() {
        assert_eq!(
            argparse_positional_names(ARGPARSE_SUBPARSER_HELP),
            Vec::<String>::new()
        );
        assert_eq!(
            argparse_positional_names(ARGPARSE_MIXED_HELP),
            vec!["path".to_string()]
        );
        assert_eq!(
            argparse_positional_names(UOBJNEW_HELP),
            vec!["pid".to_string(), "interval".to_string()]
        );
        assert_eq!(
            argparse_positional_names(ARGPARSE_FLAGS_ONLY_HELP),
            Vec::<String>::new()
        );
    }

    fn case(tool: &str, judged_defect: bool, families: &[&str], root: CommandNode) -> Case {
        Case {
            tool: tool.to_string(),
            families: families.iter().map(|f| f.to_string()).collect(),
            judged_defect,
            evidence: Some(ReplayedCase {
                raw: String::new(),
                root,
            }),
        }
    }

    /// A detector whose firing is controlled by the test, so the harness's
    /// own arithmetic is checked against a known answer rather than against
    /// whatever a real heuristic happens to do.
    struct Stub {
        fires_on: Vec<&'static str>,
    }

    impl Detector for Stub {
        fn name(&self) -> &'static str {
            "stub"
        }
        fn family(&self) -> Option<&'static str> {
            Some("verbatim-fallback")
        }
        fn describes(&self) -> &'static str {
            "fires on a fixed list of node names"
        }
        fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
            if self.fires_on.contains(&evidence.root.name.as_str()) {
                vec!["stub fired".to_string()]
            } else {
                Vec::new()
            }
        }
    }

    #[test]
    fn every_cell_of_the_matrix_gets_its_own_tool() {
        let cases = vec![
            case("hit", true, &["verbatim-fallback"], node("hit")),
            case("miss", true, &["verbatim-fallback"], node("miss")),
            case("alarm", false, &[], node("alarm")),
            case("quiet", false, &[], node("quiet")),
            case("other", true, &["dropped-alias"], node("other")),
            Case {
                tool: "nofixture".to_string(),
                families: vec!["verbatim-fallback".to_string()],
                judged_defect: true,
                evidence: None,
            },
        ];
        let stub = Stub {
            fires_on: vec!["hit", "alarm", "other"],
        };
        let cal = calibrate(&stub, &cases, vec!["mystery".to_string()]);

        assert_eq!(cal.true_positives.len(), 1);
        assert_eq!(cal.true_positives[0].0, "hit");
        assert_eq!(cal.false_negatives, vec!["miss"]);
        assert_eq!(cal.false_alarms.len(), 1);
        assert_eq!(cal.false_alarms[0].0, "alarm");
        assert_eq!(cal.true_negatives, vec!["quiet"]);
        assert_eq!(cal.fires_on_other_defect.len(), 1);
        assert_eq!(cal.fires_on_other_defect[0].0, "other");
        assert_eq!(cal.not_evaluable, vec!["nofixture"]);
        assert_eq!(cal.unclassified, vec!["mystery"]);
        assert!(
            cal.verdict() != Verdict::Passes,
            "a false alarm and a miss must both block"
        );
    }

    /// A tool a human judged wrong but nobody could classify must not count
    /// as a miss: no label says it belongs to this family, so a silent
    /// detector has not failed on it.
    #[test]
    fn an_unclassified_defect_is_not_a_false_negative() {
        let cases = vec![case("mystery", true, &[], node("mystery"))];
        let cal = calibrate(&Stub { fires_on: vec![] }, &cases, Vec::new());
        assert!(cal.false_negatives.is_empty());
        assert!(cal.true_positives.is_empty());
        assert!(
            cal.verdict() != Verdict::Passes,
            "nothing was demonstrated, so it cannot pass"
        );
    }

    /// Firing on a tool judged defective *of another family* is its own
    /// cell. Counting it as a false alarm would understate a detector that
    /// found a real second defect; counting it as a true positive would
    /// overstate one that guessed.
    #[test]
    fn a_fire_on_another_family_is_neither_a_hit_nor_an_alarm() {
        let cases = vec![case("other", true, &["dropped-alias"], node("other"))];
        let cal = calibrate(
            &Stub {
                fires_on: vec!["other"],
            },
            &cases,
            Vec::new(),
        );
        assert!(cal.false_alarms.is_empty());
        assert!(cal.true_positives.is_empty());
        assert_eq!(cal.fires_on_other_defect.len(), 1);
    }

    /// A [`Stub`] whose declared scope excludes a fixed list of tool names —
    /// the harness's stand-in for `BundledShortFlag` excluding `ssh-keygen`,
    /// so the reclassification logic is checked against a known answer
    /// rather than against `bundling::detect`'s real heuristic.
    struct StubWithScope {
        fires_on: Vec<&'static str>,
        excluded: &'static [Exclusion],
    }

    /// The declared exclusion the scope tests reuse: `ssh-keygen`'s real
    /// one-member cluster, with the same [`Ground`] the live detector
    /// declares, so the harness logic is checked against the real shape.
    const TEST_EXCLUSIONS: &[Exclusion] = &[Exclusion {
        tool: "ssh-keygen",
        ground: Ground::BelowMemberThreshold {
            cluster: "-hU",
            constant: "bundling::MIN_BUNDLED_MEMBERS",
            threshold: 2,
        },
        note: "single-member cluster, below the threshold",
    }];

    impl Detector for StubWithScope {
        fn name(&self) -> &'static str {
            "stub-with-scope"
        }
        fn family(&self) -> Option<&'static str> {
            Some("verbatim-fallback")
        }
        fn describes(&self) -> &'static str {
            "fires on a fixed list of node names, with a declared exclusion list"
        }
        fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
            if self.fires_on.contains(&evidence.root.name.as_str()) {
                vec!["stub fired".to_string()]
            } else {
                Vec::new()
            }
        }
        fn scope(&self) -> Scope {
            Scope {
                claim: "everything except the declared exclusion(s)",
                known_exclusions: self.excluded,
            }
        }
    }

    /// A declared exclusion reclassifies a genuine miss out of
    /// `false_negatives` (which blocks a pass) and into
    /// `out_of_scope_misses` (which does not) — the exact shape of
    /// `bundled-short-flag` excluding `ssh-keygen`. Recall is then computed
    /// only over what remains in scope, and the tool is never simply
    /// dropped: it is still named, with its reason, in a different cell.
    #[test]
    fn a_declared_exclusion_moves_a_miss_out_of_false_negatives_without_dropping_it() {
        let cases = vec![
            case("hit", true, &["verbatim-fallback"], node("hit")),
            case(
                "ssh-keygen",
                true,
                &["verbatim-fallback"],
                node("ssh-keygen"),
            ),
        ];
        let stub = StubWithScope {
            fires_on: vec!["hit"],
            excluded: TEST_EXCLUSIONS,
        };
        let cal = calibrate(&stub, &cases, Vec::new());

        assert!(
            cal.false_negatives.is_empty(),
            "the declared exclusion must not count as an in-scope miss: {:?}",
            cal.false_negatives
        );
        assert_eq!(cal.out_of_scope_misses.len(), 1);
        assert_eq!(cal.out_of_scope_misses[0].0, "ssh-keygen");
        assert_eq!(
            cal.out_of_scope_misses[0].1.note,
            "single-member cluster, below the threshold"
        );
        // Recall is 100% *within scope*: one expected-in-scope tool, one hit.
        assert_eq!(cal.recall(), Some(1.0));
        assert!(
            cal.verdict() == Verdict::Passes,
            "an out-of-scope miss must not block a pass on its own"
        );
    }

    /// The out-of-scope section is driven by the detector's declared scope,
    /// not by what this run happened to find — so it appears, in red, even
    /// on a run that otherwise PASSES cleanly. This is the literal
    /// requirement that a reader must not be able to skim a passing verdict
    /// and conclude nothing was missed.
    #[test]
    fn a_passing_verdict_still_renders_its_declared_exclusion_in_red() {
        let cases = vec![
            case("hit", true, &["verbatim-fallback"], node("hit")),
            case(
                "ssh-keygen",
                true,
                &["verbatim-fallback"],
                node("ssh-keygen"),
            ),
        ];
        let stub = StubWithScope {
            fires_on: vec!["hit"],
            excluded: TEST_EXCLUSIONS,
        };
        let cal = calibrate(&stub, &cases, Vec::new());
        assert!(cal.verdict() == Verdict::Passes);

        let text = render(
            &cal,
            &SetSize {
                sampled: 94,
                judged: 86,
                evaluable: 71,
            },
        );
        assert!(text.contains("VERDICT: PASSES"), "{text}");
        assert!(
            text.contains("KNOWN OUT-OF-SCOPE MISSES"),
            "the declared-exclusion section must survive a passing verdict: {text}"
        );
        assert!(text.contains("ssh-keygen"), "{text}");
        assert!(
            text.contains("single-member cluster, below the threshold"),
            "{text}"
        );
        assert!(
            text.contains(RED) && text.contains(RESET),
            "the out-of-scope section must be visually distinct: {text}"
        );
        // The literal ANSI-red tool name must appear, not merely the name
        // and the color codes somewhere unrelated in the report.
        assert!(text.contains(&format!("{RED}ssh-keygen{RESET}")), "{text}");
    }

    /// A false alarm on a tool that also happens to be named in the
    /// detector's declared exclusions must still fail calibration: scope
    /// narrows what a detector may be scored on *missing*, never what
    /// excuses it for *firing on a human-judged-correct tool*. This is the
    /// `nfsidmap` guard in miniature — the reclassification logic must
    /// never be reachable from the false-alarm arm.
    #[test]
    fn a_declared_exclusion_never_launders_a_false_alarm() {
        const EXCLUDED: &[Exclusion] = &[Exclusion {
            tool: "nfsidmap",
            ground: Ground::BelowMemberThreshold {
                cluster: "-hU",
                constant: "bundling::MIN_BUNDLED_MEMBERS",
                threshold: 2,
            },
            note: "declared out of scope for an unrelated reason",
        }];
        let cases = vec![case("nfsidmap", false, &[], node("nfsidmap"))];
        let stub = StubWithScope {
            fires_on: vec!["nfsidmap"],
            excluded: EXCLUDED,
        };
        let cal = calibrate(&stub, &cases, Vec::new());

        assert_eq!(cal.false_alarms.len(), 1, "{:?}", cal.false_alarms);
        assert_eq!(cal.false_alarms[0].0, "nfsidmap");
        assert!(cal.out_of_scope_misses.is_empty());
        assert!(
            cal.verdict() != Verdict::Passes,
            "a false alarm must still block a pass"
        );
    }

    #[test]
    fn a_detector_with_no_labelled_family_reports_not_calibratable() {
        let cal = calibrate(&ExistenceOracle, &[], Vec::new());
        assert!(cal.family.is_none());
        assert!(cal.verdict() != Verdict::Passes);
        let text = render(
            &cal,
            &SetSize {
                sampled: 94,
                judged: 86,
                evaluable: 71,
            },
        );
        assert!(text.contains("NOT CALIBRATABLE"), "{text}");
        assert!(!text.contains("VERDICT: PASSES"));
    }

    /// The caveat is not optional and not abbreviated: every rendered
    /// calibration carries the derived-label, bounded-sample and
    /// not-evaluable limits in full.
    #[test]
    fn every_rendered_calibration_states_what_the_labels_are() {
        let cal = calibrate(
            &Stub {
                fires_on: vec!["hit"],
            },
            &[case("hit", true, &["verbatim-fallback"], node("hit"))],
            Vec::new(),
        );
        let text = render(
            &cal,
            &SetSize {
                sampled: 94,
                judged: 86,
                evaluable: 71,
            },
        );
        assert!(text.contains("NOT GROUND TRUTH ABOUT THE FLEET"), "{text}");
        assert!(text.contains("MACHINE READING"), "{text}");
        assert!(text.contains("VERDICT: PASSES"), "{text}");
    }

    #[test]
    fn every_registered_detectors_family_is_a_real_one() {
        for d in registry() {
            if let Some(family) = d.family() {
                assert!(
                    audit::family_meaning(family).is_some(),
                    "{} claims unknown family {family:?}",
                    d.name()
                );
            }
            assert!(!d.describes().trim().is_empty());
        }
    }

    #[test]
    fn find_names_the_registry_when_it_misses() {
        let err = find("nope")
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(err.contains("registered:"), "{err}");
        assert!(err.contains("verbatim-fallback"), "{err}");
    }

    /// The tracked manifest's own derived labels must validate, and every
    /// one of them must still say it is derived.
    ///
    /// This is the guard on the schema's central claim. `families_derived`
    /// exists so a machine reading of a reviewer's note can never be
    /// mistaken for the reviewer's own classification; a hand edit that
    /// flipped one to `false` would silently promote a machine's opinion to
    /// a human's, and nothing else in the system would notice. It also
    /// catches a mistyped family, which would otherwise show up only as a
    /// calibration cell that quietly shrank.
    #[test]
    fn the_tracked_seed2_manifest_carries_only_derived_labels() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask/ has a parent");
        let file = audit::load(&audit::verdict_path(
            &repo.join("audit/submissions/sadigaxund"),
            2,
        ))
        .expect("the tracked seed-2 manifest loads");
        file.validate_families()
            .expect("its family labels validate");
        for entry in &file.entries {
            if entry.families.is_empty() {
                continue;
            }
            assert_eq!(
                entry.families_derived,
                Some(true),
                "{}: every label in the tracked manifest was derived by machine from the \
                 reviewer's note; a `false` here claims the reviewer classified it themselves",
                entry.tool
            );
        }
        // vim.basic was confirmed `unparsed-positional` and labelled
        // accordingly. man-recode was left unclassified for a while — its
        // note confirms all 9 flags parse correctly and the defect is
        // entirely the wrapped usage synopsis, outside the audit's
        // declared flags/subcommand scope — but task #28 gave that exact
        // shape a real closed-set label (`display-only`) rather than
        // leaving it as an unclassified guess, so man-recode now carries
        // it and the tracked manifest has nothing left unclassified.
        let unclassified: Vec<&str> = file.unclassified().map(|e| e.tool.as_str()).collect();
        assert_eq!(
            unclassified,
            Vec::<&str>::new(),
            "if this fails because a *different* tool is unclassified, that's a real, new gap — \
             go label it or investigate; don't just widen this assertion"
        );
    }

    // --- the detectors' own rules ---------------------------------------

    #[test]
    fn verbatim_fallback_needs_both_unparsed_lines_and_no_structure() {
        let mut bare = node("vgck");
        bare.unparsed = vec![Text::sanitize("[ -d|--debug ]")];
        assert_eq!(
            VerbatimFallback
                .hits(&ToolEvidence {
                    raw: "",
                    root: &bare
                })
                .len(),
            1
        );

        // Structure present alongside unparsed lines is a partial parse,
        // not a verbatim fallback.
        let mut partial = bare.clone();
        partial.subcommands.push(node("child"));
        assert!(VerbatimFallback
            .hits(&ToolEvidence {
                raw: "",
                root: &partial
            })
            .is_empty());

        // No unparsed lines at all is not this family either, however empty
        // the tree is.
        let empty = node("empty");
        assert!(VerbatimFallback
            .hits(&ToolEvidence {
                raw: "",
                root: &empty
            })
            .is_empty());
    }

    #[test]
    fn argparse_positional_names_reads_only_the_block() {
        let raw = "usage: t [-h] pid\n\npositional arguments:\n  pid    process id\n  interval \
                   secs\n\noptions:\n  -h, --help  show\n";
        assert_eq!(argparse_positional_names(raw), vec!["pid", "interval"]);
        assert!(argparse_positional_names("options:\n  -h  show\n").is_empty());
    }

    /// A tool that *did* extract positionals is silent even with the
    /// heading present — the detector claims a loss, not a shape.
    #[test]
    fn unparsed_positional_is_silent_when_positionals_were_extracted() {
        let raw = "positional arguments:\n  pid  process id\n";
        let mut with = node("t");
        let mut pid =
            mandible_core::Entity::positional("pid", Provenance::single(Source::HelpText));
        pid.required = true;
        with.entities.push(pid);
        assert!(UnparsedArgparsePositional
            .hits(&ToolEvidence { raw, root: &with })
            .is_empty());

        let without = node("t");
        assert_eq!(
            UnparsedArgparsePositional
                .hits(&ToolEvidence {
                    raw,
                    root: &without
                })
                .len(),
            1
        );
    }

    // --- a repaired family, and the evidence that says so ----------------

    fn set_size() -> SetSize {
        SetSize {
            sampled: 94,
            judged: 86,
            evaluable: 71,
        }
    }

    /// A [`Stub`] that also carries hand-built self-checks, so the REPAIRED
    /// verdict's evidence requirement is exercised against a known answer
    /// rather than against `bundling`'s real rule.
    struct StubWithSelfChecks {
        fires_on: Vec<&'static str>,
        checks: Vec<(&'static str, Expect, &'static str)>,
    }

    impl Detector for StubWithSelfChecks {
        fn name(&self) -> &'static str {
            "stub-with-self-checks"
        }
        fn family(&self) -> Option<&'static str> {
            Some("verbatim-fallback")
        }
        fn describes(&self) -> &'static str {
            "fires on a fixed list of node names, and declares self-checks"
        }
        fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
            if self.fires_on.contains(&evidence.root.name.as_str()) {
                vec!["stub fired".to_string()]
            } else {
                Vec::new()
            }
        }
        fn self_checks(&self) -> Vec<SelfCheck> {
            self.checks
                .iter()
                .map(|(name, expect, on)| SelfCheck {
                    name,
                    why: "a stub case",
                    expect: *expect,
                    raw: String::new(),
                    root: node(on),
                })
                .collect()
        }
    }

    /// A stub that is both self-checked and scoped, for the one test that
    /// needs a REPAIRED verdict and a declared exclusion together.
    struct StubWithSelfChecksAndScope {
        inner: StubWithSelfChecks,
    }

    impl Detector for StubWithSelfChecksAndScope {
        fn name(&self) -> &'static str {
            self.inner.name()
        }
        fn family(&self) -> Option<&'static str> {
            self.inner.family()
        }
        fn describes(&self) -> &'static str {
            self.inner.describes()
        }
        fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
            self.inner.hits(evidence)
        }
        fn self_checks(&self) -> Vec<SelfCheck> {
            self.inner.self_checks()
        }
        fn scope(&self) -> Scope {
            Scope {
                claim: "everything except the declared exclusion(s)",
                known_exclusions: TEST_EXCLUSIONS,
            }
        }
    }

    /// The self-check list a healthy stub declares: one case it must fire
    /// on, one it must stay silent on. Both directions, because
    /// [`self_checks_are_conclusive`] requires both.
    fn healthy_checks() -> Vec<(&'static str, Expect, &'static str)> {
        vec![
            ("the defective shape", Expect::Fires(1), "shape"),
            ("a correct parse that resembles it", Expect::Silent, "clean"),
        ]
    }

    /// Two labelled tools that no longer fire — a family that was repaired.
    fn inverted_cases() -> Vec<Case> {
        vec![
            case("was-bad", true, &["verbatim-fallback"], node("was-bad")),
            case("also-bad", true, &["verbatim-fallback"], node("also-bad")),
            case("quiet", false, &[], node("quiet")),
        ]
    }

    /// The whole of the repaired-family verdict: an inverted calibration
    /// reads REPAIRED **only** because the detector's own hand-built cases
    /// still fire. Nothing else changed — same cells, same 0% recall.
    #[test]
    fn a_repaired_family_reads_as_repaired_when_its_self_checks_still_hold() {
        let stub = StubWithSelfChecks {
            fires_on: vec!["shape"],
            checks: healthy_checks(),
        };
        let cal = calibrate(&stub, &inverted_cases(), Vec::new());

        assert!(cal.calibration_inverted());
        assert!(cal.self_checks_are_conclusive());
        assert_eq!(cal.verdict(), Verdict::Repaired);
        // The cells are untouched: the misses are still misses.
        assert_eq!(cal.false_negatives, vec!["was-bad", "also-bad"]);
        assert_eq!(cal.recall(), Some(0.0));
    }

    /// The other half, and the reason the evidence requirement exists at
    /// all: the identical inverted matrix with a *broken* detector must
    /// **not** read as repaired. This is the case spec §13.1e says the
    /// fleet number alone cannot distinguish.
    #[test]
    fn an_inverted_matrix_with_broken_self_checks_is_not_repaired() {
        // Fires on nothing at all — including its own must-fire case.
        let stub = StubWithSelfChecks {
            fires_on: vec![],
            checks: healthy_checks(),
        };
        let cal = calibrate(&stub, &inverted_cases(), Vec::new());

        assert!(cal.calibration_inverted());
        assert!(!cal.self_checks_are_conclusive());
        assert_eq!(cal.verdict(), Verdict::DoesNotPass);

        let text = render(&cal, &set_size());
        assert!(text.contains("DOES NOT PASS"), "{text}");
        assert!(
            text.contains("this is the dangerous shape"),
            "an inverted matrix with failing self-checks must be called out as its own case, \
             not rendered as an ordinary failure: {text}"
        );
        assert!(text.contains("[FAILED"), "{text}");
    }

    /// A detector that declares no self-check can never be called repaired.
    /// The empty case is called out because `[].iter().all(..)` is
    /// vacuously true, which is the exact shape of hole that would let a
    /// deleted detector claim a clean bill of health.
    #[test]
    fn a_detector_with_no_self_check_can_never_read_as_repaired() {
        let cal = calibrate(&Stub { fires_on: vec![] }, &inverted_cases(), Vec::new());
        assert!(cal.calibration_inverted());
        assert!(cal.self_checks.is_empty());
        assert_eq!(cal.verdict(), Verdict::DoesNotPass);
        let text = render(&cal, &set_size());
        assert!(text.contains("declares NO self-check"), "{text}");
    }

    /// One direction of evidence is never enough. A detector that only
    /// declares must-fire cases is satisfied by firing on everything; one
    /// that only declares must-stay-silent cases is satisfied by being
    /// deleted. Both halves are asserted because the two failures are
    /// opposites and a single-direction check would catch neither.
    #[test]
    fn self_check_evidence_needs_both_directions() {
        let fire_only = StubWithSelfChecks {
            fires_on: vec!["shape", "clean"],
            checks: vec![("the defective shape", Expect::Fires(1), "shape")],
        };
        let cal = calibrate(&fire_only, &inverted_cases(), Vec::new());
        assert!(
            !cal.self_checks_are_conclusive(),
            "a detector that fires indiscriminately satisfies every must-fire case there is"
        );
        assert_eq!(cal.verdict(), Verdict::DoesNotPass);

        let silence_only = StubWithSelfChecks {
            fires_on: vec![],
            checks: vec![("a correct parse", Expect::Silent, "clean")],
        };
        let cal = calibrate(&silence_only, &inverted_cases(), Vec::new());
        assert!(
            !cal.self_checks_are_conclusive(),
            "a deleted detector satisfies every must-stay-silent case there is"
        );
        assert_eq!(cal.verdict(), Verdict::DoesNotPass);
    }

    /// A false alarm blocks REPAIRED exactly as it blocks PASSES. Firing on
    /// a tool a human judged correct is never excused — not by a declared
    /// scope, and not by the family having been fixed.
    #[test]
    fn a_repaired_family_still_cannot_launder_a_false_alarm() {
        let stub = StubWithSelfChecks {
            fires_on: vec!["shape", "quiet"],
            checks: healthy_checks(),
        };
        let cal = calibrate(&stub, &inverted_cases(), Vec::new());
        assert_eq!(cal.false_alarms.len(), 1);
        assert_eq!(cal.verdict(), Verdict::DoesNotPass);
    }

    /// The hard constraint: REPAIRED is a *stated claim*, never a
    /// suppression. The rendered report must still print 0% recall, still
    /// name every missed tool, and still print the declared out-of-scope
    /// miss in red — otherwise "the family was repaired" becomes the excuse
    /// that hides a genuinely broken detector.
    #[test]
    fn a_repaired_verdict_suppresses_nothing() {
        let mut cases = inverted_cases();
        cases.push(case(
            "ssh-keygen",
            true,
            &["verbatim-fallback"],
            node("ssh-keygen"),
        ));
        let stub = StubWithSelfChecksAndScope {
            inner: StubWithSelfChecks {
                fires_on: vec!["shape"],
                checks: healthy_checks(),
            },
        };
        let cal = calibrate(&stub, &cases, Vec::new());
        assert_eq!(cal.verdict(), Verdict::Repaired);

        let text = render(&cal, &set_size());
        assert!(text.contains("VERDICT: REPAIRED"), "{text}");
        assert!(
            text.contains("recall over evaluable labelled: 0%"),
            "recall must still read what it reads: {text}"
        );
        assert!(
            text.contains("MISSED labelled tools (false negatives)")
                && text.contains("was-bad")
                && text.contains("also-bad"),
            "every missed tool must still be named: {text}"
        );
        assert!(
            text.contains("KNOWN OUT-OF-SCOPE MISSES")
                && text.contains(&format!("{RED}ssh-keygen{RESET}")),
            "the out-of-scope miss must still print, in red: {text}"
        );
        assert!(
            text.contains("NOTHING IS SUPPRESSED"),
            "the verdict must say so in its own words: {text}"
        );
        // The evidence the verdict rests on is printed with it.
        assert!(text.contains("SELF-CHECK EVIDENCE"), "{text}");
    }

    /// The self-check block is printed on every run, not only the ones that
    /// use it — the same rule the declared-exclusion list follows, for the
    /// same reason: the first time a reader sees the evidence must not be
    /// the run where it is being used to excuse a zero.
    #[test]
    fn the_self_check_block_prints_even_on_a_passing_verdict() {
        let stub = StubWithSelfChecks {
            fires_on: vec!["hit", "shape"],
            checks: healthy_checks(),
        };
        let cal = calibrate(
            &stub,
            &[case("hit", true, &["verbatim-fallback"], node("hit"))],
            Vec::new(),
        );
        assert_eq!(cal.verdict(), Verdict::Passes);
        let text = render(&cal, &set_size());
        assert!(text.contains("SELF-CHECK EVIDENCE"), "{text}");
        assert!(text.contains("the defective shape"), "{text}");
    }

    // --- the ratchet gate, and an attack on it ---------------------------

    /// The gate holds on the real detector at the real post-fix numbers.
    #[test]
    fn the_ratchet_holds_on_the_live_detector_at_zero() {
        let outcome = ratchet_at_zero(&BundledShortFlag, 0, 0);
        assert!(outcome.holds(), "{:?}", outcome.failures);
        assert!(outcome.report().contains("GATE HOLDS"));
    }

    /// **The attack.** `bundled-short-flag` with its rule removed — the
    /// detector deleted, its declared self-checks left behind — reporting
    /// the perfect fleet count of 0 tools / 0 destroyed flags.
    ///
    /// A gate asserting `count == 0` alone passes this. It is the whole
    /// reason the gate has a second half, and the reason that half is not
    /// optional: a metric a commit can improve by breaking the instrument
    /// that measures it is the failure spec §13.1b already records twice.
    #[test]
    fn deleting_the_detector_fails_the_ratchet_even_at_a_perfect_zero() {
        struct DeletedRule;
        impl Detector for DeletedRule {
            fn name(&self) -> &'static str {
                "bundled-short-flag"
            }
            fn family(&self) -> Option<&'static str> {
                Some("bundled-short-flag")
            }
            fn describes(&self) -> &'static str {
                "the real detector with its rule removed"
            }
            fn hits(&self, _evidence: &ToolEvidence<'_>) -> Vec<String> {
                Vec::new()
            }
            fn self_checks(&self) -> Vec<SelfCheck> {
                crate::bundling::self_checks()
            }
        }

        // The fleet count is as clean as it can possibly be.
        let outcome = ratchet_at_zero(&DeletedRule, 0, 0);
        assert!(
            !outcome.holds(),
            "a gate that only checks the count is satisfied by deleting the detector"
        );
        let report = outcome.report();
        assert!(report.contains("GATE FAILS"), "{report}");
        assert!(
            report.contains("tcpdump's real 26-member cluster"),
            "the failing case must be named: {report}"
        );
        assert!(
            outcome
                .failures
                .iter()
                .any(|f| f.contains("fleet-wide zero means nothing")),
            "{:?}",
            outcome.failures
        );
    }

    /// The third way to delete a detector, and the one no stub can model:
    /// **deregister it**. Gutting `hits()` is caught by the self-checks, and
    /// emptying the self-check list is caught by the arm below — but a
    /// detector removed from [`registry`] altogether is caught by neither,
    /// because there is no longer anything to run.
    ///
    /// What saves the gate is that the ratcheted names live *outside* the
    /// registry: `main.rs` looks each one up by literal name through
    /// [`find`], which returns `Err` when the name is absent, and the `?`
    /// fails the whole `coverage --check` run. That property is load-bearing
    /// and invisible — nothing else in the file states it — so it is pinned
    /// here. If a future change ever ratchets by *iterating* the registry
    /// instead, deregistering would silently drop that family's gate with no
    /// lookup ever failing, and this test would not notice. Ratchet by name.
    #[test]
    fn every_ratcheted_family_is_still_reachable_through_the_registry() {
        // Keep in step with `main.rs`'s `ratchet_at_zero` call sites.
        for name in [
            "bundled-short-flag",
            "unparsed-command-table",
            "repeated-char-flag",
            "single-dash-long",
        ] {
            assert!(
                find(name).is_ok(),
                "{name:?} is ratcheted at zero in main.rs but no longer registered — \
                 deregistering it removes its gate; re-register it or remove its ratchet \
                 deliberately"
            );
        }
    }

    /// The other way to delete a detector: remove its self-checks too. An
    /// empty evidence list must fail closed, not vacuously pass.
    #[test]
    fn a_detector_with_no_self_check_fails_the_ratchet_at_zero() {
        let outcome = ratchet_at_zero(&Stub { fires_on: vec![] }, 0, 0);
        assert!(!outcome.holds());
        assert!(
            outcome.failures.iter().any(|f| f.contains("NO self-check")),
            "{:?}",
            outcome.failures
        );
    }

    /// The ratchet half proper: a live, healthy detector still fails the
    /// gate the moment the fleet count leaves zero, and both columns are
    /// reported independently rather than netted.
    #[test]
    fn a_nonzero_fleet_count_fails_the_ratchet_however_healthy_the_detector() {
        let outcome = ratchet_at_zero(&BundledShortFlag, 1, 7);
        assert!(!outcome.holds());
        assert_eq!(
            outcome.failures.len(),
            2,
            "tools and destroyed flags are separate reasons: {:?}",
            outcome.failures
        );
        assert!(outcome.failures.iter().any(|f| f.contains("1 tool(s)")));
        assert!(outcome
            .failures
            .iter()
            .any(|f| f.contains("7 real flag(s) destroyed")));
    }

    // --- declared exclusions must name a structural property -------------

    /// Every exclusion every registered detector declares must survive its
    /// own witness's arithmetic. This is the guard on the last remaining
    /// goalpost-moving lever: adding an entry converts a blocking false
    /// negative into a non-blocking named miss, and before [`Ground`]
    /// existed nothing checked that the reason named a property of the
    /// *shape* rather than a preference about the tool.
    #[test]
    fn every_declared_exclusion_in_the_registry_holds_structurally() {
        validate_registry_scopes().expect("a declared exclusion must survive its own witness");
    }

    /// The live exclusion cites the constant itself, not a copy of its
    /// value — so changing `MIN_BUNDLED_MEMBERS` cannot leave a stale
    /// justification behind.
    #[test]
    fn the_live_exclusion_references_the_real_constant() {
        let Ground::BelowMemberThreshold {
            cluster,
            constant,
            threshold,
        } = BUNDLED_SHORT_FLAG_EXCLUSIONS[0].ground
        else {
            panic!("the bundled-short-flag exclusion is a member-threshold ground");
        };
        assert_eq!(threshold, crate::bundling::MIN_BUNDLED_MEMBERS);
        assert_eq!(constant, "bundling::MIN_BUNDLED_MEMBERS");
        assert_eq!(cluster, crate::bundling::SSH_KEYGEN_CLUSTER);
        assert_eq!(BUNDLED_SHORT_FLAG_EXCLUSIONS[0].tool, "ssh-keygen");
    }

    /// The exclusion's witness is also asserted as a must-stay-silent
    /// self-check, which closes the loop: the tool is out of scope because
    /// of a structural property, *and* the detector is separately shown to
    /// stay silent on that exact token.
    #[test]
    fn the_exclusions_witness_is_also_a_self_check_the_detector_must_stay_silent_on() {
        let outcomes = run_self_checks(&BundledShortFlag);
        let witness = outcomes
            .iter()
            .find(|o| o.name.starts_with("ssh-keygen"))
            .expect("the declared exclusion's witness must be asserted, not merely described");
        assert_eq!(witness.expect, Expect::Silent);
        assert!(witness.held);
    }

    /// `dropped-alias`'s two exclusions cite the constants and witnesses
    /// themselves, not copies — so neither justification can go stale
    /// behind a changed splitter or a renamed family.
    #[test]
    fn the_dropped_alias_exclusions_reference_the_real_witnesses() {
        let eqn = DROPPED_ALIAS_EXCLUSIONS
            .iter()
            .find(|e| e.tool == "eqn")
            .expect("eqn is a declared exclusion");
        let Ground::InsideAlternationGroup { token, family } = eqn.ground else {
            panic!("eqn is excluded on the brace-alternation ground");
        };
        assert_eq!(token, crate::dropped_alias::EQN_VERSION_GROUP);
        assert_eq!(family, "brace-alternation-flag");
        assert!(mandible_core::audit::family_meaning(family).is_some());

        let jdeprscan = DROPPED_ALIAS_EXCLUSIONS
            .iter()
            .find(|e| e.tool == "jdeprscan")
            .expect("jdeprscan is a declared exclusion");
        let Ground::AcrossDescriptionColumn {
            row,
            constant,
            column_gap,
        } = jdeprscan.ground
        else {
            panic!("jdeprscan is excluded on the description-column ground");
        };
        assert_eq!(row, crate::dropped_alias::JDEPRSCAN_LIST_ROW);
        assert_eq!(constant, "help_text::MIN_COLUMN_GAP_SPACES");
        assert_eq!(
            column_gap,
            mandible_extract::help_text::MIN_COLUMN_GAP_SPACES
        );
    }

    /// The description-column ground is arithmetic over the row, not a
    /// claim: the very row this detector is *supposed* to catch —
    /// `-p PID, --pid PID`, one space between its spellings — is refused as
    /// a justification, which is the whole point of the type.
    #[test]
    fn a_description_column_ground_over_a_one_space_row_is_refused() {
        assert_eq!(
            widest_gap_between_spellings(crate::dropped_alias::JDEPRSCAN_LIST_ROW),
            4
        );
        let inside = Ground::AcrossDescriptionColumn {
            row: "  -p PID, --pid PID  trace this PID only",
            constant: "help_text::MIN_COLUMN_GAP_SPACES",
            column_gap: mandible_extract::help_text::MIN_COLUMN_GAP_SPACES,
        };
        let err = inside
            .holds()
            .expect_err("one space is not a column gap, so this row is in scope");
        assert!(err.contains("false negative, not an exclusion"), "{err}");
        // ...and a gap of zero would excuse every miss there is.
        assert!(Ground::AcrossDescriptionColumn {
            row: crate::dropped_alias::JDEPRSCAN_LIST_ROW,
            constant: "SOME_CONSTANT",
            column_gap: 0,
        }
        .holds()
        .is_err());
    }

    /// The alternation ground checks both halves of what it asserts: the
    /// token really is a brace group alternating two spellings, and the
    /// family it hands the miss to is one the manifest declares.
    #[test]
    fn an_alternation_ground_with_no_alternation_or_no_family_is_refused() {
        assert!(Ground::InsideAlternationGroup {
            token: crate::dropped_alias::EQN_VERSION_GROUP,
            family: "brace-alternation-flag",
        }
        .holds()
        .is_ok());
        for token in ["-v | --version", "{--version}", "{dir|jar|class}"] {
            assert!(
                Ground::InsideAlternationGroup {
                    token,
                    family: "brace-alternation-flag",
                }
                .holds()
                .is_err(),
                "{token:?} is not an alias alternation"
            );
        }
        assert!(Ground::InsideAlternationGroup {
            token: crate::dropped_alias::EQN_VERSION_GROUP,
            family: "no-such-family",
        }
        .holds()
        .is_err());
    }

    /// An exclusion whose witness is squarely *inside* the detector's scope
    /// is refused. This is the goalpost-move the type exists to prevent:
    /// before [`Ground`], excluding `tcpdump` needed only a sentence.
    #[test]
    fn an_exclusion_whose_witness_is_in_scope_is_refused() {
        let inside = Ground::BelowMemberThreshold {
            cluster: "-AbdDefhHIJKlLnNOpqStuUvxX#",
            constant: "bundling::MIN_BUNDLED_MEMBERS",
            threshold: 2,
        };
        assert_eq!(inside.swallowed_members(), 25);
        let err = inside.holds().expect_err("25 members is not below 2");
        assert!(err.contains("is NOT below"), "{err}");
        assert!(err.contains("false negative, not an exclusion"), "{err}");
    }

    /// A threshold of zero excludes everything and justifies nothing, and a
    /// witness that is not a cluster token is not evidence about a shape.
    /// Both are refused, so the escape hatches out of the arithmetic are
    /// closed too.
    #[test]
    fn a_vacuous_ground_is_refused() {
        assert!(Ground::BelowMemberThreshold {
            cluster: "-hU",
            constant: "SOME_CONSTANT",
            threshold: 0,
        }
        .holds()
        .is_err());
        for cluster in ["", "hU", "-"] {
            assert!(
                Ground::BelowMemberThreshold {
                    cluster,
                    constant: "bundling::MIN_BUNDLED_MEMBERS",
                    threshold: 2,
                }
                .holds()
                .is_err(),
                "{cluster:?} is not a cluster token"
            );
        }
    }

    /// The structural sentence in the report is generated from the witness,
    /// not written by the exclusion's author — so the prose beside it can
    /// never be the whole justification.
    #[test]
    fn the_rendered_reason_is_generated_from_the_witness() {
        let explain = BUNDLED_SHORT_FLAG_EXCLUSIONS[0].ground.explain();
        assert!(explain.contains("\"-hU\""), "{explain}");
        assert!(explain.contains("swallows 1 member(s)"), "{explain}");
        assert!(
            explain.contains("bundling::MIN_BUNDLED_MEMBERS = 2"),
            "{explain}"
        );
        assert!(
            explain.contains("a property of the token's shape, not of the tool"),
            "{explain}"
        );
    }
}
