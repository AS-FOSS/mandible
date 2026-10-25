//! The detail pane's page as plain text, for checking it into the corpus.

use super::*;

/// The whole detail page of a parsed `node` at pane text `width`, as plain
/// text: wrapping on, Unicode glyphs, no styles, no viewport, no window
/// chrome. Built by the same function the TUI draws from. Trailing blanks
/// are trimmed from every line.
///
/// Returns `None` for a node the pane does not draw as a parsed page (raw
/// help, or the repeated-ancestor notice).
pub fn page_text(node: &CommandNode, width: usize) -> Option<Vec<String>> {
    if !node.unparsed.is_empty() || node.same_as_ancestor {
        return None;
    }
    let mut app = App::new(node.name.clone(), node.clone());
    app.glyphs = crate::glyphs::UNICODE;
    app.set_palette(style::Palette::plain());
    let built = build_lines(node, width, app.palette, None, app.glyphs, &app);
    Some(
        built
            .lines
            .iter()
            .map(|line| {
                let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                text.trim_end().to_string()
            })
            .collect(),
    )
}
