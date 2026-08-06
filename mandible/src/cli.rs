//! Command-line argument parsing.

use clap::Parser;

/// mandible: a universal, interactive TUI reference for CLI tools.
#[derive(Parser, Debug)]
#[command(name = "mandible", version, about, long_about = None)]
pub struct Cli {
    /// The tool to open, e.g. "git". Required unless `--doctor` is given.
    pub tool: Option<String>,

    /// Print extraction diagnostics for TOOL (tier statuses, node/flag
    /// counts, %described, catalog vendoring date, timing) instead of
    /// opening the TUI. See spec §5.3.
    #[arg(long, value_name = "TOOL")]
    pub doctor: Option<String>,

    /// Bypass the cache and re-extract, then repopulate it.
    #[arg(long)]
    pub refresh: bool,
}

impl Cli {
    /// The tool name this invocation is ultimately about, whether given as
    /// the positional argument or as `--doctor`'s value.
    pub fn target_tool(&self) -> Option<&str> {
        self.doctor.as_deref().or(self.tool.as_deref())
    }
}
