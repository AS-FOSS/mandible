//! Command-line argument parsing.

use crate::shell_init::ShellInit;
use clap::Parser;
use std::path::PathBuf;

/// mandible: a universal, interactive TUI reference for CLI tools.
#[derive(Parser, Debug)]
#[command(name = "mandible", version, about, long_about = None)]
pub struct Cli {
    /// The tool to open, e.g. "git". Required unless `--doctor`, `--report`,
    /// or `--review` is given.
    pub tool: Option<String>,

    /// A subcommand path within TOOL to open at, e.g. `mandible cargo
    /// clippy` or `mandible git remote add` (spec §5.4). Opens exactly
    /// where browsing to that node would land; a name the tool's own help
    /// never documents still lands when a `<tool>-<sub>` binary is on PATH,
    /// marked unverified. TUI only — `--doctor` and `--report` describe a
    /// whole tool, and take a tool name alone.
    #[arg(value_name = "SUBCOMMAND")]
    pub subcommand: Vec<String>,

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

    /// Browse as usual, but make `Enter` print the selected command (plus
    /// the selected flag, if search landed on one) to stdout and exit,
    /// instead of expanding the row. The TUI itself draws on stderr, so
    /// stdout carries the composed line and nothing else — which is what
    /// lets a shell binding put it on the prompt, ready to edit:
    /// `mandible --shell-init bash` prints one.
    ///
    /// Nothing is printed if you quit instead (`q`, `Ctrl-C`), so the
    /// binding leaves the line alone. `→`/`l` still expand.
    #[arg(long, conflicts_with_all = ["doctor", "report", "review", "completions", "shell_init"])]
    pub print_selection: bool,

    /// Print the shell integration for SHELL (`bash` or `zsh`) to stdout
    /// and exit: a widget that opens `mandible --print-selection` on the
    /// word already on your command line and replaces the line with what
    /// you select. Add `eval "$(mandible --shell-init bash)"` to your
    /// shell's rc file.
    ///
    /// Emitted by the binary rather than installed as a file, for the same
    /// reason `--completions` is (spec §15): one generator, so no packaging
    /// channel can ship a snippet that disagrees with the flags this
    /// version has.
    #[arg(long, value_name = "SHELL")]
    pub shell_init: Option<ShellInit>,

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

    /// The full node path this invocation asks to open — the tool name
    /// followed by [`Self::subcommand`] — or `None` when no subcommand was
    /// given (`mandible git`, which opens at the root like it always has).
    pub fn requested_path(&self) -> Option<Vec<String>> {
        if self.subcommand.is_empty() {
            return None;
        }
        let tool = self.tool.as_deref()?;
        let mut path = vec![tool.to_string()];
        path.extend(self.subcommand.iter().cloned());
        Some(path)
    }

    /// The refusal for the one combination no mode can honour: extra words
    /// alongside a whole-tool diagnostic.
    ///
    /// A subcommand path addresses one node of one tool's tree, which is a
    /// thing only the TUI has. `--doctor`/`--report` take their tool as the
    /// flag's own value, so **every** positional beside them is extra — with
    /// `--doctor cargo clippy`, clap binds `clippy` to the tool positional
    /// and the diagnostic still describes `cargo`.
    ///
    /// Said plainly rather than dropped. Before a path was accepted at all
    /// this combination was a parse error, and silently ignoring it now
    /// would print a report about `cargo` to a reader who believes they
    /// asked about `cargo clippy` — trading a clear refusal for a confidently
    /// mislabelled answer.
    pub fn subcommand_path_conflict(&self) -> Option<String> {
        if self.doctor.is_none() && self.report.is_none() {
            return None;
        }
        let stray: Vec<&str> = self
            .tool
            .as_deref()
            .into_iter()
            .chain(self.subcommand.iter().map(String::as_str))
            .collect();
        if stray.is_empty() {
            return None;
        }
        let words = stray.join(" ");
        let tool = self.target_tool().unwrap_or_default();
        Some(format!(
            "--doctor and --report take a tool name only; drop {words:?} or run \
             `mandible {tool} {words}` for the interactive tree"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("mandible").chain(args.iter().copied()))
            .expect("should parse")
    }

    /// Issue #70: `cargo clippy` is a real command that `cargo --help` never
    /// lists, and mandible used to refuse the words outright with
    /// "unexpected argument 'clippy' found".
    #[test]
    fn a_subcommand_path_parses_into_tool_and_words() {
        let cli = parse(&["cargo", "clippy"]);
        assert_eq!(cli.tool.as_deref(), Some("cargo"));
        assert_eq!(cli.subcommand, vec!["clippy".to_string()]);
        assert_eq!(
            cli.requested_path(),
            Some(vec!["cargo".to_string(), "clippy".to_string()])
        );
    }

    #[test]
    fn several_words_nest_in_order() {
        let cli = parse(&["git", "remote", "add"]);
        assert_eq!(
            cli.requested_path(),
            Some(vec![
                "git".to_string(),
                "remote".to_string(),
                "add".to_string()
            ])
        );
    }

    #[test]
    fn a_bare_tool_requests_no_path() {
        assert!(parse(&["git"]).requested_path().is_none());
    }

    /// A path alongside a whole-tool diagnostic is refused, never dropped:
    /// silently ignoring it reports on `cargo` while the reader believes
    /// they asked about `cargo clippy`. This was a parse error before a path
    /// was accepted at all, and must not become a mislabelled answer.
    #[test]
    fn a_path_alongside_a_whole_tool_diagnostic_is_refused() {
        let refusal = parse(&["--doctor", "cargo", "clippy"])
            .subcommand_path_conflict()
            .expect("must refuse rather than drop the words");
        assert!(refusal.contains("clippy"), "{refusal}");
        assert!(parse(&["--report", "cargo", "clippy"])
            .subcommand_path_conflict()
            .is_some());
        // One stray word is as wrong as two: `--doctor cargo clippy` binds
        // `clippy` to the tool positional, and the report is still `cargo`'s.
        assert!(parse(&["--doctor", "cargo", "clippy"])
            .subcommand
            .is_empty());
    }

    #[test]
    fn the_ordinary_forms_conflict_with_nothing() {
        assert!(parse(&["cargo", "clippy"])
            .subcommand_path_conflict()
            .is_none());
        assert!(parse(&["--doctor", "cargo"])
            .subcommand_path_conflict()
            .is_none());
    }
}
