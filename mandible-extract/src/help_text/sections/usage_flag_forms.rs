//! Three readings of a usage line that keep one flag's text from becoming
//! another flag's value (docs/shapes.md S-218), applied before the usage-flag
//! walk: a shell pipeline names the command after the pipe (`git log
//! --pretty=short | git shortlog`); a flag group nested at the end of a flag
//! group (`[--lock [--reason <string>]]`) is a flag of its own; and a bare
//! flag directly before `<name>` does not own it when the same `<name>` is an
//! unflagged operand in another form (`set-url --add <name> <newurl>`).

use std::borrow::Cow;

/// `line` with the part before a pipeline's last `| <first word>` cut away
/// and every nested flag group hoisted out of its flag group.
pub(super) fn normalize(line: &str) -> Cow<'_, str> {
    let cut = cut_pipeline(line);
    if !cut.contains('[') {
        return cut;
    }
    match hoist_nested(&cut) {
        Some(text) => Cow::Owned(text),
        None => cut,
    }
}

fn cut_pipeline(line: &str) -> Cow<'_, str> {
    let toks: Vec<&str> = line.split_whitespace().collect();
    let Some(first) = toks.iter().find(|t| !t.ends_with(':')) else {
        return Cow::Borrowed(line);
    };
    if !first
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Cow::Borrowed(line);
    }
    let first_at = toks.iter().position(|t| t == first).unwrap_or(0);
    let Some(k) = (first_at + 1..toks.len().saturating_sub(1))
        .rev()
        .find(|&k| toks[k] == "|" && toks[k + 1] == *first && !toks[first_at + 1..k].is_empty())
    else {
        return Cow::Borrowed(line);
    };
    Cow::Owned(toks[k + 1..].join(" "))
}

/// Byte length of the balanced `[...]` group opening `s`.
fn group_end(s: &str) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in s.char_indices() {
        depth += i32::from(c == '[') - i32::from(c == ']');
        if depth == 0 {
            return Some(i + 1);
        }
    }
    None
}

