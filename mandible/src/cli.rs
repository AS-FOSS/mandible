// Command-line argument parsing.
//
// A regular (not `//!` inner) doc comment deliberately: `build.rs`
// `include!`s this file verbatim to share the `Cli` definition with the
// completion-script generator (see its doc comment), and `include!`
// splices content at the call site rather than as a fresh module, where
// an inner doc comment doesn't parse.

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

    /// Print a shell completion script for SHELL to stdout and exit,
    /// instead of opening the TUI. Packaged builds also install
    /// pre-generated completions to the standard per-distro paths (spec
    /// §15); this flag exists for `cargo install` users and anyone
    /// scripting their own install (e.g. `mandible --completions zsh >
    /// ~/.zfunc/_mandible`).
    #[arg(long, value_name = "SHELL")]
    pub completions: Option<clap_complete::Shell>,
}

impl Cli {
    /// The tool name this invocation is ultimately about, whether given as
    /// the positional argument or as `--doctor`'s value.
    pub fn target_tool(&self) -> Option<&str> {
        self.doctor.as_deref().or(self.tool.as_deref())
    }
}
