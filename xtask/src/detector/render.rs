//! Rendering one detector's calibration result as the plain-text report.
use super::*;

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
pub(crate) const RED: &str = "\x1b[31m";
pub(crate) const RESET: &str = "\x1b[0m";

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
