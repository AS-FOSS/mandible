//! A flag row whose spellings overflow the flag column: the extra
//! spellings stack under the first line, beside the wrapped description
//! (spec §9.1a rule 6).

use super::*;

/// How an overflowing row is split: the first line, and the spellings
/// that continue beneath it.
pub(super) struct Stack {
    /// The first line's spellings, without the value placeholder.
    pub(super) main: String,
    /// The continuation lines' text, each already wrapped to the column.
    pub(super) rest: Vec<String>,
}

/// The first line of a stacked row: the first short spelling and the
/// first long one (`--name`, else the first one-dash word). A row
/// lacking one of them keeps the other; a row with neither keeps its
/// first spelling. Display order only, the IR is untouched.
fn main_indices(flag: &Entity) -> Vec<usize> {
    let short = flag.spellings.iter().position(is_short_spelling);
    let long = flag
        .spellings
        .iter()
        .position(|s| matches!(s.dashes, Dashes::Double))
        .or_else(|| {
            flag.spellings
                .iter()
                .position(|s| matches!(s.dashes, Dashes::Single) && !is_short_spelling(s))
        });
    let picked: Vec<usize> = short.into_iter().chain(long).collect();
    if picked.is_empty() {
        vec![0]
    } else {
        picked
    }
}

/// Greedy fill of comma-terminated `items` into lines of at most `budget`
/// columns, breaking only between items. `None` when one item alone is
/// wider than the budget.
fn fill(items: &[String], budget: usize) -> Option<Vec<String>> {
    let mut lines: Vec<String> = Vec::new();
    for item in items {
        let w = display_width(item);
        if w > budget {
            return None;
        }
        match lines.last_mut() {
            Some(last) if display_width(last) + 1 + w <= budget => {
                last.push(' ');
                last.push_str(item);
            }
            _ => lines.push(item.clone()),
        }
    }
    Some(lines)
}

/// The stack for `flag`, or `None` when the row keeps its single-line
/// head: it fits the column, it has no second spelling, or a spelling is
/// wider than the room beside the guide (it then renders as before, its
/// description one space past the head).
pub(super) fn plan(flag: &Entity, layout: SectionLayout, width: usize) -> Option<Stack> {
    if flag.kind != EntityKind::Flag || flag.spellings.len() < 2 {
        return None;
    }
    let column = layout.description;
    if entity_head_width(flag, layout.indent) < column {
        return None;
    }
    let picked = main_indices(flag);
    let main = picked
        .iter()
        .map(|&i| flag.spellings[i].render())
        .collect::<Vec<_>>()
        .join(", ");
    let rest: Vec<String> = flag
        .spellings
        .iter()
        .enumerate()
        .filter(|(i, _)| !picked.contains(i))
        .map(|(_, s)| s.render())
        .collect();
    if rest.is_empty() {
        return None;
    }
    let last = rest.len() - 1;
    let items: Vec<String> = rest
        .into_iter()
        .enumerate()
        .map(|(i, s)| if i == last { s } else { format!("{s},") })
        .collect();
    let budget = column.checked_sub(layout.indent + 3)?;
    let rest = fill(&items, budget)?;
    let head = spelling_column(flag, layout.indent)
        + display_width(&main)
        + display_width(&flag_value_suffix(flag));
    (head <= width).then_some(Stack { main, rest })
}

/// The gap and placeholder that follow a row's spellings, as drawn.
pub(super) fn flag_value_suffix(flag: &Entity) -> String {
    let Some(v) = entity_value_text(flag) else {
        return String::new();
    };
    let gap = if spelling_is_sigil(flag) || value_glues_by_prefix(flag) {
        ""
    } else {
        " "
    };
    format!("{gap}{v}")
}

/// The guide glyphs for a stacked row: every continuation line but the
/// last, then the last.
pub(super) fn guides(glyphs: Glyphs) -> (&'static str, &'static str) {
    (glyphs.guide_mid, glyphs.guide_end)
}

