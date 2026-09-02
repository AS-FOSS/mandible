//! The two commands with a human at the keyboard: `xtask audit spot-audit`
//! (targeted promotion into the sample) and `xtask audit review` (the
//! interactive raw-text-vs-tree loop).

use super::{classify_one, entry_from_classified};
use mandible_core::audit::{
    extract_tag_override, load, parse_verdict_word, save, tag_display, verdict_path, AuditFile,
    AuditMeta,
};
use mandible_core::CommandNode;
use std::collections::HashSet;
use std::io::{BufRead, Write};
use std::path::Path;

/// `xtask audit spot-audit` (spec §13.1b's sixth rule): draws `--sample`
/// tools at random from `promoted` (the tools one mass-`ok` promotion
/// event changed), classifies each with one fresh extraction pass
/// ([`classify_one`]), and merges them into `<dir>/<seed>.toml` as a
/// `spot-audit:<event>` stratum ([`effective_stratum`]).
///
/// The draw is reproducible: `draw_seed` mixed with `event` via
/// [`crate::rng::stratum_seed`] seeds a Fisher-Yates shuffle
/// ([`crate::rng::seeded_shuffle`]) over `promoted`.
///
/// When `promoted.len() < sample`, every promoted tool is drawn and the
/// shortfall is stated in the summary, never silently padded.
///
/// A tool already present in the verdict file is tagged with
/// [`Entry::spot_audit_event`], not duplicated or reclassified — its
/// existing verdict is left for a human to re-review via `xtask audit
/// amend`, never overwritten as a side effect of a draw. Re-running with
/// the same inputs is safe: an already-tagged entry is left alone.
pub fn cmd_spot_audit(
    dir: &Path,
    seed: u64,
    event: &str,
    promoted: &[String],
    sample: usize,
    draw_seed: u64,
) -> anyhow::Result<()> {
    if promoted.is_empty() {
        anyhow::bail!("--promoted named no tools — nothing to spot-audit for event {event:?}");
    }

    let mut pool = promoted.to_vec();
    crate::rng::seeded_shuffle(&mut pool, crate::rng::stratum_seed(draw_seed, event));
    let take_n = sample.min(pool.len());
    let drawn: Vec<String> = pool.into_iter().take(take_n).collect();

    let path = verdict_path(dir, seed);
    let mut file = if path.is_file() {
        load(&path)?
    } else {
        AuditFile {
            meta: AuditMeta {
                seed,
                sample_size: 0,
            },
            entries: Vec::new(),
        }
    };

    let reason = if promoted.len() < sample {
        format!(
            "spot-audit of promotion event {event:?}: {} of {} promoted tool(s) drawn (seed \
             {draw_seed}) — every promoted tool was audited because the promoted set was \
             smaller than the requested sample size ({sample})",
            drawn.len(),
            promoted.len(),
        )
    } else {
        format!(
            "spot-audit of promotion event {event:?}: {} of {} promoted tool(s) drawn at random \
             (seed {draw_seed})",
            drawn.len(),
            promoted.len(),
        )
    };

    let existing_tools: HashSet<String> = file.entries.iter().map(|e| e.tool.clone()).collect();
    let mut added = 0usize;
    let mut tagged_existing = 0usize;
    for tool in &drawn {
        if existing_tools.contains(tool) {
            if let Some(existing) = file.entries.iter_mut().find(|e| &e.tool == tool) {
                if existing.spot_audit_event.is_none() {
                    existing.spot_audit_event = Some(event.to_string());
                    if existing.include_reason.is_none() {
                        existing.include_reason = Some(reason.clone());
                    }
                    tagged_existing += 1;
                }
            }
            continue;
        }
        let classified = classify_one(tool);
        let mut entry = entry_from_classified(tool.clone(), &classified, Some(reason.clone()));
        entry.spot_audit_event = Some(event.to_string());
        file.entries.push(entry);
        added += 1;
    }
    file.entries.sort_by(|a, b| a.tool.cmp(&b.tool));
    save(&path, &file)?;

    println!(
        "spot-audit:{event}: drew {} of {} promoted tool(s)",
        drawn.len(),
        promoted.len(),
    );
    if promoted.len() < sample {
        println!(
            "note: the promoted set has only {} tool(s), fewer than the requested sample size \
             {sample} — every promoted tool was audited rather than silently sampling fewer or \
             padding the count to look like a full draw.",
            promoted.len(),
        );
    }
    println!(
        "{added} new pending entr{s} written, {tagged_existing} already-present entr{s2} tagged \
         into this stratum, at {} ({} tool(s) now in stratum spot-audit:{event})",
        path.display(),
        drawn.len(),
        s = if added == 1 { "y" } else { "ies" },
        s2 = if tagged_existing == 1 { "y" } else { "ies" },
    );
    if tagged_existing > 0 {
        println!(
            "note: {tagged_existing} of those were already in the file with a prior verdict — \
             that verdict is left exactly as recorded (it may now be stale against a changed \
             parse) for a human to re-review and correct via `xtask audit amend`, never \
             overwritten by this draw."
        );
    }
    Ok(())
}

