//! Rules reach the pane's edge at any sideways scroll, and a re-flow keeps
//! the item at the top of the pane in place (docs/design.md, "Small
//! terminals").

use super::*;
use crate::app::{Focus, RawHelp};
use mandible_core::{Provenance, Source, Spelling, Text};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;

const FLAGS: usize = 60;

/// A page with every kind of rule line (DESCRIPTION, USAGE, FLAGS and a
/// group divider every fifteen flags) and descriptions long enough that
/// wrapping changes how many rows each flag takes.
fn node() -> CommandNode {
    let mut node = CommandNode::new("tool", Provenance::single(Source::HelpText));
    node.description = Some(Text::sanitize("Search for PATTERNS in each FILE."));
    node.usage = vec![Text::sanitize_preserving_layout("tool [OPTION...] FILE...")];
    node.entities = (0..FLAGS)
        .map(|i| {
            let mut e = Entity::new(EntityKind::Flag, Provenance::single(Source::HelpText));
            e.spellings.push(Spelling::long(format!("flag-{i:02}")));
            e.description = Some(Text::sanitize(&format!(
                "does thing number {i:02} and then keeps explaining itself at length so that \
                 it needs several rows when the pane is narrow and one long row when it is not"
            )));
            if i % 15 == 0 {
                e.group = Some(format!("Group {}", i / 15));
            }
            e
        })
        .collect();
    node.children_filled = true;
    node
}

fn app(wrap: bool) -> App {
    let mut app = App::new("tool".to_string(), node());
    app.glyphs = crate::glyphs::UNICODE;
    app.parsed_wrap = wrap;
    app.focus = Focus::Detail;
    app
}

fn draw(app: &App, w: u16, h: u16) -> Buffer {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| crate::render::render(f, app)).unwrap();
    term.backend().buffer().clone()
}

/// The detail pane's rows: `(content, left gutter, right gutter)`.
fn rows(app: &App, w: u16, h: u16) -> Vec<(String, String, String)> {
    let b = draw(app, w, h);
    let r = crate::layout::compute(Rect::new(0, 0, w, h), app.focus)
        .detail
        .expect("a detail pane");
    ((r.y + 1)..(r.y + r.height - 1))
        .map(|y| {
            let text: String = ((r.x + 2)..(r.x + r.width - 2))
                .map(|x| b[(x, y)].symbol())
                .collect();
            (
                text,
                b[(r.x + 1, y)].symbol().to_string(),
                b[(r.x + r.width - 2, y)].symbol().to_string(),
            )
        })
        .collect()
}

fn right_until_stuck(app: &mut App, w: u16, h: u16) {
    let mut last = usize::MAX;
    while app.clamped_detail_hscroll() != last {
        last = app.clamped_detail_hscroll();
        rows(app, w, h);
        app.detail_hscroll_right();
    }
    rows(app, w, h);
}

#[test]
fn rules_reach_the_right_edge_at_every_sideways_position() {
    let (w, h) = (100, 120);
    let mut app = app(false);
    let at_zero = rows(&app, w, h);
    let rule_rows: Vec<usize> = at_zero
        .iter()
        .enumerate()
        .filter(|(_, r)| r.0.ends_with('─') && !r.0.starts_with('─'))
        .map(|(i, _)| i)
        .collect();
    assert!(rule_rows.len() >= 7, "{rule_rows:?}");

    let mut seen = vec![0];
    let mut positions = Vec::new();
    right_until_stuck(&mut app, w, h);
    let max = app.clamped_detail_hscroll();
    assert!(max > 20, "the page must scroll sideways: {max}");
    positions.push(max / 2);
    positions.push(max);
    for target in positions {
        let mut app = self::app(false);
        rows(&app, w, h);
        while app.clamped_detail_hscroll() < target {
            app.detail_hscroll_right();
            rows(&app, w, h);
        }
        seen.push(app.clamped_detail_hscroll());
        let now = rows(&app, w, h);
        for &i in &rule_rows {
            assert!(
                now[i].0.ends_with('─'),
                "row {i} at {}: {:?}",
                app.clamped_detail_hscroll(),
                now[i].0
            );
            assert_ne!(now[i].2, "▶", "rule row {i} marked at {}", seen[1]);
        }
        if target == max {
            assert!(!now[rule_rows[0]].0.contains("DESCRIPTION"));
        }
    }
    assert!(seen[1] > 0 && seen[2] > seen[1]);
}

