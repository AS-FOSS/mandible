//! `xtask audit report`: per-stratum and overall accuracy as a count and
//! confidence interval (Wilson score, spec §13.1b), plus the K1/K2/K3
//! sensitivity views and the wrong/incomplete/skipped/out-of-scope tool
//! listings — all in [`render_report`]'s fixed order.

use super::FORCED_INCLUSION_STRATUM;
use mandible_core::audit::{family_meaning, load, verdict_path, AuditFile, Entry};
use std::collections::BTreeMap;
use std::path::Path;

/// Wilson score interval for a binomial proportion at ~95% confidence
/// (`z = 1.96`). Chosen over the naive normal approximation, which
/// produces bounds outside `[0, 1]` at the small-`n`, near-0-or-1
/// proportions a first audit run is likely to hit (`n=5`, `k=5`). Returns
/// `(lower, upper)` in `[0, 1]`; `(0.0, 1.0)` for `n == 0`.
pub(super) fn wilson_interval(k: usize, n: usize) -> (f64, f64) {
    if n == 0 {
        return (0.0, 1.0);
    }
    let z = 1.96_f64;
    let n = n as f64;
    let p = k as f64 / n;
    let denom = 1.0 + z * z / n;
    let center = p + z * z / (2.0 * n);
    let adj = z * ((p * (1.0 - p) / n) + (z * z / (4.0 * n * n))).sqrt();
    (
        ((center - adj) / denom).max(0.0),
        ((center + adj) / denom).min(1.0),
    )
}

pub(super) struct StratumTally {
    correct: usize,
    judged: usize,
    skipped: usize,
    pending: usize,
    /// Judged `wrong`/`incomplete` entries with [`Entry::is_display_only`]
    /// true — kept out of `judged` (and therefore out of `accuracy_over`'s
    /// denominator, spec §13.1c/task #28) but still tallied and printed,
    /// the same "recorded, not omitted" treatment `skipped` already gets.
    out_of_scope: usize,
}

/// The stratum label a report groups `entry` under, checked in priority
/// order: [`Entry::spot_audit_event`] (`spot-audit:<event>`, spec §13.1b's
/// sixth rule), else [`FORCED_INCLUSION_STRATUM`] for any other
/// force-included entry, else the entry's own [`Entry::stratum`].
pub(super) fn effective_stratum(entry: &Entry) -> String {
    if let Some(event) = &entry.spot_audit_event {
        format!("spot-audit:{event}")
    } else if entry.include_reason.is_some() {
        FORCED_INCLUSION_STRATUM.to_string()
    } else {
        entry.stratum.clone()
    }
}

/// A plain `(correct, judged)` accuracy tally over whatever subset of
/// entries `keep` selects — shared machinery behind every accuracy number
/// [`cmd_report`] prints.
///
/// Reads [`Entry::effective_verdict`], never the raw [`Entry::verdict`]:
/// an amended entry's corrected verdict is what the project believes.
///
/// Also skips every [`Entry::is_display_only`] entry (task #28: a
/// display/rendering-only finding is not an accuracy finding, though it
/// stays a kept `wrong`/`incomplete` verdict elsewhere) — done here once
/// rather than at each call site.
pub(super) fn accuracy_over<'a>(entries: impl Iterator<Item = &'a Entry>) -> (usize, usize) {
    let mut correct = 0usize;
    let mut judged = 0usize;
    for entry in entries {
        if entry.is_display_only() {
            continue;
        }
        match entry.effective_verdict() {
            Some("correct") => {
                correct += 1;
                judged += 1;
            }
            Some("incomplete") | Some("wrong") => judged += 1,
            _ => {}
        }
    }
    (correct, judged)
}

