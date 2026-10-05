//! A line that ends in a backslash and carries a quoted argument, indented
//! deeper than the block's rows, is an example command line inside a
//! description, never a flag row. `docs/shapes.md` S-011,
//! `corpus/rg/14.1.0`.

/// True when `trimmed` (indented `indent`) is such an example line.
pub(super) fn is_example_command_line(
    trimmed: &str,
    indent: usize,
    min_entry_indent: Option<usize>,
) -> bool {
    min_entry_indent.is_some_and(|min| indent > min)
        && trimmed.trim_end().ends_with(" \\")
        && (trimmed.contains('\'') || trimmed.contains('"'))
}
