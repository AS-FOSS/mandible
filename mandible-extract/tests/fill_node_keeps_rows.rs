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
    replay_runner("ffplay", "ffplay/6.1.1-3ubuntu5", 0)
}

fn replay_runner(
    tool_name: &str,
    fixture: &str,
    code: i32,
) -> (Runner, mandible_extract::ResolvedTool) {
    let dir = format!("{}/../corpus/{fixture}", env!("CARGO_MANIFEST_DIR"));
    let stdout = std::fs::read(format!("{dir}/help.txt")).expect("help.txt");
    let stderr = std::fs::read(format!("{dir}/help.stderr.txt")).unwrap_or_default();
    let transcript = Transcript::new([(
        vec!["--help".to_string()],
        ExecOutput {
            stdout,
            stderr,
            exit_code: Some(code),
            timed_out: false,
        },
    )]);
    let runner = Runner::new(default_tiers_with_probe(Arc::new(transcript)));
    let mut tool = resolve_tool(tool_name);
    tool.path = Some(PathBuf::from(format!("/replayed/{tool_name}")));
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
    assert!(
        single.len() > 500,
        "fixture should parse several hundred flags"
    );

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

fn filled_and_single(
    runner: &Runner,
    tool: &mandible_extract::ResolvedTool,
) -> (CommandNode, CommandNode) {
    let root = runner.extract_full_for(tool).root.expect("root");
    let filled = runner
        .fill_node(tool, std::slice::from_ref(&tool.name), root.clone())
        .node;
    (root, filled)
}

fn spelling_rows(node: &CommandNode, name: &str) -> Vec<Option<String>> {
    node.flags()
        .filter(|f| f.spellings.iter().any(|s| s.name == name))
        .map(|f| f.group.as_ref().map(|g| g.as_str().to_string()))
        .collect()
}

/// `(heading, row text)` for every AVOptions row of the raw help, the
/// first print of an identical row under an identical heading only.
fn raw_avoption_rows(help: &str) -> Vec<(String, String)> {
    let mut heading = String::new();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for line in help.lines() {
        if line.ends_with("AVOptions:") && !line.starts_with(' ') {
            heading = line.to_string();
        } else if line.starts_with("  -") && !heading.is_empty() {
            let name = line.split_whitespace().next().unwrap().to_string();
            if seen.insert((heading.clone(), line.to_string())) {
                out.push((heading.clone(), name));
            }
        }
    }
    out
}

#[test]
fn identical_rows_under_one_heading_fold_to_one_in_raw_order() {
    let (runner, tool) = ffplay_runner();
    let (single, filled) = filled_and_single(&runner, &tool);
    let help = std::fs::read_to_string(format!(
        "{}/../corpus/ffplay/6.1.1-3ubuntu5/help.txt",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("ffplay help.txt");
    let expected = raw_avoption_rows(&help);
    for (label, node) in [("single", &single), ("filled", &filled)] {
        let got: Vec<(String, String)> = node
            .flags()
            .filter_map(|f| {
                let g = f.group.as_ref()?.as_str();
                g.ends_with("AVOptions:")
                    .then(|| (g.to_string(), format!("-{}", f.spellings[0].name)))
            })
            .collect();
        let rows: Vec<_> = got
            .iter()
            .filter(|r| r.0 != "AVCodecContext AVOptions:")
            .collect();
        let want: Vec<_> = expected
            .iter()
            .filter(|r| r.0 != "AVCodecContext AVOptions:")
            .collect();
        assert_eq!(rows, want, "{label}: AVOptions rows differ from raw order");
    }
    let heading = Some("generic raw video demuxer AVOptions:".to_string());
    let n = spelling_rows(&filled, "framerate")
        .into_iter()
        .filter(|g| *g == heading)
        .count();
    assert_eq!(n, 1, "-framerate under generic raw video demuxer");
    let vplayer = Some("text/vplayer/stl/pjs/subviewer1 decoder AVOptions:".to_string());
    let n = spelling_rows(&filled, "keep_ass_markup")
        .into_iter()
        .filter(|g| *g == vplayer)
        .count();
    assert_eq!(n, 1, "-keep_ass_markup under the text/vplayer heading");
    assert!(
        spelling_rows(&filled, "framerate").len() > 5,
        "-framerate rows under other headings must stay"
    );
}

#[test]
fn lsof_end_of_options_is_one_described_row_and_t_rows_keep_raw_order() {
    let (runner, tool) = replay_runner("lsof", "lsof/4.95.0", 1);
    let (single, filled) = filled_and_single(&runner, &tool);
    for (label, node) in [("single", &single), ("filled", &filled)] {
        let dd: Vec<_> = node
            .flags()
            .filter(|f| f.spellings.iter().any(|s| s.name == "--"))
            .collect();
        assert_eq!(dd.len(), 1, "{label}: one `--` row");
        assert!(
            dd[0]
                .description
                .as_ref()
                .is_some_and(|d| d.as_str().contains("end option scan")),
            "{label}: `--` carries its description"
        );
    }
    let order = |n: &CommandNode| -> Vec<String> {
        n.flags()
            .filter(|f| {
                f.spellings
                    .iter()
                    .any(|s| s.name == "T" || s.name == "S" || s.name == "g")
            })
            .map(|f| {
                f.spellings
                    .iter()
                    .map(|s| s.name.clone())
                    .collect::<Vec<_>>()
                    .join("/")
                    + &f.value_name.clone().unwrap_or_default()
            })
            .collect()
    };
    assert_eq!(order(&filled), order(&single), "fill_node reordered rows");
    // Raw help: the second table prints `-S [t]`, `-T fqs`, `-g [s]` in that order.
    let o = order(&filled);
    let s = o.iter().position(|x| x.starts_with('S')).unwrap();
    assert!(
        o[s + 1].starts_with('T') && o[s + 1].ends_with("fqs"),
        "{o:?}"
    );
}
