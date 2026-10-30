//! Marking the entity rows that match the active search (spec §10): the
//! matched letters of its spellings underlined, a bar in the row's left
//! margin, and the current match's whole row on the selection bar.

use super::*;
use crate::app::MatchRow;
use ratatui::style::Modifier;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

/// The search a page is marked for.
pub(super) struct PageMarks<'a> {
    pub(super) keys: &'a [FlagKey],
    pub(super) query: &'a str,
    /// The entity the last jump or `n`/`p` landed on.
    pub(super) current: Option<&'a FlagKey>,
    /// Where that entity stands, when a jump or `n`/`p` named the row.
    pub(super) current_row: Option<crate::sort::RowId>,
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
            current: app.selected_flag.as_ref(),
            current_row: app.selected_flag_row,
        })
    }
}

/// Mark every row of `body` whose entity matched, and return those rows
/// with line numbers on the page, the section starting at line `base`.
pub(super) fn mark_section(
    visible: &[&Entity],
    body: &mut SectionBody,
    marks: &PageMarks<'_>,
    (at, base): (usize, usize),
) -> Vec<MatchRow> {
    let wanted: HashSet<&FlagKey> = marks.keys.iter().collect();
    let mut by_key: HashMap<FlagKey, &Entity> = HashMap::new();
    for entity in visible {
        if let Some(key) = entity.key().filter(|k| wanted.contains(k)) {
            by_key.entry(key).or_insert(entity);
        }
    }
    let mut found = Vec::new();
    for ((row, key), &id) in body.rows.iter().zip(&body.keys).zip(&body.ids) {
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
            id: (at, id),
            current: marks.current == Some(key) && marks.current_row.is_none_or(|r| r == (at, id)),
        });
    }
    found
}

/// Underline every occurrence of every word of the query in the row's
/// spellings, including the `no-` form a `[no-]` spelling shows in
/// brackets.
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
        let Some((line, at)) = find_spelling(&texts, &spelling.name) else {
            continue;
        };
        let lead = spelling.typed().chars().count() - spelling.name.chars().count();
        for (n, form) in mandible_search::spelling_forms(spelling).iter().enumerate() {
            let chars: Vec<char> = form.chars().collect();
            for i in mandible_search::substring_indices(form, marks.query) {
                let i = i as usize;
                let Some(pos) = displayed_at(at, lead, spelling.negatable, n == 1, i) else {
                    continue;
                };
                let same = |a: char, b: char| a.to_lowercase().eq(b.to_lowercase());
                if texts[line].get(pos).is_some_and(|&c| same(c, chars[i])) {
                    underline[line].insert(pos);
                }
            }
        }
    }
    for (line, set) in lines.iter_mut().zip(&underline) {
        restyle(line, set);
    }
}

/// Where char `i` of a spelling's form stands on the row, given that its
/// name starts at char `at`, its dashes are `lead` long, and a negatable
/// spelling is drawn `--[no-]name`. `negated` is the `--no-name` form.
fn displayed_at(at: usize, lead: usize, negatable: bool, negated: bool, i: usize) -> Option<usize> {
    let name_start = if negated { lead + 3 } else { lead };
    if i >= name_start {
        return Some(at + i - name_start);
    }
    if negated && i >= lead {
        return (at + i).checked_sub(lead + 4);
    }
    let bracket = if negatable { 5 } else { 0 };
    (at + i).checked_sub(lead + bracket)
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

/// Underline the chars of `line` at `underline`.
fn restyle(line: &mut Line<'static>, underline: &BTreeSet<usize>) {
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut at = 0usize;
    for span in std::mem::take(&mut line.spans) {
        let base = span.style;
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
    let style = style::accent(app.palette);
    let right = inner.x + inner.width;
    let buf = frame.buffer_mut();
    for m in matches.iter() {
        for line in m.first_line..m.first_line + m.lines {
            let Some(row) = line.checked_sub(vscroll) else {
                continue;
            };
            if row >= inner.height as usize {
                continue;
            }
            let y = inner.y + row as u16;
            if m.current {
                for col in inner.x..right {
                    if let Some(cell) = buf.cell_mut((col, y)) {
                        style::paint_bar(cell, app.palette);
                    }
                }
            }
            let Some(cell) = buf.cell_mut((x, y)) else {
                continue;
            };
            if cell.symbol() == " " {
                cell.set_symbol(app.glyphs.match_bar).set_style(style);
            }
        }
    }
}
