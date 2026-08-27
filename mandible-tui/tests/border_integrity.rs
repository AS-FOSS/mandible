//! **The regression test for the bug that was reverted twice** (spec §9,
//! §13.3): a previous implementation of mandible had description text
//! overwrite the tree pane's `│` border while scrolling. Two attempts to
//! fix it inside the tree widget were reverted. The real fix is at the IR
//! boundary (`Text::sanitize`, mandible-core) plus display-width-safe
//! truncation at the widget layer (`mandible_tui::sanitize`) — this test
//! proves that combination holds under adversarial input.
//!
//! Renders real `App`/`render::render` frames via
//! `ratatui::backend::TestBackend` with adversarial descriptions (embedded
//! `\n`, tabs, ANSI, CJK, emoji, a 5000-char string) at several widths and
//! scroll offsets, and asserts every border cell in the frame buffer still
//! holds its expected border glyph.

use mandible_core::{CommandNode, Provenance, Source, Text};
use mandible_tui::app::App;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// Rounded border glyphs (`ratatui::widgets::BorderType::Rounded`).
const TOP_LEFT: char = '╭';
const TOP_RIGHT: char = '╮';
const BOTTOM_LEFT: char = '╰';
const BOTTOM_RIGHT: char = '╯';
const HORIZONTAL: char = '─';
const VERTICAL: char = '│';

/// Short, distinctive fragments of the adversarial descriptions below.
/// None of these may ever appear in a border cell — including the top
/// edge, which is otherwise hard to check with a strict per-cell glyph
/// match because a pane's (trusted) title text legitimately overwrites
/// some of the horizontal rule there. Checking "no adversarial content
/// leaks into any border cell, top edge included" is both a real
/// assertion (unlike a loop with no `assert!` in its body) and the exact
/// property this whole test file exists to defend, without needing to
/// reimplement ratatui's title-truncation layout to do it.
const ADVERSARIAL_MARKERS: &[&str] = &[
    "line one",
    "line two",
    "col1",
    "col2",
    "bold green",
    "visible",
    "日本語",
    "🎉",
    "🚀",
    "xxxxxxxxxx",
    "Bold text",
    "party time",
];

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
/// matches the rounded-border glyph set, and additionally that no
/// adversarial marker from [`ADVERSARIAL_MARKERS`] appears anywhere on the
/// border ring (top edge included). Interior content is not checked here.
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

    // Bottom edge never carries a title, so it must be the plain rule
    // character in every interior cell, strictly.
    for x in (x0 + 1)..x1 {
        let cell = &buffer[(x, y1)];
        let sym = cell.symbol();
        assert_eq!(
            sym,
            HORIZONTAL.to_string(),
            "bottom border at ({x},{y1}) of rect {rect:?} corrupted: got {sym:?}"
        );
    }

    // Left/right edges: strict glyph match, every content row.
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

    // Top edge: a trusted title may legitimately occupy some of these
    // cells (so we can't assert strict-HORIZONTAL like the bottom edge),
    // but no adversarial description/summary text may ever appear there.
    let top_row: String = (x0..=x1)
        .map(|x| buffer[(x, y0)].symbol().to_string())
        .collect();
    for marker in ADVERSARIAL_MARKERS {
        assert!(
            !top_row.contains(marker),
            "top border of rect {rect:?} leaked adversarial content {marker:?}: {top_row:?}"
        );
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
                        mandible_tui::render::render(frame, &app);
                    })
                    .unwrap();

                let buffer = terminal.backend().buffer().clone();
                let regions = mandible_tui::layout::compute(
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
            mandible_tui::Focus::Tree,
            mandible_tui::Focus::Detail,
            mandible_tui::Focus::Search,
        ] {
            app.selected = selected;
            app.focus = focus;

            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| {
                    mandible_tui::render::render(frame, &app);
                })
                .unwrap();

            let buffer = terminal.backend().buffer().clone();
            let regions = mandible_tui::layout::compute(
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
            mandible_tui::render::render(frame, &app);
        })
        .unwrap();
    // The overlay itself uses Clear + a fresh block; just prove the frame
    // renders without panicking and the outer search bar border (drawn
    // before the overlay) is still intact where not covered.
    let buffer = terminal.backend().buffer().clone();
    let regions =
        mandible_tui::layout::compute(ratatui::layout::Rect::new(0, 0, 80, 24), app.focus);
    assert_border_intact(&buffer, regions.search);
}

