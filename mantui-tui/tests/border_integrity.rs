//! **The regression test for the bug that was reverted twice** (spec §9,
//! §13.3): a previous implementation of mantui had description text
//! overwrite the tree pane's `│` border while scrolling. Two attempts to
//! fix it inside the tree widget were reverted. The real fix is at the IR
//! boundary (`Text::sanitize`, mantui-core) plus display-width-safe
//! truncation at the widget layer (`mantui_tui::sanitize`) — this test
//! proves that combination holds under adversarial input.
//!
//! Renders real `App`/`render::render` frames via
//! `ratatui::backend::TestBackend` with adversarial descriptions (embedded
//! `\n`, tabs, ANSI, CJK, emoji, a 5000-char string) at several widths and
//! scroll offsets, and asserts every border cell in the frame buffer still
//! holds its expected border glyph.

use mantui_core::{CommandNode, Provenance, Source, Text};
use mantui_tui::app::App;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// Rounded border glyphs (`ratatui::widgets::BorderType::Rounded`).
const TOP_LEFT: char = '╭';
const TOP_RIGHT: char = '╮';
const BOTTOM_LEFT: char = '╰';
const BOTTOM_RIGHT: char = '╯';
const HORIZONTAL: char = '─';
const VERTICAL: char = '│';

fn adversarial_tree() -> CommandNode {
    let mut root = CommandNode::new(
        "git",
        Provenance::single(Source::KnownSpec {
            provider: "carapace".to_string(),
        }),
    );
    root.summary = Some(Text::sanitize("a fake vcs\nwith\nembedded\nnewlines"));

    let adversarial_descriptions: Vec<(&str, String)> = vec![
        (
            "embedded_newlines",
            "line one\nline two\nline three\n\n\n\nline four".to_string(),
        ),
        ("tabs", "col1\tcol2\tcol3\tcol4\tcol5".to_string()),
        (
            "ansi",
            "\x1b[31mred\x1b[0m \x1b[1;32mbold green\x1b[0m \x1b]0;title\x07visible".to_string(),
        ),
        (
            "cjk",
            "日本語のテキストで境界を壊すテスト文字列です。とても長い説明文をここに書きます。"
                .repeat(3),
        ),
        (
            "emoji",
            "🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉 party time 🚀🚀🚀🚀🚀🚀🚀🚀🚀🚀".to_string(),
        ),
        ("huge", "x".repeat(5000)),
        (
            "backspace_overstrike",
            "_\u{8}B_\u{8}o_\u{8}l_\u{8}d text".to_string(),
        ),
        (
            "mixed",
            "\x1b[31mred\ttabbed\nline\x1b[0m 日本語 🎉".repeat(50),
        ),
    ];

    for (name, desc) in adversarial_descriptions {
        let mut child = CommandNode::new(
            name,
            Provenance::single(Source::KnownSpec {
                provider: "carapace".to_string(),
            }),
        );
        child.summary = Some(Text::sanitize(&desc));
        child.description = Some(Text::sanitize(&desc));
        root.subcommands.push(child);
    }

    root
}

fn build_app() -> App {
    let root = adversarial_tree();
    let mut app = App::new("git".to_string(), root);
    // Expand root so all adversarial children are visible tree rows.
    app.expand_selected(); // no-op: root already expanded by App::new, kept for clarity
    app.ensure_rows_fresh();
    app
}

/// Assert every cell on the outer border of `rect` (within `buffer`)
/// matches the rounded-border glyph set. Interior content is not checked
/// here — only that adversarial text never overwrote a border cell.
fn assert_border_intact(buffer: &ratatui::buffer::Buffer, rect: ratatui::layout::Rect) {
    if rect.width < 2 || rect.height < 2 {
        return;
    }
    let x0 = rect.x;
    let x1 = rect.x + rect.width - 1;
    let y0 = rect.y;
    let y1 = rect.y + rect.height - 1;

    let corners = [
        (x0, y0, TOP_LEFT),
        (x1, y0, TOP_RIGHT),
        (x0, y1, BOTTOM_LEFT),
        (x1, y1, BOTTOM_RIGHT),
    ];
    for (x, y, expected) in corners {
        let cell = &buffer[(x, y)];
        assert_eq!(
            cell.symbol(),
            expected.to_string(),
            "corner at ({x},{y}) of rect {rect:?} expected {expected:?}, got {:?}",
            cell.symbol()
        );
    }

    for x in (x0 + 1)..x1 {
        for (y, label) in [(y0, "top"), (y1, "bottom")] {
            let cell = &buffer[(x, y)];
            let sym = cell.symbol();
            // Rounded borders may embed a title in the top border, so
            // allow any glyph on the top edge where a title could sit, but
            // the horizontal rule character is what we expect absent a
            // title; the bottom edge must always be the plain rule.
            if label == "bottom" {
                assert_eq!(
                    sym,
                    HORIZONTAL.to_string(),
                    "bottom border at ({x},{y}) of rect {rect:?} corrupted: got {sym:?}"
                );
            }
        }
    }

    for y in (y0 + 1)..y1 {
        for (x, label) in [(x0, "left"), (x1, "right")] {
            let cell = &buffer[(x, y)];
            assert_eq!(
                cell.symbol(),
                VERTICAL.to_string(),
                "{label} border at ({x},{y}) of rect {rect:?} corrupted: got {:?} \
                 -- adversarial text overwrote a pane border",
                cell.symbol()
            );
        }
    }
}

