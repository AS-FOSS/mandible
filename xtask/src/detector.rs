//! The **family-detector calibration harness**: the seam a fleet-wide
//! defect detector registers itself in, and the confusion matrix that says
//! whether its fleet-wide number may be quoted yet.
//!
//! # What a family detector is, and what it is not
//!
//! A *family detector* generalizes one human finding across every tool on
//! `PATH`. It is **not a correctness instrument and does not need to be.**
//! The audit (`xtask audit`, spec §13.1c) is the only instrument in this
//! project that touches truth, because a human read the tool's real output
//! next to the parsed tree and judged it. A detector's job is only to say
//! "this same *shape* occurs here too", 2,300 times, in a second.
//!
//! The failure mode that follows is the one this module exists to close.
//! A detector run over the fleet produces a confident number — *"814 tools
//! exhibit this defect"* — and nothing in that number knows whether the
//! detector fires on the defect it claims to. Quoting it is the "measures
//! itself" trap this project has already fallen into twice with metrics
//! (spec §13.1b): [M-10]'s fabricated `tar` nodes *inflated* `%described`,
//! and `%flags_text` was a name that read as an accuracy claim it never
//! earned. A detector is a third instance of the same shape.
//!
//! # The precondition
//!
//! > **Before a detector's fleet-wide number is quotable, it must be
//! > calibrated against the human labels: it must fire on the known-bad
//! > tools and stay silent on the known-good ones.**
//!
//! That is what [`calibrate`] computes. A detector that has not passed it
//! is measuring itself; one that has is an amplifier of a verified human
//! judgment. Nothing here *enforces* the precondition on a detector's
//! implementation — it cannot, since the fleet number is produced by a
//! different command — so the enforcement is that the calibration report
//! exists, is cheap to run, and names every disagreeing tool.
//!
//! # What the labels actually are, and why every report says so
//!
//! The calibration set is 94 human verdicts from the seed-2 audit, sorted
//! into defect families by a **machine reading of each reviewer's prose**
//! plus the fixture evidence (`mandible_core::audit::Entry::families`, with
//! `families_derived` recording exactly that provenance). Three limits
//! follow, and [`render`] prints all three above the matrix rather than in
//! a footnote:
//!
//! 1. **The families are derived, the verdicts are not.** A human said the
//!    parse was wrong; a machine said *which family* wrong. A miscategorized
//!    label moves a tool between cells of the matrix.
//! 2. **94 tools is a bounded sample, not the fleet.** A detector that is
//!    perfect here has been shown to work on 94 tools, ~4% of `PATH`, drawn
//!    from a stratified queue. It has not been shown to work on the fleet,
//!    and the fleet number's *magnitude* is not what calibration validates.
//! 3. **Only fixtures are evaluable.** A tool with no `corpus/<tool>/
//!    audit-seed2/` fixture has no frozen bytes to replay, so it appears in
//!    neither the fires nor the silent cells. It is counted and named
//!    separately ([`Calibration::not_evaluable`]) rather than silently
//!    dropped, because a "perfect" matrix computed over half the labelled
//!    set is a worse claim than an imperfect one computed over all of it.
//!
//! # The seam
//!
//! [`Detector`] is deliberately the same shape the two existing fleet
//! oracles already have — `detect(raw, root)`, no probes of its own, reading
//! only bytes the pipeline already captured (`crate::existence`,
//! `crate::misattribution`). A new detector implements three methods and
//! adds one line to [`registry`]; it does not touch this file's calibration
//! logic at all.
//!
//! [`Detector::family`] returns an `Option` on purpose. A detector that
//! generalizes no family the audit ever labelled — the existence oracle is
//! the standing example, since not one of the 94 reviewers reported a
//! fabricated name — is **not calibratable against this set**, and saying
//! so is the honest result. Forcing it into the nearest family would
//! manufacture a matrix out of a mapping nobody verified, which is the same
//! defect one level up.

use crate::corpus;
use mandible_core::audit::{self, AuditFile};
use mandible_core::CommandNode;
use std::collections::BTreeMap;
use std::path::Path;

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

    /// What this detector claims to catch, and — by name — what it
    /// deliberately does not, mirroring `corpus/README.md`'s
    /// `verdict_scope`: a narrower, explicit claim is honest where a silent
    /// one would overclaim. The default is the full family with no declared
    /// exclusion, which is the right answer for a detector with no measured,
    /// deliberate gap.
    ///
    /// A tool named in [`Scope::known_exclusions`] moves calibration's
    /// silent-on-labelled-bad cell from a false negative to a named
    /// out-of-scope miss ([`Calibration::out_of_scope_misses`]) *only when
    /// it is actually silent this run* — it never excuses a false alarm,
    /// and [`render`] prints the whole declared list on every run,
    /// regardless of whether the detector otherwise looks clean, so the
    /// exclusion can never quietly stop being named.
    fn scope(&self) -> Scope {
        Scope::full()
    }

    /// Hand-built cases this detector asserts about *itself*: the defective
    /// shape constructed directly, plus the correct parses that look like
    /// it, each with the number of findings the detector must report.
    ///
    /// **This is the evidence that distinguishes a fixed family from a
    /// broken detector**, and it is why the method is on the trait rather
    /// than in a `#[cfg(test)]` module. Spec §13.1e: once the commit that
    /// repairs a family lands, the labelled set has nothing left to confirm
    /// against, and "zero because the bug is gone" and "zero because the
    /// detector stopped working" are indistinguishable from the fleet
    /// number alone. Two consumers need to tell them apart at *runtime*,
    /// where no test harness exists — [`calibrate`], before it will report
    /// [`Verdict::Repaired`], and [`ratchet_at_zero`], before it will accept
    /// a fleet count of zero.
    ///
    /// The default is empty, and that is the honest default: a detector
    /// that offers no self-evidence can never be called repaired and can
    /// never satisfy the ratchet gate. It is not silently treated as fine.
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
/// **The reason is not free text.** Declaring an exclusion converts a
/// blocking false negative into a non-blocking named miss, which makes this
/// the last remaining lever for moving a goalpost: before [`Ground`]
/// existed, an entry was `(tool, &str)` and nothing checked that the string
/// named a structural property rather than a preference. The one entry that
/// was ever written is correctly justified — `ssh-keygen`'s `[-hU]` swallows
/// one member, below `bundling::MIN_BUNDLED_MEMBERS` — and that is the bar
/// [`Ground`] now enforces mechanically: the exclusion carries the witness
/// token and the constant, and the arithmetic that puts the tool out of
/// scope is *computed from the witness*, never asserted by the author.
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
/// A closed set on purpose. Adding an exclusion of a genuinely new kind
/// means adding a variant here, with its own `holds` arm — a visible,
/// reviewable change to the vocabulary, which is exactly what typing a new
/// sentence into a `&str` was not. Spec §13.1e's discipline applied to the
/// one place it had not been: out-of-scope and known misses stay counted
/// and named, and now they also have to be *earned*.
pub enum Ground {
    /// The tool's real shape is a member cluster carrying fewer swallowed
    /// members than the detector's declared minimum.
    ///
    /// `cluster` is the literal token from the tool's own help text (e.g.
    /// `"-hU"`), and the swallowed-member count is derived from it rather
    /// than stated: an author who tried to exclude `tcpdump` on this ground
    /// would have to write `-AbdDefhHIJKlLnNOpqStuUvxX#`, whose 25 swallowed
    /// members are not below any threshold, and [`Ground::holds`] would
    /// refuse it. `threshold` is meant to be the constant itself
    /// (`bundling::MIN_BUNDLED_MEMBERS`), not a retyped copy of its value,
    /// so it cannot drift; `constant` names it for the report.
    BelowMemberThreshold {
        cluster: &'static str,
        constant: &'static str,
        threshold: usize,
    },
}

