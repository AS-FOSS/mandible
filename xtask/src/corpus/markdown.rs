//! Rendering the corpus run as a GitHub-flavored markdown transition report.
use super::*;

/// Cap on how many names a single markdown table cell shows inline before
/// folding the rest into "+N more" — keeps a large-fixture regression
/// scannable. Not load-bearing (informational only): the complete list is
/// never dropped, only moved into a `<details>` block below the table (see
/// [`capped_names`]), matching `path-sweep.yml`'s own summary step
/// (`<details><summary>Per-tool tables</summary>`) for exactly the same
/// reason — GitHub's step-summary UI renders `<details>` natively, so a
/// reviewer who wants the full list expands it without ever leaving the
/// page, and one who doesn't isn't shown a 1,000-line dump by default.
const MARKDOWN_NAME_CAP: usize = 8;

/// Render `names` as a capped, comma-joined inline string, plus (only when
/// truncated) the complete list for a `<details>` block.
fn capped_names(names: &BTreeSet<String>) -> (String, Option<String>) {
    if names.is_empty() {
        return (String::new(), None);
    }
    let all: Vec<&str> = names.iter().map(String::as_str).collect();
    if all.len() <= MARKDOWN_NAME_CAP {
        return (all.join(", "), None);
    }
    let shown = all[..MARKDOWN_NAME_CAP].join(", ");
    let capped = format!("{shown}, +{} more", all.len() - MARKDOWN_NAME_CAP);
    (capped, Some(all.join(", ")))
}

/// Escape the one GFM table-breaking character. Same reasoning and same
/// narrow scope as `coverage::md_escape` — tool/flag/subcommand names are
/// the only free-form content a table cell here ever holds.
fn md_escape(s: &str) -> String {
    s.replace('|', "\\|")
}

