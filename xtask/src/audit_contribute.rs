//! `xtask audit contribute`: the one-command audit submission flow described
//! in `CONTRIBUTING.md` §2 ("Audit mandible against your own tools").
//!
//! `xtask/src` cannot spawn a subprocess (`no_process_outside_exec.rs`
//! forbids `std::process` outside `mandible-extract/src/exec/`, spec
//! §6/§8), so this command does everything that is plain file I/O — the
//! freeze, the draw, review resumability, writing `<seed>.toml` and
//! `<seed>-report.txt` — and for the two steps that are actually git/gh
//! operations, [`suggest_login`] and [`finish_submission`], it prints what
//! it cannot run: no prefilled login prompt, and the contributor runs the
//! printed `git switch`/`git add`/`git commit`/`gh pr create` commands
//! themselves, with a chance to review them first.
//!
//! Same reasoning for step 4 (`mandible --review <seed> --audit-dir <dir>`,
//! needs a real tty, spec/AGENTS §3.2): [`cmd_contribute`] prints the
//! command and returns when a draw has pending entries, relying on
//! CONTRIBUTING.md §2's resumability promise — a bare rerun finds the
//! unfinished seed and continues from wherever review left it.

use crate::audit::{classify_one_with_recordings, render_report, Classified};
use crate::queue::{
    captures_dir, population_hash, queue_path, save_queue, shuffle_stratify, today_iso8601,
    write_captures_for_tool, Queue, QueueMeta,
};
use crate::rng::fnv1a64;
use crate::{finish_sweep_guard, sweep_guard};
use mandible_core::audit::{load, verdict_path, AuditFile};
use mandible_extract::exec::ExecOutput;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Carries the already-prompted-for login across the containment re-exec a
/// full-`PATH` freeze goes through (`unshare` re-execs this binary with
/// the same argv and environment). That re-exec restarts `main()` from
/// scratch, which would otherwise call [`prompt_login`] a second time
/// against a `stdin` already consumed by the first prompt, since the
/// fresh process image remembers nothing read from a pipe. Set right
/// before [`crate::sweep_guard`] is called and checked at the top of
/// [`cmd_contribute`], the same pattern `containment`'s own
/// `SCOREBOARD_FD_ENV_VAR` uses to survive the same re-exec.
const CONTRIBUTE_LOGIN_ENV_VAR: &str = "XTASK_CONTRIBUTE_LOGIN";

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
    // Masked to the non-negative i64 range for the same reason
    // `freeze_for_contribute`'s own seed is: this becomes `AuditMeta::seed`,
    // which round-trips through TOML's signed-64-bit integer type, and an
    // unmasked FNV-1a hash is out of range about half the time.
    fnv1a64(&nanos.to_le_bytes()) & 0x7fff_ffff_ffff_ffff
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

/// Print a progress line to stderr: `\r`-overwritten in place under a tty,
/// one plain line per update otherwise (brief's own requirement — a CI log
/// or a piped run must never see carriage-return noise).
fn print_progress(done: usize, total: usize, elapsed: Duration, tty: bool) {
    let msg = format!(
        "classifying tools: {done}/{total} ({}s elapsed)",
        elapsed.as_secs()
    );
    if tty {
        // Padded so a shorter later message fully overwrites a longer
        // earlier one on the same line.
        eprint!("\r{msg:<72}");
        let _ = std::io::stderr().flush();
    } else {
        eprintln!("{msg}");
    }
}

/// [`crate::audit::classify_all_with_recordings`], with a live progress
/// indicator on stderr while the parallel classification runs (spec: done
/// count, total, and elapsed time). The classification itself is unchanged
/// — this only adds a ticking reporter thread around the same
/// `par_iter().map(classify_one_with_recordings)` shape.
fn classify_with_progress(
    tools: &[String],
) -> Vec<(String, Classified, HashMap<Vec<String>, ExecOutput>)> {
    let total = tools.len();
    let done = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let start = Instant::now();
    let tty = std::io::stderr().is_terminal();

    let progress_thread = {
        let done = Arc::clone(&done);
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut last_reported = usize::MAX;
            loop {
                let now_done = done.load(Ordering::Relaxed);
                if tty {
                    print_progress(now_done, total, start.elapsed(), true);
                } else if now_done != last_reported {
                    print_progress(now_done, total, start.elapsed(), false);
                    last_reported = now_done;
                }
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        })
    };

    let results: Vec<_> = tools
        .par_iter()
        .map(|t| {
            let (classified, recordings) = classify_one_with_recordings(t);
            done.fetch_add(1, Ordering::Relaxed);
            (t.clone(), classified, recordings)
        })
        .collect();

    stop.store(true, Ordering::Relaxed);
    let _ = progress_thread.join();
    print_progress(total, total, start.elapsed(), tty);
    if tty {
        eprintln!();
    }
    results
}

