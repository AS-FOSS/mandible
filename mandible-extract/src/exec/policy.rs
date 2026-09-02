//! The allowed argv shapes (spec §6 rules 1-2) as a closed type.
//!
//! [`InertArgv`] is the only way to describe what to run: no "bare
//! invocation" or "arbitrary args" variant exists, so the type system
//! enforces the allowlist rather than a runtime check.

/// One of the inert argv shapes a tier is permitted to invoke a tool with.
/// Every variant's [`InertArgv::args`] is non-empty.
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
    /// `<words...> --help` — per-node help (e.g. `git rebase --help`).
    /// Always ends in the literal `--help`, so never a bare invocation
    /// even when `words` is empty.
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
    /// `<words...> --help <word>` — the truncation-confession follow-up:
    /// re-probes with the argv a tool's own printed text recommended when
    /// it confesses `--help` is not the complete document. `--help` always
    /// precedes `word` so this can never degrade into a bare positional.
    /// `word` must come from the tool's own printed directive, never
    /// fabricated. Spec §6 rule 2b; docs/shapes.md S-080;
    /// corpus/curl/8.5.0/help.txt.
    HelpExpand {
        /// The subcommand path words already typed, if any (empty for the
        /// root — the only case shipped so far).
        words: Vec<String>,
        /// The word the tool's own text recommended, verbatim.
        word: String,
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
            InertArgv::HelpExpand { words, word } => {
                let mut a = words.clone();
                a.push("--help".to_string());
                a.push(word.clone());
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

    /// Rule 1: no variant's argv is ever empty, including empty-words cases.
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
            InertArgv::HelpExpand {
                words: vec![],
                word: "all".to_string(),
            },
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

    /// Spec §6 rule 2b: `--help` precedes the word, root and subcommand path.
    #[test]
    fn help_expand_args_put_help_immediately_before_the_word() {
        let root = InertArgv::HelpExpand {
            words: vec![],
            word: "all".to_string(),
        };
        assert_eq!(root.args(), vec!["--help".to_string(), "all".to_string()]);

        let sub = InertArgv::HelpExpand {
            words: vec!["sub".to_string()],
            word: "all".to_string(),
        };
        assert_eq!(
            sub.args(),
            vec!["sub".to_string(), "--help".to_string(), "all".to_string()]
        );
    }
}