/// Regression for the exact bug class that shipped twice before (spec §9):
/// untrusted text corrupting the `|` border column. `unparsed` (spec §7
/// Tier B step 3, batch 6 part 4) is the tool author's *own* raw text,
/// deliberately not pre-wrapped and not handed `Paragraph::wrap` by
/// `render_unparsed` (unlike every other block in the pane) — exactly the
/// path most likely to reintroduce that bug if `Text::sanitize`'s
/// single-line/no-control-char invariant were ever bypassed. Adversarial
/// raw lines (embedded control-adjacent content, CJK, emoji, a very long
/// line) must still leave every border cell intact.
#[test]
fn borders_survive_a_node_with_unparsed_raw_help_text() {
    let mut root = CommandNode::new(
        "mystery",
        Provenance::with_confidence(Source::HelpText, 0.0),
    );
    root.unparsed = vec![
        Text::sanitize("a friendly banner with\ttabs and \x1b[31mANSI\x1b[0m codes"),
        Text::sanitize(
            "日本語のテキストで境界を壊すテスト文字列です。"
                .repeat(3)
                .as_str(),
        ),
        Text::sanitize("🎉🎉🎉 line with emoji 🚀🚀🚀"),
        Text::sanitize(&"x".repeat(5000)),
    ];

    for &width in &[40u16, 80, 120] {
        for &height in &[10u16, 24] {
            for focus in [mandible_tui::Focus::Tree, mandible_tui::Focus::Detail] {
                let mut app = App::new("mystery".to_string(), root.clone());
                app.focus = focus;
                let backend = TestBackend::new(width, height);
                let mut terminal = Terminal::new(backend).unwrap();
                terminal
                    .draw(|frame| {
                        mandible_tui::render::render(frame, &app);
                    })
                    .unwrap();
                let buffer = terminal.backend().buffer().clone();
                let regions = mandible_tui::layout::compute(
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
fn narrow_terminal_stacked_layout_borders_survive() {
    let mut app = build_app();
    for &width in &[30u16, 40, 49] {
        for focus in [mandible_tui::Focus::Tree, mandible_tui::Focus::Detail] {
            app.focus = focus;
            let backend = TestBackend::new(width, 24);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| {
                    mandible_tui::render::render(frame, &app);
                })
                .unwrap();
            let buffer = terminal.backend().buffer().clone();
            let regions = mandible_tui::layout::compute(
                ratatui::layout::Rect::new(0, 0, width, 24),
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

/// Nothing outside ASCII reaches the screen once the ASCII glyph set is
/// selected.
///
/// This is the automated replacement for eyeballing the TUI in a terminal
/// with `LANG=C`, which is the environment mandible is most often reached
/// for and least often tested in: SSH'd into an unfamiliar box, or a
/// minimal container where the locale is unset. Every chevron, border,
/// prompt, ellipsis and footer arrow has an ASCII counterpart precisely so
/// that a non-UTF-8 terminal shows readable text instead of tofu — and an
/// assertion is the only way that stays true, since the glyph a terminal
/// can actually draw cannot be probed at runtime.
///
/// Rendered over an **ASCII-only tree** on purpose. The adversarial tree
/// the border tests use contains CJK and emoji, and mandible must render a
/// tool's own text exactly as the tool wrote it — transliterating someone
/// else's output would be a far worse bug than a tofu box. So the only
/// non-ASCII a frame may contain is content that came from the tool; with
/// ASCII-only content, anything non-ASCII on screen is necessarily
/// mandible's own chrome, which is what this asserts about.
#[test]
fn ascii_glyph_set_renders_a_pure_ascii_frame() {
    for (width, height) in [(80u16, 24u16), (60, 20), (120, 40), (40, 12)] {
        let mut root = CommandNode::new("git", Provenance::single(Source::HelpText));
        root.summary = Some(Text::sanitize("the stupid content tracker"));
        for (name, summary) in [
            ("clone", "Clone a repository into a new directory"),
            ("rebase", "Reapply commits on top of another base tip"),
            ("commit", "Record changes to the repository"),
        ] {
            let mut child = CommandNode::new(name, Provenance::single(Source::HelpText));
            child.summary = Some(Text::sanitize(summary));
            child.children_filled = true;
            root.subcommands.push(child);
        }
        root.children_filled = true;

        let mut app = App::new("git".to_string(), root);
        app.glyphs = mandible_tui::glyphs::ASCII;
        app.ensure_rows_fresh();

        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                mandible_tui::render::render(frame, &app);
            })
            .unwrap();

        let buffer = terminal.backend().buffer().clone();
        for y in 0..height {
            for x in 0..width {
                let symbol = buffer[(x, y)].symbol();
                assert!(
                    symbol.is_ascii(),
                    "non-ASCII {symbol:?} at ({x},{y}) in a {width}x{height} ASCII-mode frame"
                );
            }
        }
    }
}

/// A node whose USAGE synopsis is wider than the pane: the horizontal-scroll
/// overflow affordance (spec §9: preformatted detail-pane content scrolls
/// rather than wraps) must show a right-pointing marker while
/// unscrolled, switch to a left-pointing one once fully scrolled right, and
/// — the point of testing it here rather than only in `detail_pane`'s own
/// unit tests — never disturb any of the border cells `assert_border_intact`
/// checks strictly (left/right/bottom edges, all four corners).
#[test]
fn detail_pane_hscroll_affordance_marks_overflow_without_corrupting_the_border() {
    let mut root = CommandNode::new("wide", Provenance::single(Source::HelpText));
    root.usage = vec![Text::sanitize(&format!("wide {}", "x".repeat(200)))];

    let mut app = App::new("wide".to_string(), root);
    app.focus = mandible_tui::Focus::Detail;
    assert!(app.horizontal_scroll_enabled, "default is on");

    let width = 60u16;
    let height = 20u16;
    let regions =
        mandible_tui::layout::compute(ratatui::layout::Rect::new(0, 0, width, height), app.focus);
    let detail_rect = regions.detail.expect("detail pane visible at this width");

    let render_once = |app: &App| -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| mandible_tui::render::render(frame, app))
            .unwrap();
        terminal.backend().buffer().clone()
    };

    // Unscrolled: more content to the right, none to the left.
    let buffer = render_once(&app);
    assert_border_intact(&buffer, regions.search);
    assert_border_intact(&buffer, detail_rect);
    let right_marker = detail_rect.x + detail_rect.width - 2;
    let left_marker = detail_rect.x + detail_rect.width - 3;
    assert_eq!(
        buffer[(right_marker, detail_rect.y)].symbol(),
        "→",
        "should signal more content to the right"
    );
    assert_ne!(
        buffer[(left_marker, detail_rect.y)].symbol(),
        "←",
        "nothing scrolled off the left yet"
    );

    // Scroll all the way right: the affordance should flip.
    for _ in 0..64 {
        app.detail_hscroll_right();
    }
    let buffer = render_once(&app);
    assert_border_intact(&buffer, regions.search);
    assert_border_intact(&buffer, detail_rect);
    assert_eq!(
        buffer[(left_marker, detail_rect.y)].symbol(),
        "←",
        "should signal more content to the left once scrolled"
    );
    assert_ne!(
        buffer[(right_marker, detail_rect.y)].symbol(),
        "→",
        "fully scrolled right: nothing more to reveal"
    );
}

/// Per-line clip markers (vim's `listchars extends:>,precedes:<`): a line
/// that overflows the pane carries a muted `>` in its own last column and,
/// once scrolled, a `<` in its first — while a short line beside it stays
/// untouched. The contrast between marked and unmarked neighbors is the
/// point: the pane-border affordance says "somewhere there's more", the
/// per-line marker says "this line".
#[test]
fn hscroll_clip_markers_mark_only_the_clipped_lines() {
    let mut root = CommandNode::new("wide", Provenance::single(Source::HelpText));
    root.usage = vec![
        Text::sanitize("wide -a"),
        Text::sanitize(&format!("wide {}", "x".repeat(200))),
    ];
    let mut app = App::new("wide".to_string(), root);
    app.focus = mandible_tui::Focus::Detail;
    assert!(app.horizontal_scroll_enabled, "default is on");

    let width = 60u16;
    let height = 20u16;
    let regions =
        mandible_tui::layout::compute(ratatui::layout::Rect::new(0, 0, width, height), app.focus);
    let detail_rect = regions.detail.expect("detail pane visible at this width");

    let render_once = |app: &App| -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| mandible_tui::render::render(frame, app))
            .unwrap();
        terminal.backend().buffer().clone()
    };
    // The content cells between border+padding on each side of one row.
    let row_text = |buffer: &ratatui::buffer::Buffer, y: u16| -> String {
        (detail_rect.x + 1..detail_rect.x + detail_rect.width - 1)
            .map(|x| buffer[(x, y)].symbol().to_string())
            .collect()
    };
    let find_row = |buffer: &ratatui::buffer::Buffer, needle: &str| -> u16 {
        (detail_rect.y + 1..detail_rect.y + detail_rect.height - 1)
            .find(|&y| row_text(buffer, y).contains(needle))
            .unwrap_or_else(|| panic!("no row containing {needle:?}"))
    };

    let buffer = render_once(&app);
    let long_row = row_text(&buffer, find_row(&buffer, "xxxx"));
    let short_row = row_text(&buffer, find_row(&buffer, "-a"));
    assert!(
        long_row.trim_end().ends_with('>'),
        "clipped line must end with the extends marker: {long_row:?}"
    );
    assert!(
        !short_row.contains('>') && !short_row.contains('<'),
        "a line that fits carries no marker: {short_row:?}"
    );

    for _ in 0..5 {
        app.detail_hscroll_right();
    }
    let buffer = render_once(&app);
    let long_row = row_text(&buffer, find_row(&buffer, "xxxx"));
    assert!(
        long_row.trim_start().starts_with('<'),
        "scrolled line must open with the precedes marker: {long_row:?}"
    );
    assert!(
        long_row.trim_end().ends_with('>'),
        "still more to the right at offset 5: {long_row:?}"
    );
}

/// The affordance is drawn regardless of which pane has focus — a
/// deliberate choice documented on `draw_hscroll_affordance` itself: even
/// though `h`/`l`/`←`/`→` only reach the detail pane's scroll while it is
/// focused, a marker that disappeared with the tree focused would let a
/// USAGE line clip silently until the reader happened to `Tab` over, which
/// is the worse failure. Pinned here so a future change to that decision is
/// a deliberate edit to this test, not a silent regression.
#[test]
fn detail_pane_hscroll_affordance_shows_even_with_the_tree_focused() {
    let mut root = CommandNode::new("wide", Provenance::single(Source::HelpText));
    root.usage = vec![Text::sanitize(&format!("wide {}", "x".repeat(200)))];

    let mut app = App::new("wide".to_string(), root);
    app.focus = mandible_tui::Focus::Tree;
    assert!(app.horizontal_scroll_enabled, "default is on");

    let width = 60u16;
    let height = 20u16;
    let regions =
        mandible_tui::layout::compute(ratatui::layout::Rect::new(0, 0, width, height), app.focus);
    let detail_rect = regions.detail.expect("detail pane visible at this width");

    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| mandible_tui::render::render(frame, &app))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    assert_border_intact(&buffer, detail_rect);
    let right_marker = detail_rect.x + detail_rect.width - 2;
    assert_eq!(
        buffer[(right_marker, detail_rect.y)].symbol(),
        "→",
        "the affordance is not gated on detail-pane focus"
    );
}

/// The ASCII glyph set's affordance markers (`<`/`>`) are what actually
/// reach the screen under `MANDIBLE_ASCII=1` — the Unicode arrows above are
/// exactly the kind of glyph this project refuses to rely on everywhere.
#[test]
fn detail_pane_hscroll_affordance_uses_the_ascii_glyphs() {
    let mut root = CommandNode::new("wide", Provenance::single(Source::HelpText));
    root.usage = vec![Text::sanitize(&format!("wide {}", "x".repeat(200)))];

    let mut app = App::new("wide".to_string(), root);
    app.focus = mandible_tui::Focus::Detail;
    app.glyphs = mandible_tui::glyphs::ASCII;

    let width = 60u16;
    let height = 20u16;
    let regions =
        mandible_tui::layout::compute(ratatui::layout::Rect::new(0, 0, width, height), app.focus);
    let detail_rect = regions.detail.expect("detail pane visible at this width");

    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| mandible_tui::render::render(frame, &app))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    // ASCII borders use `+` corners, not the rounded glyphs
    // `assert_border_intact` checks for — this asserts the ASCII-specific
    // property directly instead.
    let right_marker = detail_rect.x + detail_rect.width - 2;
    assert_eq!(buffer[(right_marker, detail_rect.y)].symbol(), ">");
    for y in 0..height {
        for x in 0..width {
            assert!(
                buffer[(x, y)].symbol().is_ascii(),
                "non-ASCII symbol {:?} at ({x},{y}) with the ASCII glyph set",
                buffer[(x, y)].symbol()
            );
        }
    }
}

