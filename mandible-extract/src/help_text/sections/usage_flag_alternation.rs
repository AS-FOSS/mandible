//! A flag spelling with a parenthesized alternation glued inside it,
//! `--term-(new|bad)=<term>` (git bisect), spells one flag per alternative
//! (`--term-new=<term>`, `--term-bad=<term>`; docs/shapes.md S-216). Read
//! before the usage-flag walk so the walk sees ordinary spellings. A
//! bracket group made only of such flag tokens becomes one group per
//! spelling, since the walk reads the first flag of a group.

use std::borrow::Cow;

/// `line` with every token `-[-]name(a|b)rest`, where the alternatives are
/// non-empty runs of lowercase letters, digits and `-`, replaced by its
/// expansions (one `[..]` group each when the token sat in a flag-only group).
pub(super) fn expand(line: &str) -> Cow<'_, str> {
    if !line.contains('(') {
        return Cow::Borrowed(line);
    }
    match unparen_flag_alternatives(line) {
        Cow::Owned(text) => Cow::Owned(expand_spellings(&text).into_owned()),
        Cow::Borrowed(text) => expand_spellings(text),
    }
}

/// `(-m | --message) <message>` (git stash store) is the flag `-m|--message`
/// taking `<message>`: written without the parentheses, in a bracket group
/// of its own unless it already sits in one. Only a parenthesized list of
/// dash-led words, optionally followed by one `<placeholder>`, is rewritten.
fn unparen_flag_alternatives(line: &str) -> Cow<'_, str> {
    let mut out = String::new();
    let (mut depth, mut at, mut last) = (0i32, 0usize, 0usize);
    let mut prev = ' ';
    for (i, c) in line.char_indices() {
        if i < at {
            continue;
        }
        depth += i32::from(c == '[') - i32::from(c == ']');
        if c == '(' && (prev.is_whitespace() || prev == '[') {
            if let Some((text, end)) = rewrite_paren(&line[i..], depth > 0) {
                out.push_str(&line[last..i]);
                out.push_str(&text);
                at = i + end;
                last = at;
                prev = ' ';
                continue;
            }
        }
        prev = c;
    }
    if last == 0 {
        return Cow::Borrowed(line);
    }
    out.push_str(&line[last..]);
    Cow::Owned(out)
}

fn rewrite_paren(s: &str, in_group: bool) -> Option<(String, usize)> {
    let close = s.find(')')?;
    let inner = &s[1..close];
    if inner.contains(['(', '[', ']']) {
        return None;
    }
    let alts: Vec<&str> = inner.split('|').map(str::trim).collect();
    let flag = |a: &&str| a.starts_with('-') && a.len() > 1 && !a.contains(char::is_whitespace);
    let operand =
        |a: &&str| a.starts_with('<') && a.ends_with('>') && !a.contains(char::is_whitespace);
    // At least one flag; the rest may be `<placeholder>` operands
    // (`remote set-head <name> (-a | --auto | -d | --delete | <branch>)`).
    if alts.len() < 2 || !alts.iter().any(flag) || !alts.iter().all(|a| flag(a) || operand(a)) {
        return None;
    }
    let mut end = close + 1;
    let mut text = alts.join("|");
    let after = &s[end..];
    let spaces = after.len() - after.trim_start().len();
    if !alts.iter().all(flag) {
        return Some((if in_group { text } else { format!("[{text}]") }, end));
    }
    if let Some(p) = after[spaces..]
        .split_whitespace()
        .next()
        .map(|w| w.trim_end_matches(']'))
    {
        if p.starts_with('<') && p.ends_with('>') && !p.contains(['[', ']']) {
            // One short and one long spelling pair into a single flag; any
            // other list (`(-b | -B) <new-branch>`) gives each its value.
            let shorts = alts.iter().filter(|a| !a.starts_with("--")).count();
            text = if alts.len() == 2 && shorts == 1 {
                format!("{text} {p}")
            } else {
                alts.iter()
                    .map(|a| format!("{a} {p}"))
                    .collect::<Vec<_>>()
                    .join("|")
            };
            end += spaces + p.len();
        }
    }
    Some((if in_group { text } else { format!("[{text}]") }, end))
}

fn expand_spellings(line: &str) -> Cow<'_, str> {
    let mut out = String::with_capacity(line.len());
    let mut changed = false;
    let mut rest = line;
    while let Some(c) = rest.chars().next() {
        if c.is_whitespace() {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        } else if c == '[' {
            let end = group_end(rest).unwrap_or(rest.len());
            let group = &rest[..end];
            match expand_group(group) {
                Some(text) => {
                    changed = true;
                    out.push_str(&text);
                }
                None => out.push_str(group),
            }
            rest = &rest[end..];
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let tok = &rest[..end];
            match expand_token(tok) {
                Some(words) => {
                    changed = true;
                    out.push_str(&words.join(" "));
                }
                None => out.push_str(tok),
            }
            rest = &rest[end..];
        }
    }
    if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(line)
    }
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

