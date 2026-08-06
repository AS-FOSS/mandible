//! Developer tasks for the mandible workspace: the extraction coverage
//! harness (spec §13.1).

#![forbid(unsafe_code)]

mod coverage;

use clap::{Parser, Subcommand};
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
        /// scoreboard and fail (nonzero exit) if `%described` dropped or
        /// the `no-tier` count grew — the regression gate spec §13.1
        /// describes. Without this flag, the command just (re)writes the
        /// scoreboard file.
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
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Coverage { check, out, tools } => run_coverage(check, &out, tools),
    }
}

fn run_coverage(check: bool, out: &PathBuf, tools: Option<Vec<String>>) -> anyhow::Result<()> {
    let (table, fresh) = match tools {
        Some(tools) => {
            println!(
                "scanning a fixed list of {} tool(s): {}...",
                tools.len(),
                tools.join(", ")
            );
            coverage::run_over(tools)
        }
        None => {
            println!("scanning PATH and running the extraction pipeline against every executable found...");
            coverage::run()
        }
    };
    println!("{table}");
    println!(
        "aggregate: {:.2}% described across {} tools, {} with no tier, {} suspicious",
        fresh.pct_described, fresh.total, fresh.no_tier_count, fresh.suspicious_count
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
            "previous: {:.2}% described across {} tools, {} with no tier, {} suspicious",
            previous.pct_described,
            previous.total,
            previous.no_tier_count,
            previous.suspicious_count
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
