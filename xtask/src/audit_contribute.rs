//! `xtask audit contribute`: the one-command audit submission flow described
//! in `CONTRIBUTING.md` §2 ("Audit mandible against your own tools").
//!
//! The draw reads tool NAMES off `PATH` first and probes nothing: [`draw`]
//! excludes already-audited names ([`audited_tools`]), then seeded-shuffles
//! the rest and takes `--sample` of them, with zero subprocess spawned.
//! Only the drawn names are then classified, one fresh extraction pass
//! each, the same probe [`crate::audit::classify_one`] performs for `xtask
//! audit sample`/`spot-audit`. There is no full-`PATH` freeze here and
//! nothing to stratify beforehand — a stratum is a property of a probed
//! tool, and nothing is probed until after the draw. The per-stratum
//! summary this command prints (mirroring `xtask audit sample`'s own) is
//! computed from that same probe, after the fact, since a bounded sample's
//! worth of classification is cheap: no separate frozen-population pass
//! earns its keep for a `--sample` of tens of tools.
//!
//! `xtask/src` cannot spawn a subprocess itself
//! (`no_process_outside_exec.rs` forbids `std::process` outside
//! `mandible-extract/src/exec/`, spec §6/§8); every probe here goes
//! through `mandible_extract::Runner`, which does. This command otherwise
//! does plain file I/O — the draw, review resumability, writing
//! `<seed>.toml` and `<seed>-report.txt` — and for the two steps that are
//! actually git/gh operations, [`suggest_login`] and [`finish_submission`],
//! it prints what it cannot run: no prefilled login prompt, and the
//! contributor runs the printed `git switch`/`git add`/`git commit`/`gh pr
//! create` commands themselves, with a chance to review them first.
//!
//! Same reasoning for step 4 (`mandible --review <seed> --audit-dir <dir>`,
//! needs a real tty, spec/AGENTS §3.2): [`cmd_contribute`] prints the
//! command and returns when a draw has pending entries, relying on
//! CONTRIBUTING.md §2's resumability promise — a bare rerun finds the
//! unfinished seed and continues from wherever review left it.
//!
//! **No namespace containment.** The old full-`PATH` freeze probed every
//! executable on `PATH` sight-unseen, which is what earned it the same
//! containment-or-refuse gate `xtask coverage`'s own full sweep uses. This
//! flow only ever probes a bounded `--sample`-sized list, the same risk
//! class `xtask audit sample` (drawing from an already-frozen queue) and
//! `xtask audit spot-audit` (drawing from a named `--promoted` list) are
//! already in without containment. `AuditMeta::containment` records
//! `"uncontained"` for every file this command writes accordingly.

