//! The non-interactive twin of [`super::interactive::cmd_review`]: `emit`
//! writes pending pairs to a file, `ingest` reads verdicts back in, and
//! `amend` corrects one already-recorded verdict without a second review.

use super::classify_one;
use super::interactive::render_snapshot;
use mandible_core::audit::{
    extract_tag_override, load, parse_verdict_word, save, tag_display, verdict_path, Entry,
};
use std::path::Path;

/// `xtask audit emit`: write every pending pair (raw text + parsed tree) to
/// its own file under `emit_dir`, for a reviewer without a live terminal —
/// or without this machine's tty at all — to read offline and judge on
/// their own schedule. The counterpart, [`cmd_ingest`], reads the resulting
/// verdicts back in.
pub fn cmd_emit(dir: &Path, seed: u64, emit_dir: &Path) -> anyhow::Result<()> {
    let path = verdict_path(dir, seed);
    let file = load(&path)?;
    std::fs::create_dir_all(emit_dir)
        .map_err(|e| anyhow::anyhow!("creating {}: {e}", emit_dir.display()))?;

    let pending: Vec<&Entry> = file
        .entries
        .iter()
        .filter(|e| e.verdict.is_none())
        .collect();
    for entry in &pending {
        let classified = classify_one(&entry.tool);
        let mut buf = String::new();
        buf.push_str(&format!(
            "tool: {}\nstratum: {}\n",
            entry.tool, entry.stratum
        ));
        if let Some(reason) = &entry.include_reason {
            buf.push_str(&format!("forced inclusion: {reason}\n"));
        }
        buf.push_str(&format!(
            "{}\n{}\n{}\n\n",
            tag_display("K1 (single-dash-long defect)", entry.k1, "k1"),
            tag_display("K2 (existence-detector tokenizer gap)", entry.k2, "k2"),
            tag_display("K3 (subcommand help never fetched)", entry.k3, "k3"),
        ));
        buf.push_str("=== raw --help ===\n");
        buf.push_str(
            classified
                .raw_text
                .as_deref()
                .unwrap_or("(no output captured)"),
        );
        buf.push_str("\n\n=== parsed tree ===\n");
        buf.push_str(&render_snapshot(classified.result.root.as_ref()));
        let file_path = emit_dir.join(format!("{}.txt", sanitize_filename(&entry.tool)));
        std::fs::write(&file_path, buf)
            .map_err(|e| anyhow::anyhow!("writing {}: {e}", file_path.display()))?;
    }

    println!(
        "emitted {} pending pair(s) to {}",
        pending.len(),
        emit_dir.display()
    );
    println!(
        "review offline, then write a verdicts file (one line per tool: `<tool> <verdict> \
         [note...]`, optionally including `k1=true`/`k1=false`/`k2=true`/`k2=false`/\
         `k3=true`/`k3=false` anywhere in the note to override a pre-tag) and run: \
         cargo run -p xtask -- audit ingest --seed {seed} --verdicts <file>"
    );
    Ok(())
}

/// A tool name is never empty and, on every platform this project targets,
/// never contains `/` (§ `resolve_tool`'s own PATH-search doesn't accept
/// path separators in a bare tool name either), so this exists only to be
/// defensive about the one other filesystem-hostile case worth naming.
/// `pub(crate)` so `crate::queue`'s capture directory naming
/// (`queue-captures/<tool>/`) can use the same rule.
pub(crate) fn sanitize_filename(tool: &str) -> String {
    tool.chars()
        .map(|c| if c == '/' || c == '\\' { '_' } else { c })
        .collect()
}