impl Ground {
    /// Swallowed members implied by the witness: a cluster is one leading
    /// `-`, one surviving flag character, and the rest swallowed.
    pub fn swallowed_members(&self) -> usize {
        match self {
            Ground::BelowMemberThreshold { cluster, .. } => {
                cluster.chars().count().saturating_sub(2)
            }
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
/// set that has nothing left to confirm.
///
/// All three conditions are load-bearing:
///
/// * **non-empty** — a detector with no self-evidence proves nothing, and
///   an empty `all(...)` is vacuously true, which is precisely the shape of
///   hole this project has been burned by;
/// * **every case held** — one failure means the detector is broken, which
///   is the state this whole mechanism exists to distinguish;
/// * **at least one `Fires` and at least one `Silent` case** — the first
///   shows the rule still fires on the defect, the second shows it is not
///   simply firing on everything. Either alone is satisfiable by a detector
///   that is useless in the opposite direction.
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
        Box::new(ExistenceOracle),
        Box::new(MisattributionOracle),
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
struct VerbatimFallback;

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
    node.flags.is_empty()
        && node.positionals.is_empty()
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
struct UnparsedArgparsePositional;

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
        if !evidence.root.positionals.is_empty() {
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
}

/// Names listed under an argparse `positional arguments:` heading: the
/// first token of each subsequent indented line, until the block ends at a
/// blank line or a line that is not indented.
fn argparse_positional_names(raw: &str) -> Vec<String> {
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
        out.push(word.to_string());
    }
    out
}

/// The bundled-short-flag collapse (`crate::bundling`): a synopsis bundle
/// of boolean short flags parsed as one flag swallowing the rest as a value.
///
/// This is the first detector built *after* the harness existed, and the
/// first whose family was chosen because the existing oracles are blind to
/// it: a collapsed `-2` carrying `CDlNuVv` occurs literally in the raw text,
/// so `crate::existence` attests it cleanly while the parse destroys seven
/// flags. Zero fabrications is not a claim of a correct parse.
///
/// Its family shares a structural fingerprint (`short && !long &&
/// value_name`) with `single-dash-long` and `repeated-char-flag`, all three
/// of which sit under `k1 = true` in the labelled set. `crate::bundling`
/// discriminates on what the swallowed text *is*, not on the structure —
/// which is exactly the confusion this harness's own "fired on a tool
/// judged defective of another family" cell exists to surface.
pub(crate) struct BundledShortFlag;

/// `bundled-short-flag`'s declared exclusions. The one entry is the shape
/// of every future one: a witness token, the constant it falls below, and
/// arithmetic that has to agree — see [`Exclusion`].
const BUNDLED_SHORT_FLAG_EXCLUSIONS: &[Exclusion] = &[Exclusion {
    tool: "ssh-keygen",
    ground: Ground::BelowMemberThreshold {
        cluster: crate::bundling::SSH_KEYGEN_CLUSTER,
        constant: "bundling::MIN_BUNDLED_MEMBERS",
        threshold: crate::bundling::MIN_BUNDLED_MEMBERS,
    },
    note: "a real collapse this detector knowingly does not claim, not an oversight: at one \
           swallowed member the shape is genuinely ambiguous, and the fleet scan found the \
           one-member population is about half correct parses (`xxd -ps`, `which -as`, \
           `sg_map -st`, `mandoc -ac`)",
}];

impl Detector for BundledShortFlag {
    fn name(&self) -> &'static str {
        "bundled-short-flag"
    }
    fn family(&self) -> Option<&'static str> {
        Some("bundled-short-flag")
    }
    fn describes(&self) -> &'static str {
        "a synopsis bundle of boolean short flags (`[-abcXYZ]`) parsed as one flag carrying the \
         rest as a required value, destroying every other member"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        crate::bundling::detect(evidence.raw, evidence.root)
            .collapses
            .iter()
            .map(|c| {
                format!(
                    "{:?} at {:?} swallows {} member(s) of the cluster {:?}",
                    c.spelling, c.path, c.destroyed, c.cluster
                )
            })
            .collect()
    }
    fn scope(&self) -> Scope {
        Scope {
            claim: "synopsis-sourced short-flag clusters with 2 or more swallowed members \
                    (`bundling::MIN_BUNDLED_MEMBERS`); a single swallowed member is deliberately \
                    excluded because the fleet scan found it genuinely ambiguous — see this \
                    detector's own module doc comment for the measured counter-examples \
                    (`xxd -ps`, `which -as`, `sg_map -st`, `mandoc -ac`) that a looser threshold \
                    would false-positive on",
            known_exclusions: BUNDLED_SHORT_FLAG_EXCLUSIONS,
        }
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        crate::bundling::self_checks()
    }
}

