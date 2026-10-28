//! The cached, windowed detail renderer must draw exactly what the
//! reference renderer (rebuild the whole document, wrapping `Paragraph`
//! with a scroll offset, every frame) draws: same symbols, same styles, in
//! every cell. The documents are the root pages of the corpus fixtures,
//! parsed through the real help-text tier from each fixture's captured
//! help, so the shapes are real ones.

use super::{render, render_reference};
use crate::app::{App, Focus};
use mandible_core::{CommandNode, FlagKey};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::Terminal;
use std::path::Path;
use std::sync::Arc;

pub(super) fn corpus_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus")
}

/// The quoted strings of one `key = ...` line inside the first
/// `[[capture]]` table of a fixture's `meta.toml`.
fn first_capture(meta: &str) -> (Vec<String>, Option<String>, Option<String>) {
    let mut in_capture = false;
    let (mut argv, mut stdout, mut stderr) = (Vec::new(), None, None);
    for line in meta.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            if in_capture {
                break;
            }
            in_capture = line == "[[capture]]";
            continue;
        }
        if !in_capture {
            continue;
        }
        let quoted: Vec<String> = line
            .split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_string)
            .collect();
        if line.starts_with("argv") {
            argv = quoted;
        } else if line.starts_with("stdout") {
            stdout = quoted.into_iter().next();
        } else if line.starts_with("stderr") {
            stderr = quoted.into_iter().next();
        }
    }
    (argv, stdout, stderr)
}

/// Every corpus fixture whose first capture replays into a parsed root.
fn sampled_nodes() -> &'static (Vec<(String, CommandNode)>, usize) {
    static NODES: std::sync::OnceLock<(Vec<(String, CommandNode)>, usize)> =
        std::sync::OnceLock::new();
    NODES.get_or_init(|| corpus_nodes(true))
}

pub(super) fn corpus_nodes(sampled: bool) -> (Vec<(String, CommandNode)>, usize) {
    let mut nodes = Vec::new();
    let mut total = 0;
    let mut dirs: Vec<_> = std::fs::read_dir(corpus_root())
        .expect("corpus dir")
        .flatten()
        .flat_map(|tool| {
            std::fs::read_dir(tool.path())
                .into_iter()
                .flatten()
                .flatten()
        })
        .map(|e| e.path())
        .filter(|p| p.join("expected.snap").is_file() && p.join("meta.toml").is_file())
        .collect();
    dirs.sort();
    if sampled {
        // Debug-build parsing of the whole corpus takes minutes; the
        // ignored full test below covers all of it.
        let size = |d: &std::path::PathBuf| {
            std::fs::read_dir(d)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.metadata().map_or(0, |m| m.len()))
                .sum::<u64>()
        };
        // Capped: the largest captures take minutes to parse unoptimized.
        let mut keep: Vec<_> = dirs
            .iter()
            .step_by(24)
            .filter(|d| size(d) < 40_000)
            .cloned()
            .collect();
        keep.sort();
        keep.dedup();
        dirs = keep;
    }
    for dir in dirs {
        total += 1;
        if let Some(node) = replay_fixture(&dir) {
            nodes.push((dir.display().to_string(), node));
        }
    }
    (nodes, total)
}

/// The root node a fixture's first captured `--help` parses into, or `None`
/// when the fixture has no capture to replay.
pub(super) fn replay_fixture(dir: &Path) -> Option<CommandNode> {
    let meta = std::fs::read_to_string(dir.join("meta.toml")).unwrap();
    let (argv, stdout, stderr) = first_capture(&meta);
    if argv.len() < 2 {
        return None;
    }
    let read = |f: Option<String>| {
        f.and_then(|f| std::fs::read(dir.join(f)).ok())
            .unwrap_or_default()
    };
    let transcript = mandible_extract::exec::Transcript::new([(
        argv[1..].to_vec(),
        mandible_extract::exec::ExecOutput {
            stdout: read(stdout),
            stderr: read(stderr),
            exit_code: Some(0),
            timed_out: false,
        },
    )]);
    let tier = mandible_extract::help_text::HelpTextTier::new(Arc::new(transcript));
    let tool = mandible_extract::ResolvedTool {
        name: argv[0].clone(),
        path: Some(std::path::PathBuf::from("/replayed").join(&argv[0])),
        version: None,
    };
    const ATTESTED: mandible_extract::NodeHints = mandible_extract::NodeHints {
        heading_attested: true,
        abbrev_probe_attested: false,
        help_word_attested: false,
    };
    mandible_extract::ExtractionTier::extract_node(&tier, &tool, &[argv[0].clone()], ATTESTED).ok()
}

