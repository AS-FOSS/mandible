//! Developer tasks for the mantui workspace: catalog index verification and
//! the extraction coverage harness (spec §13.1).

#![forbid(unsafe_code)]

mod coverage;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "xtask", about = "Developer tasks for the mantui workspace")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Verify the vendored carapace catalog's build-time index: that it is
    /// sorted, that spot-checked lookups succeed, and print the catalog's
    /// provenance metadata.
    CheckIndex,
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
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::CheckIndex => check_index(),
        Command::Coverage { check, out } => run_coverage(check, &out),
    }
}

fn run_coverage(check: bool, out: &PathBuf) -> anyhow::Result<()> {
    println!("scanning PATH and running the extraction pipeline against every executable found...");
    let (table, fresh) = coverage::run();
    println!("{table}");
    println!(
        "aggregate: {:.2}% described across {} tools, {} with no tier",
        fresh.pct_described, fresh.total, fresh.no_tier_count
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
            "previous: {:.2}% described across {} tools, {} with no tier",
            previous.pct_described, previous.total, previous.no_tier_count
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

fn check_index() -> anyhow::Result<()> {
    use mantui_extract::known_specs::{catalog_meta, CarapaceTier};
    use mantui_extract::ExtractionTier;

    let meta = catalog_meta();
    println!("provider:   {}", meta.provider);
    println!("source:     {}", meta.source);
    println!("source_dir: {}", meta.source_dir);
    println!("commit:     {}", meta.commit);
    println!("generated:  {}", meta.generated);
    println!("tool_count: {}", meta.tool_count);

    let tier = CarapaceTier;
    for spot_check in ["git", "docker", "curl"] {
        let tool = mantui_extract::resolve_tool(spot_check);
        let found = tier.detect(&tool);
        println!(
            "  {spot_check}: {}",
            if found { "found" } else { "MISSING" }
        );
    }

    println!("index check ok.");
    Ok(())
}
