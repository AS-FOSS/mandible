//! Plain-text rendering of a [`super::Transition`], for a terminal or a
//! plain log — same content and section order as
//! [`super::render_markdown`], no GFM syntax.

use super::diff::Transition;
use super::EXTRACT_TIMEOUT_MS;

/// Cap on inline names before folding into a count-only summary — mirrors
/// `corpus::MARKDOWN_NAME_CAP`'s reasoning at the same scale for a tool
/// list this size.
const NAME_CAP: usize = 15;

pub(super) fn capped_join(names: &[&str]) -> String {
    if names.len() <= NAME_CAP {
        names.join(", ")
    } else {
        format!(
            "{}, +{} more",
            names[..NAME_CAP].join(", "),
            names.len() - NAME_CAP
        )
    }
}

/// Plain-text rendering of [`Transition`], for a terminal or a plain log —
/// same content as [`render_markdown`], no GFM syntax. Mirrors
/// `coverage::render_text`/`render_markdown`'s own dual-format convention.
pub fn render_text(t: &Transition) -> String {
    let mut out = String::new();
    out.push_str(&text_preamble(t));
    out.push_str(&text_status_transitions_section(t));
    out.push_str(&text_flag_losses_section(t));
    out.push_str(&text_flag_gains_section(t));
    out.push_str(&text_field_level_section(t));
    out.push_str(&text_appeared_disappeared_section(t));
    out.push_str(&text_near_cap_section(t));
    out
}

/// The headline plus the truncated/unparseable-row notes — split out of
/// [`render_text`] (ratchet: `clippy::too_many_lines`).
fn text_preamble(t: &Transition) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "overall: {}\n",
        if t.is_identical() {
            "IDENTICAL"
        } else {
            "CHANGED"
        },
    ));
    out.push_str(&format!(
        "sweep transition: {before_total} -> {after_total} tools, {matched} matched, {appeared} appeared, {disappeared} disappeared, {near_cap} excluded (near {cap}s cap)\n",
        before_total = t.before.rows.len(),
        after_total = t.after.rows.len(),
        matched = t.after.rows.len() - t.appeared.len(),
        appeared = t.appeared.len(),
        disappeared = t.disappeared.len(),
        near_cap = t.near_cap.len(),
        cap = EXTRACT_TIMEOUT_MS / 1000,
    ));
    if t.before.truncated_dropped > 0 || t.after.truncated_dropped > 0 {
        out.push_str(&format!(
            "# dropped (truncated tool name): {} before, {} after — never joined, see doc comment\n",
            t.before.truncated_dropped, t.after.truncated_dropped
        ));
    }
    if t.before.unparseable_dropped > 0 || t.after.unparseable_dropped > 0 {
        out.push_str(&format!(
            "# dropped (unparseable row): {} before, {} after\n",
            t.before.unparseable_dropped, t.after.unparseable_dropped
        ));
    }
    out.push('\n');
    out
}

/// The `# status transitions` section — split out of [`render_text`]
/// (ratchet: `clippy::too_many_lines`).
fn text_status_transitions_section(t: &Transition) -> String {
    let mut out = String::new();
    out.push_str("# status transitions\n");
    if t.status_transitions.is_empty() {
        out.push_str("(none)\n");
    } else {
        for row in &t.status_transitions {
            out.push_str(&format!(
                "  {}: {} -> {}\n",
                row.tool, row.before, row.after
            ));
        }
    }
    out.push('\n');
    out
}

/// The `# flag-count losses` section — split out of [`render_text`]
/// (ratchet: `clippy::too_many_lines`).
fn text_flag_losses_section(t: &Transition) -> String {
    let mut out = String::new();
    let total_lost: i64 = t.flag_losses.iter().map(|d| -d.delta()).sum();
    out.push_str(&format!(
        "# flag-count losses (the bar — never netted): {total_lost} lost across {} tool(s)\n",
        t.flag_losses.len()
    ));
    for d in &t.flag_losses {
        out.push_str(&format!(
            "  {}: {} -> {} ({})\n",
            d.tool,
            d.before,
            d.after,
            d.delta()
        ));
    }
    out.push('\n');
    out
}

