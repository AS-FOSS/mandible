//! GitHub-flavored markdown rendering of a [`super::Transition`], for
//! `$GITHUB_STEP_SUMMARY` — split into one function per report section, in
//! the order [`render_markdown`] emits them.

use super::diff::Transition;
use super::render_text::capped_join;
use super::EXTRACT_TIMEOUT_MS;

/// Cap on how many rows a table shows before folding the rest behind a
/// count — same reasoning and same order of magnitude as
/// `coverage::WORST_PARSED_LIMIT`: a full-`PATH` sweep runs a couple
/// thousand tools, and a report nobody can scan is a report nobody reads.
const TABLE_ROW_LIMIT: usize = 40;

fn escape_md(s: &str) -> String {
    s.replace('|', "\\|")
}

/// Render [`Transition`] as GitHub-flavored markdown for
/// `$GITHUB_STEP_SUMMARY` — the format this whole module's doc comment
/// insists on over a raw scoreboard-file diff.
pub fn render_markdown(t: &Transition) -> String {
    let mut out = String::new();
    out.push_str(&md_preamble(t));
    out.push_str(&md_status_transitions_section(t));
    out.push_str(&md_flag_losses_section(t));
    out.push_str(&md_flag_gains_section(t));
    out.push_str(&md_field_level_section(t));
    out.push_str(&md_appeared_disappeared_section(t));
    out.push_str(&md_near_cap_section(t));
    out
}

/// The headline (identical/changed, tool counts) plus the truncated/
/// unparseable-row notes — split out of [`render_markdown`] (ratchet:
/// `clippy::too_many_lines`/`clippy::cognitive_complexity`).
fn md_preamble(t: &Transition) -> String {
    let mut out = String::new();
    out.push_str("## Sweep transition report\n\n");
    out.push_str(
        "A semantic per-tool diff between two scoreboards — the check that has actually \
         caught every regression on this branch so far, run by hand until now. **Non-blocking**: \
         this never fails a run (maintainer decision D4); it is a loud report during burn-in, \
         promoted to a gate later.\n\n",
    );
    out.push_str(&format!(
        "**Overall: {}.** This now accounts for field-level content (per-flag \
         description/choices/value_name), not just tool appearances, status and flag counts — a \
         run that only edits a description's text no longer reports as identical.\n\n",
        if t.is_identical() {
            "IDENTICAL"
        } else {
            "CHANGED"
        },
    ));
    out.push_str(&format!(
        "**{before_total} → {after_total} tools.** {matched} matched, {appeared} appeared, \
         {disappeared} disappeared, {near_cap} excluded (near the {cap}s timeout cap).\n\n",
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
            "> [!NOTE]\n> {before_trunc} tool name(s) truncated in the \"before\" scoreboard and \
             {after_trunc} in \"after\" were dropped from this diff entirely — a truncated name \
             (`coverage::truncate_col`'s `…` marker) can collide with a different tool's \
             truncated name, and joining on it would silently corrupt the comparison. See the raw \
             scoreboard files for full names.\n\n",
            before_trunc = t.before.truncated_dropped,
            after_trunc = t.after.truncated_dropped,
        ));
    }
    if t.before.unparseable_dropped > 0 || t.after.unparseable_dropped > 0 {
        out.push_str(&format!(
            "> [!NOTE]\n> {before_bad} row(s) in \"before\" and {after_bad} in \"after\" did not \
             parse as a scoreboard data row and were skipped.\n\n",
            before_bad = t.before.unparseable_dropped,
            after_bad = t.after.unparseable_dropped,
        ));
    }
    out
}

/// The "### Status transitions" section — split out of [`render_markdown`]
/// (ratchet: `clippy::too_many_lines`/`clippy::cognitive_complexity`).
fn md_status_transitions_section(t: &Transition) -> String {
    let mut out = String::new();
    out.push_str("### Status transitions\n\n");
    if t.status_transitions.is_empty() {
        out.push_str("No matched tool (outside the near-cap exclusion) changed status.\n\n");
    } else {
        out.push_str(&format!(
            "**{} tool(s) changed status** (near-cap tools excluded — see below):\n\n",
            t.status_transitions.len()
        ));
        out.push_str("| tool | before | after |\n|---|---|---|\n");
        for row in t.status_transitions.iter().take(TABLE_ROW_LIMIT) {
            out.push_str(&format!(
                "| {} | {} | {} |\n",
                escape_md(row.tool),
                escape_md(row.before),
                escape_md(row.after),
            ));
        }
        if t.status_transitions.len() > TABLE_ROW_LIMIT {
            out.push_str(&format!(
                "\n_{} more not shown._\n",
                t.status_transitions.len() - TABLE_ROW_LIMIT
            ));
        }
        out.push('\n');
    }
    out
}