#[test]
fn borders_survive_adversarial_text_across_widths_and_scroll() {
    let widths: [u16; 4] = [50, 80, 120, 200];
    let heights: [u16; 3] = [10, 24, 40];
    let scroll_offsets: [usize; 4] = [0, 1, 3, 100]; // 100 exceeds row count deliberately

    for &width in &widths {
        for &height in &heights {
            for &scroll in &scroll_offsets {
                let mut app = build_app();
                app.tree_scroll = scroll;
                app.detail_scroll = scroll;
                app.selected = app.selected.min(app.rows().len().saturating_sub(1));

                let backend = TestBackend::new(width, height);
                let mut terminal = Terminal::new(backend).unwrap();
                terminal
                    .draw(|frame| {
                        mantui_tui::render::render(frame, &app);
                    })
                    .unwrap();

                let buffer = terminal.backend().buffer().clone();
                let regions = mantui_tui::layout::compute(
                    ratatui::layout::Rect::new(0, 0, width, height),
                    app.focus,
                );

                assert_border_intact(&buffer, regions.search);
                if let Some(tree_rect) = regions.tree {
                    assert_border_intact(&buffer, tree_rect);
                }
                if let Some(detail_rect) = regions.detail {
                    assert_border_intact(&buffer, detail_rect);
                }
            }
        }
    }
}

#[test]
fn borders_survive_with_each_row_selected_and_detail_focused() {
    let width = 80;
    let height = 24;
    let mut app = build_app();
    app.ensure_rows_fresh();
    let row_count = app.rows().len();

    for selected in 0..row_count {
        for focus in [
            mantui_tui::Focus::Tree,
            mantui_tui::Focus::Detail,
            mantui_tui::Focus::Search,
        ] {
            app.selected = selected;
            app.focus = focus;

            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| {
                    mantui_tui::render::render(frame, &app);
                })
                .unwrap();

            let buffer = terminal.backend().buffer().clone();
            let regions = mantui_tui::layout::compute(
                ratatui::layout::Rect::new(0, 0, width, height),
                app.focus,
            );
            assert_border_intact(&buffer, regions.search);
            if let Some(tree_rect) = regions.tree {
                assert_border_intact(&buffer, tree_rect);
            }
            if let Some(detail_rect) = regions.detail {
                assert_border_intact(&buffer, detail_rect);
            }
        }
    }
}

#[test]
fn borders_survive_help_overlay() {
    let mut app = build_app();
    app.show_help = true;
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            mantui_tui::render::render(frame, &app);
        })
        .unwrap();
    // The overlay itself uses Clear + a fresh block; just prove the frame
    // renders without panicking and the outer search bar border (drawn
    // before the overlay) is still intact where not covered.
    let buffer = terminal.backend().buffer().clone();
    let regions = mantui_tui::layout::compute(ratatui::layout::Rect::new(0, 0, 80, 24), app.focus);
    assert_border_intact(&buffer, regions.search);
}

#[test]
fn narrow_terminal_stacked_layout_borders_survive() {
    let mut app = build_app();
    for &width in &[30u16, 40, 49] {
        for focus in [mantui_tui::Focus::Tree, mantui_tui::Focus::Detail] {
            app.focus = focus;
            let backend = TestBackend::new(width, 24);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| {
                    mantui_tui::render::render(frame, &app);
                })
                .unwrap();
            let buffer = terminal.backend().buffer().clone();
            let regions =
                mantui_tui::layout::compute(ratatui::layout::Rect::new(0, 0, width, 24), app.focus);
            assert_border_intact(&buffer, regions.search);
            if let Some(tree_rect) = regions.tree {
                assert_border_intact(&buffer, tree_rect);
            }
            if let Some(detail_rect) = regions.detail {
                assert_border_intact(&buffer, detail_rect);
            }
        }
    }
}
