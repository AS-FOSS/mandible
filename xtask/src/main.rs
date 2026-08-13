//! Developer tasks for the mandible workspace: the extraction coverage
//! harness (spec §13.1).

#![forbid(unsafe_code)]

mod alternation;
mod audit;
mod bundling;
mod corpus;
mod coverage;
mod detector;
mod existence;
mod misattribution;
mod queue;
mod rng;
mod status;
mod transition;

use clap::{Parser, Subcommand};
use coverage::ScoreFormat;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "xtask", about = "Developer tasks for the mandible workspace")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the extraction coverage harness across every executable on
    /// `PATH` (spec §13.1) and print/write the scoreboard.
    Coverage {
        /// Compare the freshly computed aggregate against the checked-in
        /// scoreboard and fail (nonzero exit) if `%flags_text` dropped, the
        /// `no-tier` count grew, or the `suspicious` count grew — the
        /// regression gate spec §13.1 describes. `verbatim`, framework-
        /// detection, and `misattribution_suspect_tools` counts are
        /// reported but deliberately not part of this gate (see
        /// `coverage::compute_aggregate`'s doc comment and
        /// `crate::misattribution`'s). Without this flag, the command just
        /// (re)writes the scoreboard file.
        #[arg(long)]
        check: bool,
        /// Where to read/write the scoreboard.
        ///
        /// Defaults under `tmp/` (gitignored) rather than the repo root: a
        /// full-PATH scoreboard is a snapshot of one machine's installed
        /// tools, never a portable baseline (spec §13.1a), so it is scratch
        /// by construction and does not belong beside the tracked files. The
        /// one scoreboard that *is* a baseline, `coverage-scoreboard.ci.txt`,
        /// stays at the root because CI names that path explicitly — it is
        /// checked in, and `--check` diffs against it.
        #[arg(long, default_value = "tmp/coverage-scoreboard.txt")]
        out: PathBuf,
        /// Scan only this comma-separated list of tool names instead of
        /// every executable on `PATH`. Pins a fixed, reproducible
        /// inventory — what CI uses, since the full-`PATH` scoreboard's
        /// tool set (and therefore its aggregate) varies with the runner
        /// image and can't be a meaningful regression baseline there.
        #[arg(long, value_delimiter = ',')]
        tools: Option<Vec<String>>,
        /// Scan only one slice of the tool list, as `INDEX/TOTAL` (e.g.
        /// `0/8`). Sliced by *stride*, not by contiguous block: pathological
        /// tools cluster alphabetically (a machine with 23
        /// `qemu-*-static` binaries, 4 MB each, puts them all in one
        /// contiguous chunk and that chunk alone takes longer than the
        /// other eleven combined), so a stride spreads them evenly and
        /// every shard finishes in comparable time.
        #[arg(long)]
        shard: Option<String>,
        /// Print each tool's name to stderr *before* probing it.
        ///
        /// Exists to identify which tool killed a run. The full-PATH sweep
        /// runs ~1,500 arbitrary third-party binaries, and on GitHub's
        /// runners three shards die every time with "the runner has
        /// received a shutdown signal" — a message that says something
        /// disrupted the host, not that our process ran out of memory
        /// (peak RSS is ~270 MB, and the same shards complete locally).
        /// Without a per-tool trace the logs name no suspect at all.
        #[arg(long)]
        progress: bool,
        /// Output format: fixed-width `text` (the format checked into
        /// `coverage-scoreboard.txt`) or GitHub-flavored `markdown` (spec
        /// §13.1a's framework-support workflow writes this straight to
        /// `$GITHUB_STEP_SUMMARY`).
        #[arg(long, value_enum, default_value = "text")]
        format: ScoreFormat,
        /// Run a full-`PATH` sweep (no `--tools`) directly on this machine,
        /// with no namespace containment and no canary tripwires.
        ///
        /// Without this flag, a full-`PATH` sweep re-execs itself under a
        /// fresh user/PID/mount namespace (`mandible_extract::exec::
        /// containment`) and seeds three canary tripwires
        /// (`mandible_extract::exec::canary`) before probing, refusing to
        /// run at all if the host cannot provide all three namespace
        /// types — see that module's doc comment for exactly what this
        /// buys and what it does not (notably: no network containment).
        /// A `--tools`-pinned run (a fixed, small, reviewed list) is never
        /// gated by this — only an unbounded scan of everything on `PATH`
        /// is.
        #[arg(long)]
        allow_uncontained: bool,
    },
    /// Replay every fixture under `corpus/<tool>/<version>/` through the
    /// real tiered extraction pipeline with zero subprocesses (spec
    /// §13.2, `corpus/README.md`), and fail loudly when a parse
    /// regresses: a snapshot mismatch, a violated `[contract]`, a
    /// promoted-but-still-`[xfail]`-marked fixture, or a fixture that
    /// parses slower than the coarse 100ms ceiling.
    Corpus {
        /// Rewrite every fixture's `expected.snap` to match its freshly
        /// extracted tree instead of checking it. Never fails the run
        /// (short of an I/O error) — this is the accept-the-new-snapshot
        /// step of `corpus/README.md`'s fixture workflow, the plain-file-
        /// compare equivalent of `cargo insta review`.
        #[arg(long)]
        bless: bool,
        /// The corpus root to scan.
        #[arg(long, default_value = "corpus")]
        dir: PathBuf,
        /// Output format for a *checking* run (ignored with `--bless`,
        /// which only ever rewrites and reports what it wrote): fixed
        /// per-fixture `text` lines (unchanged since this command's first
        /// version), or GitHub-flavored `markdown` — a semantic before/
        /// after transition report (status, node/flag counts, named
        /// subcommand/flag deltas), never a raw `expected.snap` diff. The
        /// corpus CI job writes this straight to `$GITHUB_STEP_SUMMARY`,
        /// same convention as `coverage`'s `--format markdown`.
        #[arg(long, value_enum, default_value = "text")]
        format: ScoreFormat,
        /// A second, plain corpus directory (never a git ref) to diff every
        /// fixture's `[contract]` against, printing a prominent `CONTRACT
        /// WEAKENED: <fixture> <field>` line for every field that got
        /// weaker — see `corpus::contract_weakened_lines`'s doc comment for
        /// why this takes a directory instead of talking to git itself
        /// (this binary has no git access, by the same workspace-wide
        /// invariant that keeps `std::process` out of every crate but
        /// `mandible-extract/src/exec/`). Reported, never gated — a
        /// contract may legitimately weaken (`corpus/README.md`'s
        /// documented lifecycle), this only makes sure nobody has to take
        /// that on faith. Populate it however you like — a CI step
        /// running `git archive <base-ref> corpus | tar -x -C <dir>` is
        /// the intended one. Omit it and nothing about this run changes.
        #[arg(long)]
        baseline_dir: Option<PathBuf>,
        /// Print one fixture's captured help text beside the tree the
        /// parser makes of it, and exit without checking anything.
        ///
        /// Exists because a fixture is otherwise only inspectable by
        /// reading a `meta.toml`, an `expected.snap` and a capture file
        /// separately and holding all three in your head. That is the same
        /// comparison `xtask audit emit` renders for a live tool, sourced
        /// from the frozen capture instead, so what a fixture actually
        /// encodes can be checked by looking rather than by trusting a
        /// summary of it. Matches on a substring of `<tool>/<version>`.
        #[arg(long)]
        show: Option<String>,
    },
    /// A semantic per-tool diff between two coverage scoreboards (WS2 part
    /// 1, `transition.rs`'s own doc comment): status transitions, flag-
    /// count gains/losses (reported separately, never netted), and tools
    /// appearing or disappearing. This is the check that has actually
    /// caught every regression on this branch so far — done by hand, by a
    /// human running a full sweep before and after a grammar change and
    /// diffing per tool, because the aggregate `%flags_text` gate and the
    /// fixed corpus both stayed green through two real regressions. Reads
    /// two already-rendered `ScoreFormat::Text` scoreboards (e.g. two
    /// `cargo xtask coverage --out <path>` runs before/after a change);
    /// never a raw text diff of the files themselves — see
    /// `render_markdown`'s doc comment for why.
    ///
    /// **Non-blocking, per maintainer decision D4**: this never fails the
    /// run (there is deliberately no `--check`-style flag here to fail on),
    /// so it can ship now and be promoted to a real gate after a burn-in
    /// period without a second command to learn.
    SweepDiff {
        /// The earlier scoreboard.
        #[arg(long)]
        before: PathBuf,
        /// The later scoreboard.
        #[arg(long)]
        after: PathBuf,
        /// Write the rendered report here in addition to printing it.
        /// Omit to only print to stdout.
        #[arg(long)]
        out: Option<PathBuf>,
        /// `markdown` for `$GITHUB_STEP_SUMMARY`, `text` for a terminal or
        /// plain log. Same convention as `coverage --format`/`corpus
        /// --format`.
        #[arg(long, value_enum, default_value = "text")]
        format: ScoreFormat,
    },
    /// A bounded, random, human-reviewed sample of real tools, comparing
    /// raw captured `--help` text against the parsed tree (`audit.rs`'s own
    /// doc comment has the full rationale). This is the first instrument
    /// that measures agreement with *truth*, not with the parser's own
    /// prior output.
    Audit {
        #[command(subcommand)]
        action: AuditAction,
    },
    /// The family-detector calibration harness (`detector.rs`'s own doc
    /// comment has the full rationale): a fleet-wide defect detector
    /// generalizes one human finding across every tool on `PATH`, which
    /// makes its fleet number confident and unverified at the same time.
    /// This checks a detector against the audit's own labelled tools —
    /// it must fire on the known-bad and stay silent on the known-good —
    /// and **a detector's fleet-wide number is not quotable until it has
    /// passed** (spec §13.1e).
    Detector {
        #[command(subcommand)]
        action: DetectorAction,
    },
}

