//! GitHub-flavored markdown scoreboard rendering, for
//! `$GITHUB_STEP_SUMMARY` (spec §13.1a, batch 6 part 6) — the markdown
//! twin of [`super::render_text`]'s plain-text table, sharing its
//! [`worst_parsed`]/[`undescribed_flags`] and per-detector sample caps.

use super::aggregate::{aggregate_footer_line, detection_rate_pct};
use super::render_text::{
    undescribed_flags, worst_parsed, ALTERNATION_SAMPLE_LIMIT, BUNDLE_SAMPLE_LIMIT,
    EXISTENCE_SAMPLE_LIMIT, MISATTRIBUTION_SAMPLE_LIMIT, SPLIT_SAMPLE_LIMIT,
};
use super::{Aggregate, Row};

/// Markdown rendering of [`worst_parsed`]'s result, for
/// [`render_markdown`].
fn worst_parsed_section_markdown(worst: &[&Row]) -> String {
    if worst.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n**Worst-parsed tools** (most missing flag descriptions, which is where grammar work pays off):\n\n| tool | undescribed | flags | %flags_text | framework |\n|---|---|---|---|---|\n",
    );
    for row in worst {
        let pct = row
            .pct_flags_with_text
            .map(|p| format!("{p:.0}%"))
            .unwrap_or_else(|| "-".to_string());
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            md_escape(&row.tool),
            undescribed_flags(row),
            row.flags,
            pct,
            md_escape(&row.framework),
        ));
    }
    out
}

/// Markdown rendering of [`alternation_sample_lines_text`]'s result, for
/// [`render_markdown`].
fn alternation_sample_section_markdown(rows: &[Row]) -> String {
    let samples: Vec<&String> = rows
        .iter()
        .flat_map(|r| r.alternation_samples.iter())
        .take(ALTERNATION_SAMPLE_LIMIT)
        .collect();
    if samples.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n**Brace-alternation-flag defects** (sample, reported and NOT gated — see `xtask/src/main.rs`):\n\n| sample |\n|---|\n",
    );
    for sample in samples {
        out.push_str(&format!("| {} |\n", md_escape(sample)));
    }
    out
}

/// Markdown twin of [`single_dash_sample_lines_text`], for
/// [`render_markdown`].
fn single_dash_sample_section_markdown(rows: &[Row]) -> String {
    sample_section_markdown(
        rows.iter().flat_map(|r| r.single_dash_samples.iter()),
        "\n**Single-dash-long splits** (sample — see \
         `xtask/src/single_dash_long.rs`):\n\n| sample |\n|---|\n",
    )
}

/// Markdown twin of [`repeated_char_sample_lines_text`].
fn repeated_char_sample_section_markdown(rows: &[Row]) -> String {
    sample_section_markdown(
        rows.iter().flat_map(|r| r.repeated_char_samples.iter()),
        "\n**Repeated-char-flag misreads** (sample — see \
         `xtask/src/repeated_char.rs`):\n\n| sample |\n|---|\n",
    )
}

/// Markdown twin of [`wrapped_prose_sample_lines_text`].
fn wrapped_prose_sample_section_markdown(rows: &[Row]) -> String {
    sample_section_markdown(
        rows.iter().flat_map(|r| r.wrapped_prose_samples.iter()),
        "\n**Wrapped-prose-row-boundary fabrications** (sample — see \
         `xtask/src/wrapped_prose.rs`):\n\n| sample |\n|---|\n",
    )
}

/// Markdown twin of [`tail_operand_sample_lines_text`].
fn tail_operand_sample_section_markdown(rows: &[Row]) -> String {
    sample_section_markdown(
        rows.iter().flat_map(|r| r.tail_operand_samples.iter()),
        "\n**Unparsed-tail-operand findings** (sample — see \
         `xtask/src/tail_operand.rs`):\n\n| sample |\n|---|\n",
    )
}

