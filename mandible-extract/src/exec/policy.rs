//! The allowed argv shapes (spec §6, rule 2) as a closed type.
//!
//! [`InertArgv`] is deliberately the *only* way to describe what to run: it
//! has no "bare invocation" or "arbitrary args" variant, so a caller cannot
//! construct an argv outside the allowlist even by mistake. This turns
//! spec §6 rules 1 ("never invoke a bare binary") and 2 ("only inert argv
//! shapes") into a property the type system enforces, not just a runtime
//! check.

/// One of the inert argv shapes a tier is permitted to invoke a tool with.
/// Every variant, once turned into an argument list via [`InertArgv::args`],
/// is non-empty — there is no shape that reduces to a bare invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InertArgv {
    /// `__complete <words...>` (cobra dynamic completion).
    CobraComplete {
        /// The path words already typed, e.g. `["pr"]` for `gh pr <TAB>`.
        words: Vec<String>,
    },
    /// `completion <shell>` (generate a completion script to parse
    /// structurally in Tier C).
    CompletionScript {
        /// The shell to request a script for, e.g. `"zsh"`.
        shell: String,
    },
    /// `--help`.
    HelpLong,
    /// `-h`.
    HelpShort,
    /// `help [<words...>]`.
    HelpSubcommand {
        /// The path words to ask for help on, if any.
        words: Vec<String>,
    },
    /// `<words...> --help` — per-node help (e.g. `git rebase --help`),
    /// used by Tier B to probe a specific subtree without recursing
    /// eagerly. Always ends in the literal `--help`, so this is never a
    /// bare invocation even when `words` is empty (the root case).
    HelpLongForPath {
        /// The subcommand path words already typed, e.g. `["rebase"]`.
        words: Vec<String>,
    },
    /// `<words...> -h`, the short-flag counterpart of
    /// [`InertArgv::HelpLongForPath`].
    HelpShortForPath {
        /// The subcommand path words already typed.
        words: Vec<String>,
    },
    /// `<tool> --` under `COMPLETE=<shell>`, used to detect clap
    /// `CompleteEnv` support (spec §7 Tier E). Never invoked without the
    /// trailing `--`.
    ClapCompleteEnvProbe {
        /// The shell name to set `COMPLETE=` to.
        shell: String,
    },
    /// `<tool> -- <partial>` under `COMPLETE=<shell>`.
    ClapCompleteEnvComplete {
        /// The shell name to set `COMPLETE=` to.
        shell: String,
        /// The partial word to request completions for.
        partial: String,
    },
}

impl InertArgv {
    /// The argument vector to pass to the child process (not including
    /// argv[0], the tool path itself).
    pub fn args(&self) -> Vec<String> {
        match self {
            InertArgv::CobraComplete { words } => {
                let mut a = vec!["__complete".to_string()];
                a.extend(words.iter().cloned());
                a
            }
            InertArgv::CompletionScript { shell } => vec!["completion".to_string(), shell.clone()],
            InertArgv::HelpLong => vec!["--help".to_string()],
            InertArgv::HelpShort => vec!["-h".to_string()],
            InertArgv::HelpSubcommand { words } => {
                let mut a = vec!["help".to_string()];
                a.extend(words.iter().cloned());
                a
            }
            InertArgv::HelpLongForPath { words } => {
                let mut a = words.clone();
                a.push("--help".to_string());
                a
            }
            InertArgv::HelpShortForPath { words } => {
                let mut a = words.clone();
                a.push("-h".to_string());
                a
            }
            InertArgv::ClapCompleteEnvProbe { .. } => vec!["--".to_string()],
            InertArgv::ClapCompleteEnvComplete { partial, .. } => {
                vec!["--".to_string(), partial.clone()]
            }
        }
    }

    /// Extra environment variables this shape requires beyond the baseline
    /// sanitized environment (spec §6 rule 6), e.g. `COMPLETE=zsh` for the
    /// clap `CompleteEnv` protocol.
    pub fn extra_env(&self) -> Vec<(String, String)> {
        match self {
            InertArgv::ClapCompleteEnvProbe { shell }
            | InertArgv::ClapCompleteEnvComplete { shell, .. } => {
                vec![("COMPLETE".to_string(), shell.clone())]
            }
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rule 1: never a bare invocation. Every variant's argv must be
    /// non-empty, including the degenerate all-empty-words cases.
    #[test]
    fn no_variant_ever_produces_empty_argv() {
        let variants = [
            InertArgv::CobraComplete { words: vec![] },
            InertArgv::CompletionScript {
                shell: "zsh".to_string(),
            },
            InertArgv::HelpLong,
            InertArgv::HelpShort,
            InertArgv::HelpSubcommand { words: vec![] },
            InertArgv::HelpLongForPath { words: vec![] },
            InertArgv::HelpShortForPath { words: vec![] },
            InertArgv::ClapCompleteEnvProbe {
                shell: "zsh".to_string(),
            },
            InertArgv::ClapCompleteEnvComplete {
                shell: "zsh".to_string(),
                partial: String::new(),
            },
        ];
        for v in &variants {
            assert!(!v.args().is_empty(), "{v:?} produced empty argv");
        }
    }

    #[test]
    fn clap_complete_env_always_carries_the_env_var() {
        let probe = InertArgv::ClapCompleteEnvProbe {
            shell: "zsh".to_string(),
        };
        assert_eq!(
            probe.extra_env(),
            vec![("COMPLETE".to_string(), "zsh".to_string())]
        );
    }

    #[test]
    fn cobra_complete_args_start_with_dunder_complete() {
        let v = InertArgv::CobraComplete {
            words: vec!["pr".to_string()],
        };
        assert_eq!(v.args(), vec!["__complete".to_string(), "pr".to_string()]);
    }
}