/// The existence oracle (`crate::existence`), registered so the harness's
/// answer for an uncalibratable detector is exercised by a real one rather
/// than only by a test double.
///
/// Its `family()` is `None` and that is a finding, not an omission: across
/// 94 human verdicts, **not one reviewer reported a fabricated subcommand
/// or flag spelling**. The defect [M-10] shipped — `tar`'s 39 invented
/// nodes — has no representative in the labelled set, so this set cannot
/// confirm or refute this oracle at all.
struct ExistenceOracle;

impl Detector for ExistenceOracle {
    fn name(&self) -> &'static str {
        "existence"
    }
    fn family(&self) -> Option<&'static str> {
        None
    }
    fn describes(&self) -> &'static str {
        "a help-text-sourced subcommand name or flag spelling that does not occur in the tool's \
         own raw text (spec §13.1)"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        crate::existence::detect(evidence.raw, evidence.root)
            .fabrications
            .iter()
            .map(|f| {
                format!(
                    "{:?} at {:?} does not occur in the raw text",
                    f.name, f.path
                )
            })
            .collect()
    }
}

/// The misattribution oracle (`crate::misattribution`), registered on the
/// same terms as [`ExistenceOracle`]. Its shape — a flag's description
/// belonging to a different flag — is adjacent to `section-header-bleed`
/// and to `missing-flag-description`, but adjacency is not identity, and
/// mapping it onto either would manufacture a matrix from a correspondence
/// nobody verified.
struct MisattributionOracle;

impl Detector for MisattributionOracle {
    fn name(&self) -> &'static str {
        "misattribution"
    }
    fn family(&self) -> Option<&'static str> {
        None
    }
    fn describes(&self) -> &'static str {
        "a flag description containing another flag's spelling, attested at a column-aligned \
         position elsewhere in the same raw text (spec §13.1)"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        crate::misattribution::detect(evidence.raw, evidence.root)
            .suspects
            .iter()
            .map(|s| format!("{:?} at {:?}", s.flag, s.path))
            .collect()
    }
}

// ----------------------------------------------------------------------
// Calibration
// ----------------------------------------------------------------------

/// One audited tool as calibration sees it.
pub struct Case {
    pub tool: String,
    /// The tool's derived family labels, empty for a `correct` verdict and
    /// for an unclassified defect alike — [`Case::expected`] is what
    /// distinguishes them.
    pub families: Vec<String>,
    /// `true` when a human judged this tool's parse defective
    /// (`wrong`/`incomplete`), `false` when they judged it `correct`.
    /// `skip` never becomes a `Case` at all.
    pub judged_defect: bool,
    /// The replayed fixture, or `None` when this tool has none — the
    /// not-evaluable population.
    pub evidence: Option<ReplayedCase>,
}

/// A case's replayed bytes and tree, kept out of [`Case`] itself so a tool
/// with no fixture is representable without a fake tree.
pub struct ReplayedCase {
    pub raw: String,
    pub root: CommandNode,
}

impl Case {
    /// Whether this tool is *expected* to fire for `family`: it must be a
    /// human-judged defect **and** carry that family label. A judged defect
    /// with no label (unclassified) is expected-silent, deliberately — an
    /// unclassified tool cannot be counted as a miss for a family nobody
    /// established it belongs to.
    fn expected(&self, family: &str) -> bool {
        self.judged_defect && self.families.iter().any(|f| f == family)
    }
}

/// One detector's confusion matrix against the labelled set, plus the two
/// populations that are not in it.
pub struct Calibration {
    pub detector: &'static str,
    pub describes: &'static str,
    /// `None` when the detector generalizes no labelled family — every cell
    /// below is then empty and [`render`] says why instead of printing a
    /// matrix of zeroes that would read as a perfect score.
    pub family: Option<&'static str>,
    /// The detector's own declared scope ([`Detector::scope`]) — carried on
    /// the result so [`render`] can print the whole declared exclusion list
    /// unconditionally, independent of what this particular run found.
    pub scope: Scope,
    /// Labelled with the family, and the detector fired. `(tool, reasons)`.
    pub true_positives: Vec<(String, Vec<String>)>,
    /// Labelled with the family, and the detector was silent — **and the
    /// tool is not a declared exclusion of [`Calibration::scope`].** This is
    /// the cell recall is computed over, i.e. recall *within* the declared
    /// scope.
    pub false_negatives: Vec<String>,
    /// Labelled with the family, the detector was silent, **and the tool is
    /// named in [`Calibration::scope`]'s declared exclusions.** A miss the
    /// detector never claimed to catch — counted, named and reasoned
    /// separately from [`Calibration::false_negatives`] rather than folded
    /// into recall, but never dropped from the report: [`render`] prints
    /// every declared exclusion every time, in red, whether or not this run
    /// found it here.
    pub out_of_scope_misses: Vec<(String, &'static Exclusion)>,
    /// Judged `correct` by a human, and the detector fired anyway.
    pub false_alarms: Vec<(String, Vec<String>)>,
    /// Judged `correct` by a human, and the detector stayed silent.
    pub true_negatives: Vec<String>,
    /// Judged a defect, but of some *other* family (or unclassified), and
    /// the detector fired. Reported in its own cell rather than folded into
    /// false alarms: the human already said this tool's parse is wrong, so
    /// a fire here is a possible mislabel or a genuine second family, not
    /// evidence that the detector is noisy. Reading it as a false positive
    /// would understate the detector; reading it as a true positive would
    /// overstate it. It is neither.
    pub fires_on_other_defect: Vec<(String, Vec<String>)>,
    /// Judged, labelled, but with no fixture to replay — evaluated by
    /// nobody, counted by name.
    pub not_evaluable: Vec<String>,
    /// Judged defects carrying no family label at all, across the whole
    /// manifest. Not a cell of this detector's matrix; printed with it
    /// because it bounds how complete *any* family's calibration can be.
    pub unclassified: Vec<String>,
    /// The detector's own hand-built cases, re-run during this calibration
    /// ([`run_self_checks`]). Not a cell of the matrix and never mixed into
    /// one: these are not labelled tools and cannot substitute for them.
    /// They answer a different question — is the *rule* still alive — which
    /// is the only question left once the family has been repaired.
    pub self_checks: Vec<SelfCheckOutcome>,
}

/// What a calibration run concluded. Three states, not two.
///
/// The third exists because the bundled-short-flag family was actually
/// repaired (spec §13.1e, "a fixed family inverts its own calibration"), and
/// with only two states the harness had to render a healthy detector in the
/// vocabulary of failure: `recall 0%`, `DOES NOT PASS`, six tools listed as
/// misses. Every word true, the impression wrong.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    /// Fires on every evaluable labelled tool it claims, and on no tool a
    /// human judged correct.
    Passes,
    /// The family this detector generalizes has been **fixed**, so the
    /// labelled set — recorded against the pre-fix parser — has nothing
    /// left to confirm.
    ///
    /// **This is a claim with evidence attached, never a suppression.**
    /// Reaching it requires the detector's own hand-built self-checks to
    /// still hold ([`self_checks_are_conclusive`]), which is exactly the
    /// evidence that separates "zero because the bug is gone" from "zero
    /// because the detector stopped working". Nothing is moved between
    /// cells to get here: recall still reads 0%, and every missed tool is
    /// still counted and still printed by name.
    Repaired,
    /// Anything else — including a family whose calibration has inverted
    /// but whose self-checks did *not* hold, which is the genuinely broken
    /// detector this state must never be confused with.
    DoesNotPass,
}

