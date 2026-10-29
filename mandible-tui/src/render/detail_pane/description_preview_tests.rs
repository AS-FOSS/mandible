//! The DESCRIPTION preview: when a page previews, what the preview and the
//! full section look like, and that every redraw decides again.

use super::*;
use crate::app::Focus;
use mandible_core::{Example, Provenance, Source, Spelling, Text};
use ratatui::backend::TestBackend;
use ratatui::style::Color;
use ratatui::Terminal;

/// A node whose description is `rows` source lines, each one drawn row at
/// any width above thirty, plus one flag and one example.
fn node(rows: usize) -> CommandNode {
    let mut node = CommandNode::new("tool", Provenance::single(Source::HelpText));
    let lines: Vec<String> = (1..=rows)
        .map(|i| format!("row {i:02} of the text"))
        .collect();
    node.description = Some(Text::sanitize_preserving_layout_lines(
        lines.iter().map(String::as_str),
    ));
    let mut flag = Entity::new(EntityKind::Flag, Provenance::single(Source::HelpText));
    flag.spellings.push(Spelling::long("verbose"));
    flag.description = Some(Text::sanitize("be chatty"));
    node.entities.push(flag);
    node.examples = vec![Example::new(Text::sanitize_preserving_layout_lines([
        "tool --verbose",
    ]))];
    node
}

fn app_for(node: &CommandNode, pane_height: usize) -> App {
    let mut app = App::new("tool".to_string(), node.clone());
    app.glyphs = crate::glyphs::UNICODE;
    app.focus = Focus::Detail;
    app.set_pane_height(pane_height);
    app
}

fn text_of(line: &Line) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn page(app: &App, width: usize) -> Vec<String> {
    let node = app.selected_node().unwrap();
    build_lines(node, width, app.palette, None, app.glyphs, app)
        .lines
        .iter()
        .map(text_of)
        .collect()
}

fn position(lines: &[String], starts_with: &str) -> Option<usize> {
    lines.iter().position(|l| l.starts_with(starts_with))
}

fn is_preview(lines: &[String]) -> bool {
    position(lines, "DESCRIPTION · preview").is_some()
}

#[test]
fn eleven_rows_in_eighteen_do_not_preview_and_twelve_do() {
    assert!(!is_preview(&page(&app_for(&node(11), 18), 60)));
    assert!(is_preview(&page(&app_for(&node(12), 18), 60)));
}

#[test]
fn the_preview_is_a_third_of_the_pane_and_never_under_six_rows() {
    for (height, rows) in [
        (1, 6),
        (12, 6),
        (18, 6),
        (20, 6),
        (23, 7),
        (24, 8),
        (30, 10),
        (34, 11),
        (40, 13),
        (50, 16),
    ] {
        assert_eq!(preview_rows(height), rows, "H={height}");
    }
}

#[test]
fn the_page_previews_past_half_the_pane_when_it_hides_six_rows() {
    for (rows, height, previews) in [
        (11, 18, false),
        (12, 18, true),
        (12, 22, false),
        (13, 22, true),
        (13, 25, false),
        (14, 25, true),
        (15, 30, false),
        (16, 30, true),
        (20, 40, false),
        (21, 40, true),
        (13, 24, false),
        (14, 24, true),
        (30, 0, false),
    ] {
        let lines = page(&app_for(&node(rows), height), 60);
        assert_eq!(is_preview(&lines), previews, "D={rows} H={height}");
    }
}

#[test]
fn a_page_that_does_not_preview_is_the_page_of_an_unmeasured_pane() {
    let plain = page(&app_for(&node(11), 0), 60);
    assert_eq!(page(&app_for(&node(11), 18), 60), plain);
    assert!(position(&plain, "DESCRIPTION").is_some_and(|i| i == 0));
    assert!(!plain.iter().any(|l| l.contains("full description")));
}

const NOTE: &str = "↓ full description at the end";

fn note_row(width: usize) -> String {
    format!("{}{NOTE}", " ".repeat(width - NOTE.chars().count()))
}

#[test]
fn the_preview_is_rows_then_an_ellipsis_row_then_the_note_then_the_next_heading() {
    let lines = page(&app_for(&node(20), 20), 60);
    assert!(lines[0].starts_with("DESCRIPTION · preview ─"), "{lines:?}");
    for (i, row) in lines[1..7].iter().enumerate() {
        assert_eq!(*row, format!("row {:02} of the text", i + 1));
    }
    assert_eq!(lines[7], "…");
    assert_eq!(lines[8], note_row(60));
    assert!(lines[9].starts_with("FLAGS"), "{lines:?}");
}

