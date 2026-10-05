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

/// When `lines[i]` is a flush example command that continues onto deeper
/// lines (`argdist -H \`), the index after that command and its indented
/// description; such a line never heads a block.
pub(super) fn example_command_end(lines: &[&str], i: usize) -> Option<usize> {
    let line = lines[i];
    let indent = line.len() - line.trim_start().len();
    let deeper = |l: &str| !l.trim().is_empty() && l.len() - l.trim_start().len() > indent;
    if !line.trim_end().ends_with(" \\") || !lines.get(i + 1).is_some_and(|l| deeper(l)) {
        return None;
    }
    let mut j = i + 1;
    while lines.get(j).is_some_and(|l| deeper(l)) {
        j += 1;
    }
    Some(j)
}