#[derive(Subcommand)]
enum DetectorAction {
    /// Every registered detector, the defect family it claims to
    /// generalize, and how many audited tools carry that family label —
    /// i.e. how much evidence a calibration run can possibly have.
    List {
        #[arg(long, default_value_t = 2)]
        seed: u64,
        #[arg(long, default_value = "audit")]
        dir: PathBuf,
    },
    /// A detector's confusion matrix against the labelled set: fires on
    /// labelled-bad, misses, silence on labelled-good, false alarms — with
    /// every cell's tools named so a human can check the disagreements.
    Calibrate {
        /// Which detector, by registered name. Omit to calibrate all of
        /// them.
        #[arg(long)]
        detector: Option<String>,
        #[arg(long, default_value_t = 2)]
        seed: u64,
        #[arg(long, default_value = "audit")]
        dir: PathBuf,
        /// The corpus root holding the audited tools' fixtures.
        #[arg(long, default_value = "corpus")]
        corpus_dir: PathBuf,
        /// The fixture directory name to replay under each tool — the
        /// audit's own staged fixtures live at
        /// `corpus/<tool>/audit-seed2/`.
        #[arg(long, default_value = "audit-seed2")]
        fixture_version: String,
    },
    /// Re-run a detector's own hand-built cases: the defective shape built
    /// directly, plus the correct parses that resemble it.
    ///
    /// This is the evidence that tells a *repaired* family apart from a
    /// *broken* detector (spec §13.1e) — a distinction the fleet-wide count
    /// cannot make, since both read zero. It is also the half of
    /// `coverage --check`'s ratchet gate that needs no `PATH` sweep: it
    /// spawns nothing, reads no fixture, and runs in a second.
    SelfCheck {
        /// Which detector, by registered name. Omit to check all of them.
        #[arg(long)]
        detector: Option<String>,
    },
}

