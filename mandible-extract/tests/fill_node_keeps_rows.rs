//! A root refill through the real `Runner::fill_node` path must not fold
//! one document's repeated spellings (docs/design.md §16, same-document
//! rows). Real `ffplay --help` bytes replay through the real argv
//! construction; ffplay documents `-framerate` once per format section.

use mandible_core::{CommandNode, Entity};
use mandible_extract::exec::{ExecOutput, Transcript};
use mandible_extract::{default_tiers_with_probe, resolve_tool, Runner};
use std::path::PathBuf;
use std::sync::Arc;

fn ffplay_runner() -> (Runner, mandible_extract::ResolvedTool) {
    let dir = format!(
        "{}/../corpus/ffplay/6.1.1-3ubuntu5",
        env!("CARGO_MANIFEST_DIR")
    );
    let stdout = std::fs::read(format!("{dir}/help.txt")).expect("ffplay help.txt");
    let transcript = Transcript::new([(
        vec!["--help".to_string()],
        ExecOutput {
            stdout,
            stderr: Vec::new(),
            exit_code: Some(0),
            timed_out: false,
        },
    )]);
    let runner = Runner::new(default_tiers_with_probe(Arc::new(transcript)));
    let mut tool = resolve_tool("ffplay");
    tool.path = Some(PathBuf::from("/replayed/ffplay"));
    (runner, tool)
}

fn rows(node: &CommandNode) -> Vec<(String, Option<String>)> {
    node.flags()
        .map(|f: &Entity| {
            (
                f.spellings
                    .iter()
                    .map(|s| s.name.clone())
                    .collect::<Vec<_>>()
                    .join("/"),
                f.group.as_ref().map(|g| g.as_str().to_string()),
            )
        })
        .collect()
}

#[test]
fn root_refill_keeps_every_row_of_one_document_in_order() {
    let (runner, tool) = ffplay_runner();
    let root = runner.extract_full_for(&tool).root.expect("ffplay root");
    let single = rows(&root);
    assert!(single.len() > 1000, "fixture should parse 1136 flags");

    let filled = runner
        .fill_node(&tool, std::slice::from_ref(&tool.name), root.clone())
        .node;
    let after = rows(&filled);

    assert_eq!(after.len(), single.len(), "flag count changed by refill");
    assert_eq!(after, single, "refill reordered or retargeted rows");
    for (a, b) in root.flags().zip(filled.flags()) {
        assert_eq!(
            a.choices, b.choices,
            "choices changed for {:?}",
            a.spellings
        );
        assert_eq!(a.value_name, b.value_name);
    }
}