/// The `# flag-count gains` section — split out of [`render_text`]
/// (ratchet: `clippy::too_many_lines`).
fn text_flag_gains_section(t: &Transition) -> String {
    let mut out = String::new();
    let total_gained: i64 = t.flag_gains.iter().map(|d| d.delta()).sum();
    out.push_str(&format!(
        "# flag-count gains: {total_gained} gained across {} tool(s)\n",
        t.flag_gains.len()
    ));
    for d in &t.flag_gains {
        out.push_str(&format!(
            "  {}: {} -> {} (+{})\n",
            d.tool,
            d.before,
            d.after,
            d.delta()
        ));
    }
    out.push('\n');
    out
}

/// The `# field-level changes` section — split out of [`render_text`]
/// (ratchet: `clippy::too_many_lines`).
fn text_field_level_section(t: &Transition) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# field-level changes: {} tool(s) (adds/removes/changes, never just a count)\n",
        t.field_diffs.len()
    ));
    for fd in &t.field_diffs {
        let mut parts = Vec::new();
        if !fd.flags_added.is_empty() {
            parts.push(format!("flags added: {}", fd.flags_added.join(", ")));
        }
        if !fd.flags_removed.is_empty() {
            parts.push(format!("flags removed: {}", fd.flags_removed.join(", ")));
        }
        if !fd.description_changed.is_empty() {
            parts.push(format!(
                "description changed: {}",
                fd.description_changed.join(", ")
            ));
        }
        if !fd.choices_changed.is_empty() {
            parts.push(format!(
                "choices changed: {}",
                fd.choices_changed.join(", ")
            ));
        }
        if !fd.value_name_changed.is_empty() {
            parts.push(format!(
                "value_name changed: {}",
                fd.value_name_changed.join(", ")
            ));
        }
        if !fd.subcommands_added.is_empty() {
            parts.push(format!(
                "subcommands added: {}",
                fd.subcommands_added.join(", ")
            ));
        }
        if !fd.subcommands_removed.is_empty() {
            parts.push(format!(
                "subcommands removed: {}",
                fd.subcommands_removed.join(", ")
            ));
        }
        if let Some((b, a)) = fd.tier_changed {
            parts.push(format!("tier: {b} -> {a}"));
        }
        if let Some((b, a)) = fd.framework_changed {
            parts.push(format!("framework: {b} -> {a}"));
        }
        out.push_str(&format!("  {}: {}\n", fd.tool, parts.join("; ")));
    }
    if t.field_diff_unmeasured > 0 {
        out.push_str(&format!(
            "# field-level comparison unavailable for {} matched tool(s) — neither scoreboard carries a #fp entry for them (this pair predates the fingerprint footer entirely)\n",
            t.field_diff_unmeasured
        ));
    }
    out.push('\n');
    out
}

/// The `# appeared`/`# disappeared` lines — split out of [`render_text`]
/// (ratchet: `clippy::too_many_lines`).
fn text_appeared_disappeared_section(t: &Transition) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# appeared ({}): {}\n",
        t.appeared.len(),
        t.appeared.join(", ")
    ));
    out.push_str(&format!(
        "# disappeared ({}): {}\n",
        t.disappeared.len(),
        t.disappeared.join(", ")
    ));
    out.push('\n');
    out
}

/// The `# excluded, near ... timeout cap` section — split out of
/// [`render_text`] (ratchet: `clippy::too_many_lines`).
fn text_near_cap_section(t: &Transition) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# excluded, near {}s timeout cap ({})\n",
        EXTRACT_TIMEOUT_MS / 1000,
        t.near_cap.len()
    ));
    for tool in &t.near_cap {
        let b = t.before.rows.get(*tool);
        let a = t.after.rows.get(*tool);
        out.push_str(&format!(
            "  {}: {}@{}ms -> {}@{}ms\n",
            tool,
            b.map(|r| r.status.as_str()).unwrap_or("—"),
            b.map(|r| r.ms).unwrap_or(0),
            a.map(|r| r.status.as_str()).unwrap_or("—"),
            a.map(|r| r.ms).unwrap_or(0),
        ));
    }

    out
}