/// Print one `label`, count, accuracy and 95% CI line, in the shared format
/// every accuracy line in this report uses — never a bare percentage.
pub(super) fn accuracy_line(label: &str, correct: usize, judged: usize) -> String {
    let (lo, hi) = wilson_interval(correct, judged);
    let acc = if judged == 0 {
        "  n/a".to_string()
    } else {
        format!("{:>4.1}%", correct as f64 / judged as f64 * 100.0)
    };
    format!(
        "{label:<24}  {correct:>5}/{judged:<6}  {acc}   [{:>5.1}%, {:>5.1}%]",
        lo * 100.0,
        hi * 100.0,
    )
}

/// How favorable a verdict word is to the parser, for [`wilson_caveat_lines`]'s
/// amendment-direction tally: `correct` is the best outcome, `wrong` the
/// worst, `incomplete` between the two. `skip` has no comparable
/// favorability (there is nothing to judge), so it is deliberately absent —
/// an amendment into or out of `skip` is not counted as a directional move
/// either way.
pub(super) fn verdict_favorability(verdict: &str) -> Option<i32> {
    match verdict {
        "correct" => Some(2),
        "incomplete" => Some(1),
        "wrong" => Some(0),
        _ => None,
    }
}

/// Print the standing caveat every accuracy figure needs: a Wilson
/// interval bounds sampling error only, not reviewer error (every verdict
/// is one person's unchecked read). Tallies [`Entry::amendments`] toward a
/// more favorable outcome (`wrong`/`incomplete` -> `correct`) versus a
/// less favorable one, via [`verdict_favorability`], and reports the
/// actual balance rather than a hardcoded claim.
pub(super) fn wilson_caveat_lines(file: &AuditFile) -> Vec<String> {
    let mut amended_count = 0usize;
    let mut toward_more_favorable = 0usize;
    let mut toward_less_favorable = 0usize;
    for entry in &file.entries {
        if !entry.amendments.is_empty() {
            amended_count += 1;
        }
        for amendment in &entry.amendments {
            if let (Some(before), Some(after)) = (
                verdict_favorability(&amendment.previous_verdict),
                verdict_favorability(&amendment.new_verdict),
            ) {
                match after.cmp(&before) {
                    std::cmp::Ordering::Greater => toward_more_favorable += 1,
                    std::cmp::Ordering::Less => toward_less_favorable += 1,
                    std::cmp::Ordering::Equal => {}
                }
            }
        }
    }
    let mut lines = vec![
        String::new(),
        "note: the 95% CI above bounds sampling error only — how much this sample's accuracy \
         could plausibly vary on a fresh draw of the same size — never reviewer error. Read the \
         accuracy figure as \"accuracy of the parser as judged by this reviewer,\" not an \
         absolute truth."
            .to_string(),
    ];
    if amended_count == 0 {
        lines.push(
            "note: no verdict in this file has been amended yet (`xtask audit amend`) — this \
             says nothing about whether the recorded verdicts are all correct, only that none \
             has been corrected so far."
                .to_string(),
        );
    } else {
        lines.push(format!(
            "note: {amended_count} verdict(s) carry a recorded amendment; of the corrections \
             with a comparable direction, {toward_less_favorable} made the verdict less \
             favorable to the parser (an originally too-generous read) and \
             {toward_more_favorable} made it more favorable (an originally too-harsh read).{}",
            if toward_less_favorable > toward_more_favorable {
                " More corrections have gone the generous-to-harsh direction than the reverse \
                 so far, so this accuracy figure likely still reads a little high."
            } else if toward_more_favorable > toward_less_favorable {
                " More corrections have gone the harsh-to-generous direction than the reverse \
                 so far, so this accuracy figure likely still reads a little low."
            } else {
                " The corrections so far do not lean toward either direction."
            }
        ));
    }
    lines
}

