//! The tree pane: one row per visible [`crate::tree::TreeRow`], at fixed
//! column offsets `[indent 2*depth][chevron 1][space 1][name][space]
//! [summary dim]` (spec §9), so mouse hit-testing (`crate::event`) can
//! compute the chevron column arithmetically.
//!
//! Spec §9.1 governs two things this file gets right on purpose:
//!
//! - **The summary column is computed once, over every row in the
//!   flattened list** (`app.rows()`, not the scrolled/visible slice), as
//!   `min(longest prefix+name over all rows, 40% of pane width)`. A
//!   column derived from only the rows currently on screen would jump
//!   every time the user scrolled past a row with a longer or shorter
//!   name — worse than no alignment at all. It's naturally stable across
//!   pure scrolling because `app.rows()` itself doesn't change on
//!   scroll — only on expand/collapse/search/fill (spec §9's caching
//!   rule) — so recomputing the column from the full list on every frame
//!   still yields the same answer while just scrolling.
//! - **The name column never yields to the summary.** The name is always
//!   truncated to fit its own budget first; the summary only gets
//!   whatever's left, and is dropped entirely rather than squeezed into
//!   a handful of useless characters.
//!
//! Spec §9.2/§10: matched search characters are underlined, and only
//! within the name — never the summary. [`mandible_search::match_indices`]
//! re-runs a match scoped to just the row's own name (not the full
//! search haystack, which for a command also includes its summary and
//! for a flag its description), so the returned positions are always
//! directly usable against that name with no offset bookkeeping.

use crate::app::{App, Focus};
use crate::glyphs::Glyphs;
use crate::sanitize::{defensive_single_line, display_width, truncate_to_width_marker};
use crate::style;
use crate::tree::TreeRow;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

/// The summary column is capped at this fraction of the pane's inner
/// width (spec §9.1), so one very long name in a deep tree can't push
/// every summary in the pane off past the edge.
const SUMMARY_COLUMN_CAP_PERCENT: usize = 40;

/// What a convention-discovered row says about itself (spec §5.4, §9.2).
///
/// A word rather than a glyph, and an ASCII one: a symbol here would have
/// to be learned, and this is the row's own claim about whether the command
/// exists — the one thing on the row that must survive a terminal with no
/// colour, no Unicode and no legend (spec §9.2's "what may be drawn").
const UNVERIFIED_BADGE: &str = "unverified";

/// Render the tree pane into `area`.
pub fn render(frame: &mut Frame, area: Rect, app: &App, hide_summaries: bool) {
    let focused = app.focus == Focus::Tree;
    let border_style = if focused {
        style::accent(app.color_enabled)
    } else {
        Style::default()
    };
    // Titled by what the pane *is*, not by the tool — the tool's own name
    // is already the root row one line below, and printing it twice in
    // adjacent lines read as a duplication bug. The count is the useful
    // thing a title can add that the rows can't: how much is in here,
    // including the part scrolled out of view.
    //
    // The detected framework joins it because it is a *tool*-level fact —
    // one generator produced all of this tool's help text — and it used to
    // be repeated in the detail pane's footer under every single command,
    // where it was the same string every time. Read from the root node,
    // which is the only node that is always present.
    let title = match &app.root.detected_framework {
        Some(framework) => format!(
            " commands ({}) {} {} ",
            app.rows().len(),
            app.glyphs.dot,
            defensive_single_line(framework)
        ),
        None => format!(" commands ({}) ", app.rows().len()),
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_set(style::border_set(app.glyphs))
        .border_style(border_style);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let inner_width = inner.width as usize;
    let visible_height = inner.height as usize;
    let rows = app.rows();

    let max_scroll = rows.len().saturating_sub(visible_height);
    let scroll = app.tree_scroll.min(max_scroll);

    // Computed once, over *all* rows — see the module doc for why.
    let summary_column = summary_column(rows, inner_width);

    let query = app.active_filter();

    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible_height)
        .map(|(i, row)| {
            build_row_line(
                row,
                inner_width,
                hide_summaries,
                i == app.selected,
                summary_column,
                app.color_enabled,
                query,
                app.glyphs,
            )
        })
        .collect();

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}

/// The display width of one row's fixed prefix: `depth`'s indent (2 cols
/// each) plus the chevron and its trailing space.
fn prefix_width(depth: usize) -> usize {
    2 * depth + 2
}