#[derive(Subcommand)]
enum AuditAction {
    /// Sweep `PATH` once, classify every tool, and write the
    /// shuffle-stratified frozen queue (`<dir>/queue.toml`) plus its
    /// captured raw bytes (`<dir>/queue-captures/`, gitignored) that
    /// `sample` draws from and `reclassify` replays — see `crate::queue`'s
    /// own doc comment for the full design. This is the ~20-minute,
    /// PATH-probing step; run it once, not on every draw.
    Freeze {
        /// Seed for the shuffle-stratification (`crate::queue::shuffle_stratify`)
        /// that decides the queue's cursor order. Distinct from `sample`'s
        /// `--seed`, which only names a verdict file — see `crate::queue`'s
        /// doc comment.
        #[arg(long)]
        seed: u64,
        /// Freeze this fixed, comma-separated list instead of scanning
        /// `PATH` — pins a reproducible population, which is what tests and
        /// CI use (mirrors `coverage --tools`).
        #[arg(long, value_delimiter = ',')]
        tools: Option<Vec<String>>,
        /// Directory holding the queue (`<dir>/queue.toml`) and its
        /// captures (`<dir>/queue-captures/`).
        #[arg(long, default_value = "audit")]
        dir: PathBuf,
        /// Skip probing entirely: just hash the current `PATH` population
        /// and report whether it still matches the existing queue's,
        /// without writing anything. Mirrors `coverage --check`.
        #[arg(long)]
        check: bool,
        /// Same escape hatch as `coverage --allow-uncontained`, and the
        /// same default: a full-`PATH` freeze (no `--tools`, and not
        /// `--check`, which probes nothing) is namespace-contained and
        /// canary-seeded unless this is passed.
        #[arg(long)]
        allow_uncontained: bool,
    },
    /// Advance `<dir>/queue.toml`'s cursor by `--sample` tools and
    /// write/merge them into a resumable verdict file at
    /// `<dir>/<seed>.toml`. Requires a queue built by `freeze` first — this
    /// no longer sweeps `PATH` or reclassifies anything itself.
    Sample {
        /// Names the verdict file (`<dir>/<seed>.toml`) this draw is merged
        /// into. No longer a draw seed — the draw's only randomness was
        /// already spent once, at `freeze` time.
        #[arg(long)]
        seed: u64,
        /// How many tools to draw from the queue's current cursor.
        #[arg(long)]
        sample: usize,
        /// Directory holding the queue (`<dir>/queue.toml`) and verdict
        /// files (`<dir>/<seed>.toml`).
        #[arg(long, default_value = "audit")]
        dir: PathBuf,
        /// A plain-text file of `<tool> <reason...>` lines (`#` comments and
        /// blank lines ignored, same convention as `ingest --verdicts`)
        /// naming tools to include in the sample *unconditionally*, on top
        /// of the queue draw. The motivating case: 14 tools an unaudited
        /// heuristic (commit `3464b0c`) promoted `low-confidence` -> `ok`
        /// mid-freeze, identified via `xtask sweep-diff` — independent of
        /// the queue's cursor, so they are named explicitly instead of left
        /// to chance.
        #[arg(long)]
        force_include_file: Option<PathBuf>,
    },
    /// Recompute every queued tool's stratum against the *current* parser
    /// from the bytes `freeze` already captured — no `PATH` sweep, no
    /// subprocess spawned, run in parallel across the queue. Reports
    /// transitions and the wall-clock cost (measured: roughly half of a
    /// live re-probe's time on this batch's evaluation machine, see
    /// `crate::queue::cmd_reclassify`'s doc comment for the honest number).
    Reclassify {
        /// Directory holding the queue (`<dir>/queue.toml`) and its
        /// captures (`<dir>/queue-captures/`).
        #[arg(long, default_value = "audit")]
        dir: PathBuf,
        /// Write the recomputed strata back into `queue.toml` in place.
        /// Without this, the command only reports what would change — the
        /// queue's order and cursor are never touched either way.
        #[arg(long)]
        update: bool,
    },
    /// The interactive review loop: raw `--help` text and the parsed tree,
    /// side by side, one verdict at a time. Reads `<word> [note...]` lines
    /// from stdin and saves after every tool, so an interrupted session
    /// resumes rather than restarts.
    Review {
        #[arg(long)]
        seed: u64,
        #[arg(long, default_value = "audit")]
        dir: PathBuf,
    },
    /// Non-interactive twin of `review`: write every still-pending tool's
    /// raw text + parsed tree to its own file under `--emit-dir`, for a
    /// reviewer (or a machine with no tty) to read offline. Pair with
    /// `ingest` to apply the resulting verdicts.
    Emit {
        #[arg(long)]
        seed: u64,
        #[arg(long, default_value = "audit")]
        dir: PathBuf,
        #[arg(long)]
        emit_dir: PathBuf,
    },
    /// Apply a plain-text verdicts file (`<tool> <verdict> [note...]` per
    /// line, `#` comments and blank lines ignored) to a sample — the
    /// counterpart to `emit`, and how a review gets recorded on a machine
    /// with no tty at all.
    Ingest {
        #[arg(long)]
        seed: u64,
        #[arg(long, default_value = "audit")]
        dir: PathBuf,
        /// The verdicts file to read.
        #[arg(long)]
        verdicts: PathBuf,
        /// Replace an already-recorded verdict instead of leaving it
        /// alone. Without this, re-running `ingest` on a file that
        /// includes already-applied lines is a safe no-op for those lines.
        #[arg(long)]
        overwrite: bool,
    },
    /// Per-stratum and overall accuracy, each stated as a count and a
    /// confidence interval — never a bare percentage — plus the list of
    /// tools judged `wrong` or `incomplete`.
    Report {
        #[arg(long)]
        seed: u64,
        #[arg(long, default_value = "audit")]
        dir: PathBuf,
    },
    /// Turn every reviewed tool into a `corpus/README.md`-shaped fixture:
    /// capture files, a pre-filled `meta.toml`, `expected.snap` for a
    /// `correct` verdict, `[xfail]` with the reviewer's note as `reason`
    /// for `wrong`/`incomplete`. Stages into `<dir>/<seed>/fixtures` by
    /// default rather than the gated `corpus/` tree — see `cmd_fixtures`'s
    /// doc comment for why.
    Fixtures {
        #[arg(long)]
        seed: u64,
        #[arg(long, default_value = "audit")]
        dir: PathBuf,
        /// Where to write fixture directories. Defaults to a staging area
        /// under `--dir`; pass `corpus` explicitly to write straight into
        /// the gated corpus (only once every `[xfail]` fixture has a real
        /// falsifying `[contract]` field — see `cmd_fixtures`).
        #[arg(long)]
        corpus_dir: Option<PathBuf>,
        /// Only emit fixtures for these tools (comma-separated) instead of
        /// every reviewed entry.
        #[arg(long, value_delimiter = ',')]
        only: Option<Vec<String>>,
        /// Overwrite an already-existing fixture directory.
        #[arg(long)]
        force: bool,
    },
    /// Correct one already-recorded verdict without destroying it: the
    /// original verdict and note stay exactly as reviewed, and a new
    /// `[[amendments]]` entry records what it became and why. Aggregate
    /// computation (`report`, `fixtures`) uses the amended value; a plain
    /// read of the TOML still shows the original. Requires a reason
    /// (distinct from `--note`, which is the note attached to the new
    /// verdict, obligatory only when the new verdict is `wrong`/
    /// `incomplete` — same rule an ordinary verdict follows). See
    /// `mandible_core::audit::amend`'s doc comment for the full mechanism.
    Amend {
        #[arg(long)]
        seed: u64,
        #[arg(long, default_value = "audit")]
        dir: PathBuf,
        /// The tool whose verdict is being corrected.
        #[arg(long)]
        tool: String,
        /// The corrected verdict (`c`/`correct`, `i`/`incomplete`,
        /// `w`/`wrong`, `s`/`skip`).
        #[arg(long)]
        verdict: String,
        /// The note attached to the corrected verdict. Required when
        /// `--verdict` is `wrong`/`incomplete`, same obligation an ordinary
        /// verdict carries.
        #[arg(long)]
        note: Option<String>,
        /// Why the original verdict was wrong and is being corrected.
        /// Always required — an amendment with nothing recorded about why
        /// is exactly the unauditable change this command exists to
        /// prevent.
        #[arg(long)]
        reason: String,
    },
    /// Spot-audit one mass-`ok` promotion event (spec §13.1b's sixth rule:
    /// "any change that promotes more than a handful of tools to `ok` must
    /// include a spot-audit of 5-10 randomly drawn promoted tools, recorded
    /// in the audit manifest as its own stratum"). Draws `--sample` tools
    /// at random — via a seeded, reproducible shuffle, never hand-picked —
    /// from `--promoted`'s own tool list, classifies each with one fresh
    /// extraction pass, and records them in `<dir>/<seed>.toml` as their
    /// own `spot-audit:<event>` stratum, reported by `report` alongside the
    /// ordinary parse-status strata and `forced-inclusion`.
    SpotAudit {
        /// Names the verdict file (`<dir>/<seed>.toml`) this draw is merged
        /// into — same meaning as `sample`'s `--seed`.
        #[arg(long)]
        seed: u64,
        #[arg(long, default_value = "audit")]
        dir: PathBuf,
        /// Names this promotion event; becomes the reported stratum label
        /// `spot-audit:<event>`.
        #[arg(long)]
        event: String,
        /// The tools this event actually promoted — the population this
        /// spot-check draws from, never the whole fleet.
        #[arg(long, value_delimiter = ',')]
        promoted: Vec<String>,
        /// How many to draw. Spec §13.1b's sixth rule asks for 5-10. If
        /// `--promoted` names fewer tools than this, every one of them is
        /// audited and the shortfall is reported explicitly — never a
        /// silently smaller sample, never a padded count.
        #[arg(long, default_value_t = 8)]
        sample: usize,
        /// Seed for the reproducible random draw over `--promoted`, mixed
        /// with `--event` (via `crate::rng::stratum_seed`) so two
        /// promotion events never share a correlated draw pattern.
        #[arg(long)]
        draw_seed: u64,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Coverage {
            check,
            out,
            tools,
            shard,
            progress,
            format,
            allow_uncontained,
        } => {
            let shard = shard.as_deref().map(parse_shard).transpose()?;
            run_coverage(
                check,
                &out,
                tools,
                shard,
                progress,
                format,
                allow_uncontained,
            )
        }
        Command::Corpus {
            bless,
            dir,
            format,
            baseline_dir,
            show,
        } => match show {
            Some(pattern) => corpus::show_fixture(&dir, &pattern),
            None => run_corpus(bless, &dir, format, baseline_dir.as_deref()),
        },
        Command::SweepDiff {
            before,
            after,
            out,
            format,
        } => run_sweep_diff(&before, &after, out.as_deref(), format),
        Command::Audit { action } => run_audit(action),
        Command::Detector { action } => match action {
            DetectorAction::List { seed, dir } => detector::cmd_list(&dir, seed),
            DetectorAction::Calibrate {
                detector: name,
                seed,
                dir,
                corpus_dir,
                fixture_version,
            } => {
                detector::cmd_calibrate(&dir, seed, &corpus_dir, &fixture_version, name.as_deref())
            }
            DetectorAction::SelfCheck { detector: name } => {
                detector::cmd_self_check(name.as_deref())
            }
        },
    }
}

