//! Spec §9: the parsed view and the raw `--help` view each keep their own
//! detail-pane scroll position, vertical and horizontal.
//!
//! Driven through the whole frame (`TestBackend`) rather than through
//! `App`'s offsets alone, because the property under test is a property of
//! what reaches the screen: "raw is exactly where I left it" is a claim
//! about the rows the reader sees, and the offsets that produce them only
//! become meaningful once the renderer has reported each view's real
//! extent. Every step here renders, the way the event loop draws a frame
//! after every key.
//!
//! The fixture is deliberately asymmetric — a long flag list against a
//! longer raw text — so no vertical assertion can pass because the two
//! views happen to agree. The parsed view has no horizontal extent of its
//! own any more (spec §9 rule 9: USAGE soft-wraps instead of
//! scrolling), so the horizontal axis is exercised through the raw view
//! only, against the parsed view's fixed zero.

use mandible_core::{CommandNode, Entity, EntityKind, Provenance, Source, Spelling, Text};
use mandible_tui::app::{App, RawHelp};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

const WIDTH: u16 = 100;
const HEIGHT: u16 = 28;

/// The tool's own name for the raw view's heading line.
const TOOL: &str = "grep";

/// The detail pane's content area, one string per row, trailing space
/// trimmed — the same reading `detail_sections.rs` takes, so column 0 here
/// is column 0 of the pane's own layout.
fn detail_rows(app: &App) -> Vec<String> {
    let backend = TestBackend::new(WIDTH, HEIGHT);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| mandible_tui::render::render(frame, app))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let regions =
        mandible_tui::layout::compute(ratatui::layout::Rect::new(0, 0, WIDTH, HEIGHT), app.focus);
    let rect = regions.detail.expect("wide enough for a detail pane");

    let mut rows = Vec::new();
    for y in (rect.y + 1)..(rect.y + rect.height - 1) {
        let mut line = String::new();
        for x in (rect.x + 2)..(rect.x + rect.width - 2) {
            line.push_str(buffer[(x, y)].symbol());
        }
        rows.push(line.trim_end().to_string());
    }
    rows
}

/// A node with far more flags than fit the pane, so the parsed view has a
/// vertical position worth remembering. The synopsis is wider than the
/// pane too, kept only to exercise USAGE's own soft-wrap (spec §9 rule 9)
/// elsewhere; it no longer gives the parsed view anything horizontal to
/// scroll.
fn parsed_node() -> CommandNode {
    let mut node = CommandNode::new(TOOL, Provenance::single(Source::HelpText));
    node.description = Some(Text::sanitize("Search for PATTERNS in each FILE."));
    node.usage = vec![Text::sanitize_preserving_layout(&format!(
        "{TOOL} [OPTION...] PATTERNS [FILE...] {}",
        (0..12)
            .map(|i| format!("[--synopsis-tail-{i:02}]"))
            .collect::<Vec<_>>()
            .join(" ")
    ))];
    node.entities = (0..40)
        .map(|i| {
            let mut e = Entity::new(EntityKind::Flag, Provenance::single(Source::HelpText));
            e.spellings.push(Spelling::long(format!("flag-{i:02}")));
            e.description = Some(Text::sanitize(&format!("does thing number {i:02}")));
            e
        })
        .collect();
    node.children_filled = true;
    node
}

/// Raw text whose lines are both more numerous and wider than the parsed
/// view's, each one self-identifying so a screenful says which line it
/// starts at.
fn raw_body(count: usize) -> Vec<Text> {
    (0..count)
        .map(|i| {
            Text::sanitize_preserving_layout(&format!("RAW-{i:02} {} TAIL-{i:02}", "-".repeat(90)))
        })
        .collect()
}

fn raw_help(count: usize) -> RawHelp {
    RawHelp::Ready(raw_body(count), format!("{TOOL} --help (verbatim)"))
}

fn app_with_raw(raw_lines: usize) -> App {
    let mut app = App::new(TOOL.to_string(), parsed_node());
    app.focus = mandible_tui::Focus::Detail;
    app.set_raw_help(vec![TOOL.to_string()], raw_help(raw_lines));
    app
}

/// Scroll down `n` lines, drawing a frame after each one exactly as the
/// event loop does — the extent a press is clamped against is only ever as
/// fresh as the last frame.
fn scroll_down(app: &mut App, n: usize) {
    for _ in 0..n {
        detail_rows(app);
        app.detail_scroll_down();
    }
    detail_rows(app);
}

