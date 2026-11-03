//! `<command> help` for a production-listed command (spec §6 rule 2f, S-257).

use std::path::Path;

use super::hint::{self, Followed};
use crate::exec::{InertArgv, Probe};

/// `<command> help` when attested, then rule 2e's quoted hints.
pub(super) fn follow(
    probe: &dyn Probe,
    tool_path: &Path,
    node_words: &[String],
    help_slot_attested: bool,
    text: &str,
    tried: &[Vec<String>],
) -> Option<Followed> {
    let mut tried = tried.to_vec();
    if let Some(argv) = InertArgv::help_after_command(node_words, help_slot_attested) {
        tried.push(argv.args());
        if let Some(found) = hint::run_kept_if_help(probe, tool_path, node_words, &argv) {
            return Some(found);
        }
    }
    hint::follow(probe, tool_path, node_words, text, &tried)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::{ExecError, ExecOutput};

    fn v(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
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
        ) -> Result<ExecOutput, ExecError> {
            let args = argv.args();
            self.asked.lock().unwrap().push(args.clone());
            match self.answers.iter().find(|(a, _)| *a == args) {
                Some((_, text)) => Ok(ExecOutput {
                    stdout: Vec::new(),
                    stderr: text.as_bytes().to_vec(),
                    exit_code: Some(0),
                    timed_out: false,
                }),
                None => Err(ExecError::TranscriptMiss {
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

    const REFUSAL: &str = "Command \"--help\" is unknown.\n";
    const PAGE: &str = "Usage: tool link add NAME\n\nOptions:\n  -a, --all  Show all\n";
    const TOOL: &str = "/x/tool";

    fn run(probe: &Scripted, attested: bool, node: &[&str], text: &str) -> Option<Followed> {
        follow(
            probe,
            Path::new(TOOL),
            &v(node),
            attested,
            text,
            &[v(&["link", "--help"])],
        )
    }

    /// The real argv reaches the probe: `link help`, once, and its
    /// help-shaped answer replaces the refusal.
    #[test]
    fn an_attested_command_is_asked_for_its_help_page() {
        let probe = scripted(&[(&["link", "help"], PAGE)]);
        let found = run(&probe, true, &["link"], REFUSAL).expect("followed");
        // The path is not repeated: the page header reads `tool link help`.
        assert_eq!(found.argv, "help");
        assert_eq!(
            super::super::render_argv("tool", &v(&["link"]), &found.argv),
            "tool link help"
        );
        assert_eq!(found.text, PAGE);
        assert_eq!(*probe.asked.lock().unwrap(), vec![v(&["link", "help"])]);
    }

    /// Without the evidence nothing is sent: a node that is not made by a
    /// production, or whose parent shows no `help`, is never asked.
    #[test]
    fn a_node_without_the_evidence_sends_nothing() {
        let probe = scripted(&[(&["link", "help"], PAGE)]);
        assert!(run(&probe, false, &["link"], REFUSAL).is_none());
        assert!(probe.asked.lock().unwrap().is_empty());
    }

    /// The root has no command word to put before `help`.
    #[test]
    fn the_root_is_never_asked() {
        let probe = scripted(&[(&["help"], PAGE)]);
        assert!(run(&probe, true, &[], REFUSAL).is_none());
        assert!(probe.asked.lock().unwrap().is_empty());
    }

    /// An answer that is not help is discarded, and a quote of the same
    /// argv is not sent a second time.
    #[test]
    fn a_refusal_is_discarded_and_the_same_argv_is_not_resent() {
        let probe = scripted(&[(&["link", "help"], REFUSAL)]);
        let text = "Command \"--help\" is unknown, try \"tool link help\".\n";
        assert!(run(&probe, true, &["link"], text).is_none());
        assert_eq!(*probe.asked.lock().unwrap(), vec![v(&["link", "help"])]);
    }

    /// A quote that names another way is still followed when the command's
    /// own `help` gave nothing.
    #[test]
    fn a_quoted_hint_follows_when_the_command_help_failed() {
        let probe = scripted(&[(&["link", "-h"], PAGE)]);
        let text = "try \"tool link -h\"\n";
        let found = run(&probe, true, &["link"], text).expect("followed");
        assert_eq!(found.argv, "-h");
        assert_eq!(
            *probe.asked.lock().unwrap(),
            vec![v(&["link", "help"]), v(&["link", "-h"])]
        );
    }

    /// Rule 0 wins: a program on the help-only list is sent nothing but
    /// `--help`, so the attested `link help` is refused before any spawn.
    #[test]
    fn a_help_only_program_is_not_asked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pkill");
        std::fs::write(&path, "#!/bin/sh\ntouch \"$0.ran\"\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let live = crate::exec::LiveProbe;
        assert!(follow(&live, &path, &v(&["link"]), true, REFUSAL, &[]).is_none());
        assert!(!dir.path().join("pkill.ran").exists(), "the shim ran");
    }
}
