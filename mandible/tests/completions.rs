//! `--completions <shell>` at the process boundary, which is where the one
//! generator spec §15 relies on actually runs.
//!
//! What these pin is the **candidate set for a tool name**. `TOOL`,
//! `--doctor <TOOL>` and `--report <TOOL>` all name a program mandible will
//! run `--help` on, so the right completions are the command names on
//! `$PATH`; without the `ValueHint::CommandName` annotation clap_complete
//! defaults them to filenames, and `mandible gi<TAB>` offers whatever
//! happens to be in the current directory.
//!
//! Bash is absent below on purpose, and it is the one honest gap here:
//! clap_complete 4.6.8's ahead-of-time bash generator has no
//! `ValueHint::CommandName` branch at all — `vals_for` falls through to
//! `compgen -f` for every hint it does not special-case, and positionals
//! never reach the emitted `case` in the first place — so the bash script
//! is byte-identical with and without the annotation. Asserting a marker
//! that no input can produce would be a decorative guard (AGENTS.md §3.4).

use std::process::Command;

fn completions(shell: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_mandible"))
        .args(["--completions", shell])
        .output()
        .expect("failed to run mandible");
    assert!(
        out.status.success(),
        "--completions {shell} exited non-zero"
    );
    String::from_utf8(out.stdout).expect("completion scripts are UTF-8")
}

/// zsh's own helper for "a command on `$PATH`". `-e` restricts it to
/// external commands, which is what mandible can actually probe.
const ZSH_COMMAND_NAMES: &str = "_command_names -e";

/// fish's equivalent.
const FISH_COMMAND_NAMES: &str = "__fish_complete_command";

fn line_containing<'a>(script: &'a str, needle: &str) -> &'a str {
    script
        .lines()
        .find(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("no line matching {needle:?} in:\n{script}"))
}

#[test]
fn zsh_completes_the_tool_positional_with_command_names() {
    let script = completions("zsh");
    let positional = line_containing(&script, "::tool --");
    assert!(
        positional.ends_with(&format!("{ZSH_COMMAND_NAMES}' \\")),
        "the TOOL positional must complete command names, got:\n{positional}"
    );
}

#[test]
fn zsh_completes_the_diagnostic_flags_with_command_names() {
    let script = completions("zsh");
    for flag in ["--doctor=[", "--report=["] {
        let spec = line_containing(&script, flag);
        assert!(
            spec.ends_with(&format!("{ZSH_COMMAND_NAMES}' \\")),
            "{flag} takes a tool name, so it must complete command names, got:\n{spec}"
        );
    }
}

#[test]
fn fish_completes_the_diagnostic_flags_with_command_names() {
    let script = completions("fish");
    for flag in ["-l doctor ", "-l report "] {
        let spec = line_containing(&script, flag);
        assert!(
            spec.contains(FISH_COMMAND_NAMES),
            "{flag} takes a tool name, so it must complete command names, got:\n{spec}"
        );
    }
}

/// The filename default is what the annotation exists to displace: a stray
/// `_default`/`-r` on any of these three is the regression, and it is
/// invisible in a script that still parses fine.
#[test]
fn no_tool_name_argument_falls_back_to_filenames() {
    let zsh = completions("zsh");
    for arg in ["::tool --", "--doctor=[", "--report=["] {
        let spec = line_containing(&zsh, arg);
        assert!(
            !spec.ends_with("_default' \\"),
            "{arg} fell back to zsh's filename default:\n{spec}"
        );
    }
}