impl Calibration {
    /// Fired-when-expected over expected, **within the detector's declared
    /// scope** — [`Calibration::out_of_scope_misses`] is deliberately not in
    /// either term, because a detector is scored against what it claims,
    /// not against every shape of the family. `None` when the family has no
    /// evaluable labelled member in scope.
    pub fn recall(&self) -> Option<f64> {
        let expected = self.true_positives.len() + self.false_negatives.len();
        (expected > 0).then(|| self.true_positives.len() as f64 / expected as f64)
    }

    /// Silent-when-judged-correct over judged-correct, `None` when no
    /// evaluable `correct` case exists.
    pub fn silence(&self) -> Option<f64> {
        let good = self.true_negatives.len() + self.false_alarms.len();
        (good > 0).then(|| self.true_negatives.len() as f64 / good as f64)
    }

    /// Whether this detector's own self-checks are strong enough to stand
    /// in for a labelled set with nothing left to confirm.
    pub fn self_checks_are_conclusive(&self) -> bool {
        self_checks_are_conclusive(&self.self_checks)
    }

    /// Whether calibration has *inverted*: there were labelled members to
    /// fire on, and not one of them fires any more.
    ///
    /// Necessary for [`Verdict::Repaired`] and nowhere near sufficient — on
    /// its own this is the exact signature of a detector that has stopped
    /// working, which is why the verdict also demands the self-checks.
    pub fn calibration_inverted(&self) -> bool {
        self.true_positives.is_empty()
            && (!self.false_negatives.is_empty() || !self.out_of_scope_misses.is_empty())
    }

    /// The run's conclusion — see [`Verdict`] for why there are three.
    ///
    /// [`Verdict::Passes`] is the precondition itself: fires on every
    /// labelled member within its declared scope, and never on a tool a
    /// human judged correct. Silence on `correct` tools is required
    /// absolutely — scope narrows what a detector may be scored on
    /// *missing*, never what excuses it for *firing wrongly*, so
    /// [`Calibration::false_alarms`] blocks every verdict but
    /// [`Verdict::DoesNotPass`]. Recall is required only over what is
    /// evaluable and in scope: a detector cannot be blamed for a tool with
    /// no fixture, nor for a tool it never claimed. A named out-of-scope
    /// miss never blocks a pass, and it never stops being printed either —
    /// see [`render`].
    pub fn verdict(&self) -> Verdict {
        // A false alarm blocks everything, unconditionally and in every
        // state: firing on a tool a human judged correct is never excused
        // by a declared scope and never excused by a repaired family. This
        // project's standing rule, checked first.
        if self.family.is_none() || !self.false_alarms.is_empty() {
            return Verdict::DoesNotPass;
        }
        if self.false_negatives.is_empty() && !self.true_positives.is_empty() {
            return Verdict::Passes;
        }
        if self.calibration_inverted() && self.self_checks_are_conclusive() {
            return Verdict::Repaired;
        }
        Verdict::DoesNotPass
    }
}

/// Load every judged audit entry as a [`Case`], replaying its
/// `corpus/<tool>/<fixture_version>/` fixture when one exists.
///
/// Validates the manifest's family labels first ([`AuditFile::validate_families`]),
/// so a mistyped family fails here rather than quietly shrinking a cell.
pub fn load_cases(
    audit_file: &AuditFile,
    corpus_root: &Path,
    fixture_version: &str,
) -> anyhow::Result<Vec<Case>> {
    audit_file.validate_families()?;
    let mut replayed: BTreeMap<String, ReplayedCase> = BTreeMap::new();
    for fixture in corpus::replay_version(corpus_root, fixture_version)? {
        if let Some(root) = fixture.root {
            replayed.insert(
                fixture.tool,
                ReplayedCase {
                    raw: fixture.raw,
                    root,
                },
            );
        }
    }

    let mut cases = Vec::new();
    for entry in &audit_file.entries {
        let judged_defect = entry.is_judged_defect();
        if !judged_defect && !entry.is_judged_correct() {
            continue;
        }
        cases.push(Case {
            tool: entry.tool.clone(),
            families: entry.families.clone(),
            judged_defect,
            evidence: replayed.remove(&entry.tool),
        });
    }
    Ok(cases)
}