fn run_audit(action: AuditAction) -> anyhow::Result<()> {
    match action {
        AuditAction::Freeze {
            seed,
            tools,
            dir,
            check,
            allow_uncontained,
        } => {
            // `--check` probes nothing (see `queue::cmd_freeze`'s early
            // return): only a real, tools-unbounded sweep needs
            // containment.
            let is_full_path_sweep = tools.is_none() && !check;
            let canaries = sweep_guard(is_full_path_sweep, allow_uncontained)?;
            let freeze_result = queue::cmd_freeze(seed, tools, &dir, check);
            finish_sweep_guard(canaries)?;
            freeze_result
        }
        AuditAction::Sample {
            seed,
            sample,
            dir,
            force_include_file,
        } => {
            let force_include = match force_include_file {
                Some(path) => audit::load_force_include(&path)?,
                None => Vec::new(),
            };
            queue::cmd_sample(seed, sample, &dir, &force_include)
        }
        AuditAction::Reclassify { dir, update } => queue::cmd_reclassify(&dir, update),
        AuditAction::Review { seed, dir } => {
            let stdin = std::io::stdin();
            let mut input = stdin.lock();
            let mut output = std::io::stdout();
            audit::cmd_review(&dir, seed, &mut input, &mut output)
        }
        AuditAction::Emit {
            seed,
            dir,
            emit_dir,
        } => audit::cmd_emit(&dir, seed, &emit_dir),
        AuditAction::Ingest {
            seed,
            dir,
            verdicts,
            overwrite,
        } => audit::cmd_ingest(&dir, seed, &verdicts, overwrite),
        AuditAction::Report { seed, dir } => audit::cmd_report(&dir, seed),
        AuditAction::Fixtures {
            seed,
            dir,
            corpus_dir,
            only,
            force,
        } => {
            let corpus_dir =
                corpus_dir.unwrap_or_else(|| dir.join(seed.to_string()).join("fixtures"));
            audit::cmd_fixtures(&dir, seed, &corpus_dir, only, force)
        }
        AuditAction::Amend {
            seed,
            dir,
            tool,
            verdict,
            note,
            reason,
        } => audit::cmd_amend(&dir, seed, &tool, &verdict, note, reason),
        AuditAction::SpotAudit {
            seed,
            dir,
            event,
            promoted,
            sample,
            draw_seed,
        } => audit::cmd_spot_audit(&dir, seed, &event, &promoted, sample, draw_seed),
    }
}