impl Stack {
    /// The lines after the first: each continuation spelling line behind
    /// its guide glyph, zipped with the description's own wrapped lines at
    /// `column`. The row is as tall as the taller of the two.
    pub(super) fn rows(
        &self,
        layout: SectionLayout,
        chunks: &[String],
        guides: (&str, &str),
        styles: (Style, Style),
    ) -> Vec<Line<'static>> {
        let (dim_style, desc_style) = styles;
        let last = self.rest.len().saturating_sub(1);
        let height = self.rest.len().max(chunks.len());
        (0..height)
            .map(|r| {
                let mut spans = Vec::new();
                let mut used = 0;
                if let Some(text) = self.rest.get(r) {
                    let guide = if r == last { guides.1 } else { guides.0 };
                    let left = format!("{}{guide} ", " ".repeat(layout.indent));
                    used = display_width(&left) + display_width(text);
                    spans.push(Span::styled(left, dim_style));
                    spans.push(Span::styled(text.clone(), dim_style));
                }
                if let Some(chunk) = chunks.get(r) {
                    let pad = layout.description.saturating_sub(used);
                    spans.push(Span::raw(" ".repeat(pad)));
                    spans.push(Span::styled(chunk.clone(), desc_style));
                }
                Line::from(spans)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::{Provenance, Source, Text};

    fn codec_flag() -> Entity {
        let mut f = Entity::flag_long("acodec", Provenance::single(Source::HelpText));
        f.spellings = ["codec", "acodec", "vcodec", "scodec", "dcodec"]
            .into_iter()
            .map(Spelling::single_dash)
            .collect();
        f.spellings.insert(0, Spelling::short('c'));
        f.spellings.push(Spelling::single_dash("c:a"));
        f.spellings.push(Spelling::single_dash("c:v"));
        f.value_name = Some("CODEC".to_string());
        f.value_kind = ValueKind::Required;
        f.description = Some(Text::sanitize(
            "force the codec for the matching stream; \"copy\" keeps the original",
        ));
        f
    }

    fn text(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    fn layout() -> SectionLayout {
        SectionLayout {
            description: 25,
            indent: 0,
        }
    }

    #[test]
    fn overflowing_spellings_stack_beside_the_description() {
        let lines = entity_line(
            &codec_flag(),
            false,
            49,
            true,
            layout(),
            crate::glyphs::UNICODE,
        );
        assert_eq!(
            text(&lines),
            vec![
                "-c, -codec CODEC         force the codec for the",
                "│ -acodec, -vcodec,      matching stream; \"copy\"",
                "│ -scodec, -dcodec,      keeps the original",
                "└ -c:a, -c:v",
            ]
        );
    }

    #[test]
    fn continuation_lines_are_dim_and_the_main_line_is_not() {
        let lines = entity_line(
            &codec_flag(),
            false,
            49,
            true,
            layout(),
            crate::glyphs::UNICODE,
        );
        assert_eq!(lines[0].spans[0].style, style::accent(true));
        for line in &lines[1..] {
            assert_eq!(line.spans[0].style, style::muted(true));
            assert_eq!(line.spans[1].style, style::muted(true));
        }
    }

    #[test]
    fn the_ascii_set_and_no_color_keep_the_guide() {
        let lines = entity_line(
            &codec_flag(),
            false,
            49,
            false,
            layout(),
            crate::glyphs::ASCII,
        );
        let t = text(&lines);
        assert!(t[1].starts_with("| -acodec"), "{t:?}");
        assert!(t[3].starts_with("` -c:a"), "{t:?}");
        assert_eq!(lines[1].spans[0].style, Style::default());
    }

    #[test]
    fn a_row_that_fits_the_column_is_not_stacked() {
        let mut f = codec_flag();
        f.spellings.truncate(2);
        let lines = entity_line(&f, false, 60, true, layout(), crate::glyphs::UNICODE);
        assert!(text(&lines)[0].starts_with("-c, -codec CODEC         force the codec"));
        assert!(!text(&lines).iter().any(|l| l.contains('│')));
    }

    #[test]
    fn a_long_only_row_leads_with_the_first_double_dash_spelling() {
        let mut f = codec_flag();
        f.spellings = vec![
            Spelling::single_dash("vcodec"),
            Spelling::long("video-codec"),
            Spelling::single_dash("c:v"),
            Spelling::single_dash("vc"),
        ];
        f.value_name = Some("CODEC_NAME_OF_VIDEO".to_string());
        let lines = entity_line(&f, false, 60, true, layout(), crate::glyphs::UNICODE);
        let t = text(&lines);
        assert!(
            t[0].starts_with("--video-codec CODEC_NAME_OF_VIDEO"),
            "{t:?}"
        );
        assert!(t[1].starts_with("└ -vcodec, -c:v, -vc"), "{t:?}");
    }

    #[test]
    fn a_spelling_wider_than_the_column_falls_back_to_the_single_head() {
        let mut f = codec_flag();
        f.spellings
            .push(Spelling::single_dash("an-extraordinarily-long-spelling"));
        let lines = entity_line(&f, false, 80, true, layout(), crate::glyphs::UNICODE);
        let t = text(&lines);
        assert!(!t.iter().any(|l| l.contains('│')), "{t:?}");
        assert!(t
            .iter()
            .any(|l| l.contains("-an-extraordinarily-long-spelling")));
    }

    #[test]
    fn a_target_on_a_stacked_row_lands_on_its_main_line() {
        let mut body = SectionBody {
            lines: vec![Line::from("above")],
            rows: Vec::new(),
            target: None,
        };
        let f = codec_flag();
        let key = f.key().expect("flag key");
        push_entity(
            &mut body,
            &f,
            false,
            RowCtx {
                width: 60,
                color_enabled: true,
                layout: layout(),
                glyphs: crate::glyphs::UNICODE,
            },
            Some(&key),
        );
        assert_eq!(body.target, Some(1));
        assert_eq!(body.rows[0].lines, 4);
    }
}
