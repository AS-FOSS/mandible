//! The EXAMPLES section: a node's worked examples, last in the pane, each
//! line as the tool printed it (spec §9.3). No styling, so nothing here
//! guesses which words are the command and which the comment.

use super::*;

/// Push the EXAMPLES heading and rows, or nothing when the node has none.
/// Examples are separated by one blank row. A long line wraps with
/// [`wrap_preformatted_hung`], which keeps interior spacing and hangs
/// continuation rows [`USAGE_HANG`] columns past the line's own indent.
pub(super) fn push_examples(
    lines: &mut Vec<Line<'static>>,
    node: &CommandNode,
    width: usize,
    palette: style::Palette,
    glyphs: Glyphs,
) {
    if node.examples.is_empty() {
        return;
    }
    open_block(lines, SECTION_BLANKS);
    lines.push(heading_line_ruled("EXAMPLES", None, width, palette, glyphs));
    for (n, example) in node.examples.iter().enumerate() {
        if n > 0 {
            lines.push(Line::default());
        }
        for line in example.text.as_str().split('\n') {
            for row in wrap_preformatted_hung(line, width, USAGE_HANG) {
                lines.push(Line::from(row));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::{Example, Provenance, Source, Text};

    fn node_with(examples: &[&str]) -> CommandNode {
        let mut node = CommandNode::new("tool", Provenance::single(Source::HelpText));
        node.examples = examples
            .iter()
            .map(|e| Example::new(Text::sanitize_preserving_layout_lines(e.split('\n'))))
            .collect();
        node
    }

    fn rows(node: &CommandNode, width: usize) -> Vec<String> {
        let mut lines = vec![Line::from("FLAGS")];
        push_examples(
            &mut lines,
            node,
            width,
            style::Palette::extended(),
            crate::glyphs::UNICODE,
        );
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn no_examples_adds_nothing() {
        assert_eq!(rows(&node_with(&[]), 40), vec!["FLAGS"]);
    }

    #[test]
    fn lines_keep_their_indent_and_interior_spacing() {
        let out = rows(&node_with(&["tool -a x  # make\n    explain"]), 40);
        assert!(out[3].starts_with("EXAMPLES"), "{out:?}");
        assert_eq!(out[4], "tool -a x  # make");
        assert_eq!(out[5], "    explain");
    }

    #[test]
    fn examples_are_one_blank_row_apart() {
        let out = rows(&node_with(&["tool a", "tool b"]), 40);
        assert_eq!(&out[4..], ["tool a", "", "tool b"]);
    }

    #[test]
    fn a_long_line_hangs_like_a_usage_form() {
        let out = rows(
            &node_with(&["  tool --one --two --three --four --five"]),
            24,
        );
        assert!(out[4].starts_with("  tool"), "{out:?}");
        assert!(out[5].starts_with(&" ".repeat(2 + USAGE_HANG)), "{out:?}");
        assert!(out[4..].iter().all(|r| r.chars().count() <= 24), "{out:?}");
    }
}