#[test]
fn the_row_count_follows_the_pane_height() {
    let lines = page(&app_for(&node(40), 33), 60);
    assert_eq!(lines[11], "row 11 of the text");
    assert_eq!(lines[12], "…");
    assert_eq!(lines[13], note_row(60));
    let lines = page(&app_for(&node(40), 18), 60);
    assert_eq!(lines[6], "row 06 of the text");
    assert_eq!(lines[7], "…");
}

#[test]
fn trailing_blank_rows_are_dropped_before_the_ellipsis() {
    let mut text = node(1);
    let rows: Vec<String> = (1..=20)
        .flat_map(|i| [format!("row {i:02}"), String::new()])
        .collect();
    text.description = Some(Text::sanitize_preserving_layout_lines(
        rows.iter().map(String::as_str),
    ));
    let lines = page(&app_for(&text, 12), 60);
    let shown: Vec<&str> = lines[1..7].iter().map(String::as_str).collect();
    assert_eq!(
        shown,
        ["row 01", "", "row 02", "", "row 03", "…"],
        "{lines:?}"
    );
    assert_eq!(lines[7], note_row(60));
}

#[test]
fn the_next_heading_follows_the_note_directly_and_other_breaks_keep_two_blanks() {
    let lines = page(&app_for(&node(20), 20), 60);
    let flags = position(&lines, "FLAGS").unwrap();
    assert_eq!(lines[flags - 1], note_row(60));
    for heading in ["EXAMPLES", "DESCRIPTION ─"] {
        let at = position(&lines, heading).unwrap();
        assert_eq!(lines[at - 1], "", "{heading}");
        assert_eq!(lines[at - 2], "", "{heading}");
        assert!(!lines[at - 3].is_empty(), "{heading}");
    }
    let mut bare = node(20);
    bare.entities.clear();
    let lines = page(&app_for(&bare, 20), 60);
    let examples = position(&lines, "EXAMPLES").unwrap();
    assert_eq!(lines[examples - 1], note_row(60));
    bare.examples.clear();
    let lines = page(&app_for(&bare, 20), 60);
    let full = position(&lines, "DESCRIPTION ─").unwrap();
    assert_eq!(lines[full - 1], note_row(60));
    let mut no_examples = node(20);
    no_examples.examples.clear();
    let lines = page(&app_for(&no_examples, 20), 60);
    let full = position(&lines, "DESCRIPTION ─").unwrap();
    assert_eq!(&lines[full - 2..full], ["", ""]);
    assert!(!lines[full - 3].is_empty());
}

#[test]
fn item_marks_follow_the_headings_and_the_note() {
    let app = app_for(&node(20), 20);
    let node = app.selected_node().unwrap();
    let built = build_lines(node, 60, app.palette, None, app.glyphs, &app);
    let text: Vec<String> = built.lines.iter().map(text_of).collect();
    for at in [0, 1, 8, position(&text, "FLAGS").unwrap()] {
        assert!(built.items.contains(&at), "{at}: {:?}", built.items);
    }
    assert!(!built.items.contains(&7), "{:?}", built.items);
    assert!(built.items.iter().all(|&i| i < text.len()));
    assert!(built.items.windows(2).all(|w| w[0] < w[1]));
}

const COLOR: style::Palette = style::Palette {
    color: true,
    extended: true,
    roles: mandible_core::config::Roles::DEFAULT,
};

/// The built page's lines, with their styles.
fn styled_page(app: &App, width: usize) -> Vec<Line<'static>> {
    let node = app.selected_node().unwrap();
    build_lines(node, width, app.palette, None, app.glyphs, app).lines
}

fn row_style(line: &Line) -> Vec<Style> {
    line.spans.iter().map(|s| s.style).collect()
}

fn preview_start(lines: &[Line]) -> usize {
    lines
        .iter()
        .position(|l| text_of(l).starts_with("DESCRIPTION · preview"))
        .expect("preview heading")
}

#[test]
fn shown_rows_and_the_ellipsis_are_unstyled_and_the_note_is_the_section_grey() {
    let mut app = app_for(&node(20), 20);
    app.palette = COLOR;
    let lines = styled_page(&app, 60);
    let at = preview_start(&lines);
    for row in 1..=7 {
        assert_eq!(row_style(&lines[at + row]), vec![Style::default()], "{row}");
    }
    let note = &lines[at + 8];
    assert!(text_of(note).trim_start().starts_with("↓ full description"));
    let section = style::section_rule(COLOR);
    assert_eq!(section.fg, Some(Color::Indexed(246)));
    assert_eq!(row_style(note), vec![section]);
}

