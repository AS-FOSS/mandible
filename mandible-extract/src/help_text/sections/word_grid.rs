//! A multi-column grid of single words under a heading that no flag owns
//! (`automake`'s `Files automatically distributed if found (always):`) is
//! prose about the tool, not the choices of the flag above it. Its text
//! joins the node description, heading first, every word in reading order.
//! See docs/shapes.md S-206 and corpus/automake/1.16.5.

use super::ParsedHelp;

/// The cells of a grid row: single words separated by two or more spaces
/// or a tab. `None` when any cell holds a single-space-separated phrase.
fn row_cells(line: &str) -> Option<Vec<&str>> {
    let mut cells = Vec::new();
    for cell in line.trim().split("  ").flat_map(|c| c.split('\t')) {
        let cell = cell.trim();
        if cell.is_empty() {
            continue;
        }
        if cell.contains(char::is_whitespace) {
            return None;
        }
        cells.push(cell);
    }
    (!cells.is_empty()).then_some(cells)
}

/// The grid's words in reading order: at least two rows, the first holding
/// three or more cells, no later row holding more.
fn grid_words(rows: &[&str]) -> Option<Vec<String>> {
    let rows: Vec<&str> = rows
        .iter()
        .copied()
        .filter(|l| !l.trim().is_empty())
        .collect();
    let cells: Vec<Vec<&str>> = rows.iter().map(|r| row_cells(r)).collect::<Option<_>>()?;
    let first = cells.first()?.len();
    if cells.len() < 2 || first < 3 || cells.iter().any(|c| c.len() > first) {
        return None;
    }
    Some(cells.concat().into_iter().map(str::to_string).collect())
}

/// Appends `heading words...` to the node description when `block` is a
/// word grid. Returns whether it was one.
pub(super) fn take_word_grid(heading: &str, block: &[&str], out: &mut ParsedHelp) -> bool {
    let heading = heading.trim();
    if !heading.ends_with(':') {
        return false;
    }
    let Some(words) = grid_words(block) else {
        return false;
    };
    let text = format!("{heading} {}", words.join(" "));
    out.description = Some(match out.description.take() {
        Some(d) => format!("{d} {text}"),
        None => text,
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grid_of_words_is_read_in_row_order() {
        let block = [
            "  ABOUT-GNU     TODO       install-sh",
            "  ABOUT-NLS     ar-lib     missing",
            "  NEWS[.md]",
        ];
        assert_eq!(
            grid_words(&block).unwrap(),
            [
                "ABOUT-GNU",
                "TODO",
                "install-sh",
                "ABOUT-NLS",
                "ar-lib",
                "missing",
                "NEWS[.md]"
            ]
        );
    }

    #[test]
    fn rows_with_phrases_are_not_a_grid() {
        let block = [
            "  cross       cross compilation issues",
            "  gnu         GNU coding standards",
        ];
        assert!(grid_words(&block).is_none());
    }

    #[test]
    fn two_columns_are_a_table_not_a_grid() {
        assert!(grid_words(&["  a    b", "  c    d"]).is_none());
    }
}