/// Step 2 of CONTRIBUTING.md §2: freeze `<dir>/queue.toml` (only ever
/// called when it does not already exist — [`cmd_contribute`] checks that),
/// scanning `PATH` once, excluding already-audited tools first
/// ([`audited_tools`], before the shuffle, never as a skip during the
/// cursor walk), and reporting live progress on stderr.
fn freeze_for_contribute(
    dir: &Path,
    submissions_root: &Path,
    corpus_root: &Path,
    login: &str,
    include_audited: bool,
    output: &mut impl Write,
) -> anyhow::Result<()> {
    let full_population = crate::coverage::unique_executables_on_path();
    let audited = if include_audited {
        HashSet::new()
    } else {
        audited_tools(submissions_root, corpus_root)?
    };
    let population: Vec<String> = full_population
        .into_iter()
        .filter(|t| !audited.contains(t))
        .collect();
    if population.is_empty() {
        anyhow::bail!(
            "no tools left to freeze after excluding {} already-audited tool(s) — pass \
             --include-audited to draw from them anyway",
            audited.len()
        );
    }

    writeln!(
        output,
        "classifying {} tool(s) on PATH for {login} ({} already-audited tool(s) excluded)...",
        population.len(),
        audited.len(),
    )?;
    let classified = classify_with_progress(&population);

    let cdir = captures_dir(dir);
    std::fs::create_dir_all(&cdir)
        .map_err(|e| anyhow::anyhow!("creating {}: {e}", cdir.display()))?;
    for (tool, _classified, recordings) in &classified {
        write_captures_for_tool(&cdir, tool, recordings)?;
    }

    let pairs: Vec<(String, String)> = classified
        .iter()
        .map(|(tool, c, _)| (tool.clone(), c.stratum.to_string()))
        .collect();
    // The shuffle-stratification seed only decides queue order, never which
    // tools are in it; deriving it from the login keeps a re-freeze of the
    // same folder stable rather than picking a fresh order every time.
    // Masked to the non-negative i64 range: `QueueMeta::seed` round-trips
    // through TOML, whose only integer type is a signed 64-bit — an
    // unmasked FNV-1a hash exceeds `i64::MAX` about half the time and
    // failed exactly that way the first time this ran for real
    // ("out-of-range value for u64 type" from the `toml` crate).
    let freeze_seed = fnv1a64(login.as_bytes()) & 0x7fff_ffff_ffff_ffff;
    let entries = shuffle_stratify(&pairs, freeze_seed);

    let qpath = queue_path(dir);
    let queue = Queue {
        meta: QueueMeta {
            freeze_date: today_iso8601(),
            population_hash: population_hash(&population),
            seed: freeze_seed,
            cursor: 0,
        },
        entries,
    };
    save_queue(&qpath, &queue)?;
    writeln!(
        output,
        "froze {} tool(s) into {}",
        queue.entries.len(),
        qpath.display(),
    )?;
    Ok(())
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
    allow_uncontained: bool,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> anyhow::Result<()> {
    // A contained freeze re-execs this whole binary from `main()` (see
    // `CONTRIBUTE_LOGIN_ENV_VAR`'s doc comment) — the second time through,
    // the login is already resolved and must not be re-prompted for
    // against a `stdin` that was already drained by the first prompt.
    let login = match std::env::var(CONTRIBUTE_LOGIN_ENV_VAR) {
        Ok(login) if is_valid_login(&login) => login,
        _ => prompt_login(input, output)?,
    };
    let dir: PathBuf = submissions_root.join(&login);

    let qpath = queue_path(&dir);
    if qpath.is_file() {
        writeln!(
            output,
            "queue already frozen at {} — reusing it",
            qpath.display()
        )?;
    } else {
        // Freezing (no `--tools`) is an unbounded PATH sweep, so it gets
        // the same namespace containment + canary guard every other
        // full-PATH sweep in this crate goes through (`xtask audit
        // freeze`, `xtask coverage`) — which, on a host where namespaces
        // are available, means this whole process is about to be replaced
        // by a re-exec'd copy of itself (see `CONTRIBUTE_LOGIN_ENV_VAR`).
        std::env::set_var(CONTRIBUTE_LOGIN_ENV_VAR, &login);
        let canaries = sweep_guard(true, allow_uncontained, None)?;
        let freeze_result = freeze_for_contribute(
            &dir,
            submissions_root,
            corpus_root,
            &login,
            include_audited,
            output,
        );
        finish_sweep_guard(canaries)?;
        freeze_result?;
    }

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

    let vpath = verdict_path(&dir, seed);
    if !vpath.is_file() {
        let drawn = crate::queue::cmd_sample(seed, sample, &dir, &[])?;
        writeln!(output, "Seed {seed} → {drawn} tool(s) drawn")?;
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
