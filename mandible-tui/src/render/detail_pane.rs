//! The detail pane: breadcrumb header, description, flags grouped by
//! [`mandible_core::Flag::group`] (inherited flags in a final dimmed group),
//! and a provenance footer (spec §2, §9, §9.2).
//!
//! **Every line handed to the `Paragraph` is already wrapped to the
//! pane's exact width before it gets there** — both the description
//! prose and each flag's spelling/description — rather than leaning on
//! `ratatui`'s own `Wrap` to do it. Two reasons:
//!
//! 1. A flag's description continuation must hang-indent under the
//!    description column, not restart at column 0 (spec-adjacent
//!    feedback: `ratatui::widgets::Wrap` re-wraps a `Line` with no memory
//!    of where useful content started, so a flag line handed to it as one
//!    long `Span` run comes back flush-left on continuation — the single
//!    biggest readability problem the pane had).
//! 2. Search selecting a flag needs to scroll the pane to *that exact
//!    on-screen row* (spec §10's "closes the loop" requirement). That's
//!    only possible if the `Line` index we compute during layout is the
//!    same index the `Paragraph` actually renders at — which requires
//!    controlling 100% of the wrapping ourselves, not delegating part of
//!    it to a widget whose reflow decisions happen after this function
//!    returns.
//!
//! `Wrap` stays enabled on the `Paragraph` purely as a defensive
//! fallback (spec §9's border-corruption lesson: untrusted text reaching
//! a `Span` unclipped is how that happened before) — every line we
//! construct should already fit, so it should never need to act.

use crate::app::{App, Focus};
use crate::sanitize::{defensive_single_line, display_width, truncate_to_width_ellipsis};
use crate::style;
use mandible_core::{CommandNode, Flag, FlagKey, ValueKind};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui::Frame;
use std::collections::BTreeMap;