fn scroll_right(app: &mut App, n: usize) {
    for _ in 0..n {
        detail_rows(app);
        app.detail_hscroll_right();
    }
    detail_rows(app);
}

/// `t` restores the line the view being entered was last left at, and the
/// two views' lines are independent.
#[test]
fn each_view_returns_to_its_own_line() {
    let mut app = app_with_raw(60);

    scroll_down(&mut app, 9);
    let parsed_place = app.clamped_detail_scroll();
    let parsed_screen = detail_rows(&app);
    assert_eq!(parsed_place, 9, "the parsed view has room to scroll");

    app.toggle_raw_mode();
    scroll_down(&mut app, 21);
    let raw_place = app.clamped_detail_scroll();
    let raw_screen = detail_rows(&app);
    assert_eq!(raw_place, 21, "the raw view has room of its own to scroll");
    assert_ne!(
        parsed_screen, raw_screen,
        "the two views must render differently for this test to mean anything"
    );

    app.toggle_raw_mode();
    assert_eq!(
        app.clamped_detail_scroll(),
        parsed_place,
        "the parsed view must return to its own line, not to a mapping of the raw view's"
    );
    assert_eq!(
        detail_rows(&app),
        parsed_screen,
        "the parsed view came back somewhere else"
    );

    app.toggle_raw_mode();
    assert_eq!(
        app.clamped_detail_scroll(),
        raw_place,
        "the raw view must return to its own line, not to a mapping of the parsed view's"
    );
    assert_eq!(
        detail_rows(&app),
        raw_screen,
        "the raw view came back somewhere else"
    );
}

/// The horizontal offset is remembered per view on the same terms: the raw
/// view's column is its own number, restored exactly. The parsed view has
/// no horizontal extent left to scroll (spec §9 rule 9): USAGE
/// soft-wraps now, and nothing else in the parsed view ever supported
/// horizontal scroll, so scrolling it right is a no-op that must not leak
/// into the raw view's own column.
#[test]
fn each_view_returns_to_its_own_column() {
    let mut app = app_with_raw(60);

    scroll_right(&mut app, 3);
    assert_eq!(
        app.clamped_detail_hscroll(),
        0,
        "the parsed view has nothing left to scroll horizontally"
    );

    app.toggle_raw_mode();
    assert_eq!(
        app.clamped_detail_hscroll(),
        0,
        "the raw view has its own column, and has not been scrolled"
    );
    scroll_right(&mut app, 7);
    let raw_column = app.clamped_detail_hscroll();
    let raw_screen = detail_rows(&app);
    assert!(raw_column > 0, "the raw text is wider than the pane");

    app.toggle_raw_mode();
    assert_eq!(
        app.clamped_detail_hscroll(),
        0,
        "the raw view's column must not leak into the parsed view"
    );

    app.toggle_raw_mode();
    assert_eq!(
        app.clamped_detail_hscroll(),
        raw_column,
        "the raw view must return to its own column"
    );
    assert_eq!(
        detail_rows(&app),
        raw_screen,
        "the raw view came back at a different column"
    );
}

/// Movement in one view never moves the other's stored position, however
/// far it goes — including past the other view's whole extent. The parsed
/// view's own `scroll_right` is a no-op (spec §9 rule 9: USAGE
/// soft-wraps now, so the parsed view has no horizontal extent), which is
/// itself part of what this test pins: it must stay at zero rather than
/// picking up the raw view's travel.
#[test]
fn scrolling_one_view_leaves_the_others_position_alone() {
    let mut app = app_with_raw(120);

    scroll_down(&mut app, 4);
    scroll_right(&mut app, 2);
    let parsed_place = (app.clamped_detail_scroll(), app.clamped_detail_hscroll());
    assert_eq!(parsed_place.1, 0, "precondition: nothing to scroll to");
    let parsed_screen = detail_rows(&app);

    app.toggle_raw_mode();
    // Deeper and further right than the parsed view can go at all.
    scroll_down(&mut app, 80);
    scroll_right(&mut app, 20);
    assert!(app.clamped_detail_scroll() > parsed_place.0 + 40);
    assert!(app.clamped_detail_hscroll() > parsed_place.1);

    app.toggle_raw_mode();
    assert_eq!(
        (app.clamped_detail_scroll(), app.clamped_detail_hscroll()),
        parsed_place,
        "the raw view's travel leaked into the parsed view"
    );
    assert_eq!(
        detail_rows(&app),
        parsed_screen,
        "the parsed view's screen changed while only the raw view was scrolled"
    );
}