/// Run `detector` over `cases` and sort every one into its cell.
pub fn calibrate(
    detector: &dyn Detector,
    cases: &[Case],
    unclassified: Vec<String>,
) -> Calibration {
    let scope = detector.scope();
    let mut cal = Calibration {
        detector: detector.name(),
        describes: detector.describes(),
        family: detector.family(),
        scope,
        true_positives: Vec::new(),
        false_negatives: Vec::new(),
        out_of_scope_misses: Vec::new(),
        false_alarms: Vec::new(),
        true_negatives: Vec::new(),
        fires_on_other_defect: Vec::new(),
        not_evaluable: Vec::new(),
        unclassified,
        // Re-run every time, on the same run that computes the matrix: the
        // two have to be read together, and a cached or separately-invoked
        // self-check could be stale exactly when it matters most.
        self_checks: run_self_checks(detector),
    };
    let Some(family) = detector.family() else {
        return cal;
    };

    for case in cases {
        let Some(evidence) = &case.evidence else {
            // A tool with no fixture is only worth naming when the labels
            // say it *should* have fired: an unevaluated `correct` tool
            // contributes nothing either way and would only pad the list.
            if case.expected(family) {
                cal.not_evaluable.push(case.tool.clone());
            }
            continue;
        };
        let hits = detector.hits(&ToolEvidence {
            raw: &evidence.raw,
            root: &evidence.root,
        });
        match (case.expected(family), case.judged_defect, hits.is_empty()) {
            (true, _, false) => cal.true_positives.push((case.tool.clone(), hits)),
            // A labelled miss is only ever *reclassified*, never dropped: a
            // declared exclusion moves it from false_negatives (which blocks
            // a pass) to out_of_scope_misses (which does not) — it is still
            // named, still counted, and render() prints the whole declared
            // list unconditionally regardless of which cell it lands in.
            (true, _, true) => {
                let exclusion = cal
                    .scope
                    .known_exclusions
                    .iter()
                    .find(|e| e.tool == case.tool);
                match exclusion {
                    Some(e) => cal.out_of_scope_misses.push((case.tool.clone(), e)),
                    None => cal.false_negatives.push(case.tool.clone()),
                }
            }
            (false, true, false) => cal.fires_on_other_defect.push((case.tool.clone(), hits)),
            (false, true, true) => {}
            // Scope narrows what a detector may be scored on missing; it
            // never excuses firing on a tool a human judged correct. A false
            // alarm reaches this arm unconditionally, declared exclusion or
            // not.
            (false, false, false) => cal.false_alarms.push((case.tool.clone(), hits)),
            (false, false, true) => cal.true_negatives.push(case.tool.clone()),
        }
    }
    cal
}

// ----------------------------------------------------------------------
// Rendering
// ----------------------------------------------------------------------

/// The caveat that must appear above every calibration result, in full,
/// every time.
///
/// Not a footnote and not abbreviated on repeat runs: a number that travels
/// (into a commit message, an issue, a release note) travels without its
/// context unless the context is printed next to it. This is the same
/// discipline `accuracy: unmeasured` enforces on the coverage scoreboard
/// (spec §13.1b).
fn caveat(set: &SetSize, unclassified: usize) -> String {
    let SetSize {
        sampled,
        judged,
        evaluable,
    } = *set;
    format!(
        "CALIBRATION IS AGAINST DERIVED LABELS OVER A BOUNDED SAMPLE — NOT GROUND TRUTH ABOUT \
         THE FLEET.\n  * the verdicts are human: {judged} judged tools out of {sampled} sampled \
         in the seed-2 audit (the rest were skipped, and a skip judges nothing either way). The \
         defect-family labels on them are a MACHINE READING of each reviewer's note plus the \
         fixture evidence (`families_derived = true`) — a weaker claim than the verdict it sits \
         on\n  * {sampled} tools is roughly 4% of PATH. Passing here means a detector works on \
         these tools; it says nothing about whether its fleet-wide count is right\n  * \
         {evaluable} of the {judged} have a replayable fixture. The rest are evaluated by nobody \
         and are listed as not-evaluable rather than quietly dropped\n  * {unclassified} judged \
         defect(s) carry no family label at all, so no family's calibration here can be complete"
    )
}

/// How big the labelled set is, in the three counts every calibration
/// report must state: what the audit sampled, how much of that carries a
/// judgment at all, and how much of *that* has frozen bytes to replay.
/// Passed around as one value so a caller cannot print two of the three and
/// leave a reader to assume the missing one.
#[derive(Clone, Copy)]
pub struct SetSize {
    /// Entries in the manifest, skips included.
    pub sampled: usize,
    /// Entries with a `wrong`/`incomplete`/`correct` verdict — the ones a
    /// detector can be right or wrong about.
    pub judged: usize,
    /// Judged entries that also have a replayable fixture.
    pub evaluable: usize,
}

