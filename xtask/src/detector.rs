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
struct BundledShortFlag;

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
    /// Labelled with the family, and the detector fired. `(tool, reasons)`.
    pub true_positives: Vec<(String, Vec<String>)>,
    /// Labelled with the family, and the detector was silent.
    pub false_negatives: Vec<String>,
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
}

impl Calibration {
    /// Fired-when-expected over expected — recall against the labelled set,
    /// `None` when the family has no evaluable labelled member.
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

    /// The precondition itself: fires on every labelled member it can see,
    /// and never on a tool a human judged correct.
    ///
    /// Silence on `correct` tools is required absolutely, while recall is
    /// required only over what is evaluable — a detector cannot be blamed
    /// for a tool with no fixture. A detector with no evaluable labelled
    /// member has demonstrated nothing and does not pass.
    pub fn passes(&self) -> bool {
        self.family.is_some()
            && self.false_alarms.is_empty()
            && self.false_negatives.is_empty()
            && !self.true_positives.is_empty()
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
    let mut cal = Calibration {
        detector: detector.name(),
        describes: detector.describes(),
        family: detector.family(),
        true_positives: Vec::new(),
        false_negatives: Vec::new(),
        false_alarms: Vec::new(),
        true_negatives: Vec::new(),
        fires_on_other_defect: Vec::new(),
        not_evaluable: Vec::new(),
        unclassified,
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
            (true, _, true) => cal.false_negatives.push(case.tool.clone()),
            (false, true, false) => cal.fires_on_other_defect.push((case.tool.clone(), hits)),
            (false, true, true) => {}
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
    s.push_str(if cal.passes() {
        "VERDICT: PASSES calibration. Fires on every evaluable labelled tool and on no \
         human-judged-correct one. Its fleet-wide count may be quoted — with the caveat above \
         attached to it.\n"
    } else {
        "VERDICT: DOES NOT PASS calibration. Its fleet-wide count is not quotable yet; see the \
         named tools below.\n"
    });

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
        assert!(!cal.passes(), "a false alarm and a miss must both block");
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
        assert!(!cal.passes(), "nothing was demonstrated, so it cannot pass");
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

    #[test]
    fn a_detector_with_no_labelled_family_reports_not_calibratable() {
        let cal = calibrate(&ExistenceOracle, &[], Vec::new());
        assert!(cal.family.is_none());
        assert!(!cal.passes());
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
        // accordingly; man-recode's defect is the wrapped usage synopsis,
        // outside the audit's declared flags/subcommand scope, so it stays
        // unclassified rather than being labelled on a guess. Still
        // recorded as such, and not quietly labelled by a later edit.
        let unclassified: Vec<&str> = file.unclassified().map(|e| e.tool.as_str()).collect();
        assert_eq!(unclassified, vec!["man-recode"]);
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
}
