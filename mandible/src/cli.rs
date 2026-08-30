//! Command-line argument parsing.

use clap::Parser;
use std::path::PathBuf;

/// mandible: a universal, interactive TUI reference for CLI tools.
#[derive(Parser, Debug)]
#[command(name = "mandible", version, about, long_about = None)]
pub struct Cli {
    /// The tool to open, e.g. "git". Required unless `--doctor`, `--report`,
    /// or `--review` is given.
    pub tool: Option<String>,

    /// Print extraction diagnostics for TOOL (tier statuses, node/flag
    /// counts, %described, catalog vendoring date, timing) instead of
    /// opening the TUI. See spec §5.3.
    #[arg(long, value_name = "TOOL")]
    pub doctor: Option<String>,

    /// Print a paste-ready bug report for TOOL (mandible's version, TOOL's
    /// version if resolvable, the `--doctor` diagnostic, and a raw `--help`
    /// capture) instead of opening the TUI, followed by the issues URL.
    /// Symmetric with `--doctor`, not a `report` subcommand: `mandible`
    /// already takes a bare `[TOOL]` positional (`mandible git`), so a
    /// `report` subcommand would be ambiguous against a tool literally
    /// named `report`.
    #[arg(long, value_name = "TOOL")]
    pub report: Option<String>,

    /// Print a shell completion script for SHELL to stdout and exit,
    /// instead of opening the TUI. Packaged builds also install
    /// pre-generated completions to the standard per-distro paths (spec
    /// §15); this flag exists for `cargo install` users and anyone
    /// scripting their own install (e.g. `mandible --completions zsh >
    /// ~/.zfunc/_mandible`).
    #[arg(long, value_name = "SHELL")]
    pub completions: Option<clap_complete::Shell>,

    /// Review `<dir>/<SEED>.toml`'s pending entries (`xtask audit sample`'s
    /// output) one at a time, inside the real TUI: each tool opens exactly
    /// as `mandible <tool>` would, and a verdict — `c`/`i`/`w`/`s` for
    /// correct/incomplete/wrong/skip, then Enter — is written back to the
    /// manifest immediately, before the next tool opens. Mutually exclusive
    /// with `TOOL`/`--doctor`/`--report` in practice (checked first).
    #[arg(long, value_name = "SEED")]
    pub review: Option<u64>,

    /// The directory `--review` reads and writes `<SEED>.toml` under.
    /// Mirrors `xtask audit`'s own `--dir` default.
    #[arg(long, default_value = "audit")]
    pub audit_dir: PathBuf,
}

impl Cli {
    /// The tool name this invocation is ultimately about, whether given as
    /// the positional argument, as `--doctor`'s value, or as `--report`'s
    /// value. `--report` is checked first: if both diagnostic flags are
    /// somehow given together, the more specific ask (a bug report) wins
    /// over the plainer diagnostic dump.
    pub fn target_tool(&self) -> Option<&str> {
        self.report
            .as_deref()
            .or(self.doctor.as_deref())
            .or(self.tool.as_deref())
    }
}
