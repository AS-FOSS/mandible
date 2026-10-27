//! The notices on a loaded command page that holds nothing past its summary:
//! the empty one, and the one for a help probe stopped at the time limit.

use super::*;

/// True when `node`, once loaded, has no section to draw: no description,
/// usage, flag, positional, modifier, environment variable, example or
/// subcommand. A node whose fill has not finished, one drawn by another branch of
/// the pane (raw help, repeated ancestor) and the tool's root are not bare.
pub(crate) fn is_bare(node: &CommandNode, is_root: bool, loaded: bool) -> bool {
    !is_root && loaded && !node.same_as_ancestor && is_empty_page(node)
}

/// True when `node` has no section to draw: no description, usage, flag,
/// positional, modifier, environment variable, example or subcommand.
pub(crate) fn is_empty_page(node: &CommandNode) -> bool {
    node.unparsed.is_empty()
        && node.description.is_none()
        && node.usage.is_empty()
        && node.entities.is_empty()
        && node.examples.is_empty()
        && node.subcommands.is_empty()
}

/// Whether the page `app` is drawing for `node` is bare.
pub(super) fn page_is_bare(node: &CommandNode, app: &App) -> bool {
    let Some(row) = app.selected_row() else {
        return false;
    };
    is_bare(node, row.path.len() < 2, is_loaded(node, app))
}

/// Whether the selected node's fill has finished: it came back filled, or a
/// fill completed for it, failed ones included. A node past the warming cap
/// was never probed and is not loaded.
pub(super) fn is_loaded(node: &CommandNode, app: &App) -> bool {
    app.selected_row().is_some_and(|row| {
        !app.is_pending(&row.path) && (node.children_filled || app.fill_finished(&row.path))
    })
}

/// Whether the page `app` is drawing is one whose help probe was
/// stopped at the time limit. A repeated-ancestor page is drawn by its own
/// branch before this one is reached, so it never shows this notice.
pub(super) fn page_timed_out(app: &App) -> bool {
    let Some(row) = app.selected_row() else {
        return false;
    };
    !app.is_pending(&row.path)
        && app.probe_outcome(&row.path) == Some(crate::probes::Outcome::TimedOut)
}

/// Push the notice below the summary, wrapped like the repeated-ancestor one.
pub(super) fn push_notice(page: &mut Page<'_>, notice: &str, text_width: usize) {
    open_block(page.lines, SECTION_BLANKS);
    mark(page.items, page.lines.len());
    for chunk in wrap_words(notice, text_width) {
        page.lines.push(Line::from(chunk));
    }
}
