//! The verbatim view shows the probes a fill ran, and runs none itself.

use mandible_extract::exec::{ExecError, ExecOutput, InertArgv, Probe};
use mandible_extract::help_text::{probe_raw_with, raw_help_kept, raw_help_with_probe};
use mandible_extract::{ExtractionTier, KeptProbes, NodeHints, ResolvedTool};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Answers from a fixed argv map and counts every call it serves.
struct Counting {
    answers: HashMap<Vec<String>, (&'static str, &'static str)>,
    calls: AtomicUsize,
}

impl Counting {
    fn new(answers: &[(&[&str], &'static str, &'static str)]) -> Arc<Self> {
        Arc::new(Self {
            answers: answers
                .iter()
                .map(|(argv, out, err)| {
                    (argv.iter().map(|s| s.to_string()).collect(), (*out, *err))
                })
                .collect(),
            calls: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Probe for Counting {
    fn run(
        &self,
        tool_path: &Path,
        argv: &InertArgv,
        _timeout: Duration,
    ) -> Result<ExecOutput, ExecError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let args = argv.args();
        match self.answers.get(&args) {
            Some((out, err)) => Ok(ExecOutput {
                stdout: out.as_bytes().to_vec(),
                stderr: err.as_bytes().to_vec(),
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

const ATTESTED: NodeHints = NodeHints {
    heading_attested: true,
    abbrev_probe_attested: false,
    help_word_attested: false,
};

const UNATTESTED: NodeHints = NodeHints {
    heading_attested: false,
    abbrev_probe_attested: false,
    help_word_attested: false,
};

const ROOT: &str = "Usage: widget [COMMAND]\n\nCommands:\n  add   Add a thing\n";

fn tool() -> ResolvedTool {
    ResolvedTool {
        name: "widget".to_string(),
        path: Some(PathBuf::from("/opt/widget")),
        version: None,
    }
}

fn path(words: &[&str]) -> Vec<String> {
    std::iter::once("widget")
        .chain(words.iter().copied())
        .map(String::from)
        .collect()
}

fn text(lines: &[mandible_core::Text]) -> String {
    lines
        .iter()
        .map(|l| l.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn fill(probe: &Arc<Counting>, words: &[&str], hints: NodeHints) -> (bool, KeptProbes) {
    let tier = mandible_extract::help_text::HelpTextTier::new(probe.clone());
    let (result, kept) = tier.extract_node_kept(&tool(), &path(words), hints);
    (result.is_ok(), kept)
}

#[test]
fn the_kept_view_equals_a_live_one_and_runs_nothing() {
    let probe = Counting::new(&[
        (&["--help"], ROOT, ""),
        (
            &["add", "--help"],
            "Usage: widget add [OPTIONS]\n\nOptions:\n  -f, --force  Overwrite\n",
            "warning: stderr too\n",
        ),
    ]);
    let (ok, kept) = fill(&probe, &["add"], ATTESTED);
    assert!(ok);
    let after_fill = probe.calls();

    let shown = raw_help_kept(&[&kept], &tool(), &path(&["add"]), ATTESTED)
        .expect("everything the view needs was kept")
        .expect("the view builds");
    assert_eq!(
        probe.calls(),
        after_fill,
        "showing the kept view ran a probe"
    );

    let live = raw_help_with_probe(probe.as_ref(), &tool(), &path(&["add"]), ATTESTED).unwrap();
    assert_eq!(text(&shown.0), text(&live.0));
    assert_eq!(shown.1, live.1);
    assert!(text(&shown.0).contains("── stderr ──"));
}

#[test]
fn a_failed_fill_keeps_what_the_tool_printed() {
    let probe = Counting::new(&[
        (&["--help"], ROOT, ""),
        (&["add", "--help"], "error: unknown option\n", ""),
    ]);
    let (ok, kept) = fill(&probe, &["add"], ATTESTED);
    assert!(!ok, "precondition: an error-only answer fails the fill");

    let shown = raw_help_kept(&[&kept], &tool(), &path(&["add"]), ATTESTED)
        .expect("the failed fill's output was kept")
        .expect("the view builds");
    assert_eq!(text(&shown.0), "error: unknown option");
}

#[test]
fn a_probe_that_errored_replays_the_same_error() {
    let probe = Counting::new(&[(&["--help"], ROOT, "")]);
    let (_, kept) = fill(&probe, &["add"], ATTESTED);
    let kept_err = raw_help_kept(&[&kept], &tool(), &path(&["add"]), ATTESTED)
        .expect("the error was kept")
        .expect_err("the probe failed");
    let live_err = raw_help_with_probe(probe.as_ref(), &tool(), &path(&["add"]), ATTESTED)
        .expect_err("the probe fails live too");
    assert_eq!(kept_err.to_string(), live_err.to_string());
}

#[test]
fn the_not_attested_fallback_reads_the_roots_kept_output() {
    let probe = Counting::new(&[(&["--help"], ROOT, "")]);
    let (_, root_kept) = fill(&probe, &[], ATTESTED);
    let (ok, ghost_kept) = fill(&probe, &["ghost"], UNATTESTED);
    assert!(!ok);
    let after_fill = probe.calls();

    let with_root = raw_help_kept(
        &[&ghost_kept, &root_kept],
        &tool(),
        &path(&["ghost"]),
        UNATTESTED,
    )
    .expect("the root's output covers the fallback")
    .expect("the view builds");
    assert!(text(&with_root.0).contains("Usage: widget [COMMAND]"));
    assert_eq!(probe.calls(), after_fill);

    assert!(
        raw_help_kept(&[&ghost_kept], &tool(), &path(&["ghost"]), UNATTESTED).is_none(),
        "without the root's output the view cannot be built"
    );
}

#[test]
fn the_expanded_document_of_a_confession_is_what_is_kept() {
    let probe = Counting::new(&[
        (
            &["--help"],
            "Usage: widget [options]\n  Use \"--help all\" for everything\n",
            "",
        ),
        (
            &["--help", "all"],
            "Usage: widget [options]\n\nOptions:\n  -a, --all  Everything\n",
            "",
        ),
    ]);
    let (ok, kept) = fill(&probe, &[], ATTESTED);
    assert!(ok);
    let shown = raw_help_kept(&[&kept], &tool(), &path(&[]), ATTESTED)
        .unwrap()
        .unwrap();
    assert!(text(&shown.0).contains("--all"));
    assert_eq!(shown.1, "--help all");
}

#[test]
fn fetching_for_the_view_keeps_the_probes_it_ran() {
    let probe = Counting::new(&[
        (&["--help"], ROOT, ""),
        (&["add", "--help"], "Usage: widget add\n", ""),
    ]);
    let kept = probe_raw_with(probe.as_ref(), &tool(), &path(&["add"]), ATTESTED);
    let ran = probe.calls();
    assert!(ran > 0 && !kept.is_empty());
    let shown = raw_help_kept(&[&kept], &tool(), &path(&["add"]), ATTESTED)
        .unwrap()
        .unwrap();
    assert!(text(&shown.0).contains("Usage: widget add"));
    assert_eq!(probe.calls(), ran);
}

#[test]
fn the_refusal_names_the_attempt_and_the_tool_and_the_header_names_the_root_argv() {
    let notice = "\"ghost town\" was not run: mandible only runs names that widget's help lists under a heading.";
    let probe = Counting::new(&[(&["--help"], ROOT, "")]);
    let (lines, flag) = raw_help_with_probe(
        probe.as_ref(),
        &tool(),
        &path(&["ghost", "town"]),
        UNATTESTED,
    )
    .expect("a refusal is an answer");
    let shown = text(&lines);
    assert!(
        shown.starts_with(&format!(
            "{notice}\n\nShowing widget --help instead:\n\nUsage: widget [COMMAND]"
        )),
        "{shown}"
    );
    assert_eq!(
        mandible_extract::help_text::render_argv("widget", &["ghost".into(), "town".into()], &flag),
        "widget --help"
    );
}

#[test]
fn without_the_root_help_the_refusal_stands_alone() {
    let probe = Counting::new(&[]);
    let (lines, _) = raw_help_with_probe(probe.as_ref(), &tool(), &path(&["ghost"]), UNATTESTED)
        .expect("a refusal is an answer");
    assert_eq!(
        text(&lines),
        "\"ghost\" was not run: mandible only runs names that widget's help lists under a heading."
    );
}
