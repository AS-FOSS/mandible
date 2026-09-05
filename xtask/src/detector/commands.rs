//! The `xtask detector` subcommands and the ratchet-at-zero gate.
use super::*;

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

/// Gate `detector`'s fleet-wide count at zero — and refuse to accept that
/// zero without evidence the detector still works.
///
/// A gate on `count == 0` alone is satisfied by deleting the detector, by
/// `hits()` returning `Vec::new()`, by any refactor that quietly stops the
/// rule firing — the same "a metric improved by breaking what measures
/// it" failure spec §13.1b already records twice ([M-10], `%flags_text`).
/// Spec §13.1e: zero-because-fixed and zero-because-broken are
/// indistinguishable from the fleet number alone.
///
/// So the gate needs both: (1) the detector's self-checks still hold,
/// conclusively ([`self_checks_are_conclusive`]), and (2) the fleet counts
/// are zero. Deleting the detector fails (1) and can never reach (2).
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

/// One vim-family detector's fleet count, ratcheted at zero between two
/// `Aggregate`s — the shared body [`crate::main`]'s `coverage --check`
/// calls per repaired family, so each new one costs one call here rather
/// than a repeated block growing that file past its size ceiling. Prints
/// the same change line and ratchet report `ratchet_at_zero`'s other
/// callers do; returns whether the gate holds.
pub fn check_vim_family_ratchet(
    name: &'static str,
    previous: &crate::coverage::Aggregate,
    fresh: &crate::coverage::Aggregate,
) -> anyhow::Result<bool> {
    let count = |aggregate: &crate::coverage::Aggregate| {
        aggregate
            .vim_family
            .iter()
            .find(|(n, ..)| *n == name)
            .map_or((0, 0), |(_, tools, flags)| (*tools, *flags))
    };
    let (prev_tools, prev_flags) = count(previous);
    let (fresh_tools, fresh_flags) = count(fresh);
    if fresh_tools != prev_tools || fresh_flags != prev_flags {
        println!(
            "{name} findings changed from {prev_tools} tool(s)/{prev_flags} flag(s) to \
             {fresh_tools} tool(s)/{fresh_flags} flag(s)",
        );
    }
    let ratchet = ratchet_at_zero(find(name)?.as_ref(), fresh_tools, fresh_flags);
    println!("\n{}", ratchet.report());
    Ok(ratchet.holds())
}

/// [`check_vim_family_ratchet`] for every round-6 parser-A family
/// repaired and gated at zero, atlas S-116, S-118 to S-120
/// (`spaced-single-dash-long`, S-117, stays open, not in this list),
/// `true` only if every one holds. One call site for the whole batch, so
/// `main.rs` gains one line rather than one per family.
pub fn check_round6_family_ratchets(
    previous: &crate::coverage::Aggregate,
    fresh: &crate::coverage::Aggregate,
) -> anyhow::Result<bool> {
    let mut all_hold = true;
    for name in [
        "comma-glued-option-value",
        "hash-in-spelling",
        "nested-bracket-value",
        "choices-after-optional-placeholder",
    ] {
        if !check_vim_family_ratchet(name, previous, fresh)? {
            all_hold = false;
        }
    }
    Ok(all_hold)
}
