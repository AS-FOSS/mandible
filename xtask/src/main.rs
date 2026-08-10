//! Developer tasks for the mandible workspace: the extraction coverage
//! harness (spec §13.1).

#![forbid(unsafe_code)]

mod corpus;
mod coverage;
mod status;

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
        /// scoreboard and fail (nonzero exit) if `%described` dropped, the
        /// `no-tier` count grew, or the `suspicious` count grew — the
        /// regression gate spec §13.1 describes. `verbatim` and framework-
        /// detection counts are reported but deliberately not part of
        /// this gate (see `coverage::compute_aggregate`'s doc comment).
        /// Without this flag, the command just (re)writes the scoreboard
        /// file.
        #[arg(long)]
        check: bool,
        /// Where to read/write the scoreboard.
        #[arg(long, default_value = "coverage-scoreboard.txt")]
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
        } => {
            let shard = shard.as_deref().map(parse_shard).transpose()?;
            run_coverage(check, &out, tools, shard, progress, format)
        }
        Command::Corpus { bless, dir, format } => run_corpus(bless, &dir, format),
    }
}

fn run_corpus(bless: bool, dir: &std::path::Path, format: ScoreFormat) -> anyhow::Result<()> {
    let report = corpus::run(dir, bless, format)?;
    println!("{}", report.text);
    if report.failed() {
        anyhow::bail!(
            "corpus regression: {} fixture(s) failed — see above",
            report.failures.len()
        );
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

fn run_coverage(
    check: bool,
    out: &PathBuf,
    tools: Option<Vec<String>>,
    shard: Option<(usize, usize)>,
    progress: bool,
    format: ScoreFormat,
) -> anyhow::Result<()> {
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
    println!("{table}");
    println!(
        "aggregate: {:.2}% described across {} tools, {} with no tier, {} suspicious, {} verbatim, {} man-shaped, {}/{} framework-detected",
        fresh.pct_described,
        fresh.total,
        fresh.no_tier_count,
        fresh.suspicious_count,
        fresh.verbatim_count,
        fresh.man_shaped_count,
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
            "previous: {:.2}% described across {} tools, {} with no tier, {} suspicious, {} verbatim, {} man-shaped",
            previous.pct_described,
            previous.total,
            previous.no_tier_count,
            previous.suspicious_count,
            previous.verbatim_count,
            previous.man_shaped_count,
        );

        let mut regressed = false;
        if fresh.pct_described + 0.01 < previous.pct_described {
            println!(
                "REGRESSION: %described dropped from {:.2}% to {:.2}%",
                previous.pct_described, fresh.pct_described
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
        // than no metric — [M-10] shipped as 100% described while 39 of
        // tar's 40 nodes were fabricated, so %described alone must never
        // be the only gate.
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
        if regressed {
            anyhow::bail!("coverage regression detected — see above");
        }
        println!("no regression.");
        return Ok(());
    }

    std::fs::write(out, &table)
        .map_err(|e| anyhow::anyhow!("failed to write scoreboard to {}: {e}", out.display()))?;
    println!("wrote {}", out.display());
    Ok(())
}