use crate::audit::{classify_one, entry_from_classified, render_report};
use crate::coverage::unique_executables_on_path;
use mandible_core::audit::{current_platform, load, save, verdict_path, AuditFile, AuditMeta};
use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// A GitHub login is validated against this shape everywhere it is read —
/// typed at the prompt here, or read back out of a folder name by
/// `scripts/check_submissions.sh` in CI.
pub(crate) fn is_valid_login(login: &str) -> bool {
    !login.is_empty() && login.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// The login prompt has no prefill. A real one would come from `gh api
/// user -q .login`, then `git config github.user` — both need a subprocess
/// this crate cannot spawn (see this module's own doc comment), so this
/// always returns `None`.
fn suggest_login() -> Option<String> {
    None
}

/// Prompt for a GitHub login on `output`, reading one line at a time from
/// `input`, until a value matching [`is_valid_login`] is given. An empty
/// line accepts [`suggest_login`]'s suggestion, when there is one.
pub(crate) fn prompt_login(
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> anyhow::Result<String> {
    let suggested = suggest_login();
    loop {
        match &suggested {
            Some(s) => write!(output, "GitHub login [{s}]: ")?,
            None => write!(output, "GitHub login: ")?,
        }
        output.flush()?;
        let mut line = String::new();
        let read = input.read_line(&mut line)?;
        let trimmed = line.trim();
        if read == 0 {
            // stdin closed. Accept a suggestion if there is one to fall
            // back on; otherwise there is nothing left to prompt with.
            if let Some(s) = suggested {
                return Ok(s);
            }
            anyhow::bail!("no GitHub login given and stdin closed");
        }
        let candidate = if trimmed.is_empty() {
            suggested.clone()
        } else {
            Some(trimmed.to_string())
        };
        match candidate {
            Some(login) if is_valid_login(&login) => return Ok(login),
            Some(login) => {
                writeln!(output, "invalid login {login:?}: must match [A-Za-z0-9-]+")?;
            }
            None => writeln!(output, "a GitHub login is required")?,
        }
    }
}

/// A random seed derived from the clock, for a draw the caller did not pin
/// with `--seed`. Not cryptographic — it only has to make two contributors'
/// draws unlikely to collide, the same job `xtask audit spot-audit`'s
/// `--draw-seed` already leaves to a human to pick by hand for anything
/// that needs reproducibility.
fn seed_from_clock() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // Masked to the non-negative i64 range: this becomes both
    // `AuditMeta::seed` and [`draw`]'s shuffle seed, and `AuditMeta` round-
    // trips through TOML's signed-64-bit integer type — an unmasked
    // FNV-1a hash is out of range about half the time.
    crate::rng::fnv1a64(&nanos.to_le_bytes()) & 0x7fff_ffff_ffff_ffff
}

/// A verdict file's name is always `<digits>.toml` — `queue.toml` and
/// anything else in a submission folder is not one. Shared by the
/// population filter (reading every submission's verdicts) and by
/// [`find_unfinished_seed`] (finding this login's own unfinished draw).
fn verdict_file_seed(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_str()?;
    let stem = name.strip_suffix(".toml")?;
    if stem.is_empty() || !stem.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    stem.parse::<u64>().ok()
}

/// Every tool with a recorded verdict in any `<submissions_root>/*/*.toml`
/// verdict file, plus every tool with a `<corpus_root>/<tool>/` fixture
/// directory — the population this flow's draw excludes before shuffling
/// (`--include-audited` restores it), never by skipping a tool mid-walk.
pub(crate) fn audited_tools(
    submissions_root: &Path,
    corpus_root: &Path,
) -> anyhow::Result<HashSet<String>> {
    let mut audited = HashSet::new();
    if submissions_root.is_dir() {
        for login_entry in std::fs::read_dir(submissions_root)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", submissions_root.display()))?
        {
            let login_entry = login_entry?;
            if !login_entry.file_type()?.is_dir() {
                continue;
            }
            let login_dir = login_entry.path();
            for file_entry in std::fs::read_dir(&login_dir)
                .map_err(|e| anyhow::anyhow!("reading {}: {e}", login_dir.display()))?
            {
                let file_entry = file_entry?;
                let path = file_entry.path();
                if verdict_file_seed(&path).is_none() {
                    continue;
                }
                // A verdict file that fails to parse is skipped rather than
                // failing the whole freeze — this filter is best-effort
                // hygiene, not a validator for someone else's submission.
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let Ok(file) = toml::from_str::<AuditFile>(&text) else {
                    continue;
                };
                for entry in &file.entries {
                    if entry.verdict.is_some() {
                        audited.insert(entry.tool.clone());
                    }
                }
            }
        }
    }
    if corpus_root.is_dir() {
        for entry in std::fs::read_dir(corpus_root)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", corpus_root.display()))?
        {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    audited.insert(name.to_string());
                }
            }
        }
    }
    Ok(audited)
}