#[test]
fn without_color_the_preview_has_no_style_at_all() {
    let mut app = app_for(&node(20), 20);
    app.palette = style::Palette {
        color: false,
        extended: false,
        roles: mandible_core::config::Roles::DEFAULT,
    };
    let lines = styled_page(&app, 60);
    let at = preview_start(&lines);
    for line in &lines[at + 1..=at + 8] {
        assert!(
            line.spans.iter().all(|s| s.style == Style::default()),
            "{line:?}"
        );
    }
}

#[test]
fn sixteen_colours_draw_the_note_in_dark_gray() {
    let mut app = app_for(&node(20), 20);
    app.palette = style::Palette {
        color: true,
        extended: false,
        roles: mandible_core::config::Roles::DEFAULT,
    };
    let lines = styled_page(&app, 60);
    let at = preview_start(&lines);
    let dark = Style::default().fg(Color::DarkGray);
    assert_eq!(row_style(&lines[at + 8]), vec![dark]);
    assert_eq!(row_style(&lines[at + 7]), vec![Style::default()]);
}

#[test]
fn the_full_description_is_the_last_section_after_examples() {
    let lines = page(&app_for(&node(20), 20), 60);
    let examples = position(&lines, "EXAMPLES").expect("examples");
    let full = position(&lines, "DESCRIPTION ─").expect("full heading");
    assert!(full > examples, "{lines:?}");
    assert_eq!(
        lines.len() - full - 1,
        20,
        "the whole text follows: {lines:?}"
    );
    assert_eq!(lines.last().unwrap(), "row 20 of the text");
    assert_eq!(lines[full + 1], "row 01 of the text");
    assert!(position(&lines, "FLAGS").unwrap() < examples);
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.starts_with("DESCRIPTION"))
            .count(),
        2
    );
}

#[test]
fn ascii_glyphs_give_an_ascii_heading_and_note() {
    let mut app = app_for(&node(20), 20);
    app.glyphs = crate::glyphs::ASCII;
    let lines = page(&app, 60);
    assert!(lines[0].starts_with("DESCRIPTION | preview -"), "{lines:?}");
    assert_eq!(lines[6], "row 06 of the text");
    assert_eq!(lines[7], "...");
    assert!(lines[8]
        .trim_start()
        .starts_with("v full description at the end"));
    assert!(lines.iter().all(|l| l.is_ascii()), "{lines:?}");
}

#[test]
fn wrap_off_measures_the_description_by_its_own_lines() {
    let mut wide = node(1);
    let sentence = "word ".repeat(40);
    let lines: Vec<String> = (0..8).map(|_| sentence.trim().to_string()).collect();
    wide.description = Some(Text::sanitize_preserving_layout_lines(
        lines.iter().map(String::as_str),
    ));
    let mut app = app_for(&wide, 20);
    let wrapped = page(&app, 40);
    assert!(is_preview(&wrapped), "8 lines wrap to many rows at 40");
    app.parsed_wrap = false;
    let own = page(&app, 40);
    assert!(!is_preview(&own), "8 own lines are below twelve rows");
    assert_eq!(position(&own, "DESCRIPTION"), Some(0));
}

#[test]
fn wrap_off_keeps_the_preview_rows_whole() {
    let mut app = app_for(&node(20), 20);
    app.parsed_wrap = false;
    let lines = page(&app, 20);
    assert!(is_preview(&lines));
    assert_eq!(lines[6], "row 06 of the text");
}

