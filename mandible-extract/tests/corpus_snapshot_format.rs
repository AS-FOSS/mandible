//! End-to-end proof that `mandible_core::snapshot`'s format survives contact
//! with real `--help` output, not just synthetic three-field nodes.
//!
//! This drives the actual `HelpTextTier::extract_node` — real argv
//! construction, real framework detection, real `sections::parse_with_profile`
//! grammar — against a genuine captured fixture
//! (`tests/fixtures/help_text/git_help.stdout`), replayed with zero
//! subprocesses through the `Transcript` probe from the previous batch of
//! work (`mandible-extract/src/exec/probe.rs`). The resulting `CommandNode`
//! is run through `mandible_core::to_snapshot` and asserted with
//! `insta::assert_yaml_snapshot!`.
//!
//! This is deliberately *not* the corpus runner (`cargo xtask corpus`) —
//! that is out of scope for this batch (see `AGENTS.md`/`corpus/README.md`).
//! It exists only to prove the snapshot format itself is sound against real
//! output, per this batch's own verification requirement: "look at the
//! actual generated snapshot for a real tool."
//!
//! Lives in `mandible-extract` rather than `mandible-core` because the
//! extraction pipeline and the fixture corpus both live here;
//! `mandible-core` has no tier/parser to run against real bytes.

use mandible_extract::exec::Transcript;
use mandible_extract::help_text::HelpTextTier;
use mandible_extract::{ExtractionTier, NodeHints, ResolvedTool};
use std::path::PathBuf;
use std::sync::Arc;

/// Both fixtures below extract a tool's root, whose name came from what the
/// user typed rather than from any parser guess — structurally attested by
/// definition, matching what `Runner::extract_full_for` passes in
/// production.
const ATTESTED: NodeHints = NodeHints {
    heading_attested: true,
};

fn fixture(name: &str) -> String {
    // `tar --help` and `git --help`'s captures live once, as the corpus
    // regression fixtures (`corpus/tar/<version>/help.txt`,
    // `corpus/git/<version>/help.txt` — see corpus/README.md), rather than
    // a byte-identical second copy under `tests/fixtures/`.
    let path = match name {
        "tar_help.stdout" => format!("{}/../corpus/tar/1.35/help.txt", env!("CARGO_MANIFEST_DIR")),
        "git_help.stdout" => format!(
            "{}/../corpus/git/2.43.0/help.txt",
            env!("CARGO_MANIFEST_DIR")
        ),
        _ => format!(
            "{}/tests/fixtures/help_text/{name}",
            env!("CARGO_MANIFEST_DIR")
        ),
    };
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading fixture {path}: {e}"))
}

fn exec_output(stdout: &str) -> mandible_extract::exec::ExecOutput {
    mandible_extract::exec::ExecOutput {
        stdout: stdout.as_bytes().to_vec(),
        stderr: Vec::new(),
        exit_code: Some(0),
        timed_out: false,
    }
}

/// `git --help`, replayed through the real `HelpTextTier` pipeline: real
/// argv construction (`Transcript` is keyed on exactly `["--help"]`, the
/// same argv `extract_node`'s own probe call produces — see
/// `src/help_text/mod.rs`'s own `extract_node_replays_from_a_transcript_keyed_on_the_real_argv`
/// test for the negative case that catches a drift here), real framework
/// detection, and the real section grammar. `git`'s command groups
/// ("start a working area", "work on the current change", ...) are exactly
/// the case `mandible_core::snapshot`'s doc comment cites for why
/// `subcommands` order is never sorted — this fixture is a real instance of
/// that grouping, not a hypothetical one.
#[test]
fn git_help_through_the_real_pipeline_snapshots_readably() {
    let raw = fixture("git_help.stdout");
    let transcript = Transcript::new([(vec!["--help".to_string()], exec_output(&raw))]);
    let tier = HelpTextTier::new(Arc::new(transcript));
    let tool = ResolvedTool {
        name: "git".to_string(),
        path: Some(PathBuf::from("/replayed/git")),
        version: None,
    };

    let node = tier
        .extract_node(&tool, &["git".to_string()], ATTESTED)
        .expect("the transcript covers the exact argv extract_node sends");

    // Sanity: this run actually parsed structure, not the level-3 verbatim
    // degradation path — otherwise this test would "pass" while proving
    // nothing about the snapshot format's handling of a real tree (AGENTS.md
    // §3.1: a green test that proves nothing is worse than a red one).
    assert!(
        !node.subcommands.is_empty(),
        "expected git --help to parse real subcommands, got none: {node:?}"
    );

    insta::assert_yaml_snapshot!(mandible_core::to_snapshot(&node));
}

/// `tar --help` (171 flags in named groups, per spec §13.2) through the same
/// real pipeline — the flag-heavy counterpart to the subcommand-heavy `git`
/// case above, exercising `FlagSnapshot`'s normalization (the `bool`
/// omission and `ValueKind::None` omission) against a real, large flag list
/// rather than a two-flag synthetic one.
#[test]
fn tar_help_through_the_real_pipeline_snapshots_readably() {
    let raw = fixture("tar_help.stdout");
    let transcript = Transcript::new([(vec!["--help".to_string()], exec_output(&raw))]);
    let tier = HelpTextTier::new(Arc::new(transcript));
    let tool = ResolvedTool {
        name: "tar".to_string(),
        path: Some(PathBuf::from("/replayed/tar")),
        version: None,
    };

    let node = tier
        .extract_node(&tool, &["tar".to_string()], ATTESTED)
        .expect("the transcript covers the exact argv extract_node sends");

    assert!(
        node.flags().next().is_some(),
        "expected tar --help to parse real flags, got none: {node:?}"
    );

    insta::assert_yaml_snapshot!(mandible_core::to_snapshot(&node));
}
