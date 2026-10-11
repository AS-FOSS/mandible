//! An option table whose every row is wrapped in square brackets
//! (`[-keystore <url>]           keystore location`, jarsigner; c++filt;
//! krb5-config): the brackets mark the option optional, the row is still a
//! flag row. The opening `[` and its matching `]` are blanked in place, on
//! the raw text before layout analysis, so every column keeps its offset and
//! the unmodified engine reads an ordinary `-keystore <url>   keystore
//! location` row. Only a row whose bracket is followed by a description gap
//! counts, and only when at least two such rows exist: a usage line
//! (`[-a] [-b]`) has no description after its bracket. See docs/shapes.md
//! S-229.

/// Minimum strong bracketed rows before any is rewritten.
const MIN_ROWS: usize = 2;

/// Byte index of the `]` matching the `[` at byte 0 of `s`, if balanced.
fn matching_close(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// One recognized bracketed row: the byte offset of its opening bracket, of
/// the matching `]` (`None` when the row wraps onto the next line and never
/// closes), and the column its description starts at.
struct Row {
    open: usize,
    close: Option<usize>,
    desc_col: usize,
    kind: Kind,
}

#[derive(PartialEq)]
enum Kind {
    /// A gap of two or more spaces before the description.
    Strong,
    /// A single space, a wrapped row, or a mistyped opener: accepted only
    /// when its description column matches a strong row's.
    Weak,
    /// Nothing after the bracket (`[-s|--format {a,b}]`): accepted only
    /// beside an accepted row.
    Bare,
}

fn starts_description(text: &str) -> bool {
    text.chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '(')
}

/// `line` as a bracketed option row: `[-x ...]` (or `[@file]`, or a `]-x ...]`
/// whose opener is mistyped), then description text.
fn bracketed_row(line: &str) -> Option<Row> {
    let open = line.len() - line.trim_start().len();
    let body = &line[open..];
    let typo = body.starts_with("]-");
    if !(body.starts_with("[-") || body.starts_with("[@") || typo) {
        return None;
    }
    let probe = if typo {
        format!("[{}", &body[1..])
    } else {
        body.to_string()
    };
    match matching_close(&probe) {
        Some(close) => {
            let after = &body[close + 1..];
            let text = after.trim_start();
            let gap = after.len() - text.len();
            if text.is_empty() {
                return (!typo).then_some(Row {
                    open,
                    close: Some(open + close),
                    desc_col: usize::MAX,
                    kind: Kind::Bare,
                });
            }
            (gap >= 1 && starts_description(text)).then(|| Row {
                open,
                close: Some(open + close),
                desc_col: open + close + 1 + gap,
                kind: if gap >= 2 && !typo {
                    Kind::Strong
                } else {
                    Kind::Weak
                },
            })
        }
        None => {
            // An open bracket that wraps (`[-addprovider <name>   text`):
            // the description follows the first run of two or more spaces.
            let gap_at = body.find("  ")?;
            let text = body[gap_at..].trim_start();
            starts_description(text).then_some(Row {
                open,
                close: None,
                desc_col: open + body.len() - text.len(),
                kind: Kind::Weak,
            })
        }
    }
}

/// `raw` with the brackets of each bracketed option row blanked, or `None`
/// when fewer than [`MIN_ROWS`] strong rows are present.
pub(super) fn unwrap_bracketed_rows(raw: &str) -> Option<String> {
    let lines: Vec<&str> = raw.lines().collect();
    let rows: Vec<Option<Row>> = lines.iter().map(|l| bracketed_row(l)).collect();
    let strong_cols: Vec<usize> = rows
        .iter()
        .flatten()
        .filter(|r| r.kind == Kind::Strong)
        .map(|r| r.desc_col)
        .collect();
    if strong_cols.len() < MIN_ROWS {
        return None;
    }
    let mut accepted: Vec<bool> = rows
        .iter()
        .map(|r| {
            r.as_ref().is_some_and(|r| match r.kind {
                Kind::Strong => true,
                Kind::Weak => strong_cols.contains(&r.desc_col),
                Kind::Bare => false,
            })
        })
        .collect();
    for i in 0..rows.len() {
        let beside = (i > 0 && accepted[i - 1]) || accepted.get(i + 1).copied().unwrap_or(false);
        if rows[i].as_ref().is_some_and(|r| r.kind == Kind::Bare) && beside {
            accepted[i] = true;
        }
    }
    let mut out = String::with_capacity(raw.len());
    for ((line, row), keep) in lines.iter().zip(&rows).zip(&accepted) {
        match row.as_ref().filter(|_| *keep) {
            Some(r) => {
                out.push_str(&line[..r.open]);
                out.push(' ');
                match r.close {
                    Some(close) => {
                        out.push_str(&line[r.open + 1..close]);
                        out.push(' ');
                        out.push_str(&line[close + 1..]);
                    }
                    None => out.push_str(&line[r.open + 1..]),
                }
            }
            None => out.push_str(line),
        }
        out.push('\n');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_keep_their_columns_with_brackets_blanked() {
        let raw = "[-keystore <url>]   keystore location\n[-verbose[:sub]]   verbose\n";
        let out = unwrap_bracketed_rows(raw).unwrap();
        assert_eq!(
            out,
            " -keystore <url>    keystore location\n -verbose[:sub]    verbose\n"
        );
    }

    #[test]
    fn usage_lines_and_a_lone_row_are_left_alone() {
        assert!(unwrap_bracketed_rows("Usage: x [-a] [-b]\n  [-a] [-b]\n").is_none());
        assert!(unwrap_bracketed_rows("[-a]   only one row\n").is_none());
    }
}