fn screen_of(app: &App, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| crate::render::render(f, app)).unwrap();
    let b = term.backend().buffer();
    (0..h)
        .map(|y| (0..w).map(|x| b[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_resize_across_the_line_flips_the_form_and_the_cache_never_serves_the_other() {
    let app = app_for(&node(14), 0);
    // Pane height is the terminal's less seven rows of chrome.
    let tall = screen_of(&app, 100, 40);
    assert!(!tall.contains("preview"), "{tall}");
    let short = screen_of(&app, 100, 24);
    assert!(short.contains("DESCRIPTION · preview"), "{short}");
    assert!(short.contains("full description at the end"), "{short}");
    let tall_again = screen_of(&app, 100, 40);
    assert_eq!(tall_again, tall);
    let short_again = screen_of(&app, 100, 24);
    assert_eq!(short_again, short);
}

#[test]
fn the_wrap_toggle_decides_again() {
    let mut text = node(1);
    let sentence = "word ".repeat(30);
    let lines: Vec<String> = (0..8).map(|_| sentence.trim().to_string()).collect();
    text.description = Some(Text::sanitize_preserving_layout_lines(
        lines.iter().map(String::as_str),
    ));
    let mut app = app_for(&text, 0);
    let wrapped = screen_of(&app, 90, 30);
    assert!(wrapped.contains("DESCRIPTION · preview"), "{wrapped}");
    app.parsed_wrap = false;
    let own = screen_of(&app, 90, 30);
    assert!(!own.contains("preview"), "{own}");
    app.parsed_wrap = true;
    assert_eq!(screen_of(&app, 90, 30), wrapped);
}

#[test]
fn a_flag_target_is_found_below_the_preview() {
    let app = app_for(&node(20), 20);
    let target = FlagKey::Long("verbose".to_string());
    let node = app.selected_node().unwrap();
    let built = build_lines(node, 60, app.palette, Some(&target), app.glyphs, &app);
    let line = built.target_flag_line.expect("flag found");
    assert!(text_of(&built.lines[line]).contains("--verbose"));
}

#[test]
fn the_corpus_page_uses_the_pane_height_of_a_160_by_40_terminal() {
    let rows_for = |n: usize| page_text(&node(n), 76, true).unwrap();
    assert!(!rows_for(17).iter().any(|l| l.contains("preview")));
    assert!(rows_for(18)
        .iter()
        .any(|l| l.starts_with("DESCRIPTION · preview")));
}

#[test]
fn a_cached_draw_never_shows_a_stale_form() {
    let app = app_for(&node(14), 0);
    for (w, h) in [(100, 24), (100, 26), (100, 40), (100, 24), (100, 40)] {
        let area = Rect::new(0, 0, w, h);
        let mut cached = Terminal::new(TestBackend::new(w, h)).unwrap();
        cached.draw(|f| render(f, area, &app)).unwrap();
        let mut reference = Terminal::new(TestBackend::new(w, h)).unwrap();
        reference.draw(|f| render_reference(f, area, &app)).unwrap();
        assert_eq!(
            cached.backend().buffer(),
            reference.backend().buffer(),
            "{w}x{h}"
        );
    }
}

#[test]
fn a_cached_page_follows_the_pane_height_across_the_line() {
    let app = app_for(&node(18), 0);
    let draw = |app: &App, h: u16, cached: bool| {
        let area = Rect::new(0, 0, 100, h);
        let mut term = Terminal::new(TestBackend::new(100, h)).unwrap();
        term.draw(|f| {
            if cached {
                render(f, area, app);
            } else {
                render_reference(f, area, app);
            }
        })
        .unwrap();
        term.backend().buffer().clone()
    };
    let short = draw(&app, 24, true);
    let tall = draw(&app, 60, true);
    assert_ne!(short, tall, "18 rows preview at 17 and not at 53");
    assert_eq!(draw(&app, 24, true), short);
    assert_eq!(draw(&app, 60, true), draw(&app, 60, false));
    assert_eq!(draw(&app, 24, true), draw(&app, 24, false));
}

fn with_summary(summary: &str, description: &str) -> CommandNode {
    let mut node = node(0);
    node.summary = Some(Text::sanitize(summary));
    node.description = Some(Text::sanitize_preserving_layout_lines(description.lines()));
    node
}

#[test]
fn a_description_equal_to_the_summary_after_whitespace_collapse_is_not_drawn() {
    let node = with_summary(
        "Attach local standard input, output, and error streams",
        "Attach local standard input,\n  output, and   error streams ",
    );
    let lines = page(&app_for(&node, 40), 80);
    assert!(position(&lines, "DESCRIPTION").is_none(), "{lines:?}");
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.contains("Attach local standard input"))
            .count(),
        1,
        "{lines:?}"
    );
    assert!(
        position(&lines, "FLAGS").is_some_and(|at| at <= 4),
        "blank rows left behind: {lines:?}"
    );
}

#[test]
fn a_description_that_differs_in_any_way_is_drawn() {
    let node = with_summary("Attach streams", "Attach streams.");
    assert!(position(&page(&app_for(&node, 40), 80), "DESCRIPTION").is_some());
    let node = with_summary("Attach streams", "Attach streams\n\nand more");
    assert!(position(&page(&app_for(&node, 40), 80), "DESCRIPTION").is_some());
}

#[test]
fn a_long_description_that_repeats_the_summary_does_not_preview() {
    let long = (1..=30)
        .map(|i| format!("word{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let one_line = long.replace('\n', " ");
    let node = with_summary(&one_line, &long);
    let lines = page(&app_for(&node, 18), 40);
    assert!(position(&lines, "DESCRIPTION").is_none(), "{lines:?}");
    let node = with_summary("other", &long);
    assert!(is_preview(&page(&app_for(&node, 18), 40)));
}