/// Step 2 of CONTRIBUTING.md §2, redesigned: draw `sample` tool NAMES off
/// `PATH` (no probe, no full sweep — this module's own doc comment), then
/// probe and classify only the drawn tools, writing/merging them into
/// `<dir>/<seed>.toml`. Only ever called when that file does not already
/// exist ([`cmd_contribute`] checks that). Returns how many tools were
/// drawn.
///
/// The stratum table this prints is computed from the same probe that
/// built each entry — no second pass, and nothing to report for the
/// un-probed rest of `PATH`, since a stratum is only known once a tool has
/// actually been probed.
#[allow(clippy::too_many_arguments)]
fn draw(
    dir: &Path,
    submissions_root: &Path,
    corpus_root: &Path,
    login: &str,
    seed: u64,
    sample: usize,
    include_audited: bool,
    output: &mut impl Write,
) -> anyhow::Result<usize> {
    let full_population = unique_executables_on_path();
    let audited = if include_audited {
        HashSet::new()
    } else {
        audited_tools(submissions_root, corpus_root)?
    };
    let mut population: Vec<String> = full_population
        .into_iter()
        .filter(|t| !audited.contains(t))
        .collect();
    if population.is_empty() {
        anyhow::bail!(
            "no tools left to draw from after excluding {} already-audited tool(s) on PATH — \
             pass --include-audited to draw from them anyway",
            audited.len()
        );
    }

    // The draw's only randomness: a seeded shuffle of tool NAMES, nothing
    // probed yet. `seed` also names the verdict file, so `--seed N`
    // reproduces the exact same draw.
    crate::rng::seeded_shuffle(&mut population, seed);
    let take_n = sample.min(population.len());
    let drawn: Vec<String> = population.into_iter().take(take_n).collect();
    writeln!(
        output,
        "drew {} tool(s) from PATH for {login} ({} already-audited tool(s) excluded)",
        drawn.len(),
        audited.len(),
    )?;
    if drawn.len() < sample {
        writeln!(
            output,
            "note: only {} tool(s) were available to draw ({sample} requested)",
            drawn.len(),
        )?;
    }

    let mut entries = Vec::with_capacity(drawn.len());
    let mut by_stratum: BTreeMap<String, usize> = BTreeMap::new();
    for tool in &drawn {
        let classified = classify_one(tool);
        *by_stratum
            .entry(classified.stratum.to_string())
            .or_insert(0) += 1;
        entries.push(entry_from_classified(tool.clone(), &classified, None));
    }

    let vpath = verdict_path(dir, seed);
    let mut file = if vpath.is_file() {
        load(&vpath)?
    } else {
        AuditFile {
            meta: AuditMeta {
                seed,
                sample_size: sample,
                platform: current_platform(),
                containment: "uncontained".to_string(),
            },
            entries: Vec::new(),
        }
    };
    let existing_tools: HashSet<String> = file.entries.iter().map(|e| e.tool.clone()).collect();
    let mut added = 0usize;
    for entry in entries {
        if !existing_tools.contains(&entry.tool) {
            file.entries.push(entry);
            added += 1;
        }
    }
    file.entries.sort_by(|a, b| a.tool.cmp(&b.tool));
    save(&vpath, &file)?;

    writeln!(output, "stratum            count")?;
    for (stratum, count) in &by_stratum {
        writeln!(output, "{stratum:<18} {count:>6}")?;
    }
    writeln!(output, "{added} tool(s) written to {}", vpath.display())?;
    Ok(drawn.len())
}

/// This login's own unfinished draw, if it has one: the smallest seed among
/// `<dir>/<seed>.toml` files whose `<dir>/<seed>-report.txt` does not exist
/// yet. Lets a bare rerun of `contribute` (no `--seed`) resume exactly the
/// draw an earlier run left off at, rather than drawing a fresh, unrelated
/// sample — `xtask audit sample`'s own cursor always advances on every
/// call, so resuming has to mean "reuse the existing file", never "draw
/// again". A missing report, not "has a pending entry", is the right test:
/// a seed whose review finished but was interrupted before the report got
/// written (a crash, a killed process) has zero pending entries and must
/// still be resumed — otherwise a bare rerun would draw an entirely new
/// sample and leave the first one's verdicts stranded, unreported and
/// uncommitted, forever.
fn find_unfinished_seed(dir: &Path) -> anyhow::Result<Option<u64>> {
    if !dir.is_dir() {
        return Ok(None);
    }
    let mut seeds: Vec<u64> = Vec::new();
    for entry in
        std::fs::read_dir(dir).map_err(|e| anyhow::anyhow!("reading {}: {e}", dir.display()))?
    {
        let entry = entry?;
        if let Some(seed) = verdict_file_seed(&entry.path()) {
            seeds.push(seed);
        }
    }
    seeds.sort_unstable();
    for seed in seeds {
        if !report_path(dir, seed).is_file() {
            return Ok(Some(seed));
        }
    }
    Ok(None)
}

/// `<dir>/<seed>-report.txt` — the counterpart to
/// [`mandible_core::audit::verdict_path`], which this module has no
/// equivalent of upstream since only `contribute` writes this file.
fn report_path(dir: &Path, seed: u64) -> PathBuf {
    dir.join(format!("{seed}-report.txt"))
}

/// Steps 5-6 of CONTRIBUTING.md §2: prints the `git`/`gh` commands that
/// commit the two files and open the pull request, rather than running them
/// (see this module's own doc comment for why). `--no-pr` drops the `gh pr
/// create` line.
fn finish_submission(
    login: &str,
    seed: u64,
    verdict_path: &Path,
    report_path: &Path,
    no_pr: bool,
    output: &mut impl Write,
) -> anyhow::Result<()> {
    let branch = format!("audit/{login}-{seed}");
    writeln!(output)?;
    writeln!(output, "Run these to finish your submission:")?;
    writeln!(output, "  git switch -c {branch}")?;
    writeln!(
        output,
        "  git add {} {}",
        verdict_path.display(),
        report_path.display(),
    )?;
    writeln!(output, "  git commit -S -m \"audit: {login} seed {seed}\"")?;
    if !no_pr {
        writeln!(output, "  gh pr create --fill")?;
    }
    Ok(())
}

