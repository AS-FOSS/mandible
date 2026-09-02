//! Loading audited cases and running a detector's confusion matrix against them.
use super::*;

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