/// The `skip` verdicts, named — the stratum table prints only a `skipped`
/// count per stratum, which makes the accuracy denominator's exclusion
/// unauditable on its own. `skip` is recorded, not omitted (spec §16):
/// every skipped tool by name, with the reviewer's reason or an explicit
/// `(no reason recorded)`.
///
/// Returns whole lines (header included) rather than printing, so content
/// is testable without capturing stdout.
pub(super) fn skipped_lines(file: &AuditFile) -> Vec<String> {
    let mut skipped: Vec<&Entry> = file
        .entries
        .iter()
        .filter(|e| e.effective_verdict() == Some("skip"))
        .collect();
    if skipped.is_empty() {
        return Vec::new();
    }
    skipped.sort_by(|a, b| a.tool.cmp(&b.tool));
    let mut lines = vec![
        String::new(),
        format!(
            "tools skipped ({} — recorded, never omitted; excluded from every accuracy figure \
         above, so this is the list that makes that exclusion checkable):",
            skipped.len()
        ),
    ];
    for entry in skipped {
        let reason = if entry.effective_note().trim().is_empty() {
            "(no reason recorded)"
        } else {
            entry.effective_note()
        };
        lines.push(format!("  {:<24} {}", entry.tool, reason));
    }
    lines
}

/// Build `xtask audit report`'s full text without printing it, so a caller
/// (`cmd_report` itself, and `xtask audit contribute`'s `<seed>-report.txt`)
/// can both use exactly the same rendering rather than one re-deriving it or
/// scraping the other's stdout (AGENTS.md §3.3: never parse human-format
/// output, including your own).
pub(crate) fn render_report(dir: &Path, seed: u64) -> anyhow::Result<String> {
    let path = verdict_path(dir, seed);
    let file = load(&path)?;

    let by_stratum = tally_by_stratum(&file);

    let mut lines: Vec<String> = Vec::new();
    lines.push(format!(
        "audit seed={seed} sample_size={} ({} entries total)",
        file.meta.sample_size,
        file.entries.len()
    ));
    lines.push(String::new());
    lines.extend(stratum_table_lines(&file, &by_stratum));
    lines.extend(k_sensitivity_lines(&file));
    lines.extend(flagged_lines(&file));
    lines.extend(skipped_lines(&file));
    lines.extend(out_of_scope_lines(&file));
    lines.push(String::new());
    Ok(lines.join("\n"))
}

/// Per-stratum tally (correct/judged/skipped/pending/out-of-scope) feeding
/// [`stratum_table_lines`] — split out of [`render_report`] (ratchet:
/// `clippy::too_many_lines`) so the tally pass and the rendering pass are
/// each their own function.
pub(super) fn tally_by_stratum(file: &AuditFile) -> BTreeMap<String, StratumTally> {
    let mut by_stratum: BTreeMap<String, StratumTally> = BTreeMap::new();
    for entry in &file.entries {
        let tally = by_stratum
            .entry(effective_stratum(entry))
            .or_insert(StratumTally {
                correct: 0,
                judged: 0,
                skipped: 0,
                pending: 0,
                out_of_scope: 0,
            });
        match entry.effective_verdict() {
            None => tally.pending += 1,
            Some("skip") => tally.skipped += 1,
            Some("correct") => {
                tally.correct += 1;
                tally.judged += 1;
            }
            // A display-only finding is judged (`wrong`/`incomplete`, never
            // `skip` — see `Entry::is_display_only`'s doc comment on why
            // `skip` is the wrong tool for this), so it must not fall into
            // the catch-all `judged` arm below: that is precisely the
            // count `accuracy_over` also excludes it from. Checked before
            // the catch-all, not after, so it can never double-count.
            Some(_) if entry.is_display_only() => tally.out_of_scope += 1,
            Some(_) => tally.judged += 1,
        }
    }
    by_stratum
}