/// `xtask audit contribute`: CONTRIBUTING.md §2 end to end, up to the limits
/// this module's own doc comment names. `submissions_root` is
/// `audit/submissions` by default; `corpus_root` is `corpus`.
#[allow(clippy::too_many_arguments)]
pub fn cmd_contribute(
    submissions_root: &Path,
    corpus_root: &Path,
    seed: Option<u64>,
    sample: usize,
    include_audited: bool,
    no_pr: bool,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> anyhow::Result<()> {
    let login = prompt_login(input, output)?;
    let dir: PathBuf = submissions_root.join(&login);

    let seed = match seed {
        Some(s) => s,
        None => match find_unfinished_seed(&dir)? {
            Some(s) => {
                writeln!(
                    output,
                    "resuming unfinished seed {s} ({})",
                    verdict_path(&dir, s).display()
                )?;
                s
            }
            None => seed_from_clock(),
        },
    };
    writeln!(output, "Seed {seed}")?;

    let vpath = verdict_path(&dir, seed);
    if !vpath.is_file() {
        draw(
            &dir,
            submissions_root,
            corpus_root,
            &login,
            seed,
            sample,
            include_audited,
            output,
        )?;
    }

    let file = load(&vpath)?;
    let pending = file.pending().count();
    if pending > 0 {
        writeln!(
            output,
            "\nRun `mandible --review {seed} --audit-dir {}` to review {pending} pending \
             tool(s), then re-run this command to continue.",
            dir.display(),
        )?;
        return Ok(());
    }

    let report_text = render_report(&dir, seed)?;
    let report_path = report_path(&dir, seed);
    std::fs::write(&report_path, &report_text)
        .map_err(|e| anyhow::anyhow!("writing {}: {e}", report_path.display()))?;
    writeln!(output, "wrote {}", report_path.display())?;

    finish_submission(&login, seed, &vpath, &report_path, no_pr, output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_valid_login_accepts_alphanumeric_and_hyphen_only() {
        assert!(is_valid_login("sadigaxund"));
        assert!(is_valid_login("a-b-C-1"));
        assert!(!is_valid_login(""));
        assert!(!is_valid_login("has space"));
        assert!(!is_valid_login("has_underscore"));
        assert!(!is_valid_login("has/slash"));
        assert!(!is_valid_login("émigré"));
    }

    #[test]
    fn prompt_login_accepts_a_typed_valid_login() {
        let mut input = std::io::Cursor::new(b"ci-test-login\n".to_vec());
        let mut output = Vec::new();
        let login = prompt_login(&mut input, &mut output).unwrap();
        assert_eq!(login, "ci-test-login");
    }

    #[test]
    fn prompt_login_reprompts_on_an_invalid_login_then_accepts() {
        let mut input = std::io::Cursor::new(b"bad login!\nok-login\n".to_vec());
        let mut output = Vec::new();
        let login = prompt_login(&mut input, &mut output).unwrap();
        assert_eq!(login, "ok-login");
        let printed = String::from_utf8(output).unwrap();
        assert!(printed.contains("invalid login"));
    }

    #[test]
    fn prompt_login_fails_on_closed_stdin_with_no_suggestion() {
        let mut input = std::io::Cursor::new(Vec::new());
        let mut output = Vec::new();
        assert!(prompt_login(&mut input, &mut output).is_err());
    }

    fn write_verdict_file(dir: &Path, seed: u64, tools: &[(&str, Option<&str>)]) {
        std::fs::create_dir_all(dir).unwrap();
        let mut text = format!("[meta]\nseed = {seed}\nsample_size = {}\n\n", tools.len());
        for (tool, verdict) in tools {
            text.push_str("[[entry]]\n");
            text.push_str(&format!("tool = \"{tool}\"\nstratum = \"ok\"\n"));
            if let Some(v) = verdict {
                text.push_str(&format!("verdict = \"{v}\"\n"));
            }
            text.push('\n');
        }
        std::fs::write(dir.join(format!("{seed}.toml")), text).unwrap();
    }

    /// §3.4 guard test: a tool with a recorded verdict in a submission file
    /// must be excluded from the population. Broken by dropping the
    /// `entry.verdict.is_some()` check (see the sibling test below, which
    /// exercises the break).
    #[test]
    fn audited_tools_includes_a_tool_with_a_recorded_verdict() {
        let tmp = tempfile::tempdir().unwrap();
        let submissions = tmp.path().join("submissions");
        write_verdict_file(
            &submissions.join("alice"),
            5,
            &[("zoxide", Some("correct")), ("curl", None)],
        );
        let corpus = tmp.path().join("corpus");
        std::fs::create_dir_all(&corpus).unwrap();

        let audited = audited_tools(&submissions, &corpus).unwrap();
        assert!(
            audited.contains("zoxide"),
            "a tool with a recorded verdict must be excluded"
        );
        assert!(
            !audited.contains("curl"),
            "a pending (unverdicted) entry must not exclude its tool"
        );
    }

    #[test]
    fn audited_tools_includes_a_tool_with_a_corpus_fixture_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let submissions = tmp.path().join("submissions");
        std::fs::create_dir_all(&submissions).unwrap();
        let corpus = tmp.path().join("corpus");
        std::fs::create_dir_all(corpus.join("tmux")).unwrap();

        let audited = audited_tools(&submissions, &corpus).unwrap();
        assert!(audited.contains("tmux"));
    }

    #[test]
    fn audited_tools_ignores_queue_toml_which_is_not_a_verdict_file() {
        let tmp = tempfile::tempdir().unwrap();
        let submissions = tmp.path().join("submissions");
        let login_dir = submissions.join("bob");
        std::fs::create_dir_all(&login_dir).unwrap();
        // `queue.toml` is a `Queue`, not an `AuditFile` — parsing it as one
        // must not blow up the whole scan, and it must not contribute any
        // tool to the audited set.
        std::fs::write(login_dir.join("queue.toml"), "not valid audit toml").unwrap();
        let corpus = tmp.path().join("corpus");
        std::fs::create_dir_all(&corpus).unwrap();

        let audited = audited_tools(&submissions, &corpus).unwrap();
        assert!(audited.is_empty());
    }

    #[test]
    fn verdict_file_seed_accepts_only_digit_stem_toml_files() {
        assert_eq!(verdict_file_seed(Path::new("audit/7.toml")), Some(7));
        assert_eq!(verdict_file_seed(Path::new("audit/queue.toml")), None);
        assert_eq!(verdict_file_seed(Path::new("audit/7-report.txt")), None);
        assert_eq!(verdict_file_seed(Path::new("audit/queue-captures")), None);
    }

    #[test]
    fn find_unfinished_seed_finds_a_file_with_a_pending_entry() {
        let tmp = tempfile::tempdir().unwrap();
        write_verdict_file(
            tmp.path(),
            9,
            &[("zoxide", Some("correct")), ("curl", None)],
        );
        assert_eq!(find_unfinished_seed(tmp.path()).unwrap(), Some(9));
    }

    /// A seed with zero pending entries is still unfinished until its
    /// report exists — this is what makes a crash between "review done"
    /// and "report written" resumable instead of silently abandoning the
    /// finished verdicts for a brand-new draw.
    #[test]
    fn find_unfinished_seed_stays_some_with_zero_pending_until_the_report_exists() {
        let tmp = tempfile::tempdir().unwrap();
        write_verdict_file(tmp.path(), 9, &[("zoxide", Some("correct"))]);
        assert_eq!(find_unfinished_seed(tmp.path()).unwrap(), Some(9));

        std::fs::write(report_path(tmp.path(), 9), "report text").unwrap();
        assert_eq!(find_unfinished_seed(tmp.path()).unwrap(), None);
    }

    #[test]
    fn finish_submission_prints_the_fallback_commands_including_pr() {
        let mut output = Vec::new();
        finish_submission(
            "alice",
            42,
            Path::new("audit/submissions/alice/42.toml"),
            Path::new("audit/submissions/alice/42-report.txt"),
            false,
            &mut output,
        )
        .unwrap();
        let printed = String::from_utf8(output).unwrap();
        assert!(printed.contains("git switch -c audit/alice-42"));
        assert!(printed.contains("git add audit/submissions/alice/42.toml"));
        assert!(printed.contains("git commit -S -m \"audit: alice seed 42\""));
        assert!(printed.contains("gh pr create --fill"));
    }

    #[test]
    fn finish_submission_omits_the_pr_command_with_no_pr() {
        let mut output = Vec::new();
        finish_submission(
            "alice",
            42,
            Path::new("audit/submissions/alice/42.toml"),
            Path::new("audit/submissions/alice/42-report.txt"),
            true,
            &mut output,
        )
        .unwrap();
        let printed = String::from_utf8(output).unwrap();
        assert!(!printed.contains("gh pr"));
    }
}