/// A view with nothing stored for this node opens at the top-left,
/// whatever the other view is showing — no proportion of the other view's
/// extent is seeded into it.
#[test]
fn a_first_visit_to_a_view_starts_at_the_top_left() {
    let mut app = app_with_raw(60);

    // The top-left of each view, recorded before anything is scrolled.
    let parsed_top = detail_rows(&app);
    app.toggle_raw_mode();
    let raw_top = detail_rows(&app);
    app.toggle_raw_mode();
    assert_ne!(parsed_top, raw_top);

    // Take the parsed view far from its top on its one remaining axis; the
    // parsed view has no horizontal extent to scroll any more (spec §9
    // rule 9: USAGE soft-wraps now), so only the vertical
    // position is worth carrying across the toggle here. The raw view has
    // never been scrolled for this node.
    let mut app = app_with_raw(60);
    scroll_down(&mut app, 14);
    assert!(app.clamped_detail_scroll() > 0);

    app.toggle_raw_mode();
    assert_eq!(
        app.clamped_detail_scroll(),
        0,
        "an unvisited view must not be seeded from the other view's line"
    );
    assert_eq!(
        detail_rows(&app),
        raw_top,
        "a first visit must land at the top-left, not at a fraction of the other view"
    );
}

/// A remembered position is clamped to the extent the target view actually
/// has when it is restored: the raw text can be re-probed shorter between
/// two visits, and the pane must land on its last screenful rather than
/// past the end of it.
#[test]
fn a_remembered_position_clamps_to_the_views_current_extent() {
    let mut app = app_with_raw(120);

    app.toggle_raw_mode();
    scroll_down(&mut app, 90);
    let deep = app.clamped_detail_scroll();
    assert!(deep > 40, "scrolled well into the long raw text");

    app.toggle_raw_mode();
    // The node is re-probed and this time says much less.
    app.set_raw_help(vec![TOOL.to_string()], raw_help(30));

    app.toggle_raw_mode();
    // Read after the frame, not before it: the extent a restored position
    // is clamped against is the one the renderer reports for the view being
    // entered, and until it draws once, the extent on hand is still the
    // outgoing view's.
    let rows = detail_rows(&app);
    let restored = app.clamped_detail_scroll();
    assert!(
        restored < deep,
        "a position past the new content was not clamped: {restored} vs {deep}"
    );
    assert!(
        rows.iter().any(|row| row.starts_with("RAW-29")),
        "the shortened text's last line is not on screen: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| !row.is_empty()),
        "the pane scrolled past the end of its own content"
    );
}

/// Changing the selected node clears what both views remember: an offset
/// into one node's document addresses nothing in another's.
#[test]
fn a_node_change_clears_both_views_positions() {
    let mut root = parsed_node();
    let mut child = CommandNode::new("child", Provenance::single(Source::HelpText));
    child.description = Some(Text::sanitize("A different node entirely."));
    child.children_filled = true;
    root.subcommands.push(child);

    let mut app = App::new(TOOL.to_string(), root);
    app.focus = mandible_tui::Focus::Detail;
    app.set_raw_help(vec![TOOL.to_string()], raw_help(60));
    app.set_raw_help(vec![TOOL.to_string(), "child".to_string()], raw_help(60));

    scroll_down(&mut app, 11);
    scroll_right(&mut app, 3);
    app.toggle_raw_mode();
    scroll_down(&mut app, 17);
    scroll_right(&mut app, 6);
    assert!(app.clamped_detail_scroll() > 0 && app.clamped_detail_hscroll() > 0);

    app.move_down();
    assert_eq!(
        app.selected_path(),
        Some(vec![TOOL.to_string(), "child".to_string()])
    );

    // The showing view starts at the top-left...
    detail_rows(&app);
    assert_eq!(
        app.clamped_detail_scroll(),
        0,
        "the showing view kept a line belonging to the previous node"
    );
    assert_eq!(
        app.clamped_detail_hscroll(),
        0,
        "the showing view kept a column belonging to the previous node"
    );

    // ...and so does the one that was not showing when the node changed.
    app.toggle_raw_mode();
    detail_rows(&app);
    assert_eq!(
        app.clamped_detail_scroll(),
        0,
        "the hidden view kept a line belonging to the previous node"
    );
    assert_eq!(
        app.clamped_detail_hscroll(),
        0,
        "the hidden view kept a column belonging to the previous node"
    );
}