/// `min(longest prefix+name over every row, 40% of pane width)`, plus the
/// 2-space gap before the summary starts (spec §9.1).
fn summary_column(rows: &[TreeRow], width: usize) -> usize {
    let longest = rows
        .iter()
        .map(|r| prefix_width(r.depth) + display_width(&defensive_single_line(&r.name)))
        .max()
        .unwrap_or(0);
    let cap = width * SUMMARY_COLUMN_CAP_PERCENT / 100;
    longest.min(cap) + 2
}

#[allow(clippy::too_many_arguments)]
fn build_row_line(
    row: &TreeRow,
    width: usize,
    hide_summary: bool,
    selected: bool,
    summary_column: usize,
    color_enabled: bool,
    query: Option<&str>,
    glyphs: Glyphs,
) -> Line<'static> {
    let indent = "  ".repeat(row.depth);
    let chevron = if !row.has_children {
        ' '
    } else if row.expanded {
        glyphs.chevron_open
    } else {
        glyphs.chevron_closed
    };
    let prefix = format!("{indent}{chevron} ");
    let prefix_w = display_width(&prefix);
    let name = defensive_single_line(&row.name);

    let base_style = if selected {
        style::selected(color_enabled)
    } else {
        Style::default()
    };

    // The name column never yields (spec §9.1): truncate the name itself
    // first, to whatever's left after the fixed prefix, before anything
    // else ever gets a chance at the row's width budget.
    let name_budget = width.saturating_sub(prefix_w);
    let truncated_name = truncate_to_width_marker(&name, name_budget, glyphs.ellipsis);
    let name_part_width = prefix_w + display_width(&truncated_name);

    // Underline matched characters within the name only (spec §9.2 /
    // §10) — computed against the pre-truncation name so match positions
    // are stable, but only ever rendered against the surviving
    // (truncated) prefix of it, which is what's actually on screen.
    let match_idx = query
        .map(|q| mandible_search::match_indices(&name, q))
        .unwrap_or_default();
    let mut spans = vec![Span::styled(prefix, base_style)];
    spans.extend(styled_name_spans(&truncated_name, &match_idx, base_style));

    if row.pending {
        // A lazy fill is in flight for this node (spec §5.2 step 3, §9
        // "designed degraded states"): show a subtle spinner instead of a
        // (possibly stale or absent) summary, aligned to the same
        // computed column a real summary would use.
        if name_part_width < width {
            // `+ 1` for the same reason the summary branch below does it,
            // and against the same failure: a name that reaches the shared
            // column leaves `pad_to` equal to its own width, so the marker
            // starts in the very next cell. `dnf`'s longest command
            // rendered as `check-update⋯ loading`, one mangled word rather
            // than a name and its status. Fixed once below for summaries
            // (apt-get's `dselect-upgradeFollow`) and missed here, because
            // no tool in the suite had a pending row at the column until
            // `dnf` gained subcommands.
            let pad_to = summary_column.max(name_part_width + 1);
            let padding = " ".repeat(pad_to.saturating_sub(name_part_width));
            let remaining = width.saturating_sub(pad_to);
            if remaining > 0 {
                spans.push(Span::raw(padding));
                let marker = truncate_to_width_marker(glyphs.loading, remaining, glyphs.ellipsis);
                spans.push(Span::styled(marker, style::muted(color_enabled)));
            }
        }
        return Line::from(spans);
    }

    // A row's summary never changes because of a search. Swapping it for a
    // "why this matched" hint made the pane's content shift under the user
    // mid-keystroke, which is a worse problem than the one it solved — the
    // filter itself is now precise enough not to need explaining.
    let summary = if hide_summary {
        None
    } else {
        row.summary
            .as_ref()
            .map(|s| defensive_single_line(s))
            .filter(|s| !s.is_empty())
    };
    // The unverified marker is *not* dropped with the summaries at narrow
    // widths (spec §9.1's width ladder): a summary is a convenience, and
    // this says whether the command on the row is one the tool documents at
    // all (spec §5.4, §9.2). It takes the column first, and the summary
    // gets what is left.
    if (row.unverified || summary.is_some()) && name_part_width < width {
        // Align to the shared column when the name is short enough to leave
        // room for it; otherwise the summary simply starts right after the
        // name (still never truncating the name to force alignment). At
        // least one space, always. When a name is longer than the shared
        // column, `pad_to` used to collapse to exactly the name's width and
        // the summary began in the very next cell:
        // `dselect-upgradeFollow dselect…` in `apt-get`, which reads as one
        // mangled word rather than a name and its description.
        let pad_to = summary_column.max(name_part_width + 1);
        let mut remaining = width.saturating_sub(pad_to);
        if remaining > 0 {
            spans.push(Span::raw(" ".repeat(pad_to - name_part_width)));
            if row.unverified {
                let badge = truncate_to_width_marker(UNVERIFIED_BADGE, remaining, glyphs.ellipsis);
                remaining -= display_width(&badge);
                spans.push(Span::styled(badge, style::warning(color_enabled)));
            }
            if let Some(summary) = summary {
                let separator = if row.unverified {
                    format!(" {} ", glyphs.dot)
                } else {
                    String::new()
                };
                // The separator only earns its cells when something legible
                // follows it.
                if remaining > display_width(&separator) {
                    remaining -= display_width(&separator);
                    let truncated = truncate_to_width_marker(&summary, remaining, glyphs.ellipsis);
                    spans.push(Span::styled(separator, style::muted(color_enabled)));
                    spans.push(Span::styled(truncated, style::muted(color_enabled)));
                }
            }
        }
    }

    Line::from(spans)
}

