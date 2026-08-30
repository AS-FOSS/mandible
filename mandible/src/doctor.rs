//! `mandible --doctor <tool>`: a non-TUI diagnostic (spec §5.3).
//!
//! Prints the detected framework (spec §7 Tier A′), tier statuses,
//! node/flag counts (plus modifiers and environment variables, for the few
//! tools that document any), and the fraction of *describable* flags that
//! carry a description — the primary way to verify extraction behavior
//! without a terminal.
//!
//! The percentage here is deliberately the **same instrument** as the
//! `cargo xtask coverage` scoreboard (spec §13.1/§13.1b), computed via the
//! same [`mandible_extract::ExtractionResult`] accessors rather than a
//! second, hand-rolled tree walk. The two used to disagree: this file used
//! to divide described flags by *every* flag, including usage-synopsis-only
//! ones that can never carry a description by construction, so a tool like
//! `git` — whose root flags are entirely synopsis-derived — reported `0.0%
//! described` here (read: total failure) while the scoreboard correctly
//! reported `—` (read: not applicable, nothing to rate). Reusing the
//! accessors makes that kind of drift a type error instead of a bug two
//! instruments can quietly disagree about.

use crate::pipeline::LoadedTool;
use std::fmt::Write as _;

/// Print the diagnostic report for `loaded` to stdout.
pub fn print_report(loaded: &LoadedTool) {
    print!("{}{}", build_report(loaded), report_hint(&loaded.tool));
}

/// Build the diagnostic report for `loaded` as a string, rather than
/// printing it directly — so `--report` (`mandible/src/report.rs`) can
/// embed the exact same text inside its paste-ready block instead of
/// re-deriving it (and risking the two drift apart, which is the same
/// mistake this file's `%described` fix exists to undo).
pub fn build_report(loaded: &LoadedTool) -> String {
    let mut out = String::new();

    writeln!(out, "mandible --doctor {}", loaded.tool).unwrap();
    writeln!(out).unwrap();

    let resolved = mandible_extract::resolve_tool(&loaded.tool);
    let framework = mandible_extract::framework::identify(&resolved);
    writeln!(out, "framework:  {}", framework.describe()).unwrap();
    writeln!(out).unwrap();

    writeln!(out, "tiers:").unwrap();
    for status in &loaded.tier_statuses {
        let state = if !status.detected {
            "not detected".to_string()
        } else if let Some(err) = &status.error {
            format!("FAILED: {err}")
        } else {
            "ok".to_string()
        };
        writeln!(out, "  {:<28} {}", status.tier, state).unwrap();
    }
    writeln!(out).unwrap();

    match &loaded.root {
        Some(_) => {
            let nodes = loaded.node_count();
            let flags = loaded.flag_count();
            let describable = loaded.describable_flag_count();
            // `—` when nothing is describable (spec §13.1b), never `0.0%`:
            // a root whose flags are entirely usage-synopsis-derived (e.g.
            // `git`) has no describable flags at all, and "not applicable"
            // is a different fact than "described nothing it could have."
            let pct = if describable == 0 {
                "—".to_string()
            } else {
                format!("{:.1}%", loaded.flag_description_ratio() * 100.0)
            };
            writeln!(out, "nodes:      {nodes}").unwrap();
            writeln!(out, "flags:      {flags} ({pct} flags with text)").unwrap();
            // Only when the tool has any. A modifier table is rare (spec
            // §7 Tier B), and a `modifiers:  0` line on every other tool
            // would be one more row to read past on the diagnostic whose
            // whole job is to be scanned quickly. When they are there,
            // saying so is the difference between `ar` reporting six
            // recovered items and twenty-three.
            let modifiers = loaded.modifier_count();
            if modifiers > 0 {
                writeln!(out, "modifiers:  {modifiers}").unwrap();
            }
            // Same rule as modifiers, for the other emission-stage kind
            // (spec §7 Tier B, "Environment sections"): an environment
            // section is rare, so the row appears only when there is one.
            let env_vars = loaded.env_var_count();
            if env_vars > 0 {
                writeln!(out, "env vars:   {env_vars}").unwrap();
            }
        }
        None => {
            writeln!(
                out,
                "result:     no tier produced a root node for {:?}",
                loaded.tool
            )
            .unwrap();
        }
    }
    writeln!(out).unwrap();

    // This percentage has only ever measured whether a flag has text
    // attached, never whether that text is *correct* — see the scoreboard's
    // own `accuracy: unmeasured` line (`xtask/src/coverage.rs`,
    // `accuracy_unmeasured_line`) and `Row::pct_flags_with_text`'s doc
    // comment for the measured case (`lsof` scored 79% "described" while
    // roughly a quarter of its flags were actually correct). Repeated here
    // so nobody reads "% flags with text" as an accuracy claim just because
    // this is the instrument they happened to run.
    writeln!(out, "accuracy:   unmeasured").unwrap();

    // Spec §11: there is no cache — every extraction is fresh.
    writeln!(
        out,
        "elapsed:    {:.2}ms",
        loaded.elapsed.as_secs_f64() * 1000.0
    )
    .unwrap();

    out
}

