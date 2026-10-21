//! The size sweep: real pages from the corpus drawn at every combination of
//! width, height and wrap state, held to the rules of docs/design.md
//! ("Small terminals").

use super::identity_tests::{corpus_root, replay_fixture};
use super::*;
use crate::app::Focus;
use crate::layout;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;

const FIXTURES: [&str; 7] = [
    "cp/9.4",
    "tar/1.35",
    "ffplay/6.1.1-3ubuntu5-values",
    "git/2.43.0-commands",
    "grep/3.11",
    "curl/8.5.0",
    "pnpm/11.22.0-commands",
];
const WIDTHS: [u16; 7] = [200, 120, 80, 60, 50, 40, 30];
const HEIGHTS: [u16; 4] = [40, 24, 16, 10];

/// The independent statement of the room rule: a description gets at least
/// twenty columns. Deliberately not `fit::has_room`, so a drifting
/// threshold fails here.
const TWENTY: usize = 20;

fn nodes() -> Vec<(String, CommandNode)> {
    FIXTURES
        .iter()
        .map(|f| {
            let node = replay_fixture(&corpus_root().join(f)).unwrap_or_else(|| panic!("{f}"));
            ((*f).to_string(), node)
        })
        .collect()
}

fn app_for(node: &CommandNode, wrap: bool, focus: Focus, last_row: bool) -> App {
    let mut app = App::new("tool".to_string(), node.clone());
    app.glyphs = crate::glyphs::UNICODE;
    app.parsed_wrap = wrap;
    app.focus = focus;
    if last_row {
        app.selected = app.rows().len().saturating_sub(1);
    }
    app
}

fn draw(app: &App, w: u16, h: u16) -> Buffer {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| crate::render::render(f, app)).unwrap();
    term.backend().buffer().clone()
}

fn row_text(b: &Buffer, y: u16, x0: u16, x1: u16) -> String {
    (x0..x1).map(|x| b[(x, y)].symbol()).collect()
}

