//! Extracting a fixture's tree and running the corpus suite: blessing, checking, and building the report.
use super::*;

/// Extract a fixture's full tree: root extraction, then a bounded
/// recursive fill into every discovered subcommand (see this module's doc
/// comment). Returns `None` when no tier produced a root at all.
pub(crate) fn extract_tree(runner: &Runner, resolved: &ResolvedTool) -> Option<CommandNode> {
    let result = runner.extract_full_for(resolved);
    let root = result.root?;
    let mut budget = MAX_FIXTURE_NODES.saturating_sub(1);
    Some(warm(
        runner,
        resolved,
        root,
        std::slice::from_ref(&resolved.name),
        &mut budget,
    ))
}

fn warm(
    runner: &Runner,
    resolved: &ResolvedTool,
    mut node: CommandNode,
    path: &[String],
    budget: &mut usize,
) -> CommandNode {
    let children = std::mem::take(&mut node.subcommands);
    let mut filled = Vec::with_capacity(children.len());
    for child in children {
        if *budget == 0 {
            filled.push(child);
            continue;
        }
        *budget -= 1;
        let mut child_path = path.to_vec();
        child_path.push(child.name.clone());
        let fill = runner.fill_node(resolved, &child_path, child);
        filled.push(warm(runner, resolved, fill.node, &child_path, budget));
    }
    node.subcommands = filled;
    node
}

/// Render `node` in exactly the format `expected.snap` fixtures use:
/// `mandible_core::to_snapshot` through plain `serde_yaml::to_string`,
/// **not** `insta`'s snapshot macro. `insta` prepends a `source:`/
/// `expression:` header meant for its own review workflow, which a CLI
/// binary can't drive sanely (`insta`'s harness assumes a test binary and
/// per-test dynamic snapshot paths) — see the corpus work order's design
/// decision on why this crate does a plain file compare instead. Using
/// the same `to_snapshot` + `serde_yaml` pair the format is defined by
/// (`mandible_core::snapshot`'s doc comment) is what keeps this single-
/// sourced rather than a second, driftable serialization.
pub(crate) fn render_snapshot(node: &CommandNode) -> anyhow::Result<String> {
    let snapshot = mandible_core::to_snapshot(node);
    serde_yaml::to_string(&snapshot).map_err(|e| anyhow::anyhow!("serializing snapshot: {e}"))
}

/// The result of comparing a fixture's freshly-rendered tree against its
/// `expected.snap`.
enum SnapshotCheck {
    /// No tier produced a root at all — there is nothing to compare.
    NoRoot,
    /// `expected.snap` doesn't exist. Legal only for an `[xfail]` fixture
    /// (`corpus/README.md` step 4: "a fixture marked broken has no
    /// expected tree yet").
    Missing,
    /// Byte-identical.
    Match,
    /// Differs, starting at the given 1-indexed line.
    Mismatch {
        line: usize,
        expected: String,
        actual: String,
    },
}

fn check_snapshot(fixture: &Fixture, root: Option<&CommandNode>) -> anyhow::Result<SnapshotCheck> {
    let Some(root) = root else {
        return Ok(SnapshotCheck::NoRoot);
    };
    let snap_path = fixture.expected_snap_path();
    if !snap_path.is_file() {
        return Ok(SnapshotCheck::Missing);
    }
    let expected = std::fs::read_to_string(&snap_path)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", snap_path.display()))?;
    let actual = render_snapshot(root)?;
    if expected == actual {
        return Ok(SnapshotCheck::Match);
    }
    let expected_lines: Vec<&str> = expected.lines().collect();
    let actual_lines: Vec<&str> = actual.lines().collect();
    let first_diff = expected_lines
        .iter()
        .zip(actual_lines.iter())
        .position(|(e, a)| e != a)
        .unwrap_or_else(|| expected_lines.len().min(actual_lines.len()));
    Ok(SnapshotCheck::Mismatch {
        line: first_diff + 1,
        expected: expected_lines
            .get(first_diff)
            .unwrap_or(&"<end of file>")
            .to_string(),
        actual: actual_lines
            .get(first_diff)
            .unwrap_or(&"<end of file>")
            .to_string(),
    })
}