fn draw(app: &App, w: u16, h: u16, reference: bool) -> Buffer {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| {
        let area = Rect::new(0, 0, w, h);
        if reference {
            render_reference(f, area, app);
        } else {
            render(f, area, app);
        }
    })
    .unwrap();
    term.backend().buffer().clone()
}

fn app_with(
    node: &CommandNode,
    palette: crate::style::Palette,
    glyphs: crate::glyphs::Glyphs,
) -> App {
    let mut app = App::new("tool".to_string(), node.clone());
    app.palette = palette;
    app.color_enabled = palette.color;
    app.glyphs = glyphs;
    app.focus = Focus::Detail;
    app
}

/// Draw both ways, twice each (the second fast frame is a cache hit), and
/// compare every cell.
fn assert_same(label: &str, fast: &App, slow: &App, w: u16, h: u16) {
    let want = draw(slow, w, h, true);
    for frame in 0..2 {
        let got = draw(fast, w, h, false);
        assert!(
            got == want,
            "{label} {w}x{h} frame {frame}: cached render differs from reference\n\
             reference:\n{}\ncached:\n{}",
            dump(&want),
            dump(&got)
        );
    }
}

type Step = Box<dyn Fn(&mut App)>;

fn dump(b: &Buffer) -> String {
    let a = b.area;
    (0..a.height)
        .map(|y| (0..a.width).map(|x| b[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn cached_windowed_render_is_cell_identical_to_the_reference_on_a_corpus_sample() {
    let (nodes, total) = corpus_nodes(true);
    assert!(
        nodes.len() >= 6 && nodes.len() * 10 >= total * 8,
        "{} of {total}",
        nodes.len()
    );
    check_identity(&nodes);
}

/// Every corpus fixture. Minutes in a debug build, so it is opt-in:
/// `cargo nextest run --release -p mandible-tui --run-ignored only identity`.
#[test]
#[ignore = "whole corpus; run in release"]
fn cached_windowed_render_is_cell_identical_to_the_reference_on_the_whole_corpus() {
    let (nodes, total) = corpus_nodes(false);
    // The check is only worth its green if it really covered the corpus.
    assert!(
        nodes.len() * 10 >= total * 8 && nodes.len() >= 150,
        "only {} of {total} fixtures replayed into a node",
        nodes.len()
    );
    check_identity(&nodes);
}

fn check_identity(nodes: &[(String, CommandNode)]) {
    let mut renders = 0usize;
    for (name, node) in nodes {
        let flags: Vec<FlagKey> = node.entities.iter().filter_map(|e| e.key()).collect();
        for &(w, h) in &[(60u16, 24u16), (100, 40), (140, 30)] {
            let probe = app_with(
                node,
                crate::style::Palette::extended(),
                crate::glyphs::UNICODE,
            );
            draw(&probe, w, h, true);
            let max = probe.detail_max_scroll_for_tests();
            let offsets = [0usize, 7, max / 2, max.saturating_sub(1), max, 1_000_000];
            for off in offsets {
                let (mut f, mut s) = (
                    app_with(
                        node,
                        crate::style::Palette::extended(),
                        crate::glyphs::UNICODE,
                    ),
                    app_with(
                        node,
                        crate::style::Palette::extended(),
                        crate::glyphs::UNICODE,
                    ),
                );
                // Establish the document extent first, as the live loop does.
                draw(&f, w, h, false);
                draw(&s, w, h, true);
                f.detail_scroll = off;
                s.detail_scroll = off;
                assert_same(&format!("{name} scroll {off}"), &f, &s, w, h);
                renders += 1;
            }
            // Search-selected flags: first, middle, last.
            for pick in [0, flags.len() / 2, flags.len().saturating_sub(1)] {
                let Some(key) = flags.get(pick) else { continue };
                let (mut f, mut s) = (
                    app_with(
                        node,
                        crate::style::Palette::extended(),
                        crate::glyphs::UNICODE,
                    ),
                    app_with(
                        node,
                        crate::style::Palette::extended(),
                        crate::glyphs::UNICODE,
                    ),
                );
                f.selected_flag = Some(key.clone());
                s.selected_flag = Some(key.clone());
                assert_same(&format!("{name} flag {key:?}"), &f, &s, w, h);
                renders += 1;
            }
        }
    }
    assert!(renders >= nodes.len() * 20, "{renders}");
}

#[test]
fn style_and_glyph_variants_and_changes_between_frames_match() {
    let (nodes, _) = sampled_nodes();
    // A few big and a few small documents.
    let mut by_size: Vec<_> = nodes.iter().collect();
    by_size.sort_by_key(|(_, n)| n.entities.len());
    let picks = [
        by_size[0],
        by_size[by_size.len() / 2],
        by_size[by_size.len() - 1],
    ];
    for (name, node) in picks {
        for palette in [
            crate::style::Palette::plain(),
            crate::style::Palette::basic(),
            crate::style::Palette::extended(),
        ] {
            for glyphs in [crate::glyphs::UNICODE, crate::glyphs::ASCII] {
                let fast = app_with(node, palette, glyphs);
                let slow = app_with(node, palette, glyphs);
                assert_same(name, &fast, &slow, 100, 30);
            }
        }
        // One long-lived cached app driven through state changes, each
        // frame compared with a long-lived reference app given the same
        // steps and the same sizes: where a re-flow leaves the pane
        // depends on the sizes it was drawn at before.
        let mut fast = app_with(
            node,
            crate::style::Palette::extended(),
            crate::glyphs::UNICODE,
        );
        let mut slow = app_with(
            node,
            crate::style::Palette::extended(),
            crate::glyphs::UNICODE,
        );
        let steps: Vec<Step> = vec![
            Box::new(|a| a.detail_scroll = 3),
            Box::new(|a| a.glyphs = crate::glyphs::ASCII),
            Box::new(|a| a.palette = crate::style::Palette::plain()),
            Box::new(|a| a.detail_scroll = 40),
            Box::new(|a| a.focus = Focus::Tree),
        ];
        for step in &steps {
            step(&mut fast);
            step(&mut slow);
            for w in [70u16, 120] {
                draw(&fast, w, 30, false);
                draw(&slow, w, 30, true);
                assert_same(name, &fast, &slow, w, 30);
            }
        }
    }
}

#[test]
fn a_node_replaced_in_place_is_redrawn_not_served_from_the_cache() {
    let (nodes, _) = sampled_nodes();
    let (_, a) = &nodes[0];
    let (_, b) = nodes.last().unwrap();
    let mut b = b.clone();
    b.name = a.name.clone();
    let mut fast = app_with(a, crate::style::Palette::extended(), crate::glyphs::UNICODE);
    draw(&fast, 100, 30, false);
    let path = vec![a.name.clone()];
    fast.splice_filled_node(&path, b.clone());
    let slow = app_with(
        &b,
        crate::style::Palette::extended(),
        crate::glyphs::UNICODE,
    );
    assert_same("spliced", &fast, &slow, 100, 30);
    // And a reload.
    fast.reload(a.clone());
    let slow = app_with(a, crate::style::Palette::extended(), crate::glyphs::UNICODE);
    assert_same("reloaded", &fast, &slow, 100, 30);
}

#[test]
fn a_whole_tree_store_assigned_over_the_old_one_is_redrawn() {
    let (nodes, _) = sampled_nodes();
    let (_, a) = &nodes[0];
    let (_, b) = nodes.last().unwrap();
    let mut b = b.clone();
    b.name = a.name.clone();
    let mut fast = app_with(a, crate::style::Palette::extended(), crate::glyphs::UNICODE);
    draw(&fast, 100, 30, false);
    fast.root = crate::tree_store::TreeStore::new(b.clone());
    let slow = app_with(
        &b,
        crate::style::Palette::extended(),
        crate::glyphs::UNICODE,
    );
    assert_same("assigned", &fast, &slow, 100, 30);
}