/// Render one detector's calibration as plain text.
pub fn render(cal: &Calibration, set: &SetSize) -> String {
    let mut s = String::new();
    s.push_str(&format!("detector: {}\n", cal.detector));
    s.push_str(&format!("checks:   {}\n", cal.describes));
    match cal.family {
        Some(f) => s.push_str(&format!("family:   {f}\n")),
        None => s.push_str("family:   (none in the labelled set)\n"),
    }
    s.push_str(&format!("scope:    {}\n", cal.scope.claim));
    s.push('\n');
    s.push_str(&caveat(set, cal.unclassified.len()));
    s.push_str("\n\n");

    if cal.family.is_none() {
        s.push_str(
            "NOT CALIBRATABLE against this set: this detector generalizes no defect family any \
             reviewer in the seed-2 audit recorded, so these tools can neither confirm nor refute \
             it. That is a property of the sample, not a defect in the detector — but its \
             fleet-wide number is not quotable on the strength of anything here.\n",
        );
        return s;
    }

    let row = |label: &str, n: usize| format!("  {label:<46}{n}\n");
    s.push_str(&row(
        "fires on labelled-bad   (true positive)",
        cal.true_positives.len(),
    ));
    s.push_str(&row(
        "silent on labelled-bad  (FALSE NEGATIVE)",
        cal.false_negatives.len(),
    ));
    s.push_str(&format!(
        "  {RED}{:<46}{}{RESET}\n",
        "silent on labelled-bad, DECLARED OUT OF SCOPE",
        cal.out_of_scope_misses.len()
    ));
    s.push_str(&row(
        "silent on labelled-good (true negative)",
        cal.true_negatives.len(),
    ));
    s.push_str(&row(
        "fires on labelled-good  (FALSE ALARM)",
        cal.false_alarms.len(),
    ));
    s.push_str(&row(
        "fires on a defect of another family",
        cal.fires_on_other_defect.len(),
    ));
    s.push_str(&row(
        "labelled-bad with no fixture (not evaluable)",
        cal.not_evaluable.len(),
    ));
    s.push('\n');
    match cal.recall() {
        Some(r) => s.push_str(&format!(
            "  recall over evaluable labelled: {:.0}%\n",
            r * 100.0
        )),
        None => s.push_str("  recall: no evaluable labelled member — nothing demonstrated\n"),
    }
    match cal.silence() {
        Some(v) => s.push_str(&format!(
            "  silence over judged-correct:    {:.0}%\n",
            v * 100.0
        )),
        None => s.push_str("  silence: no evaluable judged-correct tool\n"),
    }
    s.push('\n');
    s.push_str(&verdict_text(cal));

    // Printed unconditionally, for the same reason as the out-of-scope list
    // below: it is the evidence the REPAIRED verdict rests on, so it must be
    // visible on the runs that *don't* reach it too — otherwise the first
    // time a reader sees this block is the run where it is being used to
    // excuse a zero.
    s.push('\n');
    s.push_str(&render_self_checks(&cal.self_checks));

    // Printed unconditionally — a declared exclusion is a permanent part of
    // this detector's identity (`Detector::scope`), not data that can
    // happen to be empty this run. A PASSing verdict above must never be
    // read as "nothing was missed": if this detector declares any
    // exclusion, the tool and the reason it doesn't count sit right here,
    // in red, next to the verdict that would otherwise look complete
    // without them.
    s.push_str(&format!(
        "\n{RED}KNOWN OUT-OF-SCOPE MISSES — declared in code, not measured, never suppressed \
         ({} declared):{RESET}\n",
        cal.scope.known_exclusions.len()
    ));
    if cal.scope.known_exclusions.is_empty() {
        s.push_str("  (this detector declares no exclusion — its scope is the full family)\n");
    } else {
        for exclusion in cal.scope.known_exclusions {
            let tool = exclusion.tool;
            s.push_str(&format!(
                "  {RED}{tool}{RESET} — {}\n      why out of scope (structural): {}\n      \
                 note: {}\n",
                exclusion_status(cal, tool),
                exclusion.ground.explain(),
                exclusion.note,
            ));
        }
    }

    s.push_str(&named(
        "fires on labelled (true positives)",
        &cal.true_positives,
    ));
    s.push_str(&plain(
        "MISSED labelled tools (false negatives)",
        &cal.false_negatives,
    ));
    s.push_str(&named(
        "FALSE ALARMS on judged-correct tools",
        &cal.false_alarms,
    ));
    s.push_str(&named(
        "fires on a defect labelled as another family (neither cell — check by hand)",
        &cal.fires_on_other_defect,
    ));
    s.push_str(&plain(
        "labelled, no fixture (not evaluable)",
        &cal.not_evaluable,
    ));
    s.push_str(&plain(
        "silent on judged-correct (true negatives)",
        &cal.true_negatives,
    ));
    s.push_str(&plain(
        "judged defects with no family label at all (unclassified)",
        &cal.unclassified,
    ));
    s
}

/// The verdict paragraph — the one place a reader's impression is actually
/// formed, which is why [`Verdict::Repaired`] has to be able to say so in
/// its own words rather than being rendered as a failure.
fn verdict_text(cal: &Calibration) -> String {
    match cal.verdict() {
        Verdict::Passes => "VERDICT: PASSES calibration within its declared scope. Fires on \
                            every evaluable labelled tool it claims and on no \
                            human-judged-correct one. Its fleet-wide count may be quoted — with \
                            the caveat above attached to it, AND with the out-of-scope misses \
                            named immediately below, which a pass never erases.\n"
            .to_string(),
        Verdict::Repaired => format!(
            "VERDICT: REPAIRED — this family was FIXED, and the evidence for saying so is \
             printed below.\n  Calibration has inverted: {} labelled tool(s) that once fired \
             are silent, and there is nothing left in the labelled set to confirm against. \
             Those labels were recorded against the PRE-FIX parser, and spec §13.1e says the \
             precondition expires for a family on the commit that repairs it.\n  What carries \
             the weight instead: all {} of this detector's own hand-built self-checks still \
             hold, including {} case(s) it must fire on and {} it must stay silent on. That is \
             the evidence — and the ONLY evidence — that separates \"zero because the bug is \
             gone\" from \"zero because the detector stopped working\"; the fleet number alone \
             cannot tell them apart.\n  NOTHING IS SUPPRESSED TO REACH THIS VERDICT. Recall \
             above still reads 0%, every missed tool is still counted in the FALSE NEGATIVE \
             cell and still named below, and the declared out-of-scope miss is still printed in \
             red. A repaired family is not a clean bill of health for the fleet: run \
             `sweep-diff`, which is the instrument that answers whether fixing this broke \
             anything else.\n",
            cal.false_negatives.len() + cal.out_of_scope_misses.len(),
            cal.self_checks.len(),
            cal.self_checks
                .iter()
                .filter(|o| matches!(o.expect, Expect::Fires(_)))
                .count(),
            cal.self_checks
                .iter()
                .filter(|o| o.expect == Expect::Silent)
                .count(),
        ),
        Verdict::DoesNotPass if cal.calibration_inverted() => format!(
            "{RED}VERDICT: DOES NOT PASS calibration — and this is the dangerous shape, not the \
             ordinary one.{RESET}\n  Nothing labelled fires any more, which would be consistent \
             with the family having been repaired — but this detector's own self-checks DO NOT \
             back that reading, so a repaired family and a broken detector cannot be told apart \
             here. See the self-check block below for which case(s) stopped holding; a \
             detector that no longer fires on the hand-built defective shape is broken, and its \
             fleet-wide zero means nothing at all.\n"
        ),
        Verdict::DoesNotPass => "VERDICT: DOES NOT PASS calibration. Its fleet-wide count is not \
                                 quotable yet; see the named tools below.\n"
            .to_string(),
    }
}