/// Render `node` as the same YAML snapshot shown side by side with a tool's
/// raw `--help` text in [`cmd_review`]/[`cmd_emit`] — shared so the two
/// entry points can never render a tree differently.
pub(crate) fn render_snapshot(node: Option<&CommandNode>) -> String {
    match node {
        Some(node) => {
            let snapshot = mandible_core::to_snapshot(node);
            serde_yaml::to_string(&snapshot)
                .unwrap_or_else(|e| format!("(snapshot serialization failed: {e})\n"))
        }
        None => "(no root produced by any tier)\n".to_string(),
    }
}

/// `xtask audit review`: the interactive loop. Presents raw `--help` text
/// and the parsed tree for every pending entry, reads a verdict line
/// (`<word> [note...]`) from `input`, and persists the file after every
/// entry, so an interrupted session resumes where it stopped.
///
/// Line-buffered (`<word><Enter>`), not raw single-keystroke: this
/// environment has no tty (AGENTS.md §3.2), and line buffering works
/// identically over a real terminal or a `Cursor` in tests.
pub fn cmd_review(
    dir: &Path,
    seed: u64,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> anyhow::Result<()> {
    let path = verdict_path(dir, seed);
    let mut file = load(&path)?;
    let pending: Vec<usize> = file.pending().collect();
    if pending.is_empty() {
        writeln!(output, "nothing pending in {}", path.display())?;
        return Ok(());
    }
    writeln!(
        output,
        "{} pending of {} total. Verdict: c(orrect) / i(ncomplete) / w(rong) / s(kip), \
         optionally followed by a space and a note. Add `k1=true`/`k1=false`/`k2=true`/\
         `k2=false`/`k3=true`/`k3=false` anywhere in the note to override a pre-tag; omitting \
         it confirms the suggestion shown below. Blank line or end of input stops \
         (already-recorded verdicts are saved after every tool).",
        pending.len(),
        file.entries.len()
    )?;

    for idx in pending {
        let tool = file.entries[idx].tool.clone();
        let stratum = file.entries[idx].stratum.clone();
        let k1 = file.entries[idx].k1;
        let k2 = file.entries[idx].k2;
        let k3 = file.entries[idx].k3;
        let include_reason = file.entries[idx].include_reason.clone();
        let classified = classify_one(&tool);
        writeln!(output, "\n=== {tool}  (stratum: {stratum}) ===")?;
        if let Some(reason) = &include_reason {
            writeln!(output, "forced inclusion: {reason}")?;
        }
        writeln!(
            output,
            "{}",
            tag_display("K1 (single-dash-long defect)", k1, "k1")
        )?;
        writeln!(
            output,
            "{}",
            tag_display("K2 (existence-detector tokenizer gap)", k2, "k2")
        )?;
        writeln!(
            output,
            "{}",
            tag_display("K3 (subcommand help never fetched)", k3, "k3")
        )?;
        writeln!(output, "--- raw --help ---")?;
        writeln!(
            output,
            "{}",
            classified
                .raw_text
                .as_deref()
                .unwrap_or("(no output captured)")
        )?;
        writeln!(output, "--- parsed tree ---")?;
        writeln!(
            output,
            "{}",
            render_snapshot(classified.result.root.as_ref())
        )?;
        write!(output, "verdict> ")?;
        output.flush()?;

        let mut line = String::new();
        let bytes_read = input.read_line(&mut line)?;
        if bytes_read == 0 {
            // EOF: stop here, everything already answered is already saved.
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            break;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let word = parts.next().unwrap_or("");
        let mut note = parts.next().unwrap_or("").trim().to_string();
        let verdict = parse_verdict_word(word)?;
        let k1_override = extract_tag_override(&mut note, "k1");
        let k2_override = extract_tag_override(&mut note, "k2");
        let k3_override = extract_tag_override(&mut note, "k3");

        file.entries[idx].verdict = Some(verdict.to_string());
        file.entries[idx].note = note;
        if let Some(v) = k1_override {
            file.entries[idx].k1 = Some(v);
        }
        if let Some(v) = k2_override {
            file.entries[idx].k2 = Some(v);
        }
        if let Some(v) = k3_override {
            file.entries[idx].k3 = Some(v);
        }
        save(&path, &file)?;
        writeln!(output, "recorded: {verdict}")?;
    }

    writeln!(
        output,
        "\n{} pending remain in {}.",
        file.pending().count(),
        path.display()
    )?;
    Ok(())
}