/// The per-stratum table (header row, one row per stratum, the OVERALL
/// row, and the low-n/out-of-scope notes plus [`wilson_caveat_lines`]) —
/// split out of [`render_report`] (ratchet: `clippy::too_many_lines`).
pub(super) fn stratum_table_lines(
    file: &AuditFile,
    by_stratum: &BTreeMap<String, StratumTally>,
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    lines.push(
        "stratum             correct/judged   accuracy   95% CI            skipped   pending   \
         out-of-scope"
            .to_string(),
    );
    let mut overall_correct = 0usize;
    let mut overall_judged = 0usize;
    let mut overall_skipped = 0usize;
    let mut overall_pending = 0usize;
    let mut overall_out_of_scope = 0usize;
    for (stratum, t) in by_stratum {
        let (lo, hi) = wilson_interval(t.correct, t.judged);
        let acc = if t.judged == 0 {
            "  n/a".to_string()
        } else {
            format!("{:>4.1}%", t.correct as f64 / t.judged as f64 * 100.0)
        };
        lines.push(format!(
            "{stratum:<18}  {:>5}/{:<6}  {acc}   [{:>5.1}%, {:>5.1}%]   {:>7}   {:>7}   {:>12}",
            t.correct,
            t.judged,
            lo * 100.0,
            hi * 100.0,
            t.skipped,
            t.pending,
            t.out_of_scope,
        ));
        overall_correct += t.correct;
        overall_judged += t.judged;
        overall_skipped += t.skipped;
        overall_pending += t.pending;
        overall_out_of_scope += t.out_of_scope;
    }
    let (lo, hi) = wilson_interval(overall_correct, overall_judged);
    let overall_acc = if overall_judged == 0 {
        "  n/a".to_string()
    } else {
        format!(
            "{:>4.1}%",
            overall_correct as f64 / overall_judged as f64 * 100.0
        )
    };
    lines.push(format!(
        "{:<18}  {:>5}/{:<6}  {overall_acc}   [{:>5.1}%, {:>5.1}%]   {:>7}   {:>7}   {:>12}",
        "OVERALL",
        overall_correct,
        overall_judged,
        lo * 100.0,
        hi * 100.0,
        overall_skipped,
        overall_pending,
        overall_out_of_scope,
    ));
    if overall_judged > 0 && overall_judged < 30 {
        lines.push(format!(
            "\nnote: n={overall_judged} judged so far — the interval above is wide at this size; \
             keep reviewing for a number worth acting on (spec's own target is ~60-100)."
        ));
    }
    if overall_out_of_scope > 0 {
        let mut names: Vec<&str> = file
            .entries
            .iter()
            .filter(|e| e.is_display_only())
            .map(|e| e.tool.as_str())
            .collect();
        names.sort_unstable();
        lines.push(format!(
            "\nnote: {overall_out_of_scope} finding(s) are display-only and are excluded from \
             every accuracy figure above, not dropped — the maintainer's ruling (task #28) is \
             that a display/rendering defect is a real finding but not an accuracy one: {}. See \
             the 'display-only findings (kept, out of scope)' section below for each one's note \
             in full.",
            names.join(", "),
        ));
    }
    lines.extend(wilson_caveat_lines(file));
    lines
}

/// The K1/K2/K3 sensitivity section: four accuracy views (all-inclusive,
/// each K-family excluded, all three excluded) — split out of
/// [`render_report`] (ratchet: `clippy::too_many_lines`).
pub(super) fn k_sensitivity_lines(file: &AuditFile) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let k1_tagged = file.entries.iter().filter(|e| e.k1 == Some(true)).count();
    let k2_tagged = file.entries.iter().filter(|e| e.k2 == Some(true)).count();
    let k3_tagged = file.entries.iter().filter(|e| e.k3 == Some(true)).count();
    lines.push(format!(
        "\nK1/K2/K3 sensitivity ({k1_tagged} entr{k1_s} tagged K1, {k2_tagged} entr{k2_s} \
         tagged K2, {k3_tagged} entr{k3_s} tagged K3 — see mandible_core::audit's \
         Entry::k1/k2/k3 doc comments and this module's *_signature functions):",
        k1_s = if k1_tagged == 1 { "y" } else { "ies" },
        k2_s = if k2_tagged == 1 { "y" } else { "ies" },
        k3_s = if k3_tagged == 1 { "y" } else { "ies" },
    ));
    lines.push("view                      correct/judged   accuracy   95% CI".to_string());
    let (c, j) = accuracy_over(file.entries.iter());
    lines.push(accuracy_line("all-inclusive", c, j));
    let (c, j) = accuracy_over(file.entries.iter().filter(|e| e.k1 != Some(true)));
    lines.push(accuracy_line("K1-excluded", c, j));
    let (c, j) = accuracy_over(file.entries.iter().filter(|e| e.k2 != Some(true)));
    lines.push(accuracy_line("K2-excluded", c, j));
    let (c, j) = accuracy_over(file.entries.iter().filter(|e| e.k3 != Some(true)));
    lines.push(accuracy_line("K3-excluded", c, j));
    let (c, j) = accuracy_over(
        file.entries
            .iter()
            .filter(|e| e.k1 != Some(true) && e.k2 != Some(true) && e.k3 != Some(true)),
    );
    lines.push(accuracy_line("K1+K2+K3-excluded", c, j));
    lines
}