/// ANSI red, used only for the known-out-of-scope-misses section and for a
/// self-check that stopped holding: the parts of the report that must be
/// visually impossible to skim past even in a run that otherwise looks
/// clean.
const RED: &str = "\x1b[31m";
const RESET: &str = "\x1b[0m";

/// What happened to one of a detector's declared exclusions in this
/// particular calibration run, cross-checked against every cell it could
/// have landed in rather than assumed to be a miss.
fn exclusion_status(cal: &Calibration, tool: &str) -> &'static str {
    if cal.out_of_scope_misses.iter().any(|(t, _)| t == tool) {
        "MISSED this run, exactly as declared"
    } else if cal.true_positives.iter().any(|(t, _)| t == tool) {
        "fired anyway this run — the declared exclusion did not hold; check by hand"
    } else if cal.not_evaluable.iter().any(|t| t == tool) {
        "no fixture this run — not evaluable, so this exclusion was not exercised"
    } else {
        "not in this labelled set at all — this exclusion was not exercised"
    }
}

fn named(title: &str, rows: &[(String, Vec<String>)]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let mut s = format!("\n{title}:\n");
    for (tool, reasons) in rows {
        s.push_str(&format!("  {tool}\n"));
        for reason in reasons.iter().take(3) {
            s.push_str(&format!("      {reason}\n"));
        }
        if reasons.len() > 3 {
            s.push_str(&format!("      ... and {} more\n", reasons.len() - 3));
        }
    }
    s
}

fn plain(title: &str, rows: &[String]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    format!("\n{title}:\n  {}\n", rows.join(", "))
}

// ----------------------------------------------------------------------
// Commands
// ----------------------------------------------------------------------

/// `xtask detector list`: every registered detector, its family, and how
/// many labelled tools that family has.
pub fn cmd_list(dir: &Path, seed: u64) -> anyhow::Result<()> {
    let file = audit::load(&audit::verdict_path(dir, seed))?;
    file.validate_families()?;
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for entry in &file.entries {
        if entry.is_judged_defect() {
            for family in &entry.families {
                *counts.entry(family_static(family)).or_default() += 1;
            }
        }
    }

    println!("registered detectors:\n");
    for d in registry() {
        let family = match d.family() {
            Some(f) => format!(
                "{f} ({} labelled tool(s))",
                counts.get(f).copied().unwrap_or(0)
            ),
            None => "(none in the labelled set — not calibratable)".to_string(),
        };
        println!(
            "  {}\n      family: {family}\n      checks: {}\n",
            d.name(),
            d.describes()
        );
    }

    println!("defect families in audit/{seed}.toml (derived labels):");
    for family in audit::family_names() {
        println!(
            "  {:<26} {:>2} labelled  — {}",
            family,
            counts.get(family).copied().unwrap_or(0),
            audit::family_meaning(family).unwrap_or(""),
        );
    }
    let unclassified: Vec<&str> = file.unclassified().map(|e| e.tool.as_str()).collect();
    println!(
        "\n  {:<26} {:>2} judged defect(s) carry no family label{}",
        "(unclassified)",
        unclassified.len(),
        if unclassified.is_empty() {
            String::new()
        } else {
            format!(": {}", unclassified.join(", "))
        }
    );
    Ok(())
}

/// Resolve a family word coming out of the manifest to its `'static`
/// spelling. Safe by construction: `validate_families` has already rejected
/// anything outside the set by the time this is reached.
fn family_static(word: &str) -> &'static str {
    audit::parse_family(word).unwrap_or("(unrecognized)")
}

/// `xtask detector calibrate`: the confusion matrix, for one detector or
/// for all of them.
pub fn cmd_calibrate(
    dir: &Path,
    seed: u64,
    corpus_root: &Path,
    fixture_version: &str,
    detector: Option<&str>,
) -> anyhow::Result<()> {
    let file = audit::load(&audit::verdict_path(dir, seed))?;
    let cases = load_cases(&file, corpus_root, fixture_version)?;
    let unclassified: Vec<String> = file.unclassified().map(|e| e.tool.clone()).collect();
    let set = SetSize {
        sampled: file.entries.len(),
        judged: cases.len(),
        evaluable: cases.iter().filter(|c| c.evidence.is_some()).count(),
    };

    let detectors = match detector {
        Some(name) => vec![find(name)?],
        None => registry(),
    };
    for d in detectors {
        let cal = calibrate(d.as_ref(), &cases, unclassified.clone());
        println!("{}", "=".repeat(76));
        println!("{}", render(&cal, &set));
    }
    Ok(())
}

// ----------------------------------------------------------------------
// The ratchet gate
// ----------------------------------------------------------------------

/// One detector's ratchet-at-zero result: the fleet counts, the self-check
/// evidence they have to be read against, and every reason the gate refused.
pub struct RatchetOutcome {
    pub detector: &'static str,
    /// What the sweep's scoreboard reported for this detector's family.
    pub tools: usize,
    pub destroyed_flags: usize,
    pub self_checks: Vec<SelfCheckOutcome>,
    /// Empty when the gate holds. Each entry is one independent reason.
    pub failures: Vec<String>,
}

impl RatchetOutcome {
    pub fn holds(&self) -> bool {
        self.failures.is_empty()
    }