fn expand_group(group: &str) -> Option<String> {
    let inner = group.strip_prefix('[')?.strip_suffix(']')?;
    let toks: Vec<&str> = inner.split_whitespace().collect();
    if !toks.iter().all(|t| t.starts_with('-')) {
        return None;
    }
    let mut any = false;
    let mut parts: Vec<String> = Vec::new();
    for t in toks {
        match expand_token(t) {
            Some(words) => {
                any = true;
                parts.extend(words.into_iter().map(|w| format!("[{w}]")));
            }
            None => parts.push(format!("[{t}]")),
        }
    }
    any.then(|| parts.join(" "))
}

/// The value spec of `--name=(a|b)` stays as the author printed it: flags
/// whose value the walk read as `{a|b}` get `(a|b)` back.
pub(super) fn restore_paren_values(usage_lines: &[String], flags: &mut [mandible_core::Entity]) {
    for tok in usage_lines
        .iter()
        .flat_map(|l| l.split(|c: char| c.is_whitespace() || c == '[' || c == ']'))
    {
        let Some((name, val)) = tok.strip_prefix("--").and_then(|t| t.split_once("=(")) else {
            continue;
        };
        let Some(alts) = val.strip_suffix(')') else {
            continue;
        };
        let braced = format!("{{{alts}}}");
        for f in flags.iter_mut().filter(|f| f.long() == Some(name)) {
            if f.value_name.as_deref() == Some(braced.as_str()) {
                f.value_name = Some(format!("({alts})"));
            }
        }
    }
}

fn expand_token(tok: &str) -> Option<Vec<String>> {
    let rest = tok.trim_start_matches('-');
    let dashes = &tok[..tok.len() - rest.len()];
    if dashes.is_empty() || tok.contains(['[', ']']) {
        return None;
    }
    let open = rest.find('(')?;
    let close = open + rest[open..].find(')')?;
    let (name, alts, tail) = (&rest[..open], &rest[open + 1..close], &rest[close + 1..]);
    let alt_list = alts;
    let stem = name.trim_end_matches('=');
    if stem.is_empty() || !stem.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    let alts: Vec<&str> = alts.split('|').collect();
    let word = |a: &str| {
        !a.is_empty()
            && a.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    };
    if alts.len() < 2 || !alts.iter().all(|a| word(a)) || tail.contains('(') {
        return None;
    }
    // `--update-refs=(branches|head)`: one flag whose value is one of the
    // words; the walk reads it as `{a|b}` and `restore_paren_values` puts the
    // printed `(a|b)` back as the value text.
    if name.ends_with('=') {
        return Some(vec![format!("{dashes}{name}{{{alt_list}}}{tail}")]);
    }
    Some(
        alts.iter()
            .map(|a| format!("{dashes}{name}{a}{tail}"))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::expand;

    #[test]
    fn a_parenthesized_flag_list_is_one_flag_with_its_placeholder() {
        assert_eq!(
            expand("x [(-m | --message) <message>] [-q]"),
            "x [-m|--message <message>] [-q]"
        );
        assert_eq!(
            expand("x <name> (-a | --auto | -d | <branch>)"),
            "x <name> [-a|--auto|-d|<branch>]"
        );
        assert_eq!(
            expand("x [(-b | -B) <new-branch>] <path>"),
            "x [-b <new-branch>|-B <new-branch>] <path>"
        );
    }

    #[test]
    fn splits_a_glued_alternation_into_one_spelling_each() {
        assert_eq!(
            expand("start [--term-(new|bad)=<term> --term-(old|good)=<term>] [-x] y"),
            "start [--term-new=<term>] [--term-bad=<term>] [--term-old=<term>] [--term-good=<term>] [-x] y"
        );
        assert_eq!(expand("--a-(b|c)"), "--a-b --a-c");
        assert_eq!(
            expand("x [--dry-run] [--u={b|h}] [--e={d|k|a}]"),
            "x [--dry-run] [--u={b|h}] [--e={d|k|a}]"
        );
    }

    #[test]
    fn leaves_everything_else_alone() {
        for s in [
            "(good|bad) [<rev>...]",
            "--[no-]x",
            "-a (x|y)",
            "--a-(B|c)",
            "--a-()",
            "[--a-(b|c) <v>]",
        ] {
            assert_eq!(expand(s), s);
        }
    }
}