/// The "tools judged wrong or incomplete" section — split out of
/// [`render_report`] (ratchet: `clippy::too_many_lines`).
pub(super) fn flagged_lines(file: &AuditFile) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut flagged: Vec<&Entry> = file
        .entries
        .iter()
        .filter(|e| matches!(e.effective_verdict(), Some("wrong") | Some("incomplete")))
        .collect();
    flagged.sort_by(|a, b| a.tool.cmp(&b.tool));
    if !flagged.is_empty() {
        lines.push("\ntools judged wrong or incomplete (the next bugs):".to_string());
        for entry in flagged {
            let amended_tag = if entry.amendments.is_empty() {
                ""
            } else {
                " [amended]"
            };
            // Stays in this list — it is still a `wrong`/`incomplete`
            // verdict on disk and a real finding — but tagged so a reader
            // scanning "the next bugs" does not mistake a rendering fix
            // for a parser fix. `accuracy_over` has already excluded it
            // from every count printed above; this tag is why the two
            // views (this list and the headline) don't silently disagree
            // about which tools are counted where.
            let scope_tag = if entry.is_display_only() {
                " [display-only, excluded from accuracy — see below]"
            } else {
                ""
            };
            lines.push(format!(
                "  {:<24} {:<11} {}{amended_tag}{scope_tag}",
                entry.tool,
                entry.effective_verdict().unwrap_or(""),
                entry.effective_note(),
            ));
        }
    }
    lines
}

/// The "display-only findings (kept, out of scope)" section — split out of
/// [`render_report`] (ratchet: `clippy::too_many_lines`).
pub(super) fn out_of_scope_lines(file: &AuditFile) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut out_of_scope: Vec<&Entry> = file
        .entries
        .iter()
        .filter(|e| e.is_display_only())
        .collect();
    out_of_scope.sort_by(|a, b| a.tool.cmp(&b.tool));
    if !out_of_scope.is_empty() {
        lines.push(format!(
            "\ndisplay-only findings (kept, out of scope — real UI bugs, excluded from accuracy \
             per the maintainer's task #28 ruling; family meaning: {}):",
            family_meaning("display-only").unwrap_or("?"),
        ));
        for entry in out_of_scope {
            lines.push(format!(
                "  {:<24} {:<11} {}",
                entry.tool,
                entry.effective_verdict().unwrap_or(""),
                entry.effective_note(),
            ));
        }
    }
    lines
}

/// `xtask audit report`: per-stratum and overall accuracy as a count and
/// confidence interval, never a bare percentage (spec §13.1b). Also
/// reports four K1/K2 views (all-inclusive, K1-excluded, K2-excluded,
/// both-excluded — see [`Entry::k1`]/[`Entry::k2`]) and lists every tool
/// judged `wrong`/`incomplete`.
pub fn cmd_report(dir: &Path, seed: u64) -> anyhow::Result<()> {
    print!("{}", render_report(dir, seed)?);
    Ok(())
}