/// Markdown twin of [`ragged_command_sample_lines_text`].
fn ragged_command_sample_section_markdown(rows: &[Row]) -> String {
    sample_section_markdown(
        rows.iter().flat_map(|r| r.ragged_command_samples.iter()),
        "\n**Ragged-command-table findings** (sample — see \
         `xtask/src/ragged_command_table.rs`):\n\n| sample |\n|---|\n",
    )
}

/// Markdown twin of [`wrapped_command_sample_lines_text`].
fn wrapped_command_sample_section_markdown(rows: &[Row]) -> String {
    sample_section_markdown(
        rows.iter().flat_map(|r| r.wrapped_command_samples.iter()),
        "\n**Wrapped-command-continuation-as-subcommand findings** (sample — see \
         `xtask/src/wrapped_command_continuation.rs`):\n\n| sample |\n|---|\n",
    )
}

/// The shared body of the two markdown sections above.
fn sample_section_markdown<'a>(samples: impl Iterator<Item = &'a String>, heading: &str) -> String {
    let samples: Vec<&String> = samples.take(SPLIT_SAMPLE_LIMIT).collect();
    if samples.is_empty() {
        return String::new();
    }
    let mut out = String::from(heading);
    for sample in samples {
        out.push_str(&format!("| {} |\n", md_escape(sample)));
    }
    out
}

/// Markdown rendering of [`bundle_sample_lines_text`]'s result, for
/// [`render_markdown`].
fn bundle_sample_section_markdown(rows: &[Row]) -> String {
    let samples: Vec<&String> = rows
        .iter()
        .flat_map(|r| r.bundle_samples.iter())
        .take(BUNDLE_SAMPLE_LIMIT)
        .collect();
    if samples.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n**Bundled-short-flag collapses** (sample, not gated — see `xtask/src/bundling.rs`):\n\n| sample |\n|---|\n",
    );
    for sample in samples {
        out.push_str(&format!("| {} |\n", md_escape(sample)));
    }
    out
}

/// Markdown rendering of [`existence_sample_lines_text`]'s result, for
/// [`render_markdown`].
fn existence_sample_section_markdown(rows: &[Row]) -> String {
    let samples: Vec<&String> = rows
        .iter()
        .flat_map(|r| r.existence_samples.iter())
        .take(EXISTENCE_SAMPLE_LIMIT)
        .collect();
    if samples.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n**Existence fabrications** (sample, not gated — see `xtask/src/existence.rs`):\n\n| sample |\n|---|\n",
    );
    for sample in samples {
        out.push_str(&format!("| {} |\n", md_escape(sample)));
    }
    out
}

/// Markdown rendering of [`misattribution_sample_lines_text`]'s result, for
/// [`render_markdown`].
fn misattribution_sample_section_markdown(rows: &[Row]) -> String {
    let samples: Vec<&String> = rows
        .iter()
        .flat_map(|r| r.misattribution_samples.iter())
        .take(MISATTRIBUTION_SAMPLE_LIMIT)
        .collect();
    if samples.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n**Misattribution suspects** (sample, not gated — see `xtask/src/misattribution.rs`):\n\n| sample |\n|---|\n",
    );
    for sample in samples {
        out.push_str(&format!("| {} |\n", md_escape(sample)));
    }
    out
}