/// One fixture's outcome, for the runner's summary counters.
enum Outcome {
    /// Not marked `[xfail]`, and every check passed.
    Green,
    /// Marked `[xfail]`, and at least one check failed as expected.
    XfailAsExpected,
    /// A real failure: either a non-`[xfail]` fixture with a failing
    /// check, a mis-set-up fixture (no snapshot and no `[xfail]`), or a
    /// strict-xfail violation (an `[xfail]` fixture where every check now
    /// passes).
    Failed(String),
}

/// Run the corpus suite. `bless` rewrites every fixture's `expected.snap`
/// to match its freshly-extracted tree instead of checking it — blessing
/// an `[xfail]` fixture is legal (spec's promotion workflow blesses
/// before removing `[xfail]`; the strict-xfail check on the next plain
/// run reminds a contributor to remove it).
///
/// `format` selects a checking run's report: [`ScoreFormat::Text`] is the
/// plain per-fixture lines; [`ScoreFormat::Markdown`] additionally builds
/// a before/after transition table ([`render_markdown_report`]) for
/// `$GITHUB_STEP_SUMMARY`. Ignored while blessing.
pub fn run(corpus_root: &Path, bless: bool, format: ScoreFormat) -> anyhow::Result<CorpusReport> {
    run_with_baseline(corpus_root, bless, format, None)
}

/// [`run`], additionally comparing every fixture's `[contract]` against
/// `baseline_root` (a second, plain corpus directory — never git) and
/// printing a prominent `CONTRACT WEAKENED: ...` line for every field that
/// got weaker. See [`contract_weakened_lines`] for the full rule and why
/// this takes a directory instead of a git ref. `None` skips the check
/// entirely (unchanged behavior from before this existed) — this is what
/// [`run`] passes, so every existing caller is unaffected.
pub fn run_with_baseline(
    corpus_root: &Path,
    bless: bool,
    format: ScoreFormat,
    baseline_root: Option<&Path>,
) -> anyhow::Result<CorpusReport> {
    run_with_ceiling(
        corpus_root,
        bless,
        format,
        MAX_FIXTURE_PARSE_TIME,
        baseline_root,
    )
}

/// [`run`], with the parse-time ceiling injected rather than taken from
/// [`MAX_FIXTURE_PARSE_TIME`].
///
/// Exists so the warn-don't-block property is testable: a real fixture
/// parses in well under a millisecond, so the only way to exercise a
/// ceiling violation is to lower the ceiling. Without this seam the
/// property could only be checked by editing the constant by hand — which
/// is how it was verified originally, and exactly the kind of unguarded
/// behaviour a later refactor silently reverses.
pub(crate) fn run_with_ceiling(
    corpus_root: &Path,
    bless: bool,
    format: ScoreFormat,
    max_parse_time: Duration,
    baseline_root: Option<&Path>,
) -> anyhow::Result<CorpusReport> {
    let fixtures = discover_fixtures(corpus_root)?;
    if fixtures.is_empty() {
        anyhow::bail!(
            "no fixtures found under {} (expected corpus/<tool>/<version>/meta.toml)",
            corpus_root.display()
        );
    }
    let weakened = weakened_lines(baseline_root, &fixtures)?;
    let run = run_fixtures(&fixtures, bless, max_parse_time)?;
    Ok(finalize_report(&fixtures, bless, format, weakened, run))
}

/// Contract-weakening lines (see `contract_weakened_lines`'s doc
/// comment) — computed once, up front, from plain `meta.toml` reads
/// only (no extraction, no git), so a failure here (a malformed
/// baseline directory) surfaces before any of the real, expensive work
/// below starts.
fn weakened_lines(
    baseline_root: Option<&Path>,
    fixtures: &[Fixture],
) -> anyhow::Result<Vec<String>> {
    match baseline_root {
        Some(dir) => {
            let baseline_fixtures = discover_fixtures(dir)?;
            Ok(contract_weakened_lines(fixtures, &baseline_fixtures))
        }
        None => Ok(Vec::new()),
    }
}

/// Everything the per-fixture loop ([`run_fixtures`]) accumulates, handed
/// on to [`finalize_report`].
struct RunFixturesOutput {
    lines: Vec<String>,
    outcomes: Vec<Outcome>,
    timing_violations: Vec<String>,
    fixture_rows: Vec<FixtureRow>,
}

