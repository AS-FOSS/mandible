//! The negation notation on a flag row, `[no-]` and `[!]`, drawn muted:
//! notation is grey, names keep their colour (docs/design.md §9.2).

use ratatui::style::Style;
use ratatui::text::{Line, Span};

const MARKERS: [&str; 2] = ["[no-]", "[!]"];

/// Re-style every marker inside a span drawn in `name` as `muted`.
pub(super) fn mute(lines: &mut [Line<'static>], name: Style, muted: Style) {
    for line in lines {
        let spans = std::mem::take(&mut line.spans);
        for span in spans {
            if span.style == name {
                split(&span.content, name, muted, &mut line.spans);
            } else {
                line.spans.push(span);
            }
        }
    }
}

fn split(text: &str, name: Style, muted: Style, out: &mut Vec<Span<'static>>) {
    let mut rest = text;
    while let Some((at, marker)) = MARKERS
        .iter()
        .filter_map(|m| rest.find(m).map(|at| (at, *m)))
        .min_by_key(|(at, _)| *at)
    {
        if at > 0 {
            out.push(Span::styled(rest[..at].to_string(), name));
        }
        out.push(Span::styled(marker.to_string(), muted));
        rest = &rest[at + marker.len()..];
    }
    if !rest.is_empty() {
        out.push(Span::styled(rest.to_string(), name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn markers_are_muted_and_names_keep_their_style() {
        let name = Style::default().fg(Color::Green);
        let muted = Style::default().fg(Color::Gray);
        let mut lines = vec![Line::from(Span::styled("[!] -q, --[no-]quiet", name))];
        mute(&mut lines, name, muted);
        let got: Vec<(&str, Style)> = lines[0]
            .spans
            .iter()
            .map(|s| (s.content.as_ref(), s.style))
            .collect();
        assert_eq!(
            got,
            [
                ("[!]", muted),
                (" -q, --", name),
                ("[no-]", muted),
                ("quiet", name)
            ]
        );
    }

    use super::super::{entity_line, SectionLayout};
    use crate::style;
    use mandible_core::{Entity, Negation, Provenance, ValueKind};

    fn row(bang: bool, no_prefix: bool) -> Vec<Span<'static>> {
        let mut e = Entity::flag_spelled(
            Some('p'),
            Some("protocol".into()),
            false,
            no_prefix,
            Provenance::default(),
        );
        if bang {
            for s in &mut e.spellings {
                s.negation = Some(Negation::Bang);
            }
        }
        e.value_name = Some("proto".into());
        e.value_kind = ValueKind::Required;
        let layout = SectionLayout {
            description: 30,
            indent: 0,
            man: false,
            wrap: true,
        };
        let palette = style::Palette::extended();
        entity_line(&e, false, 60, palette, layout).remove(0).spans
    }

    #[test]
    fn a_bang_row_draws_the_marker_muted_before_the_accent_spellings() {
        let palette = style::Palette::extended();
        let spans = row(true, false);
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            text.trim_start().starts_with("[!] -p, --protocol"),
            "{text:?}"
        );
        let marker = spans.iter().find(|s| s.content == "[!]").unwrap();
        assert_eq!(marker.style, style::muted(palette));
        let name = spans
            .iter()
            .find(|s| s.content.contains("--protocol"))
            .unwrap();
        assert_eq!(name.style, style::accent(palette));
    }

    #[test]
    fn a_no_prefix_flag_draws_its_bracket_muted() {
        let palette = style::Palette::extended();
        let spans = row(false, true);
        let marker = spans.iter().find(|s| s.content == "[no-]").unwrap();
        assert_eq!(marker.style, style::muted(palette));
    }
}
