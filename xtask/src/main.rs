//! Developer tasks for the mantui workspace: catalog index verification and
//! the extraction coverage harness (spec §13.1).

#![forbid(unsafe_code)]

use clap::{Parser, Subcommand};

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
    /// `PATH` (spec §13.1). Not implemented until Tier B exists (batch 2) —
    /// a coverage scoreboard with only Tier A active would just restate
    /// "740 known tools, everything else no-tier", which isn't yet a
    /// meaningful regression signal.
    Coverage,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::CheckIndex => check_index(),
        Command::Coverage => {
            println!("xtask coverage: not implemented yet (batch 2, once Tier B exists).");
            println!("See spec.md §13.1 for the intended scoreboard shape.");
            Ok(())
        }
    }
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
