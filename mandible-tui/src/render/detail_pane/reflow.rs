//! What a re-flow keeps and what a sideways scroll extends: the item map
//! of a built page, the anchor that holds the reading position across a
//! wrap toggle or a resize, and the rules that reach the pane's edge.

use super::*;

/// Record that a logical item starts at line `at`. Items are what a
/// re-flow preserves: one entity, heading, divider or source line,
/// however many lines it wraps onto.
pub(super) fn mark(items: &mut Vec<usize>, at: usize) {
    if items.last() != Some(&at) {
        items.push(at);
    }
}

/// Drop item starts that no longer address a line (a trailing blank row
/// [`open_block`] popped).
pub(super) fn settle_items(items: &mut Vec<usize>, total: usize) {
    items.retain(|&s| s < total);
}

/// Record the last line of `lines` as a rule line when it ends in a run of
/// rule glyphs after its label: only those lines are extended at a
/// sideways scroll.
pub(super) fn note_rule(rules: &mut Vec<usize>, lines: &[Line<'static>], glyphs: Glyphs) {
    let Some(last) = lines.last() else {
        return;
    };
    let ends_in_rule = last.spans.len() >= 3
        && last.spans.last().is_some_and(|s| {
            !s.content.is_empty() && s.content.chars().all(|c| glyphs.rule.contains(c))
        });
    if ends_in_rule {
        rules.push(lines.len() - 1);
    }
}

/// `line` with its closing rule run lengthened by `extra` columns.
pub(super) fn lengthen_rule(line: &Line<'static>, extra: usize, glyphs: Glyphs) -> Line<'static> {
    let mut longer = line.clone();
    let cell = display_width(glyphs.rule).max(1);
    if let Some(span) = longer.spans.last_mut() {
        span.content = format!("{}{}", span.content, glyphs.rule.repeat(extra / cell)).into();
    }
    longer
}

/// The reading position of the last frame, kept in terms of the logical
/// item at the top of the pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReflowAnchor {
    ident: (Vec<String>, u64, bool, usize),
    width: usize,
    sideways: bool,
    item: usize,
    within: usize,
}

/// The scroll to draw with. When the page was last drawn at another width
/// or wrap setting, the item that was at the top then is put at the top
/// again (its row offset clamped to its new height); otherwise `scroll`
/// stands. Records the position drawn for the next frame.
pub(super) fn settle_scroll(
    app: &App,
    items: &[usize],
    (total, viewport): (usize, usize),
    width: usize,
    follow_target: bool,
    scroll: usize,
) -> usize {
    if items.is_empty() {
        return scroll;
    }
    let ident = (
        app.selected_row()
            .map(|r| r.path.clone())
            .unwrap_or_default(),
        app.root.version(),
        app.showing_verbatim(),
        items.len(),
    );
    let sideways = app.sideways();
    let mut slot = app.reflow_anchor.borrow_mut();
    let mut scroll = scroll;
    if let Some(a) = slot.as_ref().filter(|a| {
        a.ident == ident && (a.width != width || a.sideways != sideways) && !follow_target
    }) {
        let start = items[a.item];
        let end = items.get(a.item + 1).copied().unwrap_or(total);
        let target = start + a.within.min(end.saturating_sub(start).saturating_sub(1));
        scroll = target.min(total.saturating_sub(viewport));
        app.place_detail_scroll(scroll);
    }
    let item = items.partition_point(|&s| s <= scroll).saturating_sub(1);
    *slot = Some(ReflowAnchor {
        ident,
        width,
        sideways,
        item,
        within: scroll.saturating_sub(items[item]),
    });
    scroll
}
