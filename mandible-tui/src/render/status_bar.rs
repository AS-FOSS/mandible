//! The status bar: keybinding hints, or a transient status message (spec
//! §2's footer row: `↑↓ move   → expand   / search   y copy   ? help   q
//! quit`; `→`'s label has since grown a second meaning, spec §9, see
//! [`hints`]).

use crate::app::App;
use crate::sanitize::{defensive_single_line, display_width, truncate_to_width};
use crate::style;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// One footer, true everywhere, never focus-conditional: `Ctrl-C` rather
/// than `q` (the only key that quits from every focus, since `q` types
/// the letter in the search box), `Esc` named explicitly, wide separators
/// between hints. Built per-frame because the arrow glyphs depend on what
/// the terminal can draw ([`crate::glyphs`]).
///
/// `←→` names both of its meanings at once (tree collapse/expand vs.
/// detail-pane horizontal scroll) rather than picking one focus's
/// meaning, the same way `↑↓ move` already does for tree/detail scroll.
/// The label depends on `horizontal_scroll_enabled` (the config, not
/// focus): with `[ui] horizontal_scroll = false` the footer must not
/// advertise a disabled behavior (spec §9), so it falls back to `expand`
/// alone.
fn hints(
    glyphs: crate::glyphs::Glyphs,
    horizontal_scroll_enabled: bool,
    print_selection: bool,
) -> Vec<String> {
    let horizontal_hint = if horizontal_scroll_enabled {
        format!("{} expand/scroll", glyphs.arrows_horizontal)
    } else {
        format!("{} expand", glyphs.arrows_horizontal)
    };
    let mut hints = Vec::new();
    // First in the list, so it is the last hint a narrowing row drops:
    // under `--print-selection` this is the key the whole invocation
    // exists for, and it is the one key whose meaning differs from the
    // one a reader already knows.
    if print_selection {
        hints.push("Enter select".to_string());
    }
    hints.extend([
        format!("{} move", glyphs.arrows_vertical),
        horizontal_hint,
        "/ search".to_string(),
        "Tab pane".to_string(),
        "t raw".to_string(),
        "Esc back".to_string(),
        "y copy".to_string(),
        "r reload".to_string(),
        "? help".to_string(),
        "^C quit".to_string(),
    ]);
    hints
}

/// Gap between hints. Wide on purpose: at two spaces the row reads as one
/// long string rather than a list of separate keys.
const HINT_GAP: &str = "    ";

/// How many trailing hints are never dropped, however narrow the row.
const PINNED_HINTS: usize = 2;

/// Left margin, matching the panes' own border-plus-padding above, so the
/// hints don't start hard against the screen edge one row below a border.
const LEFT_MARGIN: &str = "  ";

/// Join as many hints as fit, always keeping `? help` and `^C quit`
/// whole: hints are dropped from the least important end, never
/// mid-word truncated. `^C quit` is the escape hatch; `?` is what makes
/// everything dropped from the middle discoverable again.
fn hints_for_width(
    width: usize,
    glyphs: crate::glyphs::Glyphs,
    horizontal_scroll_enabled: bool,
    print_selection: bool,
) -> String {
    let all = hints(glyphs, horizontal_scroll_enabled, print_selection);
    let split = all.len().saturating_sub(PINNED_HINTS);
    let (rest, pinned) = all.split_at(split);
    let pinned_len: usize = pinned.iter().map(|h| h.chars().count()).sum::<usize>()
        + HINT_GAP.chars().count() * pinned.len().saturating_sub(1);

    let mut kept: Vec<&str> = Vec::new();
    for hint in rest {
        let candidate_len = kept
            .iter()
            .copied()
            .chain(std::iter::once(hint.as_str()))
            .map(|h| h.chars().count())
            .sum::<usize>()
            + pinned_len
            + HINT_GAP.chars().count() * (kept.len() + 1);
        if candidate_len > width {
            break;
        }
        kept.push(hint);
    }
    kept.extend(pinned.iter().map(|h| h.as_str()));
    kept.join(HINT_GAP)
}