/// The "### Flag-count losses" section — split out of [`render_markdown`]
/// (ratchet: `clippy::too_many_lines`/`clippy::cognitive_complexity`).
fn md_flag_losses_section(t: &Transition) -> String {
    let mut out = String::new();
    let total_lost: i64 = t.flag_losses.iter().map(|d| -d.delta()).sum();
    out.push_str("### Flag-count losses (the bar — never netted against gains)\n\n");
    if t.flag_losses.is_empty() {
        out.push_str("No matched tool lost flags.\n\n");
    } else {
        out.push_str(&format!(
            "**{total_lost} flag(s) lost across {n} tool(s).** A gain elsewhere never offsets \
             this — see this module's doc comment.\n\n",
            n = t.flag_losses.len(),
        ));
        out.push_str("| tool | before | after | lost |\n|---|---|---|---|\n");
        for d in t.flag_losses.iter().take(TABLE_ROW_LIMIT) {
            out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                escape_md(d.tool),
                d.before,
                d.after,
                -d.delta(),
            ));
        }
        if t.flag_losses.len() > TABLE_ROW_LIMIT {
            out.push_str(&format!(
                "\n_{} more not shown._\n",
                t.flag_losses.len() - TABLE_ROW_LIMIT
            ));
        }
        out.push('\n');
    }
    out
}

/// The "### Flag-count gains" section — split out of [`render_markdown`]
/// (ratchet: `clippy::too_many_lines`/`clippy::cognitive_complexity`).
fn md_flag_gains_section(t: &Transition) -> String {
    let mut out = String::new();
    let total_gained: i64 = t.flag_gains.iter().map(|d| d.delta()).sum();
    out.push_str("### Flag-count gains\n\n");
    if t.flag_gains.is_empty() {
        out.push_str("No matched tool gained flags.\n\n");
    } else {
        out.push_str(&format!(
            "**{total_gained} flag(s) gained across {n} tool(s).**\n\n",
            n = t.flag_gains.len(),
        ));
        out.push_str("| tool | before | after | gained |\n|---|---|---|---|\n");
        for d in t.flag_gains.iter().take(TABLE_ROW_LIMIT) {
            out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                escape_md(d.tool),
                d.before,
                d.after,
                d.delta(),
            ));
        }
        if t.flag_gains.len() > TABLE_ROW_LIMIT {
            out.push_str(&format!(
                "\n_{} more not shown._\n",
                t.flag_gains.len() - TABLE_ROW_LIMIT
            ));
        }
        out.push('\n');
    }
    out
}

