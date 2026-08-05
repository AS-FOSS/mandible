//! The detail pane: breadcrumb header, description, flags grouped by
//! [`mantui_core::Flag::group`] (inherited flags in a final dimmed group),
//! and a provenance footer (spec §2, §9).

use super::ACCENT;
use crate::app::{App, Focus};
use crate::sanitize::defensive_single_line;
use mantui_core::{CommandNode, Flag};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui::Frame;
use std::collections::BTreeMap;

/// Render the detail pane into `area`.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Detail;
    let border_style = if focused {
        Style::default().fg(ACCENT)
    } else {
        Style::default()
    };

    let breadcrumb = app
        .selected_row()
        .map(|r| {
            r.path
                .iter()
                .map(|s| defensive_single_line(s))
                .collect::<Vec<_>>()
                .join(" › ")
        })
        .unwrap_or_default();
    let title = format!(" {breadcrumb} ");

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let Some(node) = app.selected_node() else {
        let paragraph = Paragraph::new("Nothing selected.");
        frame.render_widget(paragraph, inner);
        return;
    };

    let lines = build_lines(node, app.show_hidden);
    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((app.detail_scroll as u16, 0));
    frame.render_widget(paragraph, inner);
}

fn build_lines(node: &CommandNode, show_hidden: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    if let Some(summary) = &node.summary {
        lines.push(Line::from(Span::styled(
            summary.as_str().to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::default());
    }

    if let Some(description) = &node.description {
        lines.push(Line::from(Span::styled(
            "DESCRIPTION",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for paragraph_text in description.as_str().split("\n\n") {
            for line in paragraph_text.lines() {
                lines.push(Line::from(line.to_string()));
            }
            lines.push(Line::default());
        }
    }

    if !node.usage.is_empty() {
        lines.push(Line::from(Span::styled(
            "USAGE",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for u in &node.usage {
            lines.push(Line::from(format!(
                "{} {}",
                defensive_single_line(&node.name),
                u.as_str()
            )));
        }
        lines.push(Line::default());
    }

    let visible_flags: Vec<&Flag> = node
        .flags
        .iter()
        .filter(|f| show_hidden || (!f.hidden && f.deprecated.is_none()))
        .collect();

    if !visible_flags.is_empty() {
        lines.push(Line::from(Span::styled(
            "FLAGS",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.extend(flag_lines(&visible_flags));
        lines.push(Line::default());
    }

    lines.push(Line::from(Span::styled(
        provenance_footer(node),
        Style::default().add_modifier(Modifier::DIM),
    )));

    lines
}

/// Group flags by [`Flag::group`], with un-grouped flags first (under no
/// heading) and inherited flags always last as their own dimmed group,
/// regardless of their source `group` value (spec §9).
fn flag_lines(flags: &[&Flag]) -> Vec<Line<'static>> {
    let mut own_groups: BTreeMap<Option<String>, Vec<&Flag>> = BTreeMap::new();
    let mut inherited: Vec<&Flag> = Vec::new();

    for f in flags {
        if f.inherited {
            inherited.push(f);
        } else {
            own_groups.entry(f.group.clone()).or_default().push(f);
        }
    }

    let mut out = Vec::new();

    // Ungrouped flags first, with no heading.
    if let Some(ungrouped) = own_groups.remove(&None) {
        for f in ungrouped {
            out.push(flag_line(f, false));
        }
    }
    for (group, flags) in own_groups {
        if let Some(group) = group {
            out.push(Line::from(Span::styled(
                defensive_single_line(&group).to_uppercase(),
                Style::default().add_modifier(Modifier::DIM | Modifier::BOLD),
            )));
        }
        for f in flags {
            out.push(flag_line(f, false));
        }
    }

    if !inherited.is_empty() {
        out.push(Line::from(Span::styled(
            "INHERITED",
            Style::default().add_modifier(Modifier::DIM | Modifier::BOLD),
        )));
        for f in inherited {
            out.push(flag_line(f, true));
        }
    }

    out
}

fn flag_line(flag: &Flag, dim: bool) -> Line<'static> {
    let spelling_style = if dim {
        Style::default().add_modifier(Modifier::DIM)
    } else {
        Style::default().fg(ACCENT)
    };
    let mut spans = vec![Span::styled(
        format!("  {}", flag.spelling()),
        spelling_style,
    )];
    if let Some(desc) = &flag.description {
        spans.push(Span::raw("  "));
        let style = if dim {
            Style::default().add_modifier(Modifier::DIM)
        } else {
            Style::default()
        };
        spans.push(Span::styled(desc.single_line(), style));
    }
    Line::from(spans)
}

/// The provenance footer (spec §2, §4.2): which sources contributed, and
/// whether structure and prose each came from a trusted source.
fn provenance_footer(node: &CommandNode) -> String {
    if node.provenance.sources.is_empty() {
        return "no source".to_string();
    }
    let labels: Vec<String> = node.provenance.sources.iter().map(|s| s.label()).collect();
    let structural = node
        .provenance
        .effective_authority(mantui_core::Axis::Structural)
        > 0;
    let prose = node
        .provenance
        .effective_authority(mantui_core::Axis::Prose)
        > 0;
    format!(
        "{} · structure {} · prose {}",
        labels.join(" + "),
        if structural { "✓" } else { "✗" },
        if prose { "✓" } else { "✗" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mantui_core::{Provenance, Source, Text};

    fn node_with_flags() -> CommandNode {
        let mut n = CommandNode::new(
            "rebase",
            Provenance::single(Source::KnownSpec {
                provider: "carapace".to_string(),
            }),
        );
        n.summary = Some(Text::sanitize("Reapply commits on top of another base tip"));
        let mut f1 = Flag::long(
            "interactive",
            Provenance::single(Source::KnownSpec {
                provider: "carapace".to_string(),
            }),
        );
        f1.short = Some('i');
        f1.description = Some(Text::sanitize("Make a list of commits"));
        let mut f2 = Flag::long(
            "help",
            Provenance::single(Source::KnownSpec {
                provider: "carapace".to_string(),
            }),
        );
        f2.inherited = true;
        f2.description = Some(Text::sanitize("Show help"));
        n.flags = vec![f1, f2];
        n
    }

    #[test]
    fn inherited_flags_are_grouped_last() {
        let node = node_with_flags();
        let flags: Vec<&Flag> = node.flags.iter().collect();
        let lines = flag_lines(&flags);
        let text: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect())
            .collect();
        let inherited_pos = text.iter().position(|l| l.contains("INHERITED")).unwrap();
        let help_pos = text.iter().position(|l| l.contains("--help")).unwrap();
        assert!(help_pos > inherited_pos);
    }

    #[test]
    fn hidden_flags_suppressed_by_default() {
        let mut node = node_with_flags();
        node.flags[0].hidden = true;
        let lines = build_lines(&node, false);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.to_string())
            .collect();
        assert!(!joined.contains("--interactive"));
    }

    #[test]
    fn hidden_flags_shown_when_toggled() {
        let mut node = node_with_flags();
        node.flags[0].hidden = true;
        let lines = build_lines(&node, true);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.to_string())
            .collect();
        assert!(joined.contains("--interactive"));
    }

    #[test]
    fn provenance_footer_reflects_axes() {
        let node = node_with_flags();
        let footer = provenance_footer(&node);
        assert!(footer.contains("carapace"));
        assert!(footer.contains("structure ✓"));
        assert!(footer.contains("prose"));
    }
}