/// Render the status bar into `area` (a single row, no border, per spec
/// §2's layout).
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let width = area.width as usize;

    // A transient message (`copied: …`) owns the whole row while it lasts.
    if let Some(msg) = &app.status_message {
        let text = truncate_to_width(&defensive_single_line(msg), width);
        let paragraph = Paragraph::new(Line::styled(
            format!("{LEFT_MARGIN}{text}"),
            Style::default().add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(paragraph, area);
        return;
    }

    // Controls on the left, this node's provenance on the right — the
    // right-hand end of this row sits under the detail pane, which is what
    // the provenance describes. Keeping it out of the pane's own content
    // means the documentation starts at the top of the pane instead of a
    // line down, and the fact stays visible while you scroll.
    let right = right_text(app);
    let right_width = right.as_ref().map(|(t, _)| display_width(t)).unwrap_or(0);

    let margin_width = display_width(LEFT_MARGIN) * 2;
    let hints_budget = width.saturating_sub(right_width + margin_width + 2);
    let hints = hints_for_width(
        hints_budget,
        app.glyphs,
        app.horizontal_scroll_enabled,
        app.print_selection,
    );

    let mut spans = vec![
        Span::raw(LEFT_MARGIN),
        // Not muted: these are the controls, and a footer nobody can read
        // is a footer nobody uses. Muted is for text that is genuinely
        // secondary to something else on the same row.
        Span::styled(hints.clone(), Style::default()),
    ];
    if let Some((text, style)) = right {
        let used = display_width(LEFT_MARGIN) + display_width(&hints);
        let pad = width
            .saturating_sub(used + right_width + display_width(LEFT_MARGIN))
            .max(1);
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(text, style));
        spans.push(Span::raw(LEFT_MARGIN));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The right-hand end of the status row: a low-confidence warning when
/// there is one, otherwise where the selected node's data came from.
fn right_text(app: &App) -> Option<(String, Style)> {
    let node = app.selected_node()?;
    if let Some(caveat) = crate::render::detail_pane::provenance_caveat(node, app.glyphs) {
        return Some((caveat, style::warning(app.color_enabled)));
    }
    let summary = crate::render::detail_pane::provenance_summary(node);
    if summary.is_empty() {
        return None;
    }
    Some((summary, style::muted(app.color_enabled)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_terminal_shows_every_hint() {
        let rendered = hints_for_width(120, crate::glyphs::UNICODE, true, false);
        for h in hints(crate::glyphs::UNICODE, true, false) {
            assert!(rendered.contains(&h), "{h} missing from {rendered:?}");
        }
    }

    /// Every key that changes what the tool does must be reachable without
    /// opening `?` first.
    #[test]
    fn every_action_key_is_named_at_full_width() {
        let rendered = hints_for_width(140, crate::glyphs::UNICODE, true, false);
        for key in [
            "/ search", "Tab pane", "t raw", "y copy", "r reload", "? help",
        ] {
            assert!(rendered.contains(key), "{key} missing from {rendered:?}");
        }
    }

    /// Every hint stays readable without Unicode — the footer is the last
    /// thing that should turn into boxes for someone who cannot get out.
    #[test]
    fn ascii_fallback_hints_are_pure_ascii() {
        let rendered = hints_for_width(120, crate::glyphs::ASCII, true, false);
        assert!(rendered.is_ascii(), "{rendered:?}");
        assert!(rendered.contains("^C quit"));
    }

    /// The escape hatch survives at any width; regression fixture: an
    /// 88-column terminal.
    #[test]
    fn quit_hint_survives_a_narrow_terminal() {
        for width in [20, 30, 40, 60, 88] {
            let hints = hints_for_width(width, crate::glyphs::UNICODE, true, false);
            assert!(hints.contains("^C quit"), "width {width}: {hints:?}");
            // A narrow row hides most of the footer, which is exactly
            // where the reader needs to know the full list exists.
            assert!(hints.contains("? help"), "width {width}: {hints:?}");
            assert!(
                hints.chars().count() <= width.max(7),
                "width {width} overflowed: {hints:?}"
            );
        }
    }

    /// `[ui] horizontal_scroll = false` must reach the footer too, not
    /// just the pane content, since a hint for a disabled key is a
    /// promise the mode does not keep.
    #[test]
    fn horizontal_hint_matches_the_config_toggle() {
        for width in [40, 60, 80, 100, 120, 140] {
            let on = hints_for_width(width, crate::glyphs::UNICODE, true, false);
            let off = hints_for_width(width, crate::glyphs::UNICODE, false, false);

            assert!(
                !off.contains("expand/scroll"),
                "off must not advertise the disabled behavior at width {width}: {off:?}"
            );

            // `off`'s label is strictly shorter, so at any width it can
            // only fit as many or more of the non-pinned hints than `on`.
            for hint in ["t raw", "Esc back", "y copy", "r reload", "Tab pane"] {
                if on.contains(hint) {
                    assert!(
                        off.contains(hint),
                        "off dropped {hint:?} at width {width} while on kept it: on={on:?} off={off:?}"
                    );
                }
            }
        }

        // At a comfortable width, off still names collapse/expand — it
        // only drops the "/scroll" half, not the whole hint.
        let off_wide = hints_for_width(120, crate::glyphs::UNICODE, false, false);
        assert!(off_wide.contains("expand"), "{off_wide:?}");

        // `render()` subtracts the right-hand provenance text's width
        // from the terminal width before budgeting hints (`hints_budget`
        // in `render`); reproduced here at the raw-width crossover.
        let crossover = 68;
        let on = hints_for_width(crossover, crate::glyphs::UNICODE, true, false);
        let off = hints_for_width(crossover, crate::glyphs::UNICODE, false, false);
        assert!(
            !on.contains("Tab pane"),
            "expected width {crossover} to be the known trade-off point: {on:?}"
        );
        assert!(
            off.contains("Tab pane"),
            "off must restore Tab pane at the width where on drops it: {off:?}"
        );
    }

    /// `--print-selection` changes what `Enter` does, so the footer says
    /// so — and says nothing extra in every other session. The second half
    /// is the load-bearing one: the default footer is the row that shipped,
    /// hint for hint.
    #[test]
    fn the_select_hint_appears_only_in_print_selection_mode() {
        let on = hints(crate::glyphs::UNICODE, true, true);
        let off = hints(crate::glyphs::UNICODE, true, false);
        assert_eq!(on.first().map(String::as_str), Some("Enter select"));
        assert_eq!(&on[1..], &off[..], "the mode adds one hint and moves none");
        assert!(
            !off.iter().any(|h| h.contains("Enter")),
            "the default footer must not mention Enter: {off:?}"
        );
    }

    /// Being first in the list makes this the last hint dropped as the row
    /// narrows, which is what it should be in a mode named after it. It is
    /// not *pinned* — below roughly 34 columns only `? help` and `^C quit`
    /// remain, and `?` is what makes everything dropped discoverable
    /// again.
    #[test]
    fn the_select_hint_is_the_last_one_dropped() {
        for width in [40, 60, 88, 120] {
            let rendered = hints_for_width(width, crate::glyphs::UNICODE, true, true);
            assert!(
                rendered.contains("Enter select"),
                "width {width}: {rendered:?}"
            );
        }
    }

    /// The controls keep a left margin rather than starting hard against
    /// the screen edge one row below a pane border.
    #[test]
    fn hints_are_not_flush_against_the_left_edge() {
        assert!(LEFT_MARGIN.chars().all(|c| c == ' '));
        assert!(!LEFT_MARGIN.is_empty());
    }

    /// Provenance shares the row with the controls, so the hints must be
    /// budgeted for it — otherwise the two overlap at narrow widths.
    #[test]
    fn hints_shrink_to_leave_room_for_the_right_hand_text() {
        let full = hints_for_width(120, crate::glyphs::UNICODE, true, false);
        let squeezed = hints_for_width(40, crate::glyphs::UNICODE, true, false);
        assert!(
            squeezed.chars().count() < full.chars().count(),
            "hints should give way: {squeezed:?} vs {full:?}"
        );
        assert!(squeezed.contains("^C quit"));
    }
}
