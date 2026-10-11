//! A shell builtin's help, `cd: cd [-L|[-P [-e]]] [-@] [dir]`: the name is
//! written twice and the usage line is followed by the indented paragraphs
//! that describe the builtin, up to the `Options:` heading. Those paragraphs
//! are the description. `docs/shapes.md` S-227, `corpus/cd/user`.

use super::*;

/// The paragraphs between a `NAME: NAME ...` usage line and the first
/// section heading, dedented, when the usage line at `at` has that shape.
pub(super) fn description_after_usage(
    lines: &[&str],
    at: Option<usize>,
    tool_name: Option<&str>,
) -> Option<String> {
    let (at, name) = at.zip(tool_name)?;
    let first = lines.get(at)?.trim();
    let rest = first.strip_prefix(name)?.strip_prefix(": ")?;
    if starts_with_usage_prefix(rest) {
        return None;
    }
    let mut paragraphs: Vec<Vec<&str>> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for l in &lines[at + 1..] {
        let t = l.trim();
        if is_section_heading_line(t) {
            break;
        }
        if t.is_empty() {
            if !current.is_empty() {
                paragraphs.push(std::mem::take(&mut current));
            }
        } else {
            current.push(t);
        }
    }
    if !current.is_empty() {
        paragraphs.push(current);
    }
    let text = paragraphs
        .iter()
        .map(|p| p.join("\n"))
        .collect::<Vec<_>>()
        .join("\n\n");
    (!text.is_empty()).then_some(text)
}
