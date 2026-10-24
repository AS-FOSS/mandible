//! A flag row whose spellings overflow the flag column: the extra
//! spellings stack under the first line, each line led by `or`, beside
//! the wrapped description (spec §9.1a rule 6).

use super::*;

/// How an overflowing row is split: the first line, and the spellings
/// that continue beneath it.
pub(super) struct Stack {
    /// The first line's spellings, without the value placeholder.
    pub(super) main: String,
    /// The continuation lines' text, each already wrapped to the column.
    pub(super) rest: Vec<String>,
    /// Where each continuation line's `or` starts: two columns past the
    /// row's first spelling.
    pub(super) start: usize,
}

/// The word that leads every continuation line: the names after it are
/// other spellings of the same flag. The same in every glyph set.
const OR: &str = "or ";

/// The first line of a stacked row: the first short spelling and the
/// first long one (`--name`, else the first one-dash word). A row
/// lacking one of them keeps the other; a row with neither keeps its
/// first spelling. Display order only, the IR is untouched.
fn main_indices(flag: &Entity) -> Vec<usize> {
    let short = flag.spellings.iter().position(|s| is_drawn_short(flag, s));
    let long = flag
        .spellings
        .iter()
        .position(|s| matches!(s.dashes, Dashes::Double))
        .or_else(|| {
            flag.spellings.iter().position(|s| {
                matches!(s.dashes, Dashes::Single)
                    && !is_short_spelling(s)
                    && !is_range_start(flag, s)
            })
        });
    // The range always stays on the main line, after the other longs.
    let range = flag.spellings.iter().position(|s| is_range_start(flag, s));
    let picked: Vec<usize> = short.into_iter().chain(long).chain(range).collect();
    if picked.is_empty() {
        vec![0]
    } else {
        picked
    }
}

/// Greedy fill of `items` into comma-separated lines of at most `budget`
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
            Some(last) if display_width(last) + 2 + w <= budget => {
                last.push_str(", ");
                last.push_str(item);
            }
            _ => lines.push(item.clone()),
        }
    }
    Some(lines)
}

/// The stack for `flag`, or `None` when the row keeps its single-line
/// head: it fits the column, it has no second spelling, or a spelling is
/// wider than the room after `or` (it then renders as before, its
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
        .map(|&i| spelling_text(flag, i))
        .collect::<Vec<_>>()
        .join(", ");
    let rest: Vec<String> = (0..flag.spellings.len())
        .filter(|i| !picked.contains(i))
        .map(|i| spelling_text(flag, i))
        .collect();
    if rest.is_empty() {
        return None;
    }
    let start = spelling_column(flag, layout.indent) + 2;
    let budget = column.checked_sub(start + OR.len() + 1)?;
    let rest = fill(&rest, budget)?;
    let head = spelling_column(flag, layout.indent)
        + display_width(&main)
        + display_width(&flag_value_suffix(flag));
    (head <= width).then_some(Stack { main, rest, start })
}

/// The gap and placeholder that follow a row's spellings, as drawn.
pub(super) fn flag_value_suffix(flag: &Entity) -> String {
    let Some(v) = entity_value_text(flag) else {
        return String::new();
    };
    format!("{}{v}", value_gap(flag))
}

