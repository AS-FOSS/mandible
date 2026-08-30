//! Spec §9: `[ui] horizontal_scroll = false` makes every detail-pane view
//! wrap. Nothing is ever clipped at the pane's edge in that mode, in any
//! view, at any width.
//!
//! The raw/verbatim view is the one this file exists for. Its wrap-mode
//! path used to hand each tool-authored line to a `Paragraph` carrying no
//! `Wrap` — the widget then ended the line at the pane's last column and
//! everything past it was gone, with no marker, no scroll key that could
//! reach it, and no way for the reader to know. The other two views here
//! (the flag list, and the description/USAGE document around it) already
//! wrapped; they are asserted alongside so "everything wraps" is pinned as
//! one property of the mode rather than as one function's behaviour.
//!
//! Driven through the whole frame (`TestBackend`, AGENTS.md §3.2), because
//! the claim is about the cells that reach the screen: a line can be
//! present in the `Line` vector and still be cut by the widget that draws
//! it, which is exactly what happened here.

use mandible_core::{CommandNode, Entity, EntityKind, Provenance, Source, Spelling, Text};
use mandible_tui::app::{App, RawHelp};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use unicode_width::UnicodeWidthStr;

const TOOL: &str = "widetool";
const WIDTH: u16 = 100;
const HEIGHT: u16 = 44;

/// A padded command table of the shape `ar --help` prints: the run of
/// spaces before the `-` is the alignment its author drew, and it has to
/// survive to the screen (spec §4.1's layout tier).
const ALIGNED_SHORT: &str = "  m[ab]        - move file(s)";

/// Wider than the pane, and made of ordinary words, so a wrap has
/// whitespace to break at.
const LONG_PROSE: &str = "  --report-format FORMAT   choose the output format for the generated \
     report, which is a sentence long enough to run well past a narrow pane";

/// Wider than the pane with no whitespace at all: the wrap has to cut
/// between characters rather than lose the tail.
const LONG_TOKEN: &str = "  https://registry.example.com/v2/org/repo/blobs/uploads/deadbeefcafefeed0123456789abcdef0123456789abcdef0123456789abcdef01234567";

fn raw_lines() -> Vec<Text> {
    [ALIGNED_SHORT, LONG_PROSE, LONG_TOKEN, "", "TAIL-MARKER"]
        .iter()
        .map(|line| Text::sanitize_preserving_layout(line))
        .collect()
}

/// A node that parses, so the structured views have something to draw,
/// carrying raw `--help` text for the verbatim view to show.
fn app_with_raw_help() -> App {
    let mut root = CommandNode::new(TOOL, Provenance::single(Source::HelpText));
    root.children_filled = true;
    let mut app = App::new(TOOL.to_string(), root);
    app.focus = mandible_tui::Focus::Detail;
    app.set_raw_help(
        vec![TOOL.to_string()],
        RawHelp::Ready(raw_lines(), format!("{TOOL} --help")),
    );
    app.toggle_raw_mode();
    app
}

fn detail_rect(app: &App) -> Rect {
    mandible_tui::layout::compute(Rect::new(0, 0, WIDTH, HEIGHT), app.focus)
        .detail
        .expect("the detail pane is visible at this width")
}

/// The detail pane's content rows, trailing space trimmed — leading space
/// is content here, so only the right-hand end is touched.
fn detail_rows(app: &App) -> Vec<String> {
    let backend = TestBackend::new(WIDTH, HEIGHT);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| mandible_tui::render::render(frame, app))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let rect = detail_rect(app);
    ((rect.y + 1)..(rect.y + rect.height - 1))
        .map(|y| {
            let mut row = String::new();
            let mut x = rect.x + 2;
            while x < rect.x + rect.width - 2 {
                let symbol = buffer[(x, y)].symbol();
                row.push_str(symbol);
                // A double-width glyph occupies its cell plus a reserved
                // blank continuation cell; stepping over that placeholder
                // reconstructs the visual text.
                x += UnicodeWidthStr::width(symbol).max(1) as u16;
            }
            row.trim_end().to_string()
        })
        .collect()
}

