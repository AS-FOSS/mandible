//! A bracket row that opens a group it does not close, whose next line is
//! indented deeper, opens with `[-flag` and closes the group
//! (`[-addprovider <name>   add security provider` then
//! `  [-providerArg <arg>]] ...  configure argument`), is two rows: the
//! outer flag and the nested flag, each with its own description. The text
//! is rewritten line for line, so line indexes and description columns
//! stay put. See docs/shapes.md S-248 and corpus/jarsigner/21.0.12.

use std::borrow::Cow;

fn depth(s: &str) -> i32 {
    s.chars().fold(0, |d, c| match c {
        '[' => d + 1,
        ']' => d - 1,
        _ => d,
    })
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// `(spec, description column)` of a row: the text before the first run
/// of two spaces, and the byte offset where the description starts.
fn split_row(line: &str) -> Option<(&str, usize)> {
    let body = line.trim_start();
    let gap = body.find("  ")?;
    let start = indent(line) + gap;
    let col = start + line[start..].len() - line[start..].trim_start().len();
    (col < line.len()).then(|| (line[..start].trim_end(), col))
}

/// The two lines rewritten as two rows, when they have the shape.
fn rewrite_pair(outer: &str, nested: &str) -> Option<(String, String)> {
    let (outer_spec, outer_col) = split_row(outer)?;
    // The nested line's description starts in the outer row's column, with
    // no two-space gap after its `...` marker.
    let nested_col = outer_col;
    let nested_spec = nested.get(..nested_col)?.trim_end();
    if !nested.get(..nested_col)?.ends_with(' ')
        || nested.get(nested_col..)?.starts_with(char::is_whitespace)
    {
        return None;
    }
    let (outer_trim, nested_trim) = (outer_spec.trim_start(), nested_spec.trim_start());
    let nested_core = nested_trim.trim_end_matches("...").trim_end();
    if !outer_trim.starts_with("[-")
        || !nested_trim.starts_with("[-")
        || indent(nested) <= indent(outer)
        || depth(outer_trim) != 1
        || depth(nested_core) != -1
    {
        return None;
    }
    let nested_core = nested_core.strip_suffix(']')?;
    let lead = &outer[..indent(outer)];
    let outer_head = format!("{lead}{outer_trim}]");
    let nested_head = format!("{lead}{nested_core}");
    if outer_head.len() + 2 > outer_col || nested_head.len() + 2 > nested_col {
        return None;
    }
    Some((
        format!("{outer_head:<outer_col$}{}", &outer[outer_col..]),
        format!("{nested_head:<nested_col$}{}", &nested[nested_col..]),
    ))
}

/// `raw` with every such pair rewritten; borrowed when none is found.
pub(super) fn normalize(raw: &str) -> Cow<'_, str> {
    let lines: Vec<&str> = raw.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut changed = false;
    let mut i = 0;
    while i < lines.len() {
        if let Some((a, b)) = lines
            .get(i + 1)
            .and_then(|next| rewrite_pair(lines[i], next))
        {
            out.push(a);
            out.push(b);
            changed = true;
            i += 2;
        } else {
            out.push(lines[i].to_string());
            i += 1;
        }
    }
    if !changed {
        return Cow::Borrowed(raw);
    }
    let mut text = out.join("\n");
    if raw.ends_with('\n') {
        text.push('\n');
    }
    Cow::Owned(text)
}