impl Stack {
    /// The lines after the first: each continuation spelling line behind
    /// a dim `or`, zipped with the description's own wrapped lines at
    /// `column`. The row is as tall as the taller of the two. `styles` are
    /// the `or`, the spellings and the description.
    pub(super) fn rows(
        &self,
        layout: SectionLayout,
        chunks: &[String],
        styles: (Style, Style, Style),
    ) -> Vec<Line<'static>> {
        let (or_style, name_style, desc_style) = styles;
        let height = self.rest.len().max(chunks.len());
        (0..height)
            .map(|r| {
                let mut spans = Vec::new();
                let mut used = 0;
                if let Some(text) = self.rest.get(r) {
                    used = self.start + OR.len() + display_width(text);
                    spans.push(Span::raw(" ".repeat(self.start)));
                    spans.push(Span::styled(OR, or_style));
                    spans.push(Span::styled(text.clone(), name_style));
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
            man: false,
            wrap: true,
        }
    }

    #[test]
    fn overflowing_spellings_stack_beside_the_description() {
        let lines = entity_line(&codec_flag(), false, 49, true, layout());
        assert_eq!(
            text(&lines),
            vec![
                "-c, -codec CODEC         force the codec for the",
                "  or -acodec, -vcodec    matching stream; \"copy\"",
                "  or -scodec, -dcodec    keeps the original",
                "  or -c:a, -c:v",
            ]
        );
    }

    #[test]
    fn only_the_or_is_dim_and_the_names_keep_their_style() {
        let lines = entity_line(&codec_flag(), false, 49, true, layout());
        assert_eq!(lines[0].spans[0].style, style::accent(true));
        for line in &lines[1..] {
            assert_eq!(line.spans[1].content, "or ");
            assert_eq!(line.spans[1].style, style::muted(true));
            assert_eq!(line.spans[2].style, style::accent(true));
        }
    }

    #[test]
    fn no_color_keeps_the_or() {
        let lines = entity_line(&codec_flag(), false, 49, false, layout());
        let t = text(&lines);
        assert!(t[1].starts_with("  or -acodec"), "{t:?}");
        assert!(t[3].starts_with("  or -c:a"), "{t:?}");
        assert_eq!(lines[1].spans[1].style, Style::default());
    }

    #[test]
    fn a_row_without_a_short_puts_its_or_under_its_first_name() {
        let mut f = Entity::flag_long("old-dirs", Provenance::single(Source::HelpText));
        f.spellings.push(Spelling::long("old-d"));
        f.description = Some(Text::sanitize(
            "works like --dirs when talking to old rsync",
        ));
        let mut l = layout();
        l.description = 18;
        let t = text(&entity_line(&f, false, 50, true, l));
        assert!(t[0].starts_with("    --old-dirs    works"), "{t:?}");
        assert!(t[1].starts_with("      or --old-d  "), "{t:?}");
    }

    #[test]
    fn a_range_stays_on_the_main_line_of_a_stacked_row() {
        let mut f = Entity::flag_long("fast", Provenance::single(Source::HelpText));
        f.spellings = vec![
            Spelling::short('0'),
            Spelling::long("fast"),
            Spelling::long("best"),
            Spelling::long("quickest"),
            Spelling::long("fastest"),
        ];
        f.set_range(mandible_core::SpellingRange {
            start: Spelling::short('0'),
            end: Spelling::short('9'),
        })
        .expect("the start is one of the spellings");
        f.description = Some(Text::sanitize("compression level"));
        let t = text(&entity_line(&f, false, 49, true, layout()));
        assert!(t[0].starts_with("--fast, -0 ... -9"), "{t:?}");
        assert!(t[1].starts_with("  or --best, --quickest"), "{t:?}");
    }

    #[test]
    fn a_row_that_fits_the_column_is_not_stacked() {
        let mut f = codec_flag();
        f.spellings.truncate(2);
        let lines = entity_line(&f, false, 60, true, layout());
        assert!(text(&lines)[0].starts_with("-c, -codec CODEC         force the codec"));
        assert!(!text(&lines).iter().any(|l| l.contains(" or ")));
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
        let lines = entity_line(&f, false, 60, true, layout());
        let t = text(&lines);
        assert!(
            t[0].starts_with("--video-codec CODEC_NAME_OF_VIDEO"),
            "{t:?}"
        );
        assert!(t[1].starts_with("  or -vcodec, -c:v, -vc"), "{t:?}");
    }

    #[test]
    fn a_spelling_wider_than_the_column_falls_back_to_the_single_head() {
        let mut f = codec_flag();
        f.spellings
            .push(Spelling::single_dash("an-extraordinarily-long-spelling"));
        let lines = entity_line(&f, false, 80, true, layout());
        let t = text(&lines);
        assert!(!t.iter().any(|l| l.contains(" or -")), "{t:?}");
        assert!(t
            .iter()
            .any(|l| l.contains("-an-extraordinarily-long-spelling")));
    }

    #[test]
    fn a_target_on_a_stacked_row_lands_on_its_main_line() {
        let mut body = SectionBody {
            lines: vec![Line::from("above")],
            ..SectionBody::default()
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
            },
            Some(&key),
        );
        assert_eq!(body.target, Some(1));
        assert_eq!(body.rows[0].lines, 4);
    }
}