/// `xtask audit ingest`: read a plain verdicts file (`# comments` and blank
/// lines ignored; otherwise `<tool> <verdict> [note...]` per line) and
/// apply it to `path`'s entries. An unknown tool name is reported, not
/// silently dropped. An entry that already carries a verdict is left alone
/// unless `overwrite` is set — so re-running `ingest` on a file that
/// includes already-applied lines is safe and idempotent, the same
/// resumability property `crate::queue::cmd_sample`/[`cmd_review`] give the
/// rest of this workflow.
pub fn cmd_ingest(
    dir: &Path,
    seed: u64,
    verdicts_path: &Path,
    overwrite: bool,
) -> anyhow::Result<()> {
    let path = verdict_path(dir, seed);
    let mut file = load(&path)?;
    let raw = std::fs::read_to_string(verdicts_path)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", verdicts_path.display()))?;

    let mut applied = 0usize;
    let mut already = 0usize;
    let mut unknown: Vec<String> = Vec::new();

    for (lineno, raw_line) in raw.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, char::is_whitespace);
        let tool = parts.next().unwrap_or("");
        let word = parts.next().unwrap_or("");
        let mut note = parts.next().unwrap_or("").trim().to_string();
        let verdict = parse_verdict_word(word)
            .map_err(|e| anyhow::anyhow!("{}:{}: {e}", verdicts_path.display(), lineno + 1))?;
        let k1_override = extract_tag_override(&mut note, "k1");
        let k2_override = extract_tag_override(&mut note, "k2");
        let k3_override = extract_tag_override(&mut note, "k3");

        // The same obligation the TUI enforces, applied here so the two
        // entry paths cannot disagree about what a complete record is. The
        // override tokens are already stripped above, so a line whose only
        // content was `k1=false` correctly counts as noteless.
        if mandible_core::audit::verdict_requires_note(verdict) && note.trim().is_empty() {
            anyhow::bail!(
                "{}:{}: verdict {:?} for {:?} needs a note — for wrong/incomplete the note is \
                 the finding, and an entry naming a tool with nothing about what was wrong \
                 gives later triage nothing to act on",
                verdicts_path.display(),
                lineno + 1,
                verdict,
                tool,
            );
        }

        let Some(entry) = file.entries.iter_mut().find(|e| e.tool == tool) else {
            unknown.push(tool.to_string());
            continue;
        };
        if entry.verdict.is_some() && !overwrite {
            already += 1;
            continue;
        }
        entry.verdict = Some(verdict.to_string());
        entry.note = note;
        if let Some(v) = k1_override {
            entry.k1 = Some(v);
        }
        if let Some(v) = k2_override {
            entry.k2 = Some(v);
        }
        if let Some(v) = k3_override {
            entry.k3 = Some(v);
        }
        applied += 1;
    }

    save(&path, &file)?;
    println!(
        "applied {applied} verdict(s); {already} already recorded (use --overwrite to replace); \
         {} unknown tool name(s) not in the sample{}",
        unknown.len(),
        if unknown.is_empty() {
            String::new()
        } else {
            format!(": {}", unknown.join(", "))
        }
    );
    Ok(())
}

/// `xtask audit amend`: correct one already-recorded verdict without
/// destroying it — see `mandible_core::audit::amend`'s doc comment for the
/// full mechanism this wraps. The only entry point touching
/// [`mandible_core::audit::Entry::amendments`]; no TUI counterpart.
///
/// A subcommand, not a TUI flow: `mandible --review`'s loop
/// (`run_review`) walks `AuditFile::needing_attention` and never revisits
/// an already-complete `correct` verdict, so reaching an amendment needs a
/// separate navigation mode. This command needs no tty (AGENTS.md §3.2)
/// and is fully covered by `cargo nextest run`.
pub fn cmd_amend(
    dir: &Path,
    seed: u64,
    tool: &str,
    new_verdict_word: &str,
    new_note: Option<String>,
    reason: String,
) -> anyhow::Result<()> {
    let path = verdict_path(dir, seed);
    let mut file = load(&path)?;
    let new_verdict = mandible_core::audit::parse_verdict_word(new_verdict_word)?;
    let entry = file
        .entries
        .iter_mut()
        .find(|e| e.tool == tool)
        .ok_or_else(|| anyhow::anyhow!("{tool:?} not found in {}", path.display()))?;
    let previous_effective = entry.effective_verdict().map(str::to_string);
    mandible_core::audit::amend(entry, new_verdict, new_note.unwrap_or_default(), reason)?;
    let amendment_count = entry.amendments.len();
    save(&path, &file)?;
    println!(
        "amended {tool}: {} -> {new_verdict} ({amendment_count} amendment(s) now recorded for \
         this entry)",
        previous_effective.as_deref().unwrap_or("(none)"),
    );
    Ok(())
}