/// Replay every fixture's transcript through the real pipeline, checking
/// (or blessing) each one in turn — the fixture runner's main loop.
fn run_fixtures(
    fixtures: &[Fixture],
    bless: bool,
    max_parse_time: Duration,
) -> anyhow::Result<RunFixturesOutput> {
    let mut lines = Vec::new();
    let mut outcomes = Vec::new();
    let mut timing_violations = Vec::new();
    // Only populated for a checking run (`bless: false`) — see
    // `render_markdown_report`. Cheap to build unconditionally alongside
    // `lines`: every fixture here is a few KiB, never worth gating on.
    let mut fixture_rows: Vec<FixtureRow> = Vec::new();

    for fixture in fixtures {
        let transcript = fixture.build_transcript()?;
        let runner = Runner::new(default_tiers_with_probe(Arc::new(transcript)));
        let resolved = fixture.resolved_tool();

        let start = Instant::now();
        let root = extract_tree(&runner, &resolved);
        let elapsed = start.elapsed();

        if elapsed > max_parse_time {
            timing_violations.push(format!(
                "{}: parsed in {:?}, exceeding the {:?} ceiling",
                fixture.label, elapsed, max_parse_time
            ));
        }

        if bless {
            match &root {
                Some(root) => {
                    let rendered = render_snapshot(root)?;
                    std::fs::write(fixture.expected_snap_path(), &rendered).map_err(|e| {
                        anyhow::anyhow!("writing {}: {e}", fixture.expected_snap_path().display())
                    })?;
                    lines.push(format!("blessed {} ({:?})", fixture.label, elapsed));
                }
                None => {
                    lines.push(format!(
                        "{}: no root produced — nothing to bless",
                        fixture.label
                    ));
                }
            }
            continue;
        }

        let snapshot_check = check_snapshot(fixture, root.as_ref())?;
        let contract_failures = check_contract(&fixture.meta.contract, root.as_ref());
        let is_xfail = fixture.meta.xfail.as_ref().is_some_and(|x| x.broken);

        // A missing `expected.snap` is legal only for a fixture still
        // marked `[xfail]` (corpus/README.md step 4: "a fixture marked
        // broken has no expected tree yet"). For anything else it's a
        // real gap — a "green" fixture with no pinned tree asserts
        // nothing about structure at all, defeating the ratchet.
        let snapshot_ok = match &snapshot_check {
            SnapshotCheck::Match => true,
            SnapshotCheck::Missing => is_xfail,
            SnapshotCheck::NoRoot | SnapshotCheck::Mismatch { .. } => false,
        };
        let all_pass = snapshot_ok && contract_failures.is_empty();

        let mut detail = Vec::new();
        for failure in &contract_failures {
            detail.push(format!("contract: {}", failure.0));
        }
        match &snapshot_check {
            SnapshotCheck::Match => detail.push("snapshot: match".to_string()),
            SnapshotCheck::Missing if is_xfail => {
                detail.push("snapshot: none yet (legal while [xfail])".to_string())
            }
            SnapshotCheck::Missing => detail.push(
                "snapshot: missing expected.snap (required unless marked [xfail])".to_string(),
            ),
            SnapshotCheck::NoRoot => {
                detail.push("snapshot: no root produced by any tier".to_string())
            }
            SnapshotCheck::Mismatch {
                line,
                expected,
                actual,
            } => detail.push(format!(
                "snapshot mismatch at expected.snap:{line}: expected `{expected}` got `{actual}`"
            )),
        }

        // Only when a scope is actually recorded — an unscoped fixture
        // (the overwhelming majority, today) stays silent here exactly
        // as it always has; `show_fixture` is where "unscoped" is spelled
        // out explicitly for a fixture being inspected one at a time.
        // Purely informational: never affects `all_pass`, since a scope
        // claim is a record of what a human checked, not itself a check.
        if !fixture.meta.contract.verdict_scope.is_empty() {
            detail.push(format!(
                "verdict_scope: {}",
                verdict_scope_label(&fixture.meta.contract.verdict_scope)
            ));
        }

        // Unlike `verdict_scope` above, `provenance` is always set (it is
        // a required field — `discover_fixtures`'s guard), so this note
        // always prints; there is no "unscoped"-style silent case here.
        detail.push(format!(
            "provenance: {}",
            provenance_label(fixture.meta.bless.provenance)
        ));

        let outcome = if is_xfail {
            if all_pass {
                // The promote message belongs in `detail` too, not just
                // the returned `Outcome::Failed` message — otherwise it
                // only ever reaches `CorpusReport::failures` and never
                // the per-fixture line this loop prints, which is
                // supposed to be the "name the tool and what broke" text
                // a human actually reads.
                detail.push(
                    "[xfail] but every check now passes — the bug appears fixed; promote it \
                     (remove [xfail], commit expected.snap if it isn't already)"
                        .to_string(),
                );
                Outcome::Failed(format!("{}: {}", fixture.label, detail.join("; ")))
            } else {
                Outcome::XfailAsExpected
            }
        } else if all_pass {
            Outcome::Green
        } else {
            Outcome::Failed(format!("{}: {}", fixture.label, detail.join("; ")))
        };

        let status_word = match &outcome {
            Outcome::Green => "ok",
            Outcome::XfailAsExpected => "xfail (as expected)",
            Outcome::Failed(_) => "FAIL",
        };
        lines.push(format!(
            "{:<24} {:<20} ({:?})  {}",
            fixture.label,
            status_word,
            elapsed,
            detail.join("; ")
        ));

        fixture_rows.push(FixtureRow {
            label: fixture.label.clone(),
            status_word,
            detail: detail.clone(),
            current: summarize(&fixture.meta.tool.name, root.as_ref()),
            previous: previous_summary(fixture),
            verdict_scope: fixture.meta.contract.verdict_scope.clone(),
            provenance: fixture.meta.bless.provenance,
        });

        outcomes.push(outcome);
    }

    Ok(RunFixturesOutput {
        lines,
        outcomes,
        timing_violations,
        fixture_rows,
    })
}