/// Render the detail pane into `area`.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Detail;
    let border_style = if focused {
        style::accent(app.color_enabled)
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

    // Level 3 of spec §7 Tier B's staged degradation (batch 6 part 4): no
    // parse produced anything structurally plausible for this node, so it
    // carries the tool's own raw `--help` text instead of invented
    // structure. This is a fundamentally different rendering, not a
    // variant of the structured one below — see `render_unparsed`.
    if !node.unparsed.is_empty() {
        render_unparsed(frame, inner, app, node);
        return;
    }

    let width = inner.width as usize;
    let built = build_lines(
        node,
        app.show_hidden,
        width,
        app.color_enabled,
        app.selected_flag.as_ref(),
    );
    // Search selecting a flag scrolls straight to it (spec §10): the line
    // index is exact because every line above was pre-wrapped by us, not
    // by the widget's own `Wrap` after the fact. Falls back to the user's
    // own scroll position once nothing is flag-targeted.
    // Tell `App` how far this content can scroll, so `↓` stops at the end
    // instead of pushing it off the top into blank space.
    app.set_detail_extent(built.lines.len(), inner.height as usize);
    let scroll = built
        .target_flag_line
        .unwrap_or_else(|| app.clamped_detail_scroll()) as u16;
    let paragraph = Paragraph::new(built.lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(paragraph, inner);
}

/// Render a node whose parse degraded to level 3 (spec §7 Tier B step 3,
/// batch 6 part 4): `node.unparsed`, one preformatted line per entry,
/// labelled so it reads as "the author's own text", not a mandible parse.
///
/// Deliberately **not** run through [`wrap_words`] and **not** given
/// `Paragraph::wrap` the way every other block in this pane is (see this
/// module's top doc comment on why the rest of the pane pre-wraps
/// everything itself) — this is preformatted output, and re-wrapping it
/// would silently edit the tool author's own text. Without `Wrap`,
/// `ratatui::widgets::Paragraph` clips an over-width line at the pane's
/// edge rather than reflowing it, which is the "horizontal scroll rather
/// than reflow" spec §7 Tier B step 3 calls for; a horizontal scroll
/// *offset* is not yet wired to a key in this batch (it always starts at
/// column 0), but the important safety property — content never reflows,
/// and can therefore never smear into the pane border the way an
/// unsanitized newline once did (spec §9) — holds regardless. Safe to hand
/// straight to a `Span` because `Text::sanitize` already guarantees no
/// embedded control characters or newlines reach here.
fn render_unparsed(frame: &mut Frame, inner: Rect, app: &App, node: &CommandNode) {
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(node.unparsed.len() + 2);
    lines.push(Line::from(Span::styled(
        "unparsed — showing raw --help output",
        style::muted_bold(app.color_enabled),
    )));
    lines.push(Line::default());
    for text in &node.unparsed {
        lines.push(Line::from(text.as_str().to_string()));
    }
    app.set_detail_extent(lines.len(), inner.height as usize);
    let scroll = app.clamped_detail_scroll() as u16;
    let paragraph = Paragraph::new(lines).scroll((scroll, 0));
    frame.render_widget(paragraph, inner);
}

/// The rendered detail-pane content plus where a search-targeted flag
/// landed, if any.
struct BuiltLines {
    lines: Vec<Line<'static>>,
    /// The line index [`Flag`] `app.selected_flag` starts at, if it was
    /// found on `node`.
    target_flag_line: Option<usize>,
}

fn build_lines(
    node: &CommandNode,
    show_hidden: bool,
    width: usize,
    color_enabled: bool,
    target_flag: Option<&FlagKey>,
) -> BuiltLines {
    let mut lines = Vec::new();
    let mut target_flag_line = None;

    if let Some(summary) = &node.summary {
        for chunk in wrap_words(summary.as_str(), width) {
            lines.push(Line::from(Span::styled(
                chunk,
                Style::default().add_modifier(ratatui::style::Modifier::BOLD),
            )));
        }
        lines.push(Line::default());
    }

    if let Some(description) = &node.description {
        lines.push(heading_line("DESCRIPTION", color_enabled));
        for paragraph_text in description.as_str().split("\n\n") {
            for chunk in wrap_words(paragraph_text, width) {
                lines.push(Line::from(chunk));
            }
            lines.push(Line::default());
        }
    }

    if !node.usage.is_empty() {
        lines.push(heading_line("USAGE", color_enabled));
        for u in &node.usage {
            let full = format!("{} {}", defensive_single_line(&node.name), u.as_str());
            for chunk in wrap_words(&full, width) {
                lines.push(Line::from(chunk));
            }
        }
        lines.push(Line::default());
    }

    let visible_flags: Vec<&Flag> = node
        .flags
        .iter()
        .filter(|f| show_hidden || (!f.hidden && f.deprecated.is_none()))
        .collect();

    if !visible_flags.is_empty() {
        lines.push(heading_line("FLAGS", color_enabled));
        let (flag_lines_out, target) =
            flag_lines(&visible_flags, width, color_enabled, target_flag);
        let base = lines.len();
        if let Some(t) = target {
            target_flag_line = Some(base + t);
        }
        lines.extend(flag_lines_out);
        lines.push(Line::default());
    }

    lines.push(Line::from(Span::styled(
        provenance_footer(node),
        style::muted(color_enabled),
    )));

    BuiltLines {
        lines,
        target_flag_line,
    }
}

fn heading_line(text: &'static str, color_enabled: bool) -> Line<'static> {
    Line::from(Span::styled(text, style::muted_bold(color_enabled)))
}

/// Greedy word-wrap of `text` to at most `width` display columns per
/// line, never breaking a word unless it alone exceeds `width` (in which
/// case it's ellipsis-truncated rather than allowed to overflow). Always
/// returns at least one (possibly empty) chunk.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for word in text.split_whitespace() {
        let word_width = display_width(word);
        let sep_width = usize::from(!current.is_empty());
        if current_width + sep_width + word_width <= width {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
            current_width += sep_width + word_width;
            continue;
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
        }
        if word_width > width {
            lines.push(truncate_to_width_ellipsis(word, width));
        } else {
            current.push_str(word);
            current_width = word_width;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Strip a group heading's trailing colon and normalize its casing, so
/// carapace-sourced groups (already often plain, e.g. `"main"`) and
/// help-text-sourced groups (raw heading text, e.g. `"GLOBAL OPTIONS:"`
/// or `"Main operation mode:"`) render identically for the same logical
/// grouping instead of carrying their source's formatting quirks into the
/// UI verbatim.
fn normalize_group_heading(raw: &str) -> String {
    defensive_single_line(raw)
        .trim()
        .trim_end_matches(':')
        .trim()
        .to_uppercase()
}

/// Group flags by [`Flag::group`], with un-grouped flags first (under no
/// heading) and inherited flags always last as their own muted group,
/// regardless of their source `group` value (spec §9). Returns the lines
/// plus, if `target_flag` matched one of `flags`, the index of its line.
fn flag_lines(
    flags: &[&Flag],
    width: usize,
    color_enabled: bool,
    target_flag: Option<&FlagKey>,
) -> (Vec<Line<'static>>, Option<usize>) {
    let mut own_groups: BTreeMap<Option<String>, Vec<&Flag>> = BTreeMap::new();
    let mut inherited: Vec<&Flag> = Vec::new();

    for f in flags {
        if f.inherited {
            inherited.push(f);
        } else {
            own_groups
                .entry(f.group.as_ref().map(|g| normalize_group_heading(g)))
                .or_default()
                .push(f);
        }
    }

    let mut out = Vec::new();
    let mut target_line = None;
    let mut note_if_target = |out: &[Line<'static>], f: &Flag| {
        if target_line.is_none() && target_flag.is_some_and(|k| f.matches_key(k)) {
            target_line = Some(out.len());
        }
    };

    // Ungrouped flags first, with no heading.
    if let Some(ungrouped) = own_groups.remove(&None) {
        for f in ungrouped {
            note_if_target(&out, f);
            out.extend(flag_line(f, false, width, color_enabled));
        }
    }
    for (group, flags) in own_groups {
        if let Some(group) = group {
            out.push(heading_line_owned(group, color_enabled));
        }
        for f in flags {
            note_if_target(&out, f);
            out.extend(flag_line(f, false, width, color_enabled));
        }
    }

    if !inherited.is_empty() {
        out.push(heading_line("INHERITED", color_enabled));
        for f in inherited {
            note_if_target(&out, f);
            out.extend(flag_line(f, true, width, color_enabled));
        }
    }

    (out, target_line)
}

fn heading_line_owned(text: String, color_enabled: bool) -> Line<'static> {
    Line::from(Span::styled(text, style::muted_bold(color_enabled)))
}

/// One flag's spelling, value placeholder, and description — each styled
/// per spec §9.2's table (spelling: accent; value placeholder: muted
/// italic; description: default foreground) — wrapped so a multi-line
/// description hangs indented under where it started rather than
/// restarting at column 0.
fn flag_line(flag: &Flag, dim: bool, width: usize, color_enabled: bool) -> Vec<Line<'static>> {
    let mut name_spec = String::new();
    if let Some(s) = flag.short {
        name_spec.push('-');
        name_spec.push(s);
    }
    if flag.short.is_some() && flag.long.is_some() {
        name_spec.push_str(", ");
    }
    if let Some(l) = &flag.long {
        name_spec.push_str("--");
        name_spec.push_str(l);
    }

    let value_suffix = flag
        .value_name
        .as_ref()
        .and_then(|name| match flag.value_kind {
            ValueKind::Required => Some(format!(" {name}")),
            ValueKind::Optional => Some(format!("[={name}]")),
            ValueKind::None => None,
        });

    let leading = "  ";
    let spelling_style = if dim {
        style::muted(color_enabled)
    } else {
        style::accent(color_enabled)
    };
    let value_style = style::muted_italic(color_enabled);
    let desc_style = if dim {
        style::muted(color_enabled)
    } else {
        Style::default()
    };

    let mut first_line_spans = vec![Span::styled(
        format!("{leading}{name_spec}"),
        spelling_style,
    )];
    let mut prefix_width = display_width(leading) + display_width(&name_spec);
    if let Some(v) = &value_suffix {
        first_line_spans.push(Span::styled(v.clone(), value_style));
        prefix_width += display_width(v);
    }

    let deprecated_tag = flag
        .deprecated
        .as_ref()
        .map(|_| " (deprecated)".to_string());

    let mut description_text = flag.description.as_ref().map(|d| d.single_line());
    if let Some(tag) = &deprecated_tag {
        description_text = Some(match description_text {
            Some(d) => format!("{d}{tag}"),
            None => tag.trim_start().to_string(),
        });
    }

    let Some(description_text) = description_text.filter(|d| !d.is_empty()) else {
        return vec![Line::from(first_line_spans)];
    };

    let gap = "  ";
    let indent_width = prefix_width + display_width(gap);
    let available = width.saturating_sub(indent_width).max(1);
    let chunks = wrap_words(&description_text, available);

    let mut lines = Vec::new();
    let mut chunks_iter = chunks.into_iter();
    if let Some(first_chunk) = chunks_iter.next() {
        first_line_spans.push(Span::raw(gap));
        first_line_spans.push(Span::styled(first_chunk, desc_style));
    }
    lines.push(Line::from(first_line_spans));

    let indent_str = " ".repeat(indent_width);
    for chunk in chunks_iter {
        lines.push(Line::from(Span::styled(
            format!("{indent_str}{chunk}"),
            desc_style,
        )));
    }
    lines
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
        .effective_authority(mandible_core::Axis::Structural)
        > 0;
    let prose = node
        .provenance
        .effective_authority(mandible_core::Axis::Prose)
        > 0;
    let mut footer = format!(
        "{} · structure {} · prose {}",
        labels.join(" + "),
        if structural { "✓" } else { "✗" },
        if prose { "✓" } else { "✗" }
    );
    // Spec §7 Tier A′ / batch 6 part 4: surfacing the detected framework
    // turns "mandible is wrong about tool X" into "the <framework> grammar
    // mishandles Y" — the same general-not-per-tool framing `--doctor`
    // uses (`mandible/src/doctor.rs`), now visible without leaving the TUI.
    if let Some(framework) = &node.detected_framework {
        footer.push_str(" · framework: ");
        footer.push_str(&defensive_single_line(framework));
    }
    footer
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::{Provenance, Source, Text};

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

    fn text_of(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn inherited_flags_are_grouped_last() {
        let node = node_with_flags();
        let flags: Vec<&Flag> = node.flags.iter().collect();
        let (lines, _) = flag_lines(&flags, 80, true, None);
        let text: Vec<String> = lines.iter().map(text_of).collect();
        let inherited_pos = text.iter().position(|l| l.contains("INHERITED")).unwrap();
        let help_pos = text.iter().position(|l| l.contains("--help")).unwrap();
        assert!(help_pos > inherited_pos);
    }

    #[test]
    fn hidden_flags_suppressed_by_default() {
        let mut node = node_with_flags();
        node.flags[0].hidden = true;
        let built = build_lines(&node, false, 80, true, None);
        let joined: String = built.lines.iter().map(text_of).collect();
        assert!(!joined.contains("--interactive"));
    }

    #[test]
    fn hidden_flags_shown_when_toggled() {
        let mut node = node_with_flags();
        node.flags[0].hidden = true;
        let built = build_lines(&node, true, 80, true, None);
        let joined: String = built.lines.iter().map(text_of).collect();
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

    /// The reported defect: a flag description that wraps must hang-
    /// indent under the description column on continuation lines, not
    /// restart at column 0.
    #[test]
    fn wrapped_flag_description_hangs_indented_not_flush_left() {
        let mut flag = Flag::long("tlscacert", Provenance::single(Source::HelpText));
        flag.value_name = Some("string".to_string());
        flag.value_kind = ValueKind::Required;
        flag.description = Some(Text::sanitize(
            "Trust certs signed only by this CA (default \"\")",
        ));
        let lines = flag_line(&flag, false, 40, true);
        assert!(lines.len() >= 2, "expected wrapping: {lines:?}");
        let first_text = text_of(&lines[0]);
        let continuation_text = text_of(&lines[1]);
        let indent_len = continuation_text.len() - continuation_text.trim_start().len();
        // The continuation's leading whitespace must reach past the
        // spelling+value+gap on the first line — i.e. actually hang
        // indented, not just have *some* leading space.
        assert!(
            indent_len >= display_width("  --tlscacert string  "),
            "first={first_text:?} continuation={continuation_text:?} indent_len={indent_len}"
        );
    }

    /// Spec §9.2: the flag spelling is accent-styled, the value
    /// placeholder is muted italic, and the description is default
    /// foreground — three distinct spans, not one undifferentiated run.
    #[test]
    fn flag_line_has_distinctly_styled_spans() {
        let mut flag = Flag::long("output", Provenance::single(Source::HelpText));
        flag.short = Some('o');
        flag.value_name = Some("FILE".to_string());
        flag.value_kind = ValueKind::Required;
        flag.description = Some(Text::sanitize("Write output to FILE"));
        let lines = flag_line(&flag, false, 80, true);
        let spans = &lines[0].spans;
        assert!(spans.len() >= 3, "{spans:?}");
        // Spelling span carries the accent color.
        assert_eq!(spans[0].style.fg, Some(style::ACCENT));
        // Value span is styled differently from the spelling span (muted,
        // not accent).
        assert_ne!(spans[1].style, spans[0].style);
        assert_eq!(spans[1].content.as_ref(), " FILE");
    }

    #[test]
    fn deprecated_flag_gets_a_tag() {
        let mut flag = Flag::long("old-flag", Provenance::single(Source::HelpText));
        flag.deprecated = Some(Text::sanitize("use --new-flag instead"));
        flag.description = Some(Text::sanitize("Old behavior"));
        let lines = flag_line(&flag, false, 80, true);
        let joined: String = lines.iter().map(text_of).collect();
        assert!(joined.contains("(deprecated)"), "{joined:?}");
    }

    /// The coordinator's second reported defect: a group heading must not
    /// carry its source's trailing colon or casing quirks into the UI —
    /// `"GLOBAL OPTIONS:"` and `"Global Options"` must render the same.
    #[test]
    fn group_headings_strip_trailing_colon_and_normalize_case() {
        assert_eq!(normalize_group_heading("GLOBAL OPTIONS:"), "GLOBAL OPTIONS");
        assert_eq!(
            normalize_group_heading("Main operation mode:"),
            "MAIN OPERATION MODE"
        );
        assert_eq!(normalize_group_heading("main"), "MAIN");
    }

    /// Closing spec §10's open item: selecting a flag via search must
    /// scroll the detail pane to that exact flag's line.
    #[test]
    fn selected_flag_reports_its_own_line_index() {
        let node = node_with_flags();
        let built = build_lines(
            &node,
            false,
            80,
            true,
            Some(&FlagKey::Long("interactive".to_string())),
        );
        let idx = built.target_flag_line.expect("flag should be found");
        let line_text = text_of(&built.lines[idx]);
        assert!(line_text.contains("--interactive"), "{line_text:?}");
    }

    #[test]
    fn no_target_flag_means_no_scroll_override() {
        let node = node_with_flags();
        let built = build_lines(&node, false, 80, true, None);
        assert_eq!(built.target_flag_line, None);
    }

    /// Batch 6 part 4 (spec §7 Tier B step 3): a node whose parse degraded
    /// to level 3 must render its `unparsed` text, labelled as such, via
    /// the whole-frame path — not the structured `build_lines` path (which
    /// a node with `unparsed` set should never even reach, since
    /// `unparsed`/`flags`/`subcommands`/`usage` are mutually exclusive by
    /// construction).
    #[test]
    fn unparsed_node_renders_labelled_raw_text() {
        use crate::app::App;
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut root = CommandNode::new(
            "mystery",
            Provenance::with_confidence(Source::HelpText, 0.0),
        );
        root.unparsed = vec![
            Text::sanitize("a friendly banner"),
            Text::sanitize("and nothing else"),
        ];
        let app = App::new("mystery".to_string(), root);

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                render(frame, area, &app);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let rendered: String = buffer
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect::<Vec<_>>()
            .join("");
        assert!(rendered.contains("unparsed"), "{rendered}");
        assert!(rendered.contains("a friendly banner"), "{rendered}");
        assert!(rendered.contains("and nothing else"), "{rendered}");
    }
}
