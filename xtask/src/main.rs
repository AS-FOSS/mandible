//! Developer tasks for the mandible workspace: the extraction coverage
//! harness (spec §13.1).

#![forbid(unsafe_code)]
// Size ceilings. Both lints are opt-in; the thresholds live in the
// workspace `clippy.toml`, and CI's `-D warnings` makes them gates.
#![warn(clippy::too_many_lines)]
#![warn(clippy::cognitive_complexity)]

mod alternation;
mod audit;
mod audit_contribute;
mod bundling;
mod commandtable;
mod corpus;
mod coverage;
mod detector;
mod dropped_alias;
mod existence;
mod misattribution;
mod queue;
mod ragged_command_table;
mod repeated_char;
mod residue;
mod rng;
mod single_dash_long;
mod status;
mod tail_operand;
mod transition;
mod wrapped_command_continuation;
mod wrapped_prose;

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
        /// Not a dry run: probes every tool on `PATH` again (the same
        /// ~20-minute full sweep as a bare `coverage` run), then compares
        /// the freshly computed aggregate against the checked-in
        /// scoreboard and fails if `%flags_text` dropped, `no-tier` grew,
        /// or `suspicious` grew (spec §13.1). `verbatim`, framework
        /// detection, and `misattribution_suspect_tools` are reported but
        /// not gated. Without this flag, the command just (re)writes the
        /// scoreboard. For a check that spawns nothing, use `audit freeze
        /// --check` or `detector self-check`.
        #[arg(long)]
        check: bool,
        /// Where to read/write the scoreboard.
        ///
        /// Defaults under `tmp/` (gitignored): a full-PATH scoreboard is a
        /// snapshot of one machine's installed tools, never a portable
        /// baseline (spec §13.1a). `coverage-scoreboard.ci.txt`, the one
        /// scoreboard that is a baseline, stays checked in at the root,
        /// which `--check` diffs against.
        #[arg(long, default_value = "tmp/coverage-scoreboard.txt")]
        out: PathBuf,
        /// Scan only this comma-separated list of tool names instead of
        /// every executable on `PATH`. Pins a fixed, reproducible
        /// inventory — what CI uses, since the full-`PATH` tool set varies
        /// with the runner image.
        #[arg(long, value_delimiter = ',')]
        tools: Option<Vec<String>>,
        /// Scan only one slice of the tool list, as `INDEX/TOTAL` (e.g.
        /// `0/8`). Sliced by stride, not contiguous block, so tools that
        /// cluster alphabetically (e.g. many same-family binaries) don't
        /// pile into one slow shard.
        #[arg(long)]
        shard: Option<String>,
        /// Print each tool's name to stderr before probing it, to identify
        /// which tool killed a run when the host itself gets disrupted
        /// mid-sweep.
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
        /// fresh user/PID/mount namespace
        /// (`mandible_extract::exec::containment`) and seeds three canary
        /// tripwires, refusing to run if the host cannot provide all three
        /// namespace types (no network containment either way). A
        /// `--tools`-pinned run is never gated by this.
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
        /// Output format for a checking run (ignored with `--bless`):
        /// fixed per-fixture `text` lines, or GitHub-flavored `markdown` —
        /// a semantic before/after transition report, never a raw
        /// `expected.snap` diff. Corpus CI writes this to
        /// `$GITHUB_STEP_SUMMARY`, same convention as `coverage --format`.
        #[arg(long, value_enum, default_value = "text")]
        format: ScoreFormat,
        /// A second, plain corpus directory (never a git ref) to diff every
        /// fixture's `[contract]` against, printing a `CONTRACT WEAKENED:
        /// <fixture> <field>` line for every field that got weaker. This
        /// binary has no git access, so populate the directory however you
        /// like — a CI step running `git archive <base-ref> corpus | tar
        /// -x -C <dir>` is the intended one. Reported, never gated. Omit
        /// it and nothing about this run changes.
        #[arg(long)]
        baseline_dir: Option<PathBuf>,
        /// Print one fixture's captured help text beside the tree the
        /// parser makes of it, and exit without checking anything.
        /// Matches on a substring of `<tool>/<version>`.
        #[arg(long)]
        show: Option<String>,
    },
    /// A semantic per-tool diff between two coverage scoreboards
    /// (`transition.rs`): status transitions, flag-count gains/losses
    /// (reported separately, never netted), and tools appearing or
    /// disappearing. Reads two already-rendered `ScoreFormat::Text`
    /// scoreboards (e.g. two `cargo xtask coverage --out <path>` runs
    /// before/after a change), never a raw text diff of the files.
    ///
    /// Non-blocking (maintainer decision D4): never fails the run, no
    /// `--check`-style flag here.
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
    /// raw captured `--help` text against the parsed tree (`audit.rs`).
    /// The first instrument that measures agreement with truth, not with
    /// the parser's own prior output.
    Audit {
        #[command(subcommand)]
        action: AuditAction,
    },
    /// The family-detector calibration harness (`detector.rs`): checks a
    /// fleet-wide defect detector against the audit's labelled tools — it
    /// must fire on the known-bad and stay silent on the known-good.
    /// A detector's fleet-wide number is not quotable until it has passed
    /// (spec §13.1e).
    Detector {
        #[command(subcommand)]
        action: DetectorAction,
    },
    /// Rank captured `--help` documents by how much structurally
    /// interesting text the parse left on the table — the complement of
    /// the existence oracle (invention vs. omission). Replays
    /// `corpus/`-shaped fixture directories from frozen bytes, spawning
    /// nothing.
    ///
    /// A discovery instrument, never a gate: emits a reading queue for a
    /// human, who turns a confirmed finding into a calibrated,
    /// ratchet-gated rule elsewhere. No `--check` here (spec §13.1f).
    Residue {
        /// The fixture root to rank. Any directory of
        /// `<tool>/<version>/meta.toml` fixtures — `corpus/` itself, or
        /// `xtask audit fixtures`' staging output under `tmp/`.
        #[arg(long, default_value = "corpus")]
        dir: PathBuf,
        /// How many ranked tools to list.
        #[arg(long, default_value_t = 25)]
        top: usize,
        /// How many of the top entries to print block-level evidence for.
        #[arg(long, default_value_t = 10)]
        detail: usize,
        /// Also rank tools whose parse produced no structure at all
        /// (`verbatim`). Off by default: that is a status the scoreboard
        /// already counts, and every such tool trivially leaves its whole
        /// document unaccounted, which would fill the list.
        #[arg(long)]
        include_verbatim: bool,
        /// Verdict directory to cross-reference (`<dir>/<seed>.toml`).
        /// When the file exists, the run also reports how the ranking
        /// lines up against the recorded human verdicts — the only honest
        /// way to find out whether this signal separates anything.
        #[arg(long, default_value = "audit")]
        audit_dir: PathBuf,
        #[arg(long, default_value_t = 2)]
        seed: u64,
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
    /// The one-command audit submission flow (CONTRIBUTING.md §2 / README's
    /// "Contributing" section): prompts for a GitHub login, freezes a
    /// personal queue under `<dir>/<login>/` excluding already-audited
    /// tools, draws a random sample, and — once every drawn tool has a
    /// verdict — writes `<seed>-report.txt` and prints how to finish the
    /// submission. See `crate::audit_contribute`'s own doc comment for why
    /// it prints those last commands instead of running them.
    Contribute {
        /// Draw this seed instead of one derived from the clock, and reuse
        /// it (rather than resuming whatever unfinished draw is on disk) if
        /// a verdict file for it already exists. Also names the verdict
        /// file (`<dir>/<login>/<seed>.toml`).
        #[arg(long)]
        seed: Option<u64>,
        /// How many tools to draw.
        #[arg(long, default_value_t = 20)]
        sample: usize,
        /// Include tools that already carry a verdict somewhere under
        /// `--dir`, or have a `corpus/<tool>/` fixture, instead of
        /// excluding them from the population.
        #[arg(long)]
        include_audited: bool,
        /// Omit the `gh pr create` line from the printed commands. For
        /// scripts, and for exercising this command without a pull request
        /// command in the output.
        #[arg(long)]
        no_pr: bool,
        /// Root that each contributor's own folder is created under.
        #[arg(long, default_value = "audit/submissions")]
        dir: PathBuf,
        /// The corpus root consulted by the population filter.
        #[arg(long, default_value = "corpus")]
        corpus_dir: PathBuf,
        /// Same escape hatch as `freeze --allow-uncontained`: only
        /// consulted the first time a given login's queue is frozen (a
        /// full-`PATH` sweep).
        #[arg(long)]
        allow_uncontained: bool,
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
        Command::Residue {
            dir,
            top,
            detail,
            include_verbatim,
            audit_dir,
            seed,
        } => residue::run(&dir, top, detail, include_verbatim, &audit_dir, seed),
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
            // `None`: `audit freeze` writes its results under `--dir`
            // (`queue::cmd_freeze`), never through the `--out`-file path
            // this guard's fd-securing exists for.
            let canaries = sweep_guard(is_full_path_sweep, allow_uncontained, None)?;
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
            queue::cmd_sample(seed, sample, &dir, &force_include).map(|_drawn| ())
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
        AuditAction::Contribute {
            seed,
            sample,
            include_audited,
            no_pr,
            dir,
            corpus_dir,
            allow_uncontained,
        } => {
            let stdin = std::io::stdin();
            let mut input = stdin.lock();
            let mut output = std::io::stdout();
            audit_contribute::cmd_contribute(
                &dir,
                &corpus_dir,
                seed,
                sample,
                include_audited,
                no_pr,
                allow_uncontained,
                &mut input,
                &mut output,
            )
        }
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

/// Read and diff two scoreboards (`xtask sweep-diff`, WS2 part 1). Exits
/// `0` on a clean, comparable read of both files — see `transition.rs`'s
/// doc comment on why the *content* comparison is non-blocking by
/// construction (maintainer decision D4), not by a flag a caller has to
/// remember to omit. The one thing that does still fail loudly (nonzero
/// exit, same as an unreadable file) is a `#fp`/`#fp2` fingerprint-format
/// mismatch between the two scoreboards: that isn't a reportable content
/// difference, it's an input the two sides can't be legitimately compared
/// on at all — see `transition::fingerprint_format_mismatch`.
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
    // Refuse a V1/V2 fingerprint-format mismatch outright rather than let
    // `diff` join two differently-shaped entity-identity schemes: see
    // `transition::fingerprint_format_mismatch`'s doc comment for why that
    // join would otherwise misreport every entity as removed on one side
    // and added on the other.
    if let Some(msg) = transition::fingerprint_format_mismatch(&before_parsed, &after_parsed) {
        anyhow::bail!(msg);
    }
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
/// `is_full_path_sweep` is `false` for a `--tools`-pinned run or a
/// `--check`-only dry run that probes nothing; neither needs containment,
/// and `Ok(None)` is returned immediately for both.
///
/// `secure_out`, when `Some`, is a `--out` path the caller will write its
/// result to after the sweep, possibly from inside the namespace. This
/// function opens it before entering containment
/// (`containment::secure_out_file`) and carries the open fd across the
/// re-exec, so the caller writes through it later
/// (`containment::write_scoreboard`) instead of reopening the path from
/// inside the namespace, where it fails `EACCES`. `None` for a caller with
/// no `--out` write pending here (`audit freeze` writes under `--dir`
/// through a different path).
///
/// Does not itself decide whether the current process is already
/// contained — asks `containment::is_contained()`. The first invocation
/// falls through to `containment::enter_or_refuse()` (or
/// `..._with_scoreboard()`), which replaces the process image via `exec`
/// and never returns; control lands back here in the re-exec'd process
/// with the sentinel set.
pub(crate) fn sweep_guard(
    is_full_path_sweep: bool,
    allow_uncontained: bool,
    secure_out: Option<&std::path::Path>,
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
             present for this run. See docs/design.md §6/§8 and \
             mandible_extract::exec::containment's doc comment."
        );
        return Ok(None);
    }

    let err = match secure_out {
        Some(path) => match containment::secure_out_file(path) {
            Ok(file) => containment::enter_or_refuse_with_scoreboard(Some(file)),
            Err(e) => anyhow::bail!(
                "failed to pre-open --out {} before entering namespace containment: {e}",
                path.display()
            ),
        },
        None => containment::enter_or_refuse(),
    };
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
pub(crate) fn finish_sweep_guard(
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

// Ratchet: argument plumbing for the sweep, one branch per flag. Listed in scripts/ratchet.txt.
#[allow(clippy::too_many_lines)]
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
    // `--check` never writes `out` (it only reads the checked-in scoreboard
    // to diff against — see below), so there is nothing to secure for that
    // path; every other run ends with `write_out(out, ...)` below, which is
    // exactly the write that needs `out`'s fd carried across containment.
    let secure_out = if check { None } else { Some(out.as_path()) };
    let canaries = sweep_guard(is_full_path_sweep, allow_uncontained, secure_out)?;

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
        // `zero_flag_ok_count` ([M-15]) is not gated: a gate on
        // `pct_flags_with_text` alone rewards not fixing this, since a
        // recovered synopsis-only flag adds to the denominator with
        // nothing to add to the numerator. This count falling is the real
        // success signal.
        if fresh.zero_flag_ok_count != previous.zero_flag_ok_count {
            println!(
                "ok-with-zero-flags count changed from {} to {} (reported, not gated — spec [M-15]: this falling, not pct_flags_with_text, is the real success signal)",
                previous.zero_flag_ok_count, fresh.zero_flag_ok_count
            );
        }
        // `misattribution_suspect_tools` (`crate::misattribution`) is not
        // gated: a brand-new detector with a nonzero false-positive rate
        // and no fleet-wide baseline yet. Reported for visibility.
        if fresh.misattribution_suspect_tools != previous.misattribution_suspect_tools {
            println!(
                "misattribution-suspect tool count changed from {} to {} (reported, not gated)",
                previous.misattribution_suspect_tools, fresh.misattribution_suspect_tools
            );
        }
        // `existence_fabrication_tools` (`crate::existence`) is not gated,
        // same reason: a brand-new detector with no fleet-wide baseline
        // must not fail a build the first time it runs (spec §13.1b).
        if fresh.existence_fabrication_tools != previous.existence_fabrication_tools {
            println!(
                "existence-fabrication tool count changed from {} to {} (reported, not gated)",
                previous.existence_fabrication_tools, fresh.existence_fabrication_tools
            );
        }
        // `bundle_collapse_tools`/`bundle_destroyed_flags` (`crate::bundling`)
        // is ratcheted at zero, gated against a literal 0 rather than
        // `previous` (the checked-in scoreboard is itself editable, so
        // gating on it would let a regression raise its own baseline).
        // Not `count == 0` alone — satisfied by deleting the detector —
        // `detector::ratchet_at_zero` also requires the self-checks to
        // still hold, evidence that zero means repaired, not broken.
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
        // `alternation_defect_tools`/`alternation_defect_flags`
        // (`crate::alternation`) is reported, not gated, and cannot yet be
        // ratcheted at zero: the residual is `btrfs`, whose `btrfs device
        // scan [-d|--all-devices] <device>...` reads correctly, but
        // `-d`/`--all-devices` belong to a subcommand node that doesn't
        // exist in the tree (`unparsed-subcommand`, a different family
        // that hasn't parsed that catalogue). The only node to hang the
        // flags on is root, and `-d` is not a root flag — asserting it
        // there would be false, not merely incomplete. Same `btrfs`
        // `UnparsedCommandTable` excludes as shape C; whichever family
        // lands first zeroes the other's residual for free. Self-checks
        // still run and print regardless.
        if fresh.alternation_defect_tools != previous.alternation_defect_tools
            || fresh.alternation_defect_flags != previous.alternation_defect_flags
        {
            println!(
                "brace-alternation-flag defects changed from {} tool(s)/{} flag spelling(s) to {} tool(s)/{} flag spelling(s) (reported, not gated)",
                previous.alternation_defect_tools,
                previous.alternation_defect_flags,
                fresh.alternation_defect_tools,
                fresh.alternation_defect_flags,
            );
        }
        let alternation_detector = detector::find("brace-alternation-flag")?;
        let alternation_self_checks = detector::run_self_checks(alternation_detector.as_ref());
        println!(
            "\nbrace-alternation-flag: {} tool(s)/{} flag spelling(s) — REPORTED, NOT GATED (the \
             residual is `btrfs`'s repeated-prefix usage catalogue, whose flags belong to a \
             subcommand node that does not exist; see xtask/src/main.rs for why hanging them on \
             the root would be a fabrication rather than a fix).\n{}",
            fresh.alternation_defect_tools,
            fresh.alternation_defect_flags,
            detector::render_self_checks(&alternation_self_checks),
        );
        // Not gated on the count — but the self-checks ARE gated, because a
        // detector whose own evidence has stopped holding is reporting a
        // number that means nothing, and that is true whether or not the
        // number is allowed to be non-zero.
        if !detector::self_checks_are_conclusive(&alternation_self_checks) {
            println!(
                "brace-alternation-flag's own hand-built evidence no longer holds — its fleet \
                 number above cannot be read at all until that is fixed."
            );
            regressed = true;
        }
        // Same ratchet, same two halves, for shape A of the
        // `unparsed-subcommand` split (`crate::commandtable`). The fix
        // landed — `ar` and its four aliases went from 1 node to 9 with no
        // flag lost anywhere — so this is gated at a literal 0 rather than
        // against the checked-in scoreboard, for the reason spelled out
        // above: a baseline the commit can edit is a baseline the commit
        // can raise. A scoreboard written before `command_table_tools`
        // existed parses that key as 0, which is exactly what a healthy
        // fleet reports, so an older baseline stays comparable.
        //
        // This gate says nothing about shapes B, C and D — they have no
        // detector, and `mandible_core::audit`'s family table records that
        // a zero here does NOT mean `unparsed-subcommand` is finished.
        let table_ratchet = detector::ratchet_at_zero(
            detector::find("unparsed-command-table")?.as_ref(),
            fresh.command_table_tools,
            0,
        );
        println!("\n{}", table_ratchet.report());
        if !table_ratchet.holds() {
            regressed = true;
        }

        // `repeated-char-flag` (`crate::repeated_char`), the second of the
        // three families sharing the `short && !long && value_name`
        // fingerprint, on exactly the terms `bundled-short-flag` reached
        // above and after the same movement: reported-and-ungated while the
        // number had no baseline, ratcheted at a literal zero once the
        // repair landed (`help_text::sections::repair_repeated_character_flags`).
        // Gated against `0` and not against `previous` for the same reason —
        // the checked-in scoreboard is editable, so a commit reintroducing
        // the defect would otherwise raise its own baseline — and gated on
        // the detector's own self-checks alongside the count, because a gate
        // on `count == 0` alone is satisfied by deleting the detector.
        if fresh.repeated_char_tools != previous.repeated_char_tools
            || fresh.repeated_char_flags != previous.repeated_char_flags
        {
            println!(
                "repeated-char-flag misreads changed from {} tool(s)/{} flag(s) to {} tool(s)/{} flag(s)",
                previous.repeated_char_tools,
                previous.repeated_char_flags,
                fresh.repeated_char_tools,
                fresh.repeated_char_flags,
            );
        }
        let repeat_ratchet = detector::ratchet_at_zero(
            detector::find("repeated-char-flag")?.as_ref(),
            fresh.repeated_char_tools,
            fresh.repeated_char_flags,
        );
        println!("\n{}", repeat_ratchet.report());
        if !repeat_ratchet.holds() {
            regressed = true;
        }

        // `single-dash-long` (`crate::single_dash_long`), the third family
        // sharing the `short && !long && value_name` fingerprint, ratcheted
        // at zero the same way as the two above, gated against a literal
        // `0` for the same editable-baseline reason. The complementary
        // hazard — the detector staying healthy while the fix itself is
        // deleted, which no fleet count or self-check catches — is covered
        // separately by `single_dash_long::tests::
        // the_real_parser_leaves_no_split_in_any_audited_fixture`, which
        // replays frozen bytes under `cargo nextest`.
        if fresh.single_dash_split_tools != previous.single_dash_split_tools
            || fresh.single_dash_split_flags != previous.single_dash_split_flags
        {
            println!(
                "single-dash-long splits changed from {} tool(s)/{} flag(s) to {} tool(s)/{} flag(s)",
                previous.single_dash_split_tools,
                previous.single_dash_split_flags,
                fresh.single_dash_split_tools,
                fresh.single_dash_split_flags,
            );
        }
        let single_dash_ratchet = detector::ratchet_at_zero(
            detector::find("single-dash-long")?.as_ref(),
            fresh.single_dash_split_tools,
            fresh.single_dash_split_flags,
        );
        println!("\n{}", single_dash_ratchet.report());
        if !single_dash_ratchet.holds() {
            regressed = true;
        }

        // pnpm's own two families (atlas S-103, S-104), not yet fleet-
        // precise: see `detector::report_family_not_gated`.
        detector::report_family_not_gated(
            "ragged-command-table",
            fresh.ragged_command_tools,
            fresh.ragged_command_flags,
            previous.ragged_command_tools,
            previous.ragged_command_flags,
        )?;
        detector::report_family_not_gated(
            "wrapped-command-continuation-as-subcommand",
            fresh.wrapped_command_tools,
            fresh.wrapped_command_flags,
            previous.wrapped_command_tools,
            previous.wrapped_command_flags,
        )?;

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
/// `std::fs::write` does not create intermediate directories, so a fresh
/// clone with no `tmp/` would otherwise fail at the very end of a
/// twenty-minute sweep with nothing written.
///
/// Goes through `containment::write_scoreboard` rather than a bare
/// `std::fs::write`: when this process is the contained half of a
/// full-`PATH` sweep whose `out` was pre-secured by `sweep_guard`, that
/// function writes through the inherited fd instead of reopening `out` by
/// path, which fails `EACCES` from inside the namespace. Every other
/// caller falls straight through to `std::fs::write` unchanged.
fn write_out(out: &std::path::Path, contents: &str, what: &str) -> anyhow::Result<()> {
    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
                anyhow::anyhow!("failed to create {} for {what}: {e}", parent.display())
            })?;
        }
    }
    mandible_extract::exec::containment::write_scoreboard(out, contents)
        .map_err(|e| anyhow::anyhow!("failed to write {what} to {}: {e}", out.display()))
}