fn dump(b: &Buffer) -> String {
    (0..b.area.height)
        .map(|y| row_text(b, y, 0, b.area.width))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Corners, side edges and the bottom rule of a pane are intact. The only
/// thing allowed on the bottom rule is the `more below` label.
fn assert_border(b: &Buffer, r: Rect, label: &str) {
    let (x1, y1) = (r.x + r.width - 1, r.y + r.height - 1);
    for (x, y, want) in [
        (r.x, r.y, "╭"),
        (x1, r.y, "╮"),
        (r.x, y1, "╰"),
        (x1, y1, "╯"),
    ] {
        assert_eq!(b[(x, y)].symbol(), want, "{label}: corner\n{}", dump(b));
    }
    for y in (r.y + 1)..y1 {
        for x in [r.x, x1] {
            assert_eq!(b[(x, y)].symbol(), "│", "{label}: side\n{}", dump(b));
        }
    }
    let bottom = row_text(b, y1, r.x + 1, x1).replacen(" ▼ more ", &"─".repeat(8), 1);
    assert!(
        bottom.chars().all(|c| c == '─'),
        "{label}: bottom {bottom:?}"
    );
}

fn assert_no_ellipsis_outside_tree_rows(b: &Buffer, area: Rect, focus: Focus, label: &str) {
    let regions = layout::compute(area, focus);
    let mut panes = vec![regions.search, regions.status];
    panes.extend(regions.detail);
    for r in panes {
        for y in r.y..r.y + r.height {
            // Titles on a top border may be cut with `…`; everything a
            // pane's body holds may not.
            let top_border = y == r.y && r != regions.status;
            let text = row_text(b, y, r.x, r.x + r.width);
            assert!(
                top_border || !text.contains('…'),
                "{label}: cut text in {text:?}\n{}",
                dump(b)
            );
        }
    }
}

fn leading_spaces(line: &Line<'static>) -> usize {
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    text.len() - text.trim_start().len()
}

/// Per list section: man layout exactly when the description gets under
/// twenty columns, and the lines of each layout where that layout puts
/// them.
fn check_sections(name: &str, node: &CommandNode, width: usize, wrap: bool) {
    for (kind, label, indent) in LIST_SECTIONS {
        let ents: Vec<&Entity> = node.entities_of(kind).collect();
        if ents.is_empty() {
            continue;
        }
        let layout = section_layout(&ents, width, indent, wrap);
        // Room is judged against the p90 of every head, outliers included.
        let p90 = percentile_width(ents.iter().map(|e| entity_head_width(e, indent)));
        let room = width.saturating_sub(p90 + 1);
        let at = format!("{name} {label} width {width} wrap {wrap}");
        assert_eq!(layout.man, wrap && room < TWENTY, "{at}: room {room}");
        let body = section_lines(
            &ents,
            width,
            indent,
            style::Palette::extended(),
            None,
            crate::glyphs::UNICODE,
            wrap,
        );
        let texts: Vec<String> = body
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let trimmed: std::collections::HashSet<&str> = texts.iter().map(|t| t.trim()).collect();
        let broken: Vec<String> = ents
            .iter()
            .filter(|e| entity_head_width(e, indent) > width)
            .map(|e| format!("{}{}", entity_name_spec(e), flag_value_suffix(e)))
            .collect();
        if layout.man {
            for e in &ents {
                let head = format!("{}{}", entity_name_spec(e), flag_value_suffix(e));
                if spelling_column(e, indent) + display_width(&head) <= width {
                    assert!(
                        trimmed.contains(head.as_str()),
                        "{at}: no head-only line for {head:?}"
                    );
                }
            }
            assert!(
                body.lines.iter().all(|l| l.width() <= width),
                "{at}: a man-layout line is wider than the pane: {:#?}",
                texts
                    .iter()
                    .filter(|t| display_width(t) > width)
                    .collect::<Vec<_>>()
            );
            continue;
        }
        for row in &body.rows {
            let lines = &body.lines[row.first_line..row.first_line + row.lines];
            let first = texts[row.first_line].trim();
            let broken_head = broken.iter().any(|h| h.starts_with(first));
            for (n, line) in lines.iter().enumerate().skip(1) {
                let t = texts[row.first_line + n].trim_start();
                let guide = t.starts_with(crate::glyphs::UNICODE.guide_mid)
                    || t.starts_with(crate::glyphs::UNICODE.guide_end);
                if guide || broken_head || t.is_empty() {
                    continue;
                }
                assert!(
                    leading_spaces(line) >= layout.description,
                    "{at}: {t:?} sits left of the description column {}",
                    layout.description
                );
            }
        }
    }
}

#[test]
fn every_size_keeps_its_borders_and_cuts_nothing_but_the_tree() {
    for (name, node) in nodes() {
        for wrap in [true, false] {
            for &w in &WIDTHS {
                for &h in &HEIGHTS {
                    for (focus, last_row) in [
                        (Focus::Tree, false),
                        (Focus::Tree, true),
                        (Focus::Detail, false),
                    ] {
                        {
                            let app = app_for(&node, wrap, focus, last_row);
                            let b = draw(&app, w, h);
                            let area = Rect::new(0, 0, w, h);
                            let regions = layout::compute(area, focus);
                            let at = format!("{name} {w}x{h} wrap {wrap} {focus:?}");
                            assert_border(&b, regions.search, &at);
                            for r in [regions.tree, regions.detail].into_iter().flatten() {
                                assert_border(&b, r, &at);
                            }
                            assert_no_ellipsis_outside_tree_rows(&b, area, focus, &at);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn sections_are_a_man_layout_exactly_when_a_description_gets_under_twenty_columns() {
    for (name, node) in nodes() {
        for wrap in [true, false] {
            for width in [22, 26, 30, 34, 36, 38, 44, 60, 80, 120, 200] {
                check_sections(&name, &node, width, wrap);
            }
        }
    }
}

#[test]
fn under_the_minimum_only_the_too_small_line_is_drawn() {
    let (_, node) = nodes().remove(0);
    let app = app_for(&node, true, Focus::Tree, false);
    for (w, h) in [
        (fit::MIN_WIDTH - 1, 24),
        (80, fit::MIN_HEIGHT - 1),
        (12, 5),
        (200, 3),
    ] {
        let text = dump(&draw(&app, w, h));
        let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(flat.contains("Terminal too small"), "{w}x{h}: {flat:?}");
        assert!(
            !text.chars().any(|c| "╭╮╰╯│─".contains(c)),
            "{w}x{h}: chrome drawn\n{text}"
        );
    }
    let at_min = dump(&draw(&app, fit::MIN_WIDTH, fit::MIN_HEIGHT));
    assert!(!at_min.contains("too small"), "{at_min}");
}

#[test]
fn the_title_keeps_the_current_command_and_is_cut_from_the_left() {
    let g = crate::glyphs::UNICODE;
    let path = ["git".to_string(), "interpret-trailers".to_string()];
    assert_eq!(breadcrumb_title(&path, g, 40), " git › interpret-trailers ");
    assert_eq!(breadcrumb_title(&path, g, 24), " … › interpret-trailers ");
    let cut = breadcrumb_title(&path, g, 14);
    assert!(
        cut.starts_with(" …") && cut.ends_with("trailers "),
        "{cut:?}"
    );
    assert!(display_width(&cut) <= 14, "{cut:?}");
}

#[test]
fn a_token_wider_than_the_pane_hard_breaks_in_wrapped_text() {
    let token = "x".repeat(50);
    for width in [8, 20, 30] {
        let rows = wrap_hanging(&format!("see {token} now"), width, fit::USAGE_HANG);
        assert_eq!(rows.concat().replace(' ', ""), format!("see{token}now"));
        assert!(rows.iter().all(|r| display_width(r) <= width), "{rows:?}");
    }
}

#[test]
fn hidden_rows_are_marked_on_the_border() {
    let (_, node) = nodes().remove(1);
    let mut app = app_for(&node, true, Focus::Detail, false);
    // Search 3 rows, body 8 (borders at rows 3 and 10), status 1.
    let edges = |a: &App| {
        draw(a, 100, 12);
        let b = draw(a, 100, 12);
        (row_text(&b, 3, 0, 100), row_text(&b, 10, 0, 100))
    };
    let (top, bottom) = edges(&app);
    assert!(bottom.contains("▼ more") && !top.contains("▲ more"));
    app.detail_scroll = 5;
    assert!(edges(&app).0.contains("▲ more"), "rows above");
    app.glyphs = crate::glyphs::ASCII;
    let (top, bottom) = edges(&app);
    assert!(top.contains("^ more") && bottom.contains("v more"));
}