/// `[A -x [-y z] R]` -> `[A -x] [-y z] [R]`, when the nested group opens with
/// a dash and the word before it is a flag. `R` loses a leading `|`.
fn hoist_nested(line: &str) -> Option<String> {
    let mut out = String::with_capacity(line.len() + 8);
    let mut rest = line;
    let mut changed = false;
    while let Some(at) = rest.find('[') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let end = group_end(tail).unwrap_or(tail.len());
        match hoist_group(&tail[..end]) {
            Some(text) => {
                changed = true;
                out.push_str(&text);
            }
            None => out.push_str(&tail[..end]),
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    changed.then_some(out)
}

fn hoist_group(group: &str) -> Option<String> {
    let inner = group.strip_prefix('[')?.strip_suffix(']')?;
    let mut depth = 0i32;
    let mut parts: Vec<(usize, usize)> = Vec::new();
    for (i, c) in inner.char_indices() {
        if c == '[' {
            if depth == 0 {
                parts.push((i, i));
            }
            depth += 1;
        } else if c == ']' {
            depth -= 1;
            if depth == 0 {
                if let Some(p) = parts.last_mut() {
                    p.1 = i + 1;
                }
            }
        }
    }
    let &(from, to) = parts.iter().find(|&&(f, t)| {
        let nested = &inner[f + 1..t.saturating_sub(1).max(f + 1)];
        nested.trim_start().starts_with('-')
            && inner[..f].split_whitespace().next_back().is_some_and(|w| {
                w.starts_with('-')
                    && w.len() > 1
                    && !w.contains(['[', ']'])
                    && !super::entry::is_option_list_placeholder(w.trim_start_matches('-'))
            })
    })?;
    let before = inner[..from].trim_end();
    let after = inner[to..].trim().trim_start_matches('|').trim();
    let mut text = format!("[{before}] {}", &inner[from..to]);
    if !after.is_empty() {
        let tail = hoist_group(&format!("[{after}]")).unwrap_or_else(|| format!("[{after}]"));
        text = format!("{text} {tail}");
    }
    Some(text)
}

/// True when `value` (a `<placeholder>`) is also an operand of another form
/// in `usage_lines`, not directly after a dash-led word. Then the bare flag
/// before it in `line` has no value of its own.
pub(super) fn is_operand_elsewhere(usage_lines: &[String], value: &str) -> bool {
    if !(value.starts_with('<') && value.ends_with('>')) {
        return false;
    }
    usage_lines.iter().any(|l| {
        let toks: Vec<&str> = l.split_whitespace().collect();
        toks.iter().enumerate().any(|(i, t)| {
            t.trim_start_matches('[') == value
                && i > 0
                && !toks[i - 1].starts_with('-')
                && !toks[i - 1].ends_with(':')
        })
    })
}

/// True when `s` opens `<name...>`: a letter right after the `<` and a `>`
/// before any space or bracket, so `--mirror=<fetch|push>` keeps its `|`.
pub(super) fn opens_placeholder(s: &str) -> bool {
    let mut it = s.chars();
    it.next();
    if !it.next().is_some_and(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    for c in it {
        match c {
            '>' => return true,
            ' ' | '[' | ']' | '{' | '}' | '<' => return false,
            _ => {}
        }
    }
    false
}

/// Split a group's content on `|` at that content's own nesting depth 0, so
/// a nested `[...]`/`{...}` (an optional value spec, or a value alternation,
/// on one of the alternatives) is never itself split on. Empty fragments (a
/// stray leading/trailing `|`, or `||`) are dropped.
///
/// Both delimiter pairs count toward the depth. Counting only brackets read
/// `[--color={always|never}]` as the two alternatives `--color={always` and
/// `never}`, emitting a flag whose value was the fragment `{always`. See
/// S-088.
pub(super) fn split_top_level_pipe(content: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut angle = false;
    let mut start = 0usize;
    for (i, c) in content.char_indices() {
        match c {
            '[' | '{' => depth += 1,
            ']' | '}' => depth -= 1,
            '<' if opens_placeholder(&content[i..]) => angle = true,
            '>' => angle = false,
            '|' if depth == 0 && !angle => {
                out.push(content[start..i].trim());
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(content[start..].trim());
    out.retain(|s| !s.is_empty());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pipeline_keeps_the_command_after_the_pipe() {
        assert_eq!(
            normalize("   or: git log --pretty=short | git shortlog [<options>]"),
            "git shortlog [<options>]"
        );
        let alt = "usage: foo --start | --stop";
        assert_eq!(normalize(alt), alt);
    }

    #[test]
    fn a_nested_flag_group_is_hoisted() {
        assert_eq!(
            normalize("w add [--lock [--reason <string>]] [--orphan] <path>"),
            "w add [--lock] [--reason <string>] [--orphan] <path>"
        );
        assert_eq!(
            normalize("w list [-v | --porcelain [-z]]"),
            "w list [-v | --porcelain] [-z]"
        );
        assert_eq!(
            normalize("x [--dry-run | -n] [--all [--single-worktree] | <refs>...]"),
            "x [--dry-run | -n] [--all] [--single-worktree] [<refs>...]"
        );
        for s in [
            "x [-OPTIONS [-MORE_OPTIONS]] [--]",
            "x [--exec-path[=<path>]]",
            "x [[-c|-C] cmd]",
            "x [-q [errorfile]]",
        ] {
            assert_eq!(normalize(s), s);
        }
    }

    #[test]
    fn an_operand_elsewhere_is_not_a_flag_value() {
        let lines: Vec<String> = [
            "usage: git remote set-url [--push] <name> <newurl>",
            "   or: git remote set-url --add <name> <newurl>",
        ]
        .map(String::from)
        .to_vec();
        assert!(is_operand_elsewhere(&lines, "<name>"));
        assert!(!is_operand_elsewhere(&lines, "<other>"));
    }

    #[test]
    fn a_bar_inside_a_placeholder_does_not_split() {
        assert_eq!(
            split_top_level_pipe("--mirror=<fetch|push>"),
            ["--mirror=<fetch|push>"]
        );
        assert_eq!(split_top_level_pipe("-a | <b"), ["-a", "<b"]);
    }
}