/// The "### Field-level changes" section — split out of [`render_markdown`]
/// (ratchet: `clippy::too_many_lines`/`clippy::cognitive_complexity`).
fn md_field_level_section(t: &Transition) -> String {
    let mut out = String::new();
    out.push_str("### Field-level changes\n\n");
    if t.field_diffs.is_empty() {
        out.push_str(
            "No matched tool's flag set, per-flag description/choices/value_name, \
             subcommand set, tier, or framework changed.\n\n",
        );
    } else {
        out.push_str(&format!(
            "**{} tool(s) changed at field granularity** (adds/removes/changes, never just a \
             count — see this module's doc comment):\n\n",
            t.field_diffs.len(),
        ));
        for fd in t.field_diffs.iter().take(TABLE_ROW_LIMIT) {
            out.push_str(&format!("- **{}**", escape_md(fd.tool)));
            let mut parts = Vec::new();
            if !fd.flags_added.is_empty() {
                parts.push(format!("flags added: {}", capped_join(&fd.flags_added)));
            }
            if !fd.flags_removed.is_empty() {
                parts.push(format!("flags removed: {}", capped_join(&fd.flags_removed)));
            }
            if !fd.description_changed.is_empty() {
                parts.push(format!(
                    "description changed: {}",
                    capped_join(&fd.description_changed)
                ));
            }
            if !fd.choices_changed.is_empty() {
                parts.push(format!(
                    "choices changed: {}",
                    capped_join(&fd.choices_changed)
                ));
            }
            if !fd.value_name_changed.is_empty() {
                parts.push(format!(
                    "value_name changed: {}",
                    capped_join(&fd.value_name_changed)
                ));
            }
            if !fd.subcommands_added.is_empty() {
                parts.push(format!(
                    "subcommands added: {}",
                    capped_join(&fd.subcommands_added)
                ));
            }
            if !fd.subcommands_removed.is_empty() {
                parts.push(format!(
                    "subcommands removed: {}",
                    capped_join(&fd.subcommands_removed)
                ));
            }
            if let Some((b, a)) = fd.tier_changed {
                parts.push(format!("tier: {} -> {}", escape_md(b), escape_md(a)));
            }
            if let Some((b, a)) = fd.framework_changed {
                parts.push(format!("framework: {} -> {}", escape_md(b), escape_md(a)));
            }
            out.push_str(&format!(" — {}\n", parts.join("; ")));
        }
        if t.field_diffs.len() > TABLE_ROW_LIMIT {
            out.push_str(&format!(
                "\n_{} more not shown._\n",
                t.field_diffs.len() - TABLE_ROW_LIMIT
            ));
        }
        out.push('\n');
    }
    if t.field_diff_unmeasured > 0 {
        out.push_str(&format!(
            "> [!NOTE]\n> {} matched tool(s) could not be compared at field granularity — \
             neither scoreboard carries a `#fp` fingerprint entry for them, meaning this pair \
             predates the fingerprint footer entirely (a scoreboard that does carry it emits an \
             entry for every tool, including ones with no flags and no subcommands). Not counted \
             as \"no field-level changes.\"\n\n",
            t.field_diff_unmeasured,
        ));
    }
    out
}

/// The "### Appeared / disappeared" section — split out of
/// [`render_markdown`] (ratchet: `clippy::too_many_lines`/
/// `clippy::cognitive_complexity`).
fn md_appeared_disappeared_section(t: &Transition) -> String {
    let mut out = String::new();
    if !t.appeared.is_empty() || !t.disappeared.is_empty() {
        out.push_str("### Appeared / disappeared\n\n");
        if !t.appeared.is_empty() {
            out.push_str(&format!(
                "**Appeared ({}):** {}\n\n",
                t.appeared.len(),
                escape_md(&capped_join(&t.appeared)),
            ));
        }
        if !t.disappeared.is_empty() {
            out.push_str(&format!(
                "**Disappeared ({}):** {}\n\n",
                t.disappeared.len(),
                escape_md(&capped_join(&t.disappeared)),
            ));
        }
    }
    out
}

/// The "### Excluded — near the timeout cap" section — split out of
/// [`render_markdown`] (ratchet: `clippy::too_many_lines`/
/// `clippy::cognitive_complexity`).
fn md_near_cap_section(t: &Transition) -> String {
    let mut out = String::new();
    if !t.near_cap.is_empty() {
        out.push_str(&format!(
            "### Excluded — near the {}s timeout cap\n\n",
            EXTRACT_TIMEOUT_MS / 1000
        ));
        out.push_str(
            "Elapsed time on at least one side was at or past half the single-probe extract \
             cap (spec §13.1b rule 3) — a status or flag count here may reflect machine load, \
             not a parser change. Reported for visibility, excluded from every number above. \
             See this module's doc comment (`near_timeout_cap`) for why the bound is one-sided.\n\n",
        );
        out.push_str("| tool | before status | before ms | after status | after ms |\n|---|---|---|---|---|\n");
        for tool in t.near_cap.iter().take(TABLE_ROW_LIMIT) {
            let b = t.before.rows.get(*tool);
            let a = t.after.rows.get(*tool);
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                escape_md(tool),
                b.map(|r| r.status.as_str()).unwrap_or("—"),
                b.map(|r| r.ms.to_string())
                    .unwrap_or_else(|| "—".to_string()),
                a.map(|r| r.status.as_str()).unwrap_or("—"),
                a.map(|r| r.ms.to_string())
                    .unwrap_or_else(|| "—".to_string()),
            ));
        }
        if t.near_cap.len() > TABLE_ROW_LIMIT {
            out.push_str(&format!(
                "\n_{} more not shown._\n",
                t.near_cap.len() - TABLE_ROW_LIMIT
            ));
        }
        out.push('\n');
    }
    out
}