/// GitHub-flavored markdown table plus the same aggregate footer,
/// rendered as prose — spec §13.1a's framework-support workflow (batch 6
/// part 6) writes this straight to `$GITHUB_STEP_SUMMARY`, which GitHub
/// renders as markdown in the run's summary UI.
pub(super) fn render_markdown(rows: &[Row], aggregate: &Aggregate) -> String {
    let mut out = String::new();
    out.push_str(
        "| tool | tier(s) | framework | nodes | flags | %flags_text | ms | suspect | man | misattr | exist | bundle | status |\n",
    );
    out.push_str("|---|---|---|---|---|---|---|---|---|---|---|---|---|\n");
    for row in rows {
        let pct = row
            .pct_flags_with_text
            .map(|p| format!("{p:.0}%"))
            .unwrap_or_else(|| "—".to_string());
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            md_escape(&row.tool),
            md_escape(&row.tiers),
            md_escape(&row.framework),
            row.nodes,
            row.flags,
            pct,
            row.ms,
            row.suspicious_nodes,
            if row.man_shaped { "yes" } else { "-" },
            row.misattribution_suspect_count,
            row.existence_fabrication_count,
            row.bundle_collapse_count,
            row.status,
        ));
    }
    out.push('\n');
    out.push_str(&format!(
        "**Aggregate:** {:.2}% of flags carry text across {} tools, {} no-tier, {} suspicious, {} verbatim, {} man-shaped.\n\n",
        aggregate.pct_flags_with_text,
        aggregate.total,
        aggregate.no_tier_count,
        aggregate.suspicious_count,
        aggregate.verbatim_count,
        aggregate.man_shaped_count,
    ));
    // See `accuracy_unmeasured_line`'s doc comment (the text format's
    // twin): `%flags_text` is presence, never correctness, and nothing yet
    // measures the latter fleet-wide.
    out.push_str("**Accuracy:** unmeasured.\n\n");
    out.push_str(&format!(
        "**Misattribution suspects:** {} tool(s) with at least one flag description containing \
         a flag-shaped token attested at a column-aligned position elsewhere in that tool's own \
         raw help text — not gated, see `xtask/src/misattribution.rs`.\n\n",
        aggregate.misattribution_suspect_tools,
    ));
    out.push_str(&format!(
        "**Existence fabrications:** {} tool(s) with at least one help-text-sourced subcommand \
         name or flag spelling not found literally in that tool's own raw help text — [M-10]'s \
         shape, generalized; not gated, see `xtask/src/existence.rs`.\n\n",
        aggregate.existence_fabrication_tools,
    ));
    out.push_str(&format!(
        "**Bundled-short-flag collapses:** {} tool(s) whose synopsis flag cluster was read as one \
         value-taking flag, destroying {} real flag(s) fleet-wide — not gated, see \
         `xtask/src/bundling.rs`.\n\n",
        aggregate.bundle_collapse_tools, aggregate.bundle_destroyed_flags,
    ));
    out.push_str(&format!(
        "**Brace-alternation-flag defects:** {} tool(s) whose delimited flag alternation \
         (`{{-i|--input}}`, `[[-c|-C] cmd]`) lost or mangled {} flag spelling(s) fleet-wide — \
         reported and NOT gated, see `xtask/src/main.rs` for the named residual.\n\n",
        aggregate.alternation_defect_tools, aggregate.alternation_defect_flags,
    ));
    out.push_str(&format!(
        "**Single-dash-long splits:** {} tool(s) whose option table names a single-dash long \
         option that was read as a one-character short flag plus a required value, losing {} \
         real flag(s) fleet-wide — see `xtask/src/single_dash_long.rs`.\n\n",
        aggregate.single_dash_split_tools, aggregate.single_dash_split_flags,
    ));
    out.push_str(&format!(
        "**Repeated-char-flag misreads:** {} tool(s) whose `-vv`-shaped flag was read as `-v` \
         carrying its own letter as a value, losing {} real flag(s) fleet-wide — see \
         `xtask/src/repeated_char.rs`.\n\n",
        aggregate.repeated_char_tools, aggregate.repeated_char_flags,
    ));
    out.push_str(&format!(
        "**Wrapped-prose-row-boundary fabrications:** {} tool(s) whose own leading spelling was \
         fabricated into a flag when a description wrapped onto a dash-led continuation line \
         (atlas S-027), {} line(s) fleet-wide — not calibratable against a reviewed audit \
         verdict yet, see `xtask/src/wrapped_prose.rs`.\n\n",
        aggregate.wrapped_prose_tools, aggregate.wrapped_prose_flags,
    ));
    out.push_str(&format!(
        "**Unparsed-tail-operand findings:** {} tool(s) whose usage line's own trailing operand \
         token never became a positional (atlas S-041), {} operand(s) fleet-wide — see \
         `xtask/src/tail_operand.rs`.\n\n",
        aggregate.tail_operand_tools, aggregate.tail_operand_flags,
    ));
    out.push_str(&format!(
        "**Ragged-command-table findings:** {} tool(s) whose ragged-indent command-table row \
         never reached the tree (atlas S-104, `unparsed-subcommand` shape E), {} command(s) \
         fleet-wide — see `xtask/src/ragged_command_table.rs`.\n\n",
        aggregate.ragged_command_tools, aggregate.ragged_command_flags,
    ));
    out.push_str(&format!(
        "**Wrapped-command-continuation-as-subcommand findings:** {} tool(s) with a fabricated \
         subcommand from a bare continuation line (atlas S-103), {} command(s) fleet-wide — see \
         `xtask/src/wrapped_command_continuation.rs`.\n\n",
        aggregate.wrapped_command_tools, aggregate.wrapped_command_flags,
    ));
    out.push_str(&format!(
        "**Framework detection:** {}/{} tools ({:.1}%).\n",
        aggregate.framework_detected_count,
        aggregate.total,
        detection_rate_pct(aggregate),
    ));
    if !aggregate.framework_counts.is_empty() {
        out.push_str("\n**Per-framework counts:**\n\n");
        for (name, count) in &aggregate.framework_counts {
            out.push_str(&format!("- {}: {count}\n", md_escape(name)));
        }
    }
    out.push_str(&worst_parsed_section_markdown(&worst_parsed(rows)));
    out.push_str(&misattribution_sample_section_markdown(rows));
    out.push_str(&existence_sample_section_markdown(rows));
    out.push_str(&bundle_sample_section_markdown(rows));
    out.push_str(&alternation_sample_section_markdown(rows));
    out.push_str(&single_dash_sample_section_markdown(rows));
    out.push_str(&repeated_char_sample_section_markdown(rows));
    out.push_str(&wrapped_prose_sample_section_markdown(rows));
    out.push_str(&tail_operand_sample_section_markdown(rows));
    out.push_str(&ragged_command_sample_section_markdown(rows));
    out.push_str(&wrapped_command_sample_section_markdown(rows));
    // The same machine-readable footer the text format carries, wrapped in
    // an HTML comment so it stays invisible when rendered but parseable by
    // whatever recombines shards. Without it a sharded markdown run could
    // only be merged by re-deriving totals from the rounded per-row
    // %flags_text column, which is exactly the approximation
    // `described_flags`/`total_flags` exist to avoid.
    out.push_str("\n<!-- ");
    out.push_str(aggregate_footer_line(aggregate).trim_end());
    out.push_str(" -->\n");
    out
}

/// Escape the one character (`|`) that would otherwise break a GFM table
/// cell. Tool names and framework labels are the only free-form content
/// here; a `|` in either is exotic but not impossible on a real `PATH`.
fn md_escape(s: &str) -> String {
    s.replace('|', "\\|")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coverage::aggregate::compute_aggregate;
    use crate::coverage::row;

    #[test]
    fn markdown_format_produces_a_gfm_table_and_footer() {
        let rows = vec![row("git", 10, Some(90.0), "ok")];
        let agg = compute_aggregate(&rows);
        let md = render_markdown(&rows, &agg);
        assert!(md.starts_with("| tool |"));
        assert!(md.contains("|---|"));
        assert!(md.contains("| git |"));
        assert!(md.contains("**Aggregate:**"));
        assert!(md.contains("**Framework detection:**"));
    }

    #[test]
    fn render_markdown_includes_the_worst_parsed_audit_section() {
        let rows = vec![row("half-parsed", 42, Some(50.0), "ok")];
        let agg = compute_aggregate(&rows);
        let md = render_markdown(&rows, &agg);
        assert!(md.contains("**Worst-parsed tools**"));
        assert!(md.contains("| half-parsed |"));
    }
}
