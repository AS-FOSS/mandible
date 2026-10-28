//! Marking the entity rows that match the active search (spec §10): the
//! whole row bold, the matched letters of its spellings underlined, and a
//! bar in the row's left margin.

use super::*;
use crate::app::MatchRow;
use ratatui::style::Modifier;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

/// The search a page is marked for.
pub(super) struct PageMarks<'a> {
    pub(super) keys: &'a [FlagKey],
    pub(super) query: &'a str,
    pub(super) wide: bool,
}

impl<'a> PageMarks<'a> {
    /// The marks for the page of the selected command, if a search is
    /// active and matched any of its entities.
    pub(super) fn of(app: &'a App) -> Option<PageMarks<'a>> {
        let row = app.selected_row()?;
        let hits = app.hit_marks();
        let keys = hits.entities.get(&row.path)?;
        (!hits.query.is_empty()).then_some(PageMarks {
            keys,
            query: &hits.query,
            wide: hits.wide,
        })
    }
}

/// Mark every row of `body` whose entity matched, and return those rows
/// with line numbers on the page, the section starting at line `base`.
pub(super) fn mark_section(
    visible: &[&Entity],
    body: &mut SectionBody,
    marks: &PageMarks<'_>,
    base: usize,
) -> Vec<MatchRow> {
    let wanted: HashSet<&FlagKey> = marks.keys.iter().collect();
    let mut by_key: HashMap<FlagKey, &Entity> = HashMap::new();
    for entity in visible {
        if let Some(key) = entity.key().filter(|k| wanted.contains(k)) {
            by_key.entry(key).or_insert(entity);
        }
    }
    let mut found = Vec::new();
    for (row, key) in body.rows.iter().zip(&body.keys) {
        let Some(key) = key else { continue };
        let Some(entity) = by_key.get(key) else {
            continue;
        };
        let end = (row.first_line + row.lines).min(body.lines.len());
        emphasize(&mut body.lines[row.first_line..end], entity, marks);
        found.push(MatchRow {
            key: key.clone(),
            first_line: base + row.first_line,
            lines: row.lines,
        });
    }
    found
}

/// Bold the row and underline the matched letters of its spellings.
fn emphasize(lines: &mut [Line<'static>], entity: &Entity, marks: &PageMarks<'_>) {
    let mut underline: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); lines.len()];
    let texts: Vec<Vec<char>> = lines
        .iter()
        .map(|l| l.spans.iter().flat_map(|s| s.content.chars()).collect())
        .collect();
    let all = entity
        .spellings
        .iter()
        .chain(entity.range().map(|r| &r.end));
    for spelling in all {
        let Some((dashes, letters)) = matched_letters(spelling, marks) else {
            continue;
        };
        let Some((line, at)) = find_spelling(&texts, &spelling.name) else {
            continue;
        };
        let set = &mut underline[line];
        set.extend(letters.iter().map(|i| at + i));
        let before = &texts[line][..at];
        if dashes > 0 && before.len() >= dashes && before[at - dashes..].iter().all(|&c| c == '-') {
            set.extend(at - dashes..at);
        }
    }
    for (line, set) in lines.iter_mut().zip(&underline) {
        restyle(line, set);
    }
}

/// The dashes and name letters of `spelling` the search matched.
fn matched_letters(spelling: &Spelling, marks: &PageMarks<'_>) -> Option<(usize, Vec<usize>)> {
    if marks.wide {
        let idx = mandible_search::match_indices(&spelling.name, marks.query);
        return (!idx.is_empty()).then(|| (0, idx.into_iter().map(|i| i as usize).collect()));
    }
    crate::search_rank::marked_in_spelling(spelling, marks.query).map(|m| (m.dashes, m.name))
}

/// The first place `name` stands as a whole word in the row's lines:
/// `(line, char offset)`.
fn find_spelling(texts: &[Vec<char>], name: &str) -> Option<(usize, usize)> {
    let name: Vec<char> = name.chars().collect();
    let word = |c: char| c.is_alphanumeric() || c == '_';
    for (line, text) in texts.iter().enumerate() {
        if name.is_empty() || text.len() < name.len() {
            continue;
        }
        for at in 0..=text.len() - name.len() {
            let before = at.checked_sub(1).map(|i| text[i]);
            let after = text.get(at + name.len()).copied();
            if text[at..at + name.len()] == name[..]
                && !before.is_some_and(word)
                && !after.is_some_and(word)
            {
                return Some((line, at));
            }
        }
    }
    None
}

/// Bold every span of `line` and underline the chars at `underline`.
fn restyle(line: &mut Line<'static>, underline: &BTreeSet<usize>) {
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut at = 0usize;
    for span in std::mem::take(&mut line.spans) {
        let base = span.style.add_modifier(Modifier::BOLD);
        let mut run = String::new();
        let mut run_marked = false;
        for c in span.content.chars() {
            let marked = underline.contains(&at);
            if !run.is_empty() && marked != run_marked {
                out.push(styled(std::mem::take(&mut run), base, run_marked));
            }
            run.push(c);
            run_marked = marked;
            at += 1;
        }
        if !run.is_empty() {
            out.push(styled(run, base, run_marked));
        }
    }
    line.spans = out;
}

fn styled(text: String, base: Style, underlined: bool) -> Span<'static> {
    let style = if underlined {
        base.add_modifier(Modifier::UNDERLINED)
    } else {
        base
    };
    Span::styled(text, style)
}

/// Draw the match bar in the left margin of every visible line of a
/// matching row. A margin cell that already holds a marker is left alone.
pub(super) fn draw_match_bars(
    frame: &mut Frame,
    inner: Rect,
    app: &App,
    matches: &Rc<Vec<MatchRow>>,
    vscroll: usize,
) {
    if inner.x == 0 || matches.is_empty() {
        return;
    }
    let x = inner.x - 1;
    let style = style::accent(app.color_enabled);
    let buf = frame.buffer_mut();
    for m in matches.iter() {
        for line in m.first_line..m.first_line + m.lines {
            let Some(row) = line.checked_sub(vscroll) else {
                continue;
            };
            if row >= inner.height as usize {
                continue;
            }
            let Some(cell) = buf.cell_mut((x, inner.y + row as u16)) else {
                continue;
            };
            if cell.symbol() == " " {
                cell.set_symbol(app.glyphs.match_bar).set_style(style);
            }
        }
    }
}
