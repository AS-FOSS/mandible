//! The allowed argv shapes (spec §6 rules 1-2) as a closed type.
//!
//! [`InertArgv`] is the only way to describe what to run: no "bare
//! invocation" or "arbitrary args" variant exists, so the type system
//! enforces the allowlist rather than a runtime check.

/// The words a followed help hint (spec §6 rule 2e) may be made of, besides
/// the command names the same text lists and the node's own path. The one
/// place this list lives; adding a word is a maintainer decision (§16).
pub const HINT_SAFE_WORDS: [&str; 4] = ["--help", "-help", "-h", "help"];

/// Also safe, but only directly after `--help` (as rule 2b uses it).
pub const HINT_WORD_AFTER_HELP: &str = "all";

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
    /// `help <option>` — the listing invocation a root text names in the
    /// closed sentence `'<tool> help <option>' lists commands` (spec §6
    /// rule 2d, docs/shapes.md S-214). `option` is copied verbatim from
    /// that sentence, is `-X` or `--word`, and is never `-h` or `--help`.
    HelpListing {
        /// The option the root text's own sentence quoted, e.g. `"-a"`.
        option: String,
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
    /// `<words...> <flag> <word>` — the truncation-confession follow-up:
    /// re-probes with the argv a tool's own printed text recommended when
    /// it confesses `--help` is not the complete document. `flag` (`--help`
    /// or `-h`, as the tool wrote it) always precedes `word` so this can
    /// never degrade into a bare positional.
    /// `word` must come from the tool's own printed directive, never
    /// fabricated. Spec §6 rule 2b; docs/shapes.md S-080;
    /// corpus/curl/8.5.0/help.txt.
    HelpExpand {
        /// The subcommand path words already typed, if any (empty for the
        /// root — the only case shipped so far).
        words: Vec<String>,
        /// The help flag the tool's own text printed before the word: one of
        /// `"--help"` and `"-h"`.
        flag: &'static str,
        /// The word the tool's own text recommended, verbatim.
        word: String,
    },
    /// The argv of a quoted self-invocation the tool's own answer named as
    /// the way to get help (spec §6 rule 2e). Only [`InertArgv::help_hint`]
    /// builds one, so every word is on [`HINT_SAFE_WORDS`], is
    /// [`HINT_WORD_AFTER_HELP`] after `--help`, is a command the same text
    /// lists, or belongs to the node's own path; at least one is a help word.
    HelpHint {
        /// The words after the tool's name, verbatim from the quote.
        words: Vec<String>,
    },
    /// `<words...> help` — the node's own path, then the literal `help` the
    /// parent's usage form shows in the slot after the command placeholder
    /// (spec §6 rule 2f, docs/shapes.md S-258). Only
    /// [`InertArgv::help_after_command`] builds one.
    HelpAfterCommand {
        /// The node's path words, non-empty, none starting with `-`.
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
            InertArgv::HelpListing { option } => vec!["help".to_string(), option.clone()],
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
            InertArgv::HelpExpand { words, flag, word } => {
                let mut a = words.clone();
                a.push((*flag).to_string());
                a.push(word.clone());
                a
            }
            InertArgv::HelpHint { words } => words.clone(),
            InertArgv::HelpAfterCommand { words } => {
                let mut a = words.clone();
                a.push("help".to_string());
                a
            }
            InertArgv::ClapCompleteEnvProbe { .. } => vec!["--".to_string()],
            InertArgv::ClapCompleteEnvComplete { partial, .. } => {
                vec!["--".to_string(), partial.clone()]
            }
        }
    }

    /// The hint argv `quoted` (the words after the tool's name) names for the
    /// node at `node_words`, or `None` when any word is not safe. The node's
    /// own path may open the argv; `listed` are the command names the
    /// answer's text lists. Spec §6 rule 2e.
    pub fn help_hint(quoted: &[String], node_words: &[String], listed: &[String]) -> Option<Self> {
        let rest = quoted.strip_prefix(node_words)?;
        let mut helped = false;
        for (i, w) in rest.iter().enumerate() {
            let prev = i.checked_sub(1).map(|p| rest[p].as_str());
            let next = rest.get(i + 1).map(String::as_str);
            // `help` only opens the argv, so no command reads it as an operand.
            let is_help = HINT_SAFE_WORDS.contains(&w.as_str()) && (w != "help" || i == 0);
            let after_help = w == HINT_WORD_AFTER_HELP && prev == Some("--help");
            // A listed command only as `help <cmd>` or `<cmd> --help`, the
            // forms rule 2c and the subcommand probe already send.
            let command = listed.contains(w)
                && (prev == Some("help")
                    || next.is_some_and(|n| n != "help" && HINT_SAFE_WORDS.contains(&n)));
            if !(is_help || after_help || command) {
                return None;
            }
            helped |= is_help;
        }
        helped.then(|| InertArgv::HelpHint {
            words: quoted.to_vec(),
        })
    }

    /// `<node_words> help` for a command node, or `None` unless `attested`:
    /// the node came from a grammar production in a command slot whose
    /// parent usage form shows `help` right after it (spec §6 rule 2f).
    /// The path must be non-empty and made of command words, never a flag
    /// and never `help` itself.
    pub fn help_after_command(node_words: &[String], attested: bool) -> Option<Self> {
        let plain = node_words.last().is_some_and(|w| w != "help")
            && node_words
                .iter()
                .all(|w| !w.is_empty() && !w.starts_with('-'));
        (attested && plain).then(|| InertArgv::HelpAfterCommand {
            words: node_words.to_vec(),
        })
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
            InertArgv::HelpListing {
                option: "-a".to_string(),
            },
            InertArgv::HelpLongForPath { words: vec![] },
            InertArgv::HelpShortForPath { words: vec![] },
            InertArgv::HelpExpand {
                words: vec![],
                flag: "--help",
                word: "all".to_string(),
            },
            InertArgv::HelpHint {
                words: vec!["-help".to_string()],
            },
            InertArgv::HelpAfterCommand {
                words: vec!["link".to_string()],
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

    /// Rule 2f: the constructor checks the evidence and the path.
    #[test]
    fn help_after_command_needs_the_evidence_and_a_plain_path() {
        let v = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let made = InertArgv::help_after_command(&v(&["link"]), true).expect("attested");
        assert_eq!(made.args(), v(&["link", "help"]));
        assert!(InertArgv::help_after_command(&v(&["link"]), false).is_none());
        assert!(InertArgv::help_after_command(&[], true).is_none());
        assert!(InertArgv::help_after_command(&v(&["help"]), true).is_none());
        assert!(InertArgv::help_after_command(&v(&["-x"]), true).is_none());
        assert!(InertArgv::help_after_command(&v(&["a", "--b"]), true).is_none());
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
            flag: "--help",
            word: "all".to_string(),
        };
        assert_eq!(root.args(), vec!["--help".to_string(), "all".to_string()]);

        let short = InertArgv::HelpExpand {
            words: vec![],
            flag: "-h",
            word: "full".to_string(),
        };
        assert_eq!(short.args(), vec!["-h".to_string(), "full".to_string()]);

        let sub = InertArgv::HelpExpand {
            words: vec!["sub".to_string()],
            flag: "--help",
            word: "all".to_string(),
        };
        assert_eq!(
            sub.args(),
            vec!["sub".to_string(), "--help".to_string(), "all".to_string()]
        );
    }
}
