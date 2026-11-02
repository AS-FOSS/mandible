//! The help hint (spec §6 rule 2e, docs/shapes.md S-253).
//!
//! When an answer gave nothing usable and quotes the way to get help, that
//! quoted invocation is run, at most twice and never from a page a hint
//! reached. The quote opens with the tool's own name; every later word must
//! pass [`InertArgv::help_hint`]. The new answer is kept only if it is help.
//! Models: `try "<tool> -help".`, `try '<tool> help'`, `run "<tool> --help all"`.

use std::path::Path;

use super::{looks_like_help_output, pick_stream, sections};
use crate::exec::{ExecOutput, InertArgv, Probe};

/// Most hints one node follows.
const MAX_HINT_FOLLOWS: usize = 2;

/// Longest quoted invocation looked at, in bytes.
const MAX_QUOTE_LEN: usize = 120;

/// True when the parser found nothing in `text`: no flags, commands,
/// operands or usage line (nor the modifiers and variables it also reads).
pub(super) fn gives_nothing(text: &str) -> bool {
    let p = sections::parse_with_profile(text, None, None);
    p.flags.is_empty()
        && p.subcommands.is_empty()
        && p.positionals.is_empty()
        && p.usage.is_empty()
        && p.modifiers.is_empty()
        && p.env_vars.is_empty()
}

/// The hint invocations `text` quotes, in text order, at most
/// [`MAX_HINT_FOLLOWS`], none equal to an argv in `tried`.
pub(super) fn hints(
    text: &str,
    tool: &str,
    node_words: &[String],
    tried: &[Vec<String>],
) -> Vec<InertArgv> {
    let listed: Vec<String> = sections::parse_with_profile(text, None, None)
        .subcommands
        .into_iter()
        .map(|c| c.name.to_string())
        .collect();
    let mut out: Vec<InertArgv> = Vec::new();
    for quote in quotes(text) {
        let mut words = quote.split_whitespace().map(str::to_string);
        if words.next().as_deref() != Some(tool) {
            continue;
        }
        let words: Vec<String> = words.collect();
        let Some(hint) = InertArgv::help_hint(&words, node_words, &listed) else {
            continue;
        };
        if tried.contains(&words) || out.contains(&hint) {
            continue;
        }
        out.push(hint);
        if out.len() == MAX_HINT_FOLLOWS {
            break;
        }
    }
    out
}

/// The text of every `"..."`, `'...'` and `` `...' `` pair on one line. An
/// opening quote follows a non-alphanumeric and a closing one precedes one,
/// so an apostrophe inside a word opens nothing.
fn quotes(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    for line in text.lines() {
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let open = bytes[i];
            let opens = matches!(open, b'"' | b'\'' | b'`')
                && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric());
            if !opens {
                i += 1;
                continue;
            }
            let close = (i + 1..bytes.len().min(i + 1 + MAX_QUOTE_LEN)).find(|&j| {
                let end = if open == b'`' {
                    matches!(bytes[j], b'\'' | b'`')
                } else {
                    bytes[j] == open
                };
                end && bytes.get(j + 1).is_none_or(|b| !b.is_ascii_alphanumeric())
            });
            match close {
                Some(j) => {
                    found.push(&line[i + 1..j]);
                    i = j + 1;
                }
                None => i += 1,
            }
        }
    }
    found
}

/// A followed hint's answer: the output, its parse-path text and its argv.
pub(super) struct Followed {
    pub out: ExecOutput,
    pub text: String,
    pub argv: String,
}

