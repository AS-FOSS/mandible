//! Regression for the display-only refusal surfaced by the raw-help view.
//!
//! `mandible apt-ftparchive`, followed by `jjjj,t`, reaches a `release`
//! node that is invocation-attested but deliberately not heading-attested.
//! The probe refusal is correct. Its explanatory notice is mandible-authored
//! prose, though, so it must wrap even while the root tool output that follows
//! it remains preformatted.

use mandible_core::{CommandNode, Provenance, Source, Text};
use mandible_tui::app::{App, RawHelp};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::Terminal;
use unicode_width::UnicodeWidthStr;

const NOTICE: &str = "mandible could not verify \"release\" as a real subcommand name: it came from a source the probe-safety gate does not accept as evidence a word is safe to run (a native/cobra artifact scan, or a headingless invocation table's layout evidence — neither is a recognized --help heading), so it was never sent as an argument. This is a known limitation of the gate, not something already worked around.";
const CONTROL_USAGE: &str = "Usage: apt [options]";
const END_MARKER: &str = "END-OF-ROOT-HELP";

fn notice_app(notice: &str) -> App {
    let mut root = CommandNode::new("apt-ftparchive", Provenance::single(Source::HelpText));
    let mut release = CommandNode::new("release", Provenance::single(Source::HelpText));
    release.invocation_attested = true;
    root.subcommands.push(release);
    root.children_filled = true;

    let mut app = App::new("apt-ftparchive".to_string(), root);
    app.ensure_rows_fresh();
    app.move_down();
    assert_eq!(
        app.selected_path(),
        Some(vec!["apt-ftparchive".to_string(), "release".to_string()])
    );
    app.toggle_raw_mode();
    app.set_raw_help(
        vec!["apt-ftparchive".to_string(), "release".to_string()],
        RawHelp::Ready(
            vec![
                Text::sanitize_preserving_layout(notice),
                Text::sanitize_preserving_layout(""),
                Text::sanitize_preserving_layout(
                    "Showing the tool's own root --help instead, labelled below:",
                ),
                Text::sanitize_preserving_layout(""),
                Text::sanitize_preserving_layout(CONTROL_USAGE),
                Text::sanitize_preserving_layout(END_MARKER),
            ],
            "apt-ftparchive (name not heading-attested — showing root --help as a fallback)"
                .to_string(),
        ),
    );
    app
}

fn render(width: u16, height: u16, app: &App) -> (Buffer, Rect) {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| mandible_tui::render::render(frame, app))
        .unwrap();
    let detail = mandible_tui::layout::compute(Rect::new(0, 0, width, height), app.focus)
        .detail
        .expect("the tested widths must render the detail pane");
    (terminal.backend().buffer().clone(), detail)
}

fn detail_lines(buffer: &Buffer, detail: Rect) -> Vec<String> {
    let start_x = detail.x + 2;
    let end_x = detail.x + detail.width - 2;
    ((detail.y + 1)..(detail.y + detail.height - 1))
        .map(|y| {
            let mut line = String::new();
            let mut x = start_x;
            while x < end_x {
                let symbol = buffer[(x, y)].symbol();
                line.push_str(symbol);
                // Ratatui stores a double-width glyph in its leading cell
                // and reserves the following cell as a blank continuation.
                // Skipping that placeholder reconstructs the visual text
                // instead of inventing spaces between adjacent CJK glyphs.
                x += UnicodeWidthStr::width(symbol).max(1) as u16;
            }
            line.trim_end().to_string()
        })
        .collect()
}

fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn assert_detail_border_intact(buffer: &Buffer, detail: Rect) {
    let right = detail.x + detail.width - 1;
    let bottom = detail.y + detail.height - 1;
    assert_eq!(buffer[(detail.x, detail.y)].symbol(), "╭");
    assert_eq!(buffer[(right, detail.y)].symbol(), "╮");
    assert_eq!(buffer[(detail.x, bottom)].symbol(), "╰");
    assert_eq!(buffer[(right, bottom)].symbol(), "╯");
    for y in (detail.y + 1)..bottom {
        assert_eq!(buffer[(detail.x, y)].symbol(), "│");
        assert_eq!(buffer[(right, y)].symbol(), "│");
    }
}

fn assert_visual_lines_fit(lines: &[String], detail: Rect) {
    let inner_width = detail.width.saturating_sub(4) as usize;
    for line in lines {
        assert!(
            UnicodeWidthStr::width(line.as_str()) <= inner_width,
            "visual line exceeds {inner_width} cells: {line:?}"
        );
    }
}

#[test]
fn unverified_subcommand_notice_wraps_and_survives_resizing() {
    let app = notice_app(NOTICE);

    for (width, height) in [(90, 48), (60, 48)] {
        let (buffer, detail) = render(width, height, &app);
        let lines = detail_lines(&buffer, detail);
        let joined = normalize(&lines.join("\n"));

        assert_detail_border_intact(&buffer, detail);
        assert_visual_lines_fit(&lines, detail);
        assert!(
            joined.contains(&normalize(NOTICE)),
            "the complete notice was not recoverable at {width}x{height}: {joined:?}"
        );
        assert!(
            lines.iter().any(|line| line == CONTROL_USAGE),
            "ordinary preformatted usage changed at {width}x{height}: {lines:?}"
        );
    }
}

#[test]
fn wrapped_notice_contributes_to_scroll_extent() {
    let mut app = notice_app(NOTICE);
    let (mut buffer, mut detail) = render(60, 24, &app);
    let mut saw_end = detail_lines(&buffer, detail)
        .iter()
        .any(|line| line == END_MARKER);

    loop {
        let before = app.detail_scroll;
        app.detail_scroll_down();
        if app.detail_scroll == before {
            break;
        }
        (buffer, detail) = render(60, 24, &app);
        saw_end |= detail_lines(&buffer, detail)
            .iter()
            .any(|line| line == END_MARKER);
    }

    assert!(
        app.detail_scroll > 0,
        "wrapped notice lines were not counted in the scroll extent"
    );
    assert!(saw_end, "scrolling never reached the root-help tail");
    assert_detail_border_intact(&buffer, detail);
}

#[test]
fn overlong_unicode_notice_token_is_split_by_display_width() {
    let token = format!("release{}", "界".repeat(40));
    let notice = NOTICE.replacen("release", &token, 1);
    let app = notice_app(&notice);
    let (buffer, detail) = render(60, 80, &app);
    let lines = detail_lines(&buffer, detail);

    assert_detail_border_intact(&buffer, detail);
    assert_visual_lines_fit(&lines, detail);
    let compact: String = lines
        .join("\n")
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    assert!(
        compact.contains(&token),
        "the double-width token did not survive wrapping: {compact:?}"
    );
}