/// Split `name` into spans alternating between `base_style` and
/// `base_style` + underline wherever `match_idx` (character indices)
/// says a character matched the active search query (spec §9.2:
/// "Search match characters: Underline, within the name only").
fn styled_name_spans(name: &str, match_idx: &[u32], base_style: Style) -> Vec<Span<'static>> {
    if match_idx.is_empty() {
        return vec![Span::styled(name.to_string(), base_style)];
    }
    let match_set: std::collections::HashSet<u32> = match_idx.iter().copied().collect();
    let match_style = base_style.add_modifier(ratatui::style::Modifier::UNDERLINED);

    let mut spans = Vec::new();
    let mut current = String::new();
    let mut current_matched = false;
    for (i, c) in name.chars().enumerate() {
        let is_matched = match_set.contains(&(i as u32));
        if !current.is_empty() && is_matched != current_matched {
            let style = if current_matched {
                match_style
            } else {
                base_style
            };
            spans.push(Span::styled(std::mem::take(&mut current), style));
        }
        current.push(c);
        current_matched = is_matched;
    }
    if !current.is_empty() {
        let style = if current_matched {
            match_style
        } else {
            base_style
        };
        spans.push(Span::styled(current, style));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(depth: usize, name: &str, summary: Option<&str>, has_children: bool) -> TreeRow {
        TreeRow {
            path: vec![name.to_string()],
            depth,
            name: name.to_string(),
            summary: summary.map(|s| s.to_string()),
            has_children,
            expanded: false,
            children_filled: true,
            hidden: false,
            pending: false,
            unverified: false,
        }
    }

    fn rendered(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn chevron_position_matches_two_times_depth() {
        let r = row(2, "onto", None, false);
        let line = build_row_line(&r, 80, false, false, 20, true, None, crate::glyphs::UNICODE);
        let text = rendered(&line);
        // indent "    " (4 chars = 2*depth) then chevron/space/name.
        assert_eq!(&text[0..4], "    ");
    }

    #[test]
    fn adversarial_name_never_produces_embedded_newline() {
        let r = row(0, "evil\nname\x1b[31m", None, true);
        let line = build_row_line(&r, 80, false, false, 20, true, None, crate::glyphs::UNICODE);
        let text = rendered(&line);
        assert!(!text.contains('\n'));
    }

    #[test]
    fn long_summary_is_truncated_to_width() {
        let long_summary = "x".repeat(500);
        let r = row(0, "cmd", Some(&long_summary), false);
        let line = build_row_line(&r, 40, false, false, 10, true, None, crate::glyphs::UNICODE);
        let text = rendered(&line);
        assert!(display_width(&text) <= 40);
    }

    /// Spec §5.4/§9.2: a node the parent's own help never listed says so on
    /// its row, in the warning shade — the one sanctioned non-accent colour.
    #[test]
    fn an_unverified_row_carries_the_badge() {
        let mut r = row(1, "clippy", None, false);
        r.unverified = true;
        let line = build_row_line(&r, 80, false, false, 20, true, None, crate::glyphs::UNICODE);
        assert!(rendered(&line).contains("unverified"), "{line:?}");
        let badge = line
            .spans
            .iter()
            .find(|s| s.content.contains("unverified"))
            .expect("badge span");
        assert_eq!(badge.style, style::warning(true));
    }

    #[test]
    fn an_ordinary_row_carries_no_badge() {
        let r = row(1, "clean", Some("Remove the target directory"), false);
        let line = build_row_line(&r, 80, false, false, 20, true, None, crate::glyphs::UNICODE);
        assert!(!rendered(&line).contains("unverified"), "{line:?}");
    }

    /// The badge takes the column first and the summary follows it, so a
    /// discovered node that *does* have a summary shows both.
    #[test]
    fn a_badged_row_still_shows_its_summary_behind_a_separator() {
        let mut r = row(1, "clippy", Some("Checks a package"), false);
        r.unverified = true;
        let line = build_row_line(&r, 80, false, false, 20, true, None, crate::glyphs::UNICODE);
        let text = rendered(&line);
        assert!(text.contains("unverified · Checks a package"), "{text:?}");
    }

    /// Spec §9.1's width ladder drops summaries on a narrow pane. The badge
    /// is not a summary: it says whether the command exists at all.
    #[test]
    fn the_badge_survives_the_narrow_width_ladder() {
        let mut r = row(1, "clippy", Some("Checks a package"), false);
        r.unverified = true;
        let line = build_row_line(&r, 40, true, false, 12, true, None, crate::glyphs::UNICODE);
        let text = rendered(&line);
        assert!(text.contains("unverified"), "{text:?}");
        assert!(!text.contains("Checks a package"), "{text:?}");
    }

    #[test]
    fn a_badged_row_still_respects_the_width_budget() {
        // From the width at which the row's fixed prefix (indent, chevron,
        // space) fits at all — narrower than that, no row of any kind fits,
        // badge or no badge.
        for width in prefix_width(2)..40 {
            let mut r = row(
                2,
                "generate-rpm",
                Some("a summary long enough to spill"),
                false,
            );
            r.unverified = true;
            let line = build_row_line(
                &r,
                width,
                false,
                false,
                10,
                true,
                None,
                crate::glyphs::UNICODE,
            );
            assert!(
                display_width(&rendered(&line)) <= width,
                "overflowed at width {width}: {:?}",
                rendered(&line)
            );
        }
    }

    #[test]
    fn pending_row_shows_spinner_not_summary() {
        let mut r = row(0, "get", Some("should not show while pending"), true);
        r.pending = true;
        let line = build_row_line(&r, 80, false, false, 20, true, None, crate::glyphs::UNICODE);
        let text = rendered(&line);
        assert!(text.contains("loading"), "{text:?}");
        assert!(!text.contains("should not show while pending"), "{text:?}");
    }

    #[test]
    fn pending_row_still_respects_width_budget() {
        let mut r = row(0, "get", None, true);
        r.pending = true;
        let line = build_row_line(&r, 12, false, false, 5, true, None, crate::glyphs::UNICODE);
        let text = rendered(&line);
        assert!(display_width(&text) <= 12);
    }

    /// A pending row's spinner must never touch the name, exactly as a
    /// summary must not. `dnf`'s longest command rendered
    /// `check-update⋯ loading` — the name and its status reading as one
    /// mangled word, the same defect apt-get's `dselect-upgradeFollow`
    /// produced in the summary branch and which was only fixed there.
    #[test]
    fn a_pending_row_keeps_a_gap_between_the_name_and_the_spinner() {
        // Column set exactly at the name's own width, which is what a row
        // holding the longest name in the tree gets.
        let mut r = row(0, "check-update", None, true);
        r.pending = true;
        let name_width = prefix_width(0) + display_width("check-update");
        let line = build_row_line(
            &r,
            60,
            false,
            false,
            name_width,
            true,
            None,
            crate::glyphs::UNICODE,
        );
        let text = rendered(&line);
        assert!(
            !text.contains("check-update\u{22ef}"),
            "spinner is touching the name: {text:?}"
        );
        assert!(text.contains("check-update "), "{text:?}");
    }

    /// Spec §9.1: the summary column is computed over the *whole*
    /// flattened row set, not whichever rows happen to be on screen — so
    /// scrolling to reveal a row with a much longer or shorter name must
    /// not shift where every other summary starts.
    #[test]
    fn summary_column_is_computed_over_every_row_not_just_visible_ones() {
        let rows = vec![
            row(0, "a", Some("short name summary"), false),
            row(
                0,
                "a-much-longer-command-name",
                Some("other summary"),
                false,
            ),
        ];
        let col = summary_column(&rows, 80);
        // Must reflect the longer name (prefix 2 + 27 chars = 29) plus
        // the 2-space gap, not just the short "a" row.
        assert_eq!(
            col,
            prefix_width(0) + display_width("a-much-longer-command-name") + 2
        );
    }

    #[test]
    fn summary_column_is_capped_at_40_percent_of_width() {
        let rows = vec![row(0, &"x".repeat(200), Some("summary"), false)];
        let col = summary_column(&rows, 100);
        assert!(col <= 40 + 2, "col={col}");
    }

    /// The name column must never be sacrificed to make room for a
    /// summary — a summary that would leave no budget for itself is
    /// simply omitted, but the name is always shown (truncated at a word
    /// boundary with an ellipsis if it doesn't fit on its own).
    #[test]
    fn name_column_never_yields_to_the_summary() {
        let r = row(
            0,
            "a-genuinely-quite-long-subcommand-name",
            Some("summary"),
            false,
        );
        let line = build_row_line(&r, 15, false, false, 5, true, None, crate::glyphs::UNICODE);
        let text = rendered(&line);
        assert!(display_width(&text) <= 15);
        assert!(text.contains('…'), "{text:?}");
        assert!(!text.contains("summary"), "{text:?}");
    }

    /// Truncation happens at a word boundary, not mid-word — spec §9.1.
    #[test]
    fn summary_truncates_at_word_boundary_with_ellipsis() {
        let r = row(
            0,
            "add",
            Some("Add file contents to the index right now please"),
            false,
        );
        let line = build_row_line(&r, 30, false, false, 6, true, None, crate::glyphs::UNICODE);
        let text = rendered(&line);
        assert!(text.contains('…'), "{text:?}");
        assert!(display_width(&text) <= 30);
    }

    #[test]
    fn no_color_selected_row_still_readable_via_reverse() {
        let r = row(0, "add", None, false);
        let line = build_row_line(&r, 40, false, true, 10, false, None, crate::glyphs::UNICODE);
        // With color disabled the base style must still carry REVERSED so
        // the selection is visible without any color at all.
        assert!(line.spans[0]
            .style
            .add_modifier
            .contains(ratatui::style::Modifier::REVERSED));
        assert_eq!(line.spans[0].style.fg, None);
    }

    /// Spec §9.2 / §10: matched characters within the name are underlined
    /// — and only within the name, never the summary.
    #[test]
    fn matched_characters_within_the_name_are_underlined() {
        let r = row(0, "rebase", Some("Reapply commits"), true);
        let line = build_row_line(
            &r,
            80,
            false,
            false,
            20,
            true,
            Some("rb"),
            crate::glyphs::UNICODE,
        );
        let underlined: String = line
            .spans
            .iter()
            .filter(|s| {
                s.style
                    .add_modifier
                    .contains(ratatui::style::Modifier::UNDERLINED)
            })
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(underlined, "rb", "expected just 'r' and 'b' underlined");
        // The summary must never be underlined, even though it also
        // contains matchable characters.
        let summary_span = line
            .spans
            .iter()
            .find(|s| s.content.contains("Reapply"))
            .expect("summary span present");
        assert!(!summary_span
            .style
            .add_modifier
            .contains(ratatui::style::Modifier::UNDERLINED));
    }

    #[test]
    fn no_query_means_no_underline() {
        let r = row(0, "rebase", None, false);
        let line = build_row_line(&r, 80, false, false, 20, true, None, crate::glyphs::UNICODE);
        assert!(line.spans.iter().all(|s| !s
            .style
            .add_modifier
            .contains(ratatui::style::Modifier::UNDERLINED)));
    }
}