/// Run the hints `text` quotes through `probe`; the first answer that is
/// help wins. `None` leaves the earlier answer standing.
pub(super) fn follow(
    probe: &dyn Probe,
    tool_path: &Path,
    node_words: &[String],
    text: &str,
    tried: &[Vec<String>],
) -> Option<Followed> {
    let tool = tool_path.file_name()?.to_str()?;
    for hint in hints(text, tool, node_words, tried) {
        let Ok(out) = crate::exec::run_probe(probe, tool_path, &hint) else {
            continue;
        };
        if out.timed_out {
            continue;
        }
        let answer = pick_stream(&out.stdout, &out.stderr);
        if looks_like_help_output(&answer) && !sections::is_man_page_banner(&answer) {
            return Some(Followed {
                argv: hint.args().join(" "),
                text: answer,
                out,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argvs(text: &str, tool: &str) -> Vec<Vec<String>> {
        hints(text, tool, &[], &[])
            .iter()
            .map(InertArgv::args)
            .collect()
    }

    fn v(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn a_quoted_self_invocation_is_followed() {
        let text = "Option \"--help\" is unknown, try \"tc -help\".\n";
        assert_eq!(argvs(text, "tc"), vec![v(&["-help"])]);
        assert_eq!(
            argvs(
                "Argument \"--help\" is unknown, try \"rtmon help\".",
                "rtmon"
            ),
            vec![v(&["help"])]
        );
        assert_eq!(argvs("try 'x help' now", "x"), vec![v(&["help"])]);
        assert_eq!(
            argvs("run \"x --help all\"", "x"),
            vec![v(&["--help", "all"])]
        );
        assert_eq!(argvs("see `x -h'", "x"), vec![v(&["-h"])]);
    }

    #[test]
    fn a_quote_naming_another_program_is_not_followed() {
        assert!(argvs("try \"other -help\"", "tc").is_empty());
        assert!(argvs("try \"tcx -help\"", "tc").is_empty());
    }

    #[test]
    fn a_word_off_the_safe_list_is_not_followed() {
        for text in [
            "try \"tc rm\"",
            "try \"tc --force\"",
            "try \"tc -help rm\"",
            "try \"tc -help --force\"",
            "try \"tc -?\"",
            "try \"tc all\"",
            "try \"tc -help all\"",
            "see 'tc help <command>'",
            "try \"tc\"",
        ] {
            assert!(argvs(text, "tc").is_empty(), "{text}");
        }
    }

    #[test]
    fn a_listed_command_may_stand_in_the_quote() {
        let listed = v(&["qdisc"]);
        let words = v(&["help", "qdisc"]);
        assert!(InertArgv::help_hint(&words, &[], &listed).is_some());
        assert!(InertArgv::help_hint(&words, &[], &[]).is_none());
        // A command alone would run it, so a help word is required.
        assert!(InertArgv::help_hint(&v(&["qdisc"]), &[], &listed).is_none());
    }

    #[test]
    fn a_subcommand_hint_opens_with_the_nodes_own_path() {
        let node = v(&["sub"]);
        assert!(InertArgv::help_hint(&v(&["sub", "-h"]), &node, &[]).is_some());
        assert!(InertArgv::help_hint(&v(&["-h"]), &node, &[]).is_none());
        assert!(InertArgv::help_hint(&v(&["other", "-h"]), &node, &[]).is_none());
    }

    #[test]
    fn at_most_two_hints_and_none_already_run() {
        let text = "try \"x -h\", \"x -help\" or \"x help\"";
        assert_eq!(argvs(text, "x"), vec![v(&["-h"]), v(&["-help"])]);
        assert_eq!(
            hints(text, "x", &[], &[v(&["-h"])])
                .iter()
                .map(InertArgv::args)
                .collect::<Vec<_>>(),
            vec![v(&["-help"]), v(&["help"])]
        );
    }

    #[test]
    fn an_apostrophe_inside_a_word_opens_no_quote() {
        assert!(argvs("don't try x -help' ever", "x").is_empty());
    }

    /// Answers each argv from a table and records every argv it is asked.
    struct Scripted {
        answers: Vec<(Vec<String>, &'static str)>,
        asked: std::sync::Mutex<Vec<Vec<String>>>,
    }

    impl Probe for Scripted {
        fn run(
            &self,
            tool_path: &Path,
            argv: &InertArgv,
            _timeout: std::time::Duration,
        ) -> Result<ExecOutput, crate::exec::ExecError> {
            let args = argv.args();
            self.asked.lock().unwrap().push(args.clone());
            match self.answers.iter().find(|(a, _)| *a == args) {
                Some((_, text)) => Ok(ExecOutput {
                    stdout: Vec::new(),
                    stderr: text.as_bytes().to_vec(),
                    exit_code: Some(0),
                    timed_out: false,
                }),
                None => Err(crate::exec::ExecError::TranscriptMiss {
                    tool: tool_path.display().to_string(),
                    argv: args,
                }),
            }
        }
    }

    fn scripted(answers: &[(&[&str], &'static str)]) -> Scripted {
        Scripted {
            answers: answers.iter().map(|(a, t)| (v(a), *t)).collect(),
            asked: Default::default(),
        }
    }

    const REFUSAL: &str = "Option \"--help\" is unknown, try \"tool -help\".\n";
    const PAGE: &str = "Usage: tool [OPTIONS]\n\nOptions:\n  -a, --all  Show all\n";

    /// The real argv reaches the probe: the hint is run as `-help`, and its
    /// help-shaped answer replaces the refusal.
    #[test]
    fn a_hint_that_answers_with_help_replaces_the_refusal() {
        let probe = scripted(&[(&["-help"], PAGE)]);
        let found = follow(
            &probe,
            Path::new("/x/tool"),
            &[],
            REFUSAL,
            &[v(&["--help"])],
        )
        .expect("followed");
        assert_eq!(found.argv, "-help");
        assert_eq!(found.text, PAGE);
        assert_eq!(*probe.asked.lock().unwrap(), vec![v(&["-help"])]);
    }

    /// An answer that is not help is discarded, so the first answer stands.
    #[test]
    fn a_hint_that_answers_with_a_refusal_is_discarded() {
        let probe = scripted(&[(&["-help"], REFUSAL)]);
        assert!(follow(&probe, Path::new("/x/tool"), &[], REFUSAL, &[]).is_none());
    }

    /// A quote naming another program, or an unsafe word, sends nothing.
    #[test]
    fn nothing_is_sent_for_an_unsafe_or_foreign_quote() {
        let probe = scripted(&[]);
        for text in [
            "try \"other -help\"",
            "try \"tool --force\"",
            "try \"tool rm -h\"",
        ] {
            assert!(follow(&probe, Path::new("/x/tool"), &[], text, &[]).is_none());
        }
        assert!(probe.asked.lock().unwrap().is_empty());
    }

    /// A page that parses has structure, so the rule's trigger is false even
    /// when the page quotes a help invocation in prose.
    #[test]
    fn a_usable_page_does_not_trigger_the_rule() {
        let page = format!("{PAGE}\nSee 'tool help <command>' or try \"tool -help\".\n");
        assert!(!gives_nothing(&page));
        assert!(gives_nothing(REFUSAL));
        assert!(gives_nothing(""));
    }
}