fn run_corpus(
    bless: bool,
    dir: &std::path::Path,
    format: ScoreFormat,
    baseline_dir: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    // `corpus::run` (no baseline) stays the default path, unchanged from
    // before `--baseline-dir` existed; `run_with_baseline` is only reached
    // when a caller actually asks for the contract-weakening check.
    let report = match baseline_dir {
        Some(baseline) => corpus::run_with_baseline(dir, bless, format, Some(baseline))?,
        None => corpus::run(dir, bless, format)?,
    };
    println!("{}", report.text);
    if report.failed() {
        anyhow::bail!(
            "corpus regression: {} fixture(s) failed — see above",
            report.failures.len()
        );
    }
    Ok(())
}

/// Read and diff two scoreboards (`xtask sweep-diff`, WS2 part 1). Always
/// exits `0` on a clean read of both files — see `transition.rs`'s doc
/// comment on why this is non-blocking by construction (maintainer decision
/// D4), not by a flag a caller has to remember to omit.
fn run_sweep_diff(
    before: &std::path::Path,
    after: &std::path::Path,
    out: Option<&std::path::Path>,
    format: ScoreFormat,
) -> anyhow::Result<()> {
    let before_text = std::fs::read_to_string(before).map_err(|e| {
        anyhow::anyhow!(
            "could not read --before scoreboard at {}: {e}",
            before.display()
        )
    })?;
    let after_text = std::fs::read_to_string(after).map_err(|e| {
        anyhow::anyhow!(
            "could not read --after scoreboard at {}: {e}",
            after.display()
        )
    })?;

    let before_parsed = transition::parse_scoreboard(&before_text);
    let after_parsed = transition::parse_scoreboard(&after_text);
    let t = transition::diff(&before_parsed, &after_parsed);

    let rendered = match format {
        ScoreFormat::Text => transition::render_text(&t),
        ScoreFormat::Markdown => transition::render_markdown(&t),
    };
    println!("{rendered}");

    if let Some(out) = out {
        write_out(out, &rendered, "report")?;
        println!("wrote {}", out.display());
    }
    Ok(())
}