    /// The full report, printed whether the gate holds or not — the counts
    /// are meaningless without the evidence beside them, so they are never
    /// printed apart.
    pub fn report(&self) -> String {
        let mut s = format!(
            "RATCHET GATE — {} is gated at zero, with evidence.\n  fleet: {} tool(s) with a \
             collapse, {} real flag(s) destroyed (both must be 0)\n\n",
            self.detector, self.tools, self.destroyed_flags
        );
        s.push_str(&render_self_checks(&self.self_checks));
        s.push('\n');
        if self.holds() {
            s.push_str(
                "GATE HOLDS: the fleet count is zero AND the detector still fires on its own \
                 hand-built defective shape while staying silent on the correct parses that \
                 resemble it. Both halves are required — see `ratchet_at_zero`.\n",
            );
        } else {
            s.push_str(&format!("{RED}GATE FAILS:{RESET}\n"));
            for failure in &self.failures {
                s.push_str(&format!("  {RED}*{RESET} {failure}\n"));
            }
        }
        s
    }
}

/// Gate `detector`'s fleet-wide count at zero — **and refuse to accept that
/// zero without evidence that the detector still works.**
///
/// # The trap this exists to close
///
/// A gate that only asserts `count == 0` is satisfied by deleting the
/// detector. It is satisfied by `hits()` returning `Vec::new()`, by the
/// registry entry being removed, by any refactor that quietly stops the
/// rule from firing. Every one of those makes the number look perfect,
/// which is the same "a metric improved by breaking the thing that measures
/// it" failure spec §13.1b already records twice ([M-10]'s fabricated `tar`
/// nodes inflating `%described`; `%flags_text`'s name overclaiming). Spec
/// §13.1e states it for detectors specifically: a detector reading zero
/// because the bug is gone and one reading zero because it stopped working
/// are indistinguishable from the fleet number alone.
///
/// So the gate has two halves and needs both:
///
/// 1. the detector's own hand-built self-checks still hold, conclusively
///    ([`self_checks_are_conclusive`] — non-empty, all held, and covering
///    both the must-fire and must-stay-silent directions), and
/// 2. the fleet counts are zero.
///
/// Deleting the detector fails half 1 and can never reach half 2's pass.
pub fn ratchet_at_zero(
    detector: &dyn Detector,
    tools: usize,
    destroyed_flags: usize,
) -> RatchetOutcome {
    let self_checks = run_self_checks(detector);
    let mut failures = Vec::new();

    // Half 1 first, deliberately: the counts mean nothing until the
    // instrument that produced them is shown to be alive.
    if self_checks.is_empty() {
        failures.push(format!(
            "{} declares NO self-check, so a fleet count of zero cannot be distinguished from \
             the detector having been deleted. A gate on the count alone is satisfied by \
             deleting the detector — that is the whole reason this half exists.",
            detector.name()
        ));
    } else {
        for outcome in &self_checks {
            if !outcome.held {
                failures.push(format!(
                    "self-check {:?} did not hold: it {} but reported {} hit(s) {:?}. The \
                     detector no longer behaves as its own evidence says it must, so its \
                     fleet-wide zero means nothing.",
                    outcome.name,
                    match outcome.expect {
                        Expect::Fires(n) => format!("must fire {n} time(s)"),
                        Expect::Silent => "must stay silent".to_string(),
                    },
                    outcome.hits.len(),
                    outcome.hits,
                ));
            }
        }
        if !self_checks
            .iter()
            .any(|o| matches!(o.expect, Expect::Fires(_)))
        {
            failures.push(format!(
                "{} declares no self-check it must FIRE on. Without one, nothing here shows the \
                 rule is still alive.",
                detector.name()
            ));
        }
        if !self_checks.iter().any(|o| o.expect == Expect::Silent) {
            failures.push(format!(
                "{} declares no self-check it must STAY SILENT on. Without one, a detector \
                 firing indiscriminately would satisfy the must-fire half — and this project's \
                 standing rule is no false positives over recall.",
                detector.name()
            ));
        }
    }

    // Half 2: the ratchet itself.
    if tools != 0 {
        failures.push(format!(
            "{tools} tool(s) exhibit this collapse; the ratchet is at 0. A commit may not \
             reintroduce a family that has been repaired."
        ));
    }
    if destroyed_flags != 0 {
        failures.push(format!(
            "{destroyed_flags} real flag(s) destroyed by a collapse; the ratchet is at 0. This \
             is the count that says how much recall the defect costs — `tools` alone badly \
             understates it."
        ));
    }

    RatchetOutcome {
        detector: detector.name(),
        tools,
        destroyed_flags,
        self_checks,
        failures,
    }
}

/// `xtask detector self-check`: re-run one detector's own hand-built cases,
/// or every detector's.
///
/// The cheap half of [`ratchet_at_zero`], usable without a `PATH` sweep —
/// it spawns nothing and reads no fixture, so CI can run it in a second on
/// every commit while the fleet half only runs where a sweep does.
pub fn cmd_self_check(detector: Option<&str>) -> anyhow::Result<()> {
    validate_registry_scopes().map_err(|e| anyhow::anyhow!("{e}"))?;
    let detectors = match detector {
        Some(name) => vec![find(name)?],
        None => registry(),
    };
    let mut broken = Vec::new();
    for d in &detectors {
        let outcomes = run_self_checks(d.as_ref());
        println!("{}", "=".repeat(76));
        println!("detector: {}", d.name());
        println!("{}", render_self_checks(&outcomes));
        if outcomes.iter().any(|o| !o.held) {
            broken.push(d.name());
        }
    }
    if !broken.is_empty() {
        anyhow::bail!(
            "self-check failed for: {} — see the FAILED case(s) above",
            broken.join(", ")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::{Provenance, Source, Text};

    fn node(name: &str) -> CommandNode {
        CommandNode::new(name, Provenance::single(Source::HelpText))
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
        let file = audit::load(&audit::verdict_path(&repo.join("audit"), 2))
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
        with.positionals.push(mandible_core::Positional {
            name: "pid".to_string(),
            required: true,
            variadic: false,
            description: None,
            provenance: Provenance::single(Source::HelpText),
        });
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
        } = BUNDLED_SHORT_FLAG_EXCLUSIONS[0].ground;
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
