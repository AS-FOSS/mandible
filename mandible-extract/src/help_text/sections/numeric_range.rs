//! A row spelling a run of single-digit flags as `-N .. -M`.

use super::*;

/// Replace the one flag a `-1 .. -9` row parses as (short `1`, value `..`)
/// with one flag per digit in the range, each carrying the row's
/// description and no value. The row must spell both endpoints
/// (`-N .. -M`, `N < M`); a digit another flag already owns is skipped.
/// See docs/shapes.md S-181 and corpus/savelog/5.17.
pub(super) fn expand_numeric_range_flags(flags: &mut Vec<Entity>, lines: &[&str]) {
    let mut out: Vec<Entity> = Vec::with_capacity(flags.len());
    for flag in flags.iter() {
        let range = flag
            .short()
            .filter(|c| c.is_ascii_digit() && flag.long().is_none())
            .filter(|_| matches!(flag.value_name.as_deref(), Some("..") | Some("...")))
            .and_then(|first| row_range_end(lines, first).map(|last| (first, last)));
        let Some((first, last)) = range else {
            out.push(flag.clone());
            continue;
        };
        for digit in first..=last {
            let taken = flags
                .iter()
                .any(|f| f.short() == Some(digit) && !std::ptr::eq(f, flag));
            if taken {
                continue;
            }
            let mut member = flag.clone();
            member.spellings = vec![Spelling::short(digit)];
            member.value_name = None;
            member.value_kind = ValueKind::None;
            out.push(member);
        }
    }
    *flags = out;
}

/// The upper endpoint of the row `-<first> .. -<last>`, when the document
/// has exactly that row and `last` is a larger digit.
fn row_range_end(lines: &[&str], first: char) -> Option<char> {
    let prefix = format!("-{first}");
    lines.iter().find_map(|line| {
        let rest = line.trim_start().strip_prefix(prefix.as_str())?;
        let rest = rest.trim_start().strip_prefix("..")?;
        let rest = rest
            .trim_start_matches('.')
            .trim_start()
            .strip_prefix('-')?;
        let mut chars = rest.chars();
        let last = chars.next().filter(|c| c.is_ascii_digit() && *c > first)?;
        chars.next().is_none_or(char::is_whitespace).then_some(last)
    })
}

/// A positional with no description takes it from the one table row that
/// opens with the positional's own name as its first token, then either
/// `- ` (`savelog`'s `file - log file names`) or a column gap of two or
/// more spaces on an indented row (`lsof`'s `names  select named files`).
/// Skipped when two rows match, so it never picks between them. See
/// docs/shapes.md S-182, S-153 and corpus/savelog/5.17.
pub(super) fn describe_positionals_from_name_rows(positionals: &mut [Entity], lines: &[&str]) {
    for positional in positionals.iter_mut() {
        if positional.description.is_some() {
            continue;
        }
        let name = positional.primary_name().to_string();
        let mut rows = lines.iter().enumerate().filter_map(|(at, line)| {
            let rest = line.trim().strip_prefix(name.as_str())?;
            if !rest.starts_with(char::is_whitespace) {
                return None;
            }
            if let Some(d) = rest.trim_start().strip_prefix("- ") {
                let d = d.trim();
                return (!d.is_empty()).then(|| d.to_string());
            }
            if !(rest.starts_with("  ") && line.starts_with(char::is_whitespace)) {
                return None;
            }
            // Lines indented to the description column continue it.
            let col = line.len() - rest.trim_start().len();
            let mut desc = rest.trim().to_string();
            for next in &lines[at + 1..] {
                let indent = next.len() - next.trim_start().len();
                if next.trim().is_empty() || indent < col {
                    break;
                }
                desc.push(' ');
                desc.push_str(next.trim());
            }
            (!desc.is_empty()).then_some(desc)
        });
        let (Some(desc), None) = (rows.next(), rows.next()) else {
            continue;
        };
        positional.description = non_empty_text(&desc);
    }
}

/// A `-- word word` flag row whose value restates the operands of a usage
/// `[[--] word word...]` group is the separator row: the row's text
/// describes the first operand, and the `--` is not a flag. See
/// docs/shapes.md S-096 and corpus/lldb-server/18.1.3-dashdash.
pub(super) fn fold_separator_row_into_operand(
    flags: &mut Vec<Entity>,
    positionals: &mut [Entity],
    usage: &[String],
) {
    let Some(at) = flags.iter().position(|f| {
        f.spellings.len() == 1 && f.spellings[0].name == "--" && f.value_name.is_some()
    }) else {
        return;
    };
    let value = flags[at].value_name.clone().unwrap_or_default();
    let words: Vec<&str> = value.split_whitespace().collect();
    let Some(first) = words.first() else {
        return;
    };
    let marked = usage.iter().any(|u| u.contains(&format!("[[--] {first}")));
    let Some(pos) = positionals.iter().position(|p| p.primary_name() == *first) else {
        return;
    };
    let names_match = words.iter().enumerate().all(|(i, w)| {
        positionals
            .get(pos + i)
            .is_some_and(|p| p.primary_name() == w.trim_end_matches('.'))
    });
    if !marked || !names_match || positionals[pos].description.is_some() {
        return;
    }
    let flag = flags.remove(at);
    positionals[pos].description = flag.description;
}