/// Parse an `INDEX/TOTAL` shard spec, rejecting the off-by-one mistakes
/// that would silently drop or duplicate tools.
fn parse_shard(spec: &str) -> anyhow::Result<(usize, usize)> {
    let (index, total) = spec
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("--shard must look like INDEX/TOTAL, e.g. 0/8"))?;
    let index: usize = index.trim().parse()?;
    let total: usize = total.trim().parse()?;
    if total == 0 {
        anyhow::bail!("--shard TOTAL must be greater than zero");
    }
    if index >= total {
        anyhow::bail!("--shard INDEX ({index}) must be less than TOTAL ({total})");
    }
    Ok((index, total))
}

/// Guard the front of a full-`PATH` sweep entrypoint (`coverage`, `audit
/// freeze`): namespace-contain it by default and seed the three canary
/// tripwires, refusing to run uncontained unless `allow_uncontained` was
/// passed explicitly.
///
/// `is_full_path_sweep` is `false` for a `--tools`-pinned run (a fixed,
/// small, reviewed list — what CI and tests use) or a `--check`-only dry
/// run that probes nothing; neither needs containment, and `Ok(None)` is
/// returned immediately for both, matching every earlier caller's
/// unguarded behavior exactly.
///
/// **This function does not itself decide whether the current process
/// already is the contained one.** It asks
/// `mandible_extract::exec::containment::is_contained()`. The *first*
/// invocation of `xtask coverage`/`xtask audit freeze` (no sentinel env
/// var set) falls through to `containment::enter_or_refuse()`, which —
/// when namespaces are available — replaces this process's image via
/// `exec` and never returns; control lands back here a second time, in a
/// freshly re-exec'd process, with the sentinel now set, and this
/// function's first branch fires instead.
fn sweep_guard(
    is_full_path_sweep: bool,
    allow_uncontained: bool,
) -> anyhow::Result<Option<mandible_extract::exec::canary::CanarySet>> {
    use mandible_extract::exec::{canary::CanarySet, containment};

    if !is_full_path_sweep {
        return Ok(None);
    }

    if containment::is_contained() {
        let watch_dir = containment::default_watch_dir()
            .map_err(|e| anyhow::anyhow!("failed to create canary watch directory: {e}"))?;
        let set = CanarySet::spawn(watch_dir)
            .map_err(|e| anyhow::anyhow!("failed to spawn canary tripwires: {e}"))?;
        println!(
            "sweep is namespace-contained; canary tripwires armed (pty={:?})",
            set.pty_slave_path()
        );
        return Ok(Some(set));
    }

    if allow_uncontained {
        eprintln!(
            "WARNING: running a full-PATH sweep WITHOUT namespace containment or canary \
             tripwires (--allow-uncontained was passed). Evidence-before-argv gating (spec §6) \
             is still in effect — this does not loosen what argv a probe can be sent — but the \
             containment and detection layers this project's third safety layer adds are not \
             present for this run. See spec.md §6/§8 and \
             mandible_extract::exec::containment's doc comment."
        );
        return Ok(None);
    }

    let err = containment::enter_or_refuse();
    anyhow::bail!(
        "refusing to run a full-PATH sweep uncontained: {err}\n\
         Namespace containment (user+PID+mount, spec §6/§8) is the default for a full-PATH \
         sweep — a coverage/audit run invokes --help on every executable on PATH, and no \
         static check can enumerate what those binaries do. Pass --allow-uncontained to run \
         without it anyway (not recommended off CI/a disposable machine), or run somewhere \
         `unshare --user --map-root-user --pid --mount` works."
    )
}

/// Check every canary after a contained sweep, tear them all down
/// regardless of the outcome (so a tripped canary's own processes never
/// leak past this function), and fail loudly if anything tripped.
///
/// A no-op — correctly — when `canaries` is `None`: either this was not a
/// full-`PATH` sweep, or the operator explicitly passed
/// `--allow-uncontained`, and either way there was nothing to check.
fn finish_sweep_guard(
    canaries: Option<mandible_extract::exec::canary::CanarySet>,
) -> anyhow::Result<()> {
    let Some(mut set) = canaries else {
        return Ok(());
    };
    let trips = set.check();
    set.teardown();
    if trips.is_empty() {
        println!("canary tripwires: none fired");
        return Ok(());
    }
    for trip in &trips {
        eprintln!("CANARY TRIPPED: {trip}");
    }
    anyhow::bail!(
        "{} canary tripwire(s) fired during the sweep — a probed tool had a real side effect \
         the namespace did not prevent (see mandible_extract::exec::canary's doc comment for \
         what each canary catches)",
        trips.len()
    );
}