/// Every non-whitespace character, in order. Two renderings of the same
/// content agree here whether the wrap fell on a space (the run is the
/// break, not content) or mid-token (nothing is inserted), so this is the
/// reading that answers "was anything lost".
fn squash(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

fn assert_rows_fit(app: &App, rows: &[String]) {
    let inner = detail_rect(app).width.saturating_sub(4) as usize;
    for row in rows {
        assert!(
            UnicodeWidthStr::width(row.as_str()) <= inner,
            "a rendered row is wider than the pane's {inner} columns: {row:?}"
        );
    }
}

/// The defect: with the toggle off the raw view drew each tool line once
/// and let the widget end it at the border, so every column past the
/// pane's width was gone from the one view whose whole purpose is showing
/// the reader what the tool printed.
#[test]
fn raw_view_wraps_long_lines_instead_of_clipping_them_when_scroll_is_off() {
    let mut app = app_with_raw_help();
    app.horizontal_scroll_enabled = false;

    let rows = detail_rows(&app);
    let joined = squash(&rows.join(""));

    assert_rows_fit(&app, &rows);
    for line in [LONG_PROSE, LONG_TOKEN] {
        assert!(
            joined.contains(&squash(line)),
            "content was clipped, not wrapped — {:?} is not recoverable from the screen: {rows:?}",
            line
        );
    }
    assert!(
        rows.iter().any(|row| row == "TAIL-MARKER"),
        "the lines after a wrapped one must still be drawn: {rows:?}"
    );
}

/// Wrapping is not reflowing. A line that already fits comes back byte for
/// byte, columns and all, and a line that does not keeps its own indent on
/// the rows it continues onto instead of drifting to column 0.
#[test]
fn a_wrapped_raw_line_keeps_the_authors_columns_and_its_own_indent() {
    let mut app = app_with_raw_help();
    app.horizontal_scroll_enabled = false;

    let rows = detail_rows(&app);
    assert!(
        rows.iter().any(|row| row == ALIGNED_SHORT),
        "a line that fits must reach the screen unedited: {rows:?}"
    );

    let first = rows
        .iter()
        .position(|row| row.starts_with("  --report-format"))
        .expect("the long line's first row");
    let continuation = &rows[first + 1];
    assert!(
        continuation.starts_with("  ") && !continuation.starts_with("   "),
        "a continuation row must carry the line's own indent, not restart at \
         column 0 or gain one: {continuation:?}"
    );
    // And it must be a continuation of *that* line rather than the next
    // one: the two rows together read as a prefix of the source line, and
    // the second adds to what the first showed.
    let two = squash(&format!("{}{}", rows[first], continuation));
    assert!(
        squash(LONG_PROSE).starts_with(&two) && two.len() > squash(&rows[first]).len(),
        "the row after a wrapped line did not continue it: {rows:?}"
    );
    // The author's own run of spaces inside the row survives; a prose
    // word-wrap would have collapsed it to one.
    assert!(
        rows[first].contains("FORMAT   choose"),
        "interior columns were reflowed away: {:?}",
        rows[first]
    );
}

/// The default state is untouched: `horizontal_scroll = true` still draws
/// one row per tool line, clipped at the pane edge, with `l`/`→` revealing
/// the rest.
#[test]
fn the_default_still_scrolls_sideways_rather_than_wrapping() {
    let app = app_with_raw_help();
    assert!(app.horizontal_scroll_enabled, "default is on");

    let rows = detail_rows(&app);
    assert!(
        rows.iter().any(|row| row == ALIGNED_SHORT),
        "a line that fits is the same in both modes: {rows:?}"
    );
    assert!(
        !squash(&rows.join("")).contains(&squash(LONG_TOKEN)),
        "with scrolling on, an over-wide line shows a prefix and the rest \
         waits behind `l`: {rows:?}"
    );

    // The tail is reachable by scrolling, which is what makes the clip
    // legitimate in this mode and not in the other.
    let mut app = app;
    let tail = "0123456789abcdef";
    let mut reached = false;
    for _ in 0..40 {
        app.detail_hscroll_right();
        if squash(&detail_rows(&app).join("")).contains(tail) {
            reached = true;
            break;
        }
    }
    assert!(reached, "`l` never revealed the scrolled-off tail");
}

/// The flag list is mandible's own layout and has always wrapped; asserted
/// here so "everything wraps with the toggle off" is one checked property
/// of the mode rather than a claim about one function.
#[test]
fn the_flag_list_wraps_at_a_narrow_width_when_scroll_is_off() {
    let description = "a description far longer than this narrow pane can \
                       hold on one row, which must therefore continue onto \
                       the next rather than run off the edge";
    let mut root = CommandNode::new(TOOL, Provenance::single(Source::HelpText));
    let mut flag = Entity::new(EntityKind::Flag, Provenance::single(Source::HelpText));
    flag.spellings.push(Spelling::long("report-format"));
    flag.description = Some(Text::sanitize(description));
    root.entities.push(flag);
    root.children_filled = true;

    let mut app = App::new(TOOL.to_string(), root);
    app.focus = mandible_tui::Focus::Detail;
    app.horizontal_scroll_enabled = false;

    let rows = detail_rows(&app);
    assert_rows_fit(&app, &rows);
    assert!(
        squash(&rows.join("")).contains(&squash(description)),
        "the flag description was clipped, not wrapped: {rows:?}"
    );
}

/// The rest of the parsed document — DESCRIPTION prose and the USAGE
/// synopsis, the other content `[ui] horizontal_scroll` governs — at the
/// same narrow width with the toggle off.
#[test]
fn the_detail_pane_document_wraps_at_a_narrow_width_when_scroll_is_off() {
    let description = "Search for PATTERNS in each FILE, where this sentence \
                       is deliberately longer than the pane it is drawn in so \
                       that it has to wrap somewhere.";
    let usage = "widetool [OPTION...] PATTERNS [FILE...] [--synopsis-tail-00] \
                 [--synopsis-tail-01] [--synopsis-tail-02]";
    let mut root = CommandNode::new(TOOL, Provenance::single(Source::HelpText));
    root.description = Some(Text::sanitize(description));
    root.usage = vec![Text::sanitize_preserving_layout(usage)];
    root.children_filled = true;

    let mut app = App::new(TOOL.to_string(), root);
    app.focus = mandible_tui::Focus::Detail;
    app.horizontal_scroll_enabled = false;

    let rows = detail_rows(&app);
    let joined = squash(&rows.join(""));
    assert_rows_fit(&app, &rows);
    assert!(
        joined.contains(&squash(description)),
        "DESCRIPTION was clipped, not wrapped: {rows:?}"
    );
    assert!(
        joined.contains(&squash(usage)),
        "the USAGE synopsis was clipped, not wrapped: {rows:?}"
    );
}