#[test]
fn rules_do_not_widen_the_page() {
    let (w, h) = (100, 120);
    let mut app = app(false);
    let pane = usize::from(
        crate::layout::compute(Rect::new(0, 0, w, h), app.focus)
            .detail
            .unwrap()
            .width,
    ) - 4;
    let built = build_lines(
        app.selected_node().unwrap(),
        pane,
        app.palette,
        None,
        app.glyphs,
        &app,
    );
    let widest = built
        .lines
        .iter()
        .enumerate()
        .filter(|(i, _)| !built.rules.contains(i))
        .map(|(_, l)| l.width())
        .max()
        .unwrap();
    right_until_stuck(&mut app, w, h);
    assert_eq!(app.clamped_detail_hscroll(), widest - pane);
}

/// The first word of the first row in the pane that starts a flag.
fn top_flag(app: &App, w: u16, h: u16) -> String {
    rows(app, w, h)
        .iter()
        .map(|r| r.0.split_whitespace().next().unwrap_or("").to_string())
        .find(|t| t.starts_with("--flag-"))
        .unwrap_or_default()
}

/// Scroll until a flag head is the first row of the pane, deep in the page.
fn scroll_to_flag_at_top(app: &mut App, w: u16, h: u16, depth: usize) -> String {
    for _ in 0..depth {
        rows(app, w, h);
        app.detail_scroll_down();
    }
    loop {
        let first = rows(app, w, h)[0]
            .0
            .split_whitespace()
            .next()
            .map(str::to_string);
        if first.as_deref().is_some_and(|t| t.starts_with("--flag-")) {
            return first.unwrap();
        }
        app.detail_scroll_down();
    }
}

fn first_word(app: &App, w: u16, h: u16) -> String {
    rows(app, w, h)[0]
        .0
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string()
}

#[test]
fn toggling_wrap_keeps_the_top_flag_both_ways() {
    let (w, h) = (100, 30);
    for start_wrapped in [true, false] {
        let mut app = app(start_wrapped);
        let flag = scroll_to_flag_at_top(&mut app, w, h, 60);
        assert!(flag.as_str() > "--flag-08", "deep enough: {flag}");
        app.toggle_wrap();
        assert_eq!(first_word(&app, w, h), flag, "after the first toggle");
        app.toggle_wrap();
        assert_eq!(first_word(&app, w, h), flag, "after toggling back");
    }
}

#[test]
fn resizing_keeps_the_top_flag() {
    let mut app = app(true);
    let flag = scroll_to_flag_at_top(&mut app, 100, 30, 60);
    assert_eq!(first_word(&app, 70, 30), flag, "narrower");
    assert_eq!(first_word(&app, 130, 30), flag, "wider");
    assert_eq!(top_flag(&app, 100, 30), flag, "and back");
}

#[test]
fn a_scroll_after_a_reflow_starts_from_the_placed_position() {
    let mut app = app(true);
    let flag = scroll_to_flag_at_top(&mut app, 100, 30, 60);
    app.toggle_wrap();
    assert_eq!(first_word(&app, 100, 30), flag);
    let placed = app.clamped_detail_scroll();
    assert!(placed > 0);
    app.detail_scroll_down();
    rows(&app, 100, 30);
    assert_eq!(app.clamped_detail_scroll(), placed + 1);
}

fn raw_app(wrap: bool) -> App {
    let mut app = app(true);
    app.horizontal_scroll_enabled = !wrap;
    let body: Vec<Text> = (0..80)
        .map(|i| {
            Text::sanitize_preserving_layout(&format!("RAW-{i:02} {}", "word ".repeat(40 + i % 7)))
        })
        .collect();
    app.set_raw_help(
        vec!["tool".to_string()],
        RawHelp::Ready(body, "tool --help".to_string()),
    );
    let _ = app.toggle_raw_mode();
    app
}

#[test]
fn the_raw_view_keeps_its_top_line_across_a_wrap_toggle() {
    let (w, h) = (100, 30);
    for wrapped in [true, false] {
        let mut app = raw_app(wrapped);
        for _ in 0..70 {
            rows(&app, w, h);
            app.detail_scroll_down();
        }
        let top = loop {
            let t = first_word(&app, w, h);
            if t.starts_with("RAW-") {
                break t;
            }
            app.detail_scroll_down();
        };
        app.toggle_wrap();
        assert_eq!(first_word(&app, w, h), top, "toggled");
        app.toggle_wrap();
        assert_eq!(first_word(&app, w, h), top, "toggled back");
        assert_eq!(first_word(&app, 60, h), top, "resized");
    }
}