fn run_coverage(
    check: bool,
    out: &PathBuf,
    tools: Option<Vec<String>>,
    shard: Option<(usize, usize)>,
    progress: bool,
    format: ScoreFormat,
    allow_uncontained: bool,
) -> anyhow::Result<()> {
    let is_full_path_sweep = tools.is_none();
    let canaries = sweep_guard(is_full_path_sweep, allow_uncontained)?;

    let (table, fresh) = match tools {
        Some(tools) => {
            println!(
                "scanning a fixed list of {} tool(s): {}...",
                tools.len(),
                tools.join(", ")
            );
            coverage::run_over(tools, shard, progress, format)
        }
        None => {
            println!("scanning PATH and running the extraction pipeline against every executable found...");
            coverage::run(shard, progress, format)
        }
    };

    // Check and tear down the canaries right after the sweep that could
    // have tripped them, before this function's own regression-check
    // logic below (an unrelated concern) — a tripped canary is reported
    // immediately rather than buried after a possibly-long `--check` diff.
    finish_sweep_guard(canaries)?;

    println!("{table}");
    println!(
        "aggregate: {:.2}% of flags carry text across {} tools (accuracy: unmeasured), {} with no tier, {} suspicious, {} verbatim, {} man-shaped, {} ok-with-zero-flags, {} misattribution-suspect, {} existence-fabrication, {} bundle-collapse ({} real flags destroyed), {}/{} framework-detected",
        fresh.pct_flags_with_text,
        fresh.total,
        fresh.no_tier_count,
        fresh.suspicious_count,
        fresh.verbatim_count,
        fresh.man_shaped_count,
        fresh.zero_flag_ok_count,
        fresh.misattribution_suspect_tools,
        fresh.existence_fabrication_tools,
        fresh.bundle_collapse_tools,
        fresh.bundle_destroyed_flags,
        fresh.framework_detected_count,
        fresh.total,
    );

    if check {
        let previous_text = std::fs::read_to_string(out).map_err(|e| {
            anyhow::anyhow!(
                "could not read checked-in scoreboard at {}: {e}",
                out.display()
            )
        })?;
        let previous = coverage::parse_aggregate_footer(&previous_text).ok_or_else(|| {
            anyhow::anyhow!(
                "checked-in scoreboard at {} has no parseable aggregate footer",
                out.display()
            )
        })?;

        println!(
            "previous: {:.2}% of flags carried text across {} tools, {} with no tier, {} suspicious, {} verbatim, {} man-shaped, {} ok-with-zero-flags",
            previous.pct_flags_with_text,
            previous.total,
            previous.no_tier_count,
            previous.suspicious_count,
            previous.verbatim_count,
            previous.man_shaped_count,
            previous.zero_flag_ok_count,
        );

        let mut regressed = false;
        if fresh.pct_flags_with_text + 0.01 < previous.pct_flags_with_text {
            println!(
                "REGRESSION: %flags_text dropped from {:.2}% to {:.2}%",
                previous.pct_flags_with_text, fresh.pct_flags_with_text
            );
            regressed = true;
        }
        if fresh.no_tier_count > previous.no_tier_count {
            println!(
                "REGRESSION: no-tier count grew from {} to {}",
                previous.no_tier_count, fresh.no_tier_count
            );
            regressed = true;
        }
        // Gated exactly like no_tier_count (spec §13.1): a metric that
        // can be gamed by the failure mode it's meant to detect is worse
        // than no metric — [M-10] shipped as 100% "described" (this
        // column's old name) while 39 of tar's 40 nodes were fabricated,
        // so `%flags_text` alone must never be the only gate.
        if fresh.suspicious_count > previous.suspicious_count {
            println!(
                "REGRESSION: suspicious count grew from {} to {}",
                previous.suspicious_count, fresh.suspicious_count
            );
            regressed = true;
        }
        // `verbatim_count` is intentionally NOT gated here (spec §13.1,
        // batch 6 part 5): a correct new framework grammar can legitimately
        // move a tool from fabricated structure to honest verbatim, and
        // failing the build on that would block exactly the improvement
        // this whole batch is about. Reported for visibility only.
        if fresh.verbatim_count != previous.verbatim_count {
            println!(
                "verbatim count changed from {} to {} (reported, not gated)",
                previous.verbatim_count, fresh.verbatim_count
            );
        }
        // `man_shaped_count` (spec [M-16]'s exposure enumeration for the
        // pending `-h`-fallback decision) is a brand-new measurement with
        // no baseline to regress against, so — like `verbatim_count` — it
        // is reported for visibility only and never gated.
        if fresh.man_shaped_count != previous.man_shaped_count {
            println!(
                "man-shaped count changed from {} to {} (reported, not gated)",
                previous.man_shaped_count, fresh.man_shaped_count
            );
        }
        // `zero_flag_ok_count` ([M-15]) is deliberately **not** gated, and
        // deliberately reported even though `pct_flags_with_text` already is:
        // [M-15]'s whole point is that a synopsis-only flag grammar makes
        // `pct_flags_with_text` fall (a usage-only flag adds to the denominator
        // with no description to add to the numerator) at the exact moment
        // real recall improves. A gate on `pct_flags_with_text` alone therefore
        // rewards *not* fixing this, which is the metric trap this column
        // exists to make visible instead of silently blocking. This count
        // falling is the actual signal that a fix like this one worked;
        // `pct_flags_with_text` falling alongside it is the expected, correct
        // cost, not a second regression.
        if fresh.zero_flag_ok_count != previous.zero_flag_ok_count {
            println!(
                "ok-with-zero-flags count changed from {} to {} (reported, not gated — spec [M-15]: this falling, not pct_flags_with_text, is the real success signal)",
                previous.zero_flag_ok_count, fresh.zero_flag_ok_count
            );
        }
        // `misattribution_suspect_tools` (this task's own instrument,
        // `crate::misattribution`) is deliberately **not gated**: it is a
        // brand-new detector with a measured, nonzero false-positive rate
        // and no fleet-wide baseline to regress against yet — see that
        // module's doc comment. Reported so a grammar change's effect on
        // it is visible, exactly like `verbatim_count`/`man_shaped_count`
        // above.
        if fresh.misattribution_suspect_tools != previous.misattribution_suspect_tools {
            println!(
                "misattribution-suspect tool count changed from {} to {} (reported, not gated)",
                previous.misattribution_suspect_tools, fresh.misattribution_suspect_tools
            );
        }
        // `existence_fabrication_tools` (`crate::existence`, this task's own
        // instrument — the twin of `misattribution_suspect_tools` above, a
        // different check with a different victim: does a name/spelling the
        // help-text tier emitted actually occur in the tool's own raw
        // output, rather than whether an attached description belongs to
        // the right flag) is deliberately **not gated**, for the identical
        // reason: a brand-new detector with no fleet-wide baseline must not
        // fail a build the first time it runs (spec §13.1b). Reported so a
        // grammar change's effect on it is visible.
        if fresh.existence_fabrication_tools != previous.existence_fabrication_tools {
            println!(
                "existence-fabrication tool count changed from {} to {} (reported, not gated)",
                previous.existence_fabrication_tools, fresh.existence_fabrication_tools
            );
        }
        // `bundle_collapse_tools`/`bundle_destroyed_flags`
        // (`crate::bundling`, the third oracle) used to be reported and not
        // gated, because the numbers were expected to move the moment the
        // synopsis grammar learned to split a bundle — and that movement was
        // the fix landing, not a regression.
        //
        // **The fix landed** (`help_text::grammar::parse_bundled_shorts`;
        // 58 tools / 465 destroyed flags -> 0 / 0 fleet-wide), so the
        // number is now ratcheted at zero instead. It is gated against a
        // literal 0 rather than against `previous`: the checked-in
        // scoreboard is itself editable, so gating on it would let a commit
        // that reintroduced the defect raise its own baseline.
        //
        // The gate is deliberately NOT `count == 0` on its own — that is
        // satisfied by deleting the detector, and it is the exact metric
        // trap spec §13.1b records twice. `detector::ratchet_at_zero` also
        // requires the detector's own hand-built self-checks to still hold,
        // which is the evidence that a zero means "repaired" rather than
        // "broken".
        if fresh.bundle_collapse_tools != previous.bundle_collapse_tools
            || fresh.bundle_destroyed_flags != previous.bundle_destroyed_flags
        {
            println!(
                "bundled-short-flag collapse changed from {} tool(s)/{} destroyed flag(s) to {} tool(s)/{} destroyed flag(s)",
                previous.bundle_collapse_tools,
                previous.bundle_destroyed_flags,
                fresh.bundle_collapse_tools,
                fresh.bundle_destroyed_flags,
            );
        }
        let ratchet = detector::ratchet_at_zero(
            detector::find("bundled-short-flag")?.as_ref(),
            fresh.bundle_collapse_tools,
            fresh.bundle_destroyed_flags,
        );
        println!("\n{}", ratchet.report());
        if !ratchet.holds() {
            regressed = true;
        }
        if regressed {
            anyhow::bail!("coverage regression detected — see above");
        }
        println!("no regression.");
        return Ok(());
    }

    write_out(out, &table, "scoreboard")?;
    println!("wrote {}", out.display());
    Ok(())
}

/// Write `contents` to `out`, creating `out`'s parent directory first.
///
/// `std::fs::write` does not create intermediate directories, so without
/// this a perfectly reasonable `--out tmp/scoreboard.txt` fails on any
/// checkout that doesn't already happen to have a `tmp/`. That matters now
/// that the default `--out` *is* under `tmp/` (see `Coverage::out`): a fresh
/// clone has no such directory, and the sweep would die at the very end,
/// after twenty minutes of work, with nothing written.
fn write_out(out: &std::path::Path, contents: &str, what: &str) -> anyhow::Result<()> {
    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
                anyhow::anyhow!("failed to create {} for {what}: {e}", parent.display())
            })?;
        }
    }
    std::fs::write(out, contents)
        .map_err(|e| anyhow::anyhow!("failed to write {what} to {}: {e}", out.display()))
}