/// The config toggle off must reproduce today's rendering exactly: no
/// affordance marker anywhere on the detail pane's border, even for a node
/// whose USAGE is far wider than the pane — because with the toggle off
/// that content wraps instead of overflowing, so there is nothing to mark.
#[test]
fn detail_pane_hscroll_affordance_absent_when_the_config_toggle_is_off() {
    let mut root = CommandNode::new("wide", Provenance::single(Source::HelpText));
    root.usage = vec![Text::sanitize(&format!("wide {}", "x".repeat(200)))];

    let mut app = App::new("wide".to_string(), root);
    app.focus = mandible_tui::Focus::Detail;
    app.horizontal_scroll_enabled = false;

    let width = 60u16;
    let height = 20u16;
    let regions =
        mandible_tui::layout::compute(ratatui::layout::Rect::new(0, 0, width, height), app.focus);
    let detail_rect = regions.detail.expect("detail pane visible at this width");

    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| mandible_tui::render::render(frame, &app))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    assert_border_intact(&buffer, detail_rect);
    let top_row: String = (detail_rect.x..detail_rect.x + detail_rect.width)
        .map(|x| buffer[(x, detail_rect.y)].symbol().to_string())
        .collect();
    assert!(!top_row.contains('→') && !top_row.contains('←'));
}

/// The Unicode set is still what a UTF-8 terminal gets — the fallback must
/// not quietly become the default for everyone.
#[test]
fn unicode_glyph_set_still_draws_rounded_borders() {
    let mut app = build_app();
    app.glyphs = mandible_tui::glyphs::UNICODE;
    app.ensure_rows_fresh();

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            mandible_tui::render::render(frame, &app);
        })
        .unwrap();

    let buffer = terminal.backend().buffer().clone();
    let corner = buffer[(0u16, 0u16)].symbol().to_string();
    assert_eq!(corner, "╭", "expected a rounded corner, got {corner:?}");
}
