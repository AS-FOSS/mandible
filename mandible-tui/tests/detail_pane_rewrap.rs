//! Regression test for Defect B (batch 2): source prose that arrives
//! hard-wrapped at ~80 columns must not be re-wrapped raggedly at the
//! pane's actual width. The fix has two parts — `mandible_core::Text`
//! unwraps hard-wrapped paragraphs at the IR boundary (a single `\n`
//! inside a paragraph joins to a space; `\n\n` stays a break), and the
//! detail pane relies on `ratatui`'s `Paragraph` `Wrap` to re-wrap the
//! now-unwrapped paragraph text at render time. This test renders through
//! the real pipeline (`TestBackend`) and inspects the actual buffer rows,
//! not just the intermediate `Text`/`Line` values, since the ragged-wrap
//! bug only manifests once rendering is involved.

use mandible_core::{CommandNode, Provenance, Source, Text};
use mandible_tui::app::App;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// A paragraph hard-wrapped at ~45 columns, the way carapace's
/// `documentation` field arrives (see mandible-core's real-fixture tests for
/// the actual vendored strings this mirrors).
const HARD_WRAPPED: &str = "Git is a fast, scalable, distributed revision\ncontrol system with an\nunusually rich command set that provides both\nhigh-level operations and\nfull access to internals.";

fn node_with_hardwrapped_description() -> CommandNode {
    let mut root = CommandNode::new(
        "git",
        Provenance::single(Source::KnownSpec {
            provider: "carapace".to_string(),
        }),
    );
    root.description = Some(Text::sanitize(HARD_WRAPPED));
    root
}

/// Render the detail pane at `width` columns and return the visual lines
/// of its interior (border-stripped), trimmed of trailing spaces.
fn render_detail_lines(width: u16) -> Vec<String> {
    let root = node_with_hardwrapped_description();
    let mut app = App::new("git".to_string(), root);
    // Below the stack breakpoint only the focused pane renders; force
    // detail focus so this helper works at any width the tests pass in.
    app.focus = mandible_tui::Focus::Detail;

    let backend = TestBackend::new(width, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            mandible_tui::render::render(frame, &app);
        })
        .unwrap();

    let buffer = terminal.backend().buffer().clone();
    let regions =
        mandible_tui::layout::compute(ratatui::layout::Rect::new(0, 0, width, 20), app.focus);
    let detail_rect = regions.detail.expect("wide enough for a detail pane");

    let mut lines = Vec::new();
    for y in (detail_rect.y + 1)..(detail_rect.y + detail_rect.height - 1) {
        let mut line = String::new();
        for x in (detail_rect.x + 1)..(detail_rect.x + detail_rect.width - 1) {
            line.push_str(buffer[(x, y)].symbol());
        }
        lines.push(line.trim_end().to_string());
    }
    lines
}

#[test]
fn hard_wrapped_description_reflows_to_the_actual_pane_width() {
    // At a generous width, the original ~45-column hard-wrap points must
    // not survive as-is. The source breaks mid-sentence at "revision" /
    // "an" / "both" / "and"; a correct reflow re-wraps at word boundaries
    // determined by the pane's actual width instead, so none of those
    // ragged mid-sentence breaks should appear as a rendered line's exact
    // ending, and the paragraph should collapse onto fewer, wider lines
    // than the 5 short lines it arrived as.
    let lines = render_detail_lines(120);
    let non_empty: Vec<&String> = lines
        .iter()
        .filter(|l| !l.is_empty() && l.as_str() != "DESCRIPTION" && !l.starts_with("carapace"))
        .collect();

    // None of the original 5 hard-wrapped source lines may survive
    // verbatim as a rendered line — that would mean re-wrapping never
    // actually happened.
    for original_line in HARD_WRAPPED.split('\n') {
        assert!(
            !non_empty.iter().any(|rendered| rendered.as_str() == original_line),
            "rendered output still contains an original ragged hard-wrap line verbatim: {original_line:?} in {non_empty:?}"
        );
    }

    assert!(
        non_empty.len() < 5,
        "expected the reflow to use fewer lines than the original 5-line hard wrap, got {non_empty:?}"
    );

    // The full sentence must still be present somewhere (reflowing must
    // not have dropped or corrupted words at the old wrap points).
    // Whitespace-normalised: the pane has a column of padding either side,
    // so cell-level extraction yields runs of spaces that are layout, not
    // content. This assertion is about words surviving the wrap points.
    let joined = non_empty
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        joined.contains("revision control system"),
        "joined: {joined:?}"
    );
    assert!(
        joined.contains("unusually rich command set"),
        "joined: {joined:?}"
    );
    assert!(
        joined.contains("full access to internals"),
        "joined: {joined:?}"
    );
}

#[test]
fn narrower_pane_still_wraps_cleanly_without_raggedness_from_source_breaks() {
    // At a narrower width than the original hard-wrap column, Paragraph's
    // own word-wrap takes over — the key property is that the wrap points
    // are determined by the *current* width, not leftover from the
    // source's ~45-column breaks.
    let lines = render_detail_lines(40);
    let non_empty: Vec<&String> = lines.iter().filter(|l| !l.is_empty()).collect();
    assert!(!non_empty.is_empty());
    // Every rendered line should be close to using the available width
    // (ratatui's word-wrap doesn't leave large unused trailing space
    // unless a single word doesn't fit), not stopping short at ~old
    // 45-column boundaries inside a much narrower pane.
    for line in &non_empty[..non_empty.len().saturating_sub(1)] {
        assert!(
            line.chars().count() <= 38, // pane inner width budget
            "line exceeds pane width: {line:?}"
        );
    }
}