/// Turn [`run_fixtures`]'s raw accumulators into the final report: the
/// summary line, the markdown-or-text rendering, and the failure list.
fn finalize_report(
    fixtures: &[Fixture],
    bless: bool,
    format: ScoreFormat,
    weakened: Vec<String>,
    run: RunFixturesOutput,
) -> CorpusReport {
    let RunFixturesOutput {
        mut lines,
        outcomes,
        timing_violations,
        fixture_rows,
    } = run;

    let failed_count = outcomes
        .iter()
        .filter(|o| matches!(o, Outcome::Failed(_)))
        .count();
    let failures: Vec<String> = outcomes
        .iter()
        .filter_map(|o| match o {
            Outcome::Failed(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect();
    // Deliberately NOT extended with `timing_violations` — the parse-time
    // ceiling warns, it does not fail the run. See
    // [`MAX_FIXTURE_PARSE_TIME`] for why.

    let green = outcomes
        .iter()
        .filter(|o| matches!(o, Outcome::Green))
        .count();
    let xfail = outcomes
        .iter()
        .filter(|o| matches!(o, Outcome::XfailAsExpected))
        .count();

    lines.push(String::new());
    if bless {
        lines.push(format!("blessed {} fixture(s)", fixtures.len()));
    } else {
        let ok_provenance = provenance_split_label(provenance_counts(
            fixture_rows
                .iter()
                .filter(|r| r.status_word == "ok")
                .map(|r| r.provenance),
        ));
        lines.push(format!(
            "{} fixture(s): {green} ok ({ok_provenance}), {xfail} xfail (as expected), \
             {failed_count} failed",
            fixtures.len(),
        ));
        for violation in &timing_violations {
            // "warning", not a bare label: this line does not fail the
            // run, and a reader who cannot tell that from the output will
            // either chase a non-failure or learn to skim past real ones.
            lines.push(format!("warning: slow parse (does not fail): {violation}"));
        }
    }

    // Markdown only applies to a checking run — see `run`'s doc comment.
    // It fully replaces `lines.join(...)` rather than appending to it: the
    // markdown report is a self-contained artifact (table plus `<details>`
    // for anything capped), not the plain-text report with formatting
    // bolted on.
    let text = if !bless && matches!(format, ScoreFormat::Markdown) {
        render_markdown_report(
            &fixture_rows,
            fixtures.len(),
            green,
            xfail,
            failed_count,
            &weakened,
        )
    } else {
        // Contract-weakening lines go first, ahead of every per-fixture
        // line and the summary — "prominent... so a reviewer skimming a
        // green run cannot miss it" means the very top of the output, not
        // one more line in a footer nobody reads once green.
        let mut prefixed = weakened;
        prefixed.extend(lines);
        prefixed.join("\n")
    };

    CorpusReport {
        text,
        failures,
        bless,
    }
}
