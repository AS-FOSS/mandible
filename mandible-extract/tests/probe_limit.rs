//! A slow help probe is read when it finishes inside the cap and reported as
//! timed out when it does not, never answered from another probe's text.

use mandible_core::{CommandNode, Provenance, Source};
use mandible_extract::exec::{
    with_extract_limit, ExecError, ExecOutput, InertArgv, Probe, EXTRACT_LIMIT,
};
use mandible_extract::{ExtractionTier, ResolvedTool, Runner};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const MAN_PAGE: &str =
    "WIDGET-RUN(1)                Widget Manual                WIDGET-RUN(1)\n\n\
                        NAME\n       widget-run - run a widget\n";
const USAGE: &str = "Usage: widget run [OPTIONS]\n\nOptions:\n  -f, --force  Overwrite\n";

/// Answers by argv and takes a pretend amount of time: an answer slower than
/// the cap it was given comes back stopped and empty.
struct Slow {
    answers: HashMap<Vec<String>, (&'static str, Duration)>,
}

impl Slow {
    fn new(answers: &[(&[&str], &'static str, u64)]) -> Arc<Self> {
        Arc::new(Self {
            answers: answers
                .iter()
                .map(|(argv, out, secs)| {
                    (
                        argv.iter().map(|s| s.to_string()).collect(),
                        (*out, Duration::from_secs(*secs)),
                    )
                })
                .collect(),
        })
    }
}

impl Probe for Slow {
    fn run(
        &self,
        tool_path: &Path,
        argv: &InertArgv,
        timeout: Duration,
    ) -> Result<ExecOutput, ExecError> {
        let args = argv.args();
        let Some((out, takes)) = self.answers.get(&args) else {
            return Err(ExecError::TranscriptMiss {
                tool: tool_path.display().to_string(),
                argv: args,
            });
        };
        let stopped = *takes > timeout;
        Ok(ExecOutput {
            stdout: if stopped {
                Vec::new()
            } else {
                out.as_bytes().to_vec()
            },
            stderr: Vec::new(),
            exit_code: (!stopped).then_some(0),
            timed_out: stopped,
        })
    }
}

fn fill(probe: Arc<Slow>) -> mandible_extract::FillResult {
    let runner = Runner::new(vec![
        Box::new(mandible_extract::help_text::HelpTextTier::new(probe)) as Box<dyn ExtractionTier>,
    ]);
    let tool = ResolvedTool {
        name: "widget".to_string(),
        path: Some(PathBuf::from("/opt/widget")),
        version: None,
    };
    let mut existing = CommandNode::new("run", Provenance::single(Source::HelpText));
    existing.heading_attested = true;
    runner.fill_node(&tool, &["widget".to_string(), "run".to_string()], existing)
}

/// `--help` execs the man page at once; `-h` answers after `secs`.
fn man_page_then_slow_short(secs: u64) -> Arc<Slow> {
    Slow::new(&[
        (&["run", "--help"], MAN_PAGE, 0),
        (&["run", "-h"], USAGE, secs),
    ])
}

#[test]
fn the_default_cap_is_thirty_seconds() {
    assert_eq!(EXTRACT_LIMIT, Duration::from_secs(30));
}

#[test]
fn a_twelve_second_probe_is_read() {
    let filled = fill(man_page_then_slow_short(12));
    assert!(!filled.timed_out);
    assert!(
        filled.node.flags().count() == 1,
        "the -h page was not parsed"
    );
}

#[test]
fn a_probe_over_the_cap_is_reported_and_not_answered_from_the_man_page() {
    let filled = fill(man_page_then_slow_short(40));
    assert!(filled.timed_out);
    assert!(filled.node.flags().next().is_none());
    assert!(filled.node.unparsed.is_empty(), "the man page was shown");
}

#[test]
fn the_same_slow_probe_gives_the_same_page_every_time() {
    for _ in 0..5 {
        assert!(fill(man_page_then_slow_short(40)).timed_out);
    }
}

#[test]
fn a_scope_with_no_limit_reads_a_slow_probe() {
    let filled = with_extract_limit(None, || fill(man_page_then_slow_short(40)));
    assert!(!filled.timed_out);
    assert!(filled.node.flags().count() == 1);
}

#[test]
fn a_scope_with_a_shorter_cap_stops_a_twelve_second_probe() {
    let filled = with_extract_limit(Some(Duration::from_secs(10)), || {
        fill(man_page_then_slow_short(12))
    });
    assert!(filled.timed_out);
}

#[test]
fn a_long_help_that_timed_out_falls_through_to_the_short_flag() {
    let probe = Slow::new(&[(&["run", "--help"], USAGE, 40), (&["run", "-h"], USAGE, 0)]);
    let filled = fill(probe);
    assert!(!filled.timed_out);
    assert!(filled.node.flags().count() == 1);
}

#[test]
fn both_flags_timing_out_is_a_timeout() {
    let probe = Slow::new(&[(&["run", "--help"], USAGE, 40), (&["run", "-h"], USAGE, 40)]);
    assert!(fill(probe).timed_out);
}

#[test]
fn the_verbatim_view_of_a_timed_out_page_says_so_from_the_kept_probes() {
    use mandible_extract::help_text::{raw_help_kept, raw_help_with_probe};
    use mandible_extract::{ExtractError, NodeHints};

    let probe = man_page_then_slow_short(40);
    let tool = ResolvedTool {
        name: "widget".to_string(),
        path: Some(PathBuf::from("/opt/widget")),
        version: None,
    };
    let path = vec!["widget".to_string(), "run".to_string()];
    let hints = NodeHints {
        heading_attested: true,
        abbrev_probe_attested: false,
        help_word_attested: false,
    };
    let tier = mandible_extract::help_text::HelpTextTier::new(probe.clone());
    let (_, kept) = tier.extract_node_kept(&tool, &path, hints);

    let shown = raw_help_kept(&[&kept], &tool, &path, hints)
        .expect("the fill kept every probe the view needs");
    assert!(matches!(shown, Err(ExtractError::TimedOut)));
    let live = raw_help_with_probe(probe.as_ref(), &tool, &path, hints);
    assert!(matches!(live, Err(ExtractError::TimedOut)));
}