/// Render the corpus run as a GitHub-flavored markdown transition report
/// for `$GITHUB_STEP_SUMMARY` — never a text diff of `expected.snap`
/// (a 1,000+ line YAML diff is unreviewable and teaches reviewers to
/// approve unread). Reports what changed semantically instead: status,
/// node/flag counts, and which named subcommands/flags appeared or
/// disappeared.
///
/// Passing fixtures are included too, compactly ("no change") — an
/// all-failures report gives no sense of what the ratchet protects.
///
/// The remedy named for a failing row depends on which check failed: a
/// snapshot mismatch names `--bless`; a `[contract]` violation names a
/// deliberate `meta.toml` edit (the contract may only weaken by an
/// explicit change, never by blessing).
pub(crate) fn render_markdown_report(
    rows: &[FixtureRow],
    total: usize,
    green: usize,
    xfail: usize,
    failed: usize,
    weakened: &[String],
) -> String {
    let mut out = String::new();
    // Contract weakening, ahead of the report's own heading — legal per
    // corpus/README.md but must never be quiet. `> [!WARNING]` renders
    // with its own colored icon on GitHub, unlike a plain bullet.
    if !weakened.is_empty() {
        out.push_str("> [!WARNING]\n");
        for line in weakened {
            out.push_str(&format!("> {}\n", md_escape(line)));
        }
        out.push('\n');
    }
    out.push_str("## Corpus regression report\n\n");
    out.push_str(
        "Every fixture below is replayed through the real tiered extraction pipeline from \
         frozen bytes — zero subprocesses, nothing environment-dependent, no live tool version \
         to drift. A red row is always a real parser change, never runner flakiness \
         (`corpus/README.md`); that is exactly why this check is a hard gate rather than a \
         reported-only sweep like the PATH sweep.\n\n",
    );
    let ok_provenance = provenance_split_label(provenance_counts(
        rows.iter()
            .filter(|r| r.status_word == "ok")
            .map(|r| r.provenance),
    ));
    out.push_str(&format!(
        "**{total} fixture(s):** {green} ok ({ok_provenance}), {xfail} xfail (as expected), \
         {failed} failed.\n\n",
    ));
    out.push_str("| fixture | outcome | status | nodes | flags | scope | provenance | change |\n");
    out.push_str("|---|---|---|---|---|---|---|---|\n");

    let mut details_sections: Vec<String> = Vec::new();

    for row in rows {
        let (status_cell, nodes_cell, flags_cell, mut change_parts) = match &row.previous {
            None => (
                format!("{} (no baseline)", row.current.status),
                row.current.nodes.to_string(),
                row.current.flags.to_string(),
                vec!["no `expected.snap` yet".to_string()],
            ),
            Some(prev) => {
                let status_cell = if prev.status == row.current.status {
                    prev.status.to_string()
                } else {
                    format!("{} → {}", prev.status, row.current.status)
                };
                let nodes_cell = if prev.nodes == row.current.nodes {
                    prev.nodes.to_string()
                } else {
                    format!("{}→{}", prev.nodes, row.current.nodes)
                };
                let flags_cell = if prev.flags == row.current.flags {
                    prev.flags.to_string()
                } else {
                    format!("{}→{}", prev.flags, row.current.flags)
                };

                let mut parts = Vec::new();
                let removed_subs: BTreeSet<String> = prev
                    .subcommands
                    .difference(&row.current.subcommands)
                    .cloned()
                    .collect();
                let added_subs: BTreeSet<String> = row
                    .current
                    .subcommands
                    .difference(&prev.subcommands)
                    .cloned()
                    .collect();
                let removed_flags: BTreeSet<String> = prev
                    .flag_names
                    .difference(&row.current.flag_names)
                    .cloned()
                    .collect();
                let added_flags: BTreeSet<String> = row
                    .current
                    .flag_names
                    .difference(&prev.flag_names)
                    .cloned()
                    .collect();

                for (kind, names) in [
                    ("removed subcommands", &removed_subs),
                    ("added subcommands", &added_subs),
                    ("removed flags", &removed_flags),
                    ("added flags", &added_flags),
                ] {
                    if names.is_empty() {
                        continue;
                    }
                    let count = names.len();
                    let (capped, full) = capped_names(names);
                    parts.push(format!("{kind} ({count}): {capped}"));
                    if let Some(full) = full {
                        details_sections.push(format!(
                            "<details><summary>{} — {count} {kind}</summary>\n\n{full}\n\n</details>",
                            row.label,
                        ));
                    }
                }
                if parts.is_empty() {
                    // A failing fixture whose tracked dimensions all match
                    // still has a real snapshot mismatch somewhere the
                    // summary cannot see: confidence, provenance, usage
                    // lines, a reworded description. Printing "no change"
                    // beside FAIL reads as a bug in the runner rather than
                    // a regression in the parser, and sends the reviewer
                    // hunting in the wrong place. Measured: scaling
                    // `compute_confidence` by 0.8 produced exactly that
                    // row in a real CI run.
                    parts.push(
                        if row.status_word == "FAIL" {
                            "differs outside status/nodes/flags — check confidence, \
                             usage or descriptions in the snapshot diff"
                        } else {
                            "no change"
                        }
                        .to_string(),
                    );
                }
                (status_cell, nodes_cell, flags_cell, parts)
            }
        };

        if row.status_word == "FAIL" {
            let has_snapshot_mismatch = row
                .detail
                .iter()
                .any(|d| d.starts_with("snapshot mismatch") || d.contains("missing expected.snap"));
            let has_contract_failure = row.detail.iter().any(|d| d.starts_with("contract:"));
            if has_snapshot_mismatch {
                change_parts.push(
                    "**fix:** `cargo run -p xtask -- corpus --bless`, then review the \
                     `expected.snap` diff"
                        .to_string(),
                );
            }
            if has_contract_failure {
                change_parts.push(
                    "**fix:** `[contract]` is normative — edit `meta.toml` deliberately \
                     (never `--bless`) and justify the change in the PR"
                        .to_string(),
                );
            }
        }

        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            md_escape(&row.label),
            row.status_word,
            md_escape(&status_cell),
            nodes_cell,
            flags_cell,
            md_escape(&verdict_scope_label(&row.verdict_scope)),
            provenance_label(row.provenance),
            md_escape(&change_parts.join("; ")),
        ));
    }

    if !details_sections.is_empty() {
        out.push('\n');
        for section in &details_sections {
            out.push_str(section);
            out.push('\n');
        }
    }

    out
}