/// The `--report` pointer `--doctor` prints after its diagnostic.
///
/// Deliberately **not** part of [`build_report`]. `report.rs` embeds
/// `build_report`'s output verbatim inside the paste-ready block, and this
/// line is addressed to the person at the terminal, not to the maintainer
/// reading the pasted issue — inside that block it reads as an instruction
/// to someone who has already followed it.
pub fn report_hint(tool: &str) -> String {
    format!("\nFound a bad parse? Run `mandible --report {tool}` for a paste-ready bug report.\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::{CommandNode, Entity, Provenance, Source};
    use mandible_extract::{ExtractionResult, TierStatus};

    /// The exact defect this file was fixed for: a root whose flags are
    /// entirely usage-synopsis-derived (undescribable by construction —
    /// real `git`'s shape before [M-16]'s `-h` fallback recovered any
    /// describable ones) must report `—`, never `0.0%`. The old
    /// `described/total` arithmetic divided 0 described by a nonzero flag
    /// count and printed a confident `0.0% described` — total failure —
    /// for a tool that had nothing describable to fail at, which is
    /// exactly the dishonesty spec §13.1b's denominator redefinition
    /// exists to remove.
    #[test]
    fn reports_an_em_dash_not_zero_percent_when_nothing_is_describable() {
        let mut root = CommandNode::new("git", Provenance::single(Source::HelpText));
        for name in ["paginate", "git-dir", "no-pager"] {
            root.entities.push(Entity::flag_long(
                name,
                Provenance::single(Source::HelpTextSynopsis),
            ));
        }
        let loaded = ExtractionResult {
            tool: "git".to_string(),
            root: Some(root),
            tier_statuses: vec![TierStatus {
                tier: "help_text",
                detected: true,
                error: None,
            }],
            elapsed: std::time::Duration::default(),
        };

        let report = build_report(&loaded);
        assert!(
            report.contains("flags:      3 (— flags with text)"),
            "expected the em-dash line, got:\n{report}"
        );
        assert!(
            !report.contains("0.0%"),
            "must never fall back to the old described/total arithmetic:\n{report}"
        );
    }

    /// The companion case: when there *is* something describable, the
    /// percentage still renders normally (not always `—`).
    #[test]
    fn reports_a_percentage_when_some_flags_are_describable() {
        let mut root = CommandNode::new("sometool", Provenance::single(Source::HelpText));
        let mut f = Entity::flag_long("verbose", Provenance::single(Source::HelpText));
        f.description = Some(mandible_core::Text::sanitize("be more talkative"));
        root.entities.push(f);
        let loaded = ExtractionResult {
            tool: "sometool".to_string(),
            root: Some(root),
            tier_statuses: Vec::new(),
            elapsed: std::time::Duration::default(),
        };

        let report = build_report(&loaded);
        assert!(
            report.contains("flags:      1 (100.0% flags with text)"),
            "expected a real percentage, got:\n{report}"
        );
    }

    /// Modifiers are counted and reported separately from flags — `ar`
    /// recovers six of one and seventeen of the other, and a diagnostic
    /// that says "flags: 6" alone describes a quarter of what the tier
    /// produced. The line appears only when there are any, so every tool
    /// without a modifier table reads exactly as it did before.
    #[test]
    fn modifiers_are_counted_separately_and_only_shown_when_present() {
        let mut root = CommandNode::new("ar", Provenance::single(Source::HelpText));
        let mut f = Entity::flag_long("thin", Provenance::single(Source::HelpText));
        f.description = Some(mandible_core::Text::sanitize("make a thin archive"));
        root.entities.push(f);
        let flags_only = ExtractionResult {
            tool: "ar".to_string(),
            root: Some(root.clone()),
            tier_statuses: Vec::new(),
            elapsed: std::time::Duration::default(),
        };
        assert!(
            !build_report(&flags_only).contains("modifiers:"),
            "a tool with no modifier table must not grow a row for it"
        );

        for letter in ['v', 'S'] {
            root.entities.push(Entity::modifier(
                letter,
                Provenance::single(Source::HelpText),
            ));
        }
        let with_modifiers = ExtractionResult {
            tool: "ar".to_string(),
            root: Some(root),
            tier_statuses: Vec::new(),
            elapsed: std::time::Duration::default(),
        };
        let report = build_report(&with_modifiers);
        assert!(report.contains("modifiers:  2"), "{report}");
        // ...and they did not inflate the flag count or its ratio.
        assert!(
            report.contains("flags:      1 (100.0% flags with text)"),
            "{report}"
        );
    }

    /// Environment variables are counted and reported separately from
    /// flags, the same way modifiers are, and only when there are any.
    #[test]
    fn env_vars_are_counted_separately_and_only_shown_when_present() {
        let mut root = CommandNode::new("node", Provenance::single(Source::HelpText));
        let mut f = Entity::flag_long("version", Provenance::single(Source::HelpText));
        f.description = Some(mandible_core::Text::sanitize("print node's version"));
        root.entities.push(f);
        let flags_only = ExtractionResult {
            tool: "node".to_string(),
            root: Some(root.clone()),
            tier_statuses: Vec::new(),
            elapsed: std::time::Duration::default(),
        };
        assert!(
            !build_report(&flags_only).contains("env vars:"),
            "a tool with no environment section must not grow a row for it"
        );

        for name in ["NODE_DEBUG", "NO_COLOR"] {
            root.entities.push(Entity::env_var_item(
                name,
                Provenance::single(Source::HelpText),
            ));
        }
        let with_env_vars = ExtractionResult {
            tool: "node".to_string(),
            root: Some(root),
            tier_statuses: Vec::new(),
            elapsed: std::time::Duration::default(),
        };
        let report = build_report(&with_env_vars);
        assert!(report.contains("env vars:   2"), "{report}");
        // ...and they did not inflate the flag count or its ratio.
        assert!(
            report.contains("flags:      1 (100.0% flags with text)"),
            "{report}"
        );
    }
}
