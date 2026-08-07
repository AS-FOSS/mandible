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
use crate::glyphs::Glyphs;
use crate::sanitize::{defensive_single_line, display_width, truncate_to_width_marker};
use crate::style;
use mandible_core::{CommandNode, Flag, FlagKey, ValueKind};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Wrap};
use ratatui::Frame;
use std::collections::HashMap;

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
                .join(&format!(" {} ", app.glyphs.breadcrumb))
        })
        .unwrap_or_default();
    let title = format!(" {breadcrumb} ");

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_set(style::border_set(app.glyphs))
        .border_style(border_style)
        // A column of breathing room either side, so prose and flag rows
        // don't butt against the border. `Block::padding` takes it out of
        // the inner rect, so every width calculation downstream — wrapping,
        // the description column, truncation — accounts for it without
        // knowing it exists.
        .padding(Padding::horizontal(1));
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
        app.glyphs,
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
        format!("unparsed {} showing raw --help output", app.glyphs.absent),
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
    glyphs: Glyphs,
) -> BuiltLines {
    let mut lines = Vec::new();
    let mut target_flag_line = None;

    if let Some(summary) = &node.summary {
        for chunk in wrap_words(summary.as_str(), width, glyphs.ellipsis) {
            lines.push(Line::from(Span::styled(
                chunk,
                Style::default().add_modifier(ratatui::style::Modifier::BOLD),
            )));
        }
        lines.push(Line::default());
    }

    if let Some(description) = &node.description {
        lines.push(heading_line_ruled(
            "DESCRIPTION",
            width,
            color_enabled,
            glyphs,
        ));
        for paragraph_text in description.as_str().split("\n\n") {
            for chunk in wrap_words(paragraph_text, width, glyphs.ellipsis) {
                lines.push(Line::from(chunk));
            }
            lines.push(Line::default());
        }
    }

    if !node.usage.is_empty() {
        lines.push(heading_line_ruled("USAGE", width, color_enabled, glyphs));
        for u in &node.usage {
            let full = usage_signature(&node.name, u.as_str());
            // Indented as a block, the way API documentation sets a
            // signature apart from its prose.
            let indent = "  ";
            let avail = width.saturating_sub(display_width(indent)).max(1);
            for chunk in wrap_words(&full, avail, glyphs.ellipsis) {
                lines.push(Line::from(format!("{indent}{chunk}")));
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
        lines.push(heading_line_ruled("FLAGS", width, color_enabled, glyphs));
        let (flag_lines_out, target) =
            flag_lines(&visible_flags, width, color_enabled, target_flag, glyphs);
        let base = lines.len();
        if let Some(t) = target {
            target_flag_line = Some(base + t);
        }
        lines.extend(flag_lines_out);
        lines.push(Line::default());
    }

    // Provenance is not rendered here at all any more: it describes where
    // this node's data came from, which belongs beside the pane rather than
    // inside its content. See `render::status_bar`.

    BuiltLines {
        lines,
        target_flag_line,
    }
}

/// A section heading followed by a rule to the pane's edge.
///
/// The rule is what gives the pane hierarchy: without it, a bold word and
/// the body text beneath it are two lines of similar weight, and the eye
/// has nothing to anchor a section boundary to. Drawn in the muted style so
/// it separates without competing, and through the glyph set so a
/// non-UTF-8 terminal gets `-` rather than tofu.
fn heading_line_ruled(
    text: &str,
    width: usize,
    color_enabled: bool,
    glyphs: Glyphs,
) -> Line<'static> {
    let heading = text.to_string();
    let used = display_width(&heading) + 1;
    let rule_width = width.saturating_sub(used);
    let mut spans = vec![Span::styled(heading, style::muted_bold(color_enabled))];
    if rule_width > 0 {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            glyphs.rule.repeat(rule_width),
            style::muted(color_enabled),
        ));
    }
    Line::from(spans)
}

/// One usage line, with the redundancy stripped.
///
/// The raw string frequently already carries both a `Usage:` label and the
/// tool's own name — `tar --help` yields `Usage: tar [OPTION...]` — and
/// prepending the node name to that produced `tar Usage: tar [OPTION...]`,
/// with the name twice and a label the `USAGE` heading directly above
/// already supplies. The name is prepended only when the line does not
/// already begin with it, which is what makes a bare pattern like
/// `[OPTIONS] <url>` still render as a complete invocation.
fn usage_signature(node_name: &str, usage: &str) -> String {
    let name = defensive_single_line(node_name);
    let mut text = defensive_single_line(usage);

    // Drop a leading `usage:` label, case-insensitively — the heading says
    // it.
    let trimmed = text.trim_start();
    if trimmed.len() >= 6 && trimmed[..6].eq_ignore_ascii_case("usage:") {
        text = trimmed[6..].trim_start().to_string();
    }

    let starts_with_name = text
        .split_whitespace()
        .next()
        .is_some_and(|first| first == name);
    if starts_with_name || name.is_empty() {
        text
    } else {
        format!("{name} {text}")
    }
}

/// Greedy word-wrap of `text` to at most `width` display columns per
/// line, never breaking a word unless it alone exceeds `width` (in which
/// case it's ellipsis-truncated rather than allowed to overflow). Always
/// returns at least one (possibly empty) chunk.
fn wrap_words(text: &str, width: usize, marker: &str) -> Vec<String> {
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
            lines.push(truncate_to_width_marker(word, width, marker));
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
/// The description column is capped at this fraction of the pane, so one
/// very long flag spelling cannot push every description in the list off
/// the right-hand edge. Mirrors the tree pane's summary-column rule
/// (spec §9.1).
const DESC_COLUMN_CAP_PERCENT: usize = 45;

/// The column every flag description starts at: the widest spelling in the
/// list (plus a two-space gutter), capped.
///
/// Computed once over *all* the flags being rendered rather than per flag,
/// which is the whole point — a shared column is what turns a list of
/// options into a parameter table. Includes inherited flags so the two
/// blocks line up with each other.
/// Where a flag row's two right-hand columns begin: the value placeholder,
/// then the description.
///
/// Three columns rather than two, because a value placeholder is a
/// different *kind* of thing from a spelling — `--env` and `list` answer
/// "what do I type" and "what does it take". Run together as
/// `--env list` they read as one token; in their own columns the whole
/// list can be scanned down either one, which is what a parameter table in
/// API documentation is for.
#[derive(Debug, Clone, Copy)]
struct FlagColumns {
    value: usize,
    description: usize,
}

fn flag_columns(flags: &[&Flag], width: usize) -> FlagColumns {
    let widest_spec = flags
        .iter()
        .map(|f| display_width(&flag_name_spec(f)))
        .max()
        .unwrap_or(0);
    let widest_value = flags
        .iter()
        .filter_map(|f| flag_value_text(f))
        .map(|v| display_width(&v))
        .max()
        .unwrap_or(0);

    let cap = width * DESC_COLUMN_CAP_PERCENT / 100;
    // 2 leading + spelling + 1 gutter.
    let value = (2 + widest_spec + 1).min(cap.saturating_sub(2));
    // ...then the value column, + 2 gutter. When nothing in this list takes
    // a value the column collapses, rather than leaving a blank strip.
    let gutter = if widest_value == 0 {
        0
    } else {
        widest_value + 2
    };
    let description = (value + gutter).min(cap);
    FlagColumns { value, description }
}

fn flag_lines(
    flags: &[&Flag],
    width: usize,
    color_enabled: bool,
    target_flag: Option<&FlagKey>,
    glyphs: Glyphs,
) -> (Vec<Line<'static>>, Option<usize>) {
    let columns = flag_columns(flags, width);
    // Groups keep the order the tool printed them in, which is editorial:
    // `tar --help` leads with "Main operation mode" because that is what you
    // need first, and its 17 groups are sequenced deliberately. A BTreeMap
    // here sorted them alphabetically, so "Archive format selection" came
    // first and the author's ordering was silently discarded.
    let mut group_order: Vec<Option<String>> = Vec::new();
    let mut own_groups: HashMap<Option<String>, Vec<&Flag>> = HashMap::new();
    let mut inherited: Vec<&Flag> = Vec::new();

    for f in flags {
        if f.inherited {
            inherited.push(f);
        } else {
            let key = f.group.as_ref().map(|g| normalize_group_heading(g));
            if !own_groups.contains_key(&key) {
                group_order.push(key.clone());
            }
            own_groups.entry(key).or_default().push(f);
        }
    }

    let mut out = Vec::new();
    let mut target_line = None;
    let mut note_if_target = |out: &[Line<'static>], f: &Flag| {
        if target_line.is_none() && target_flag.is_some_and(|k| f.matches_key(k)) {
            target_line = Some(out.len());
        }
    };

    // Ungrouped flags first, with no heading, then each group in the order
    // the tool introduced it.
    if let Some(ungrouped) = own_groups.remove(&None) {
        for f in ungrouped {
            note_if_target(&out, f);
            out.extend(flag_line(f, false, width, color_enabled, glyphs, columns));
        }
    }
    for key in group_order {
        let Some(flags) = own_groups.remove(&key) else {
            continue;
        };
        if let Some(group) = key {
            out.push(heading_line_owned(group, color_enabled));
        }
        for f in flags {
            note_if_target(&out, f);
            out.extend(flag_line(f, false, width, color_enabled, glyphs, columns));
        }
    }

    if !inherited.is_empty() {
        out.push(heading_line_ruled(
            "INHERITED",
            width,
            color_enabled,
            glyphs,
        ));
        for f in inherited {
            note_if_target(&out, f);
            out.extend(flag_line(f, true, width, color_enabled, glyphs, columns));
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
/// A flag's spelling, e.g. `-i, --interactive`.
fn flag_name_spec(flag: &Flag) -> String {
    let mut spec = String::new();
    if let Some(s) = flag.short {
        spec.push('-');
        spec.push(s);
    }
    if flag.short.is_some() && flag.long.is_some() {
        spec.push_str(", ");
    }
    if let Some(l) = &flag.long {
        spec.push_str("--");
        spec.push_str(l);
    }
    spec
}

/// A flag's value placeholder as its own column entry, e.g. `FILE` or
/// `[FILE]` when optional. `None` when the flag takes no value.
fn flag_value_text(flag: &Flag) -> Option<String> {
    flag.value_name
        .as_ref()
        .and_then(|name| match flag.value_kind {
            ValueKind::Required => Some(name.clone()),
            ValueKind::Optional => Some(format!("[{name}]")),
            ValueKind::None => None,
        })
}

fn flag_line(
    flag: &Flag,
    dim: bool,
    width: usize,
    color_enabled: bool,
    glyphs: Glyphs,
    // Where the value and description columns begin, shared across the
    // whole flag list — see `flag_columns`.
    columns: FlagColumns,
) -> Vec<Line<'static>> {
    let name_spec = flag_name_spec(flag);
    let value_text = flag_value_text(flag);

    let leading = "  ";
    let spelling_style = if dim {
        style::muted(color_enabled)
    } else {
        style::accent(color_enabled)
    };
    // Muted, not italic. Italic is unreliable — spec §9.2 lists it among
    // the modifiers many terminals silently ignore, and where it *is*
    // honoured the glyphs frequently overflow their cell and leave
    // artefacts behind (reported on a `--log-level` value rendering
    // `error|info|debug`). It was also redundant the moment values moved
    // into their own column: position now carries the distinction, which
    // is the more robust signal anyway.
    let value_style = style::muted(color_enabled);
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
    if let Some(v) = &value_text {
        // Padded to its own column, so values line up down the list rather
        // than sitting wherever each spelling happens to end.
        let pad = columns.value.saturating_sub(prefix_width).max(1);
        first_line_spans.push(Span::raw(" ".repeat(pad)));
        first_line_spans.push(Span::styled(v.clone(), value_style));
        prefix_width += pad + display_width(v);
    }

    let deprecated_tag = flag
        .deprecated
        .as_ref()
        .map(|_| " (deprecated)".to_string());

    let mut description_text = flag.description.as_ref().map(|d| d.single_line());

    // The IR carries a flag's permitted values (spec §7 Tier B rule 4:
    // `gnu`/`oldgnu`/`pax`/`posix` under `tar --format=` are enum values,
    // which is why they are *not* subcommands) and the pane was extracting
    // them and then dropping them on the floor. Knowing that `--format`
    // takes exactly six spellings is precisely the sort of thing you open a
    // reference to find out.
    if !flag.choices.is_empty() {
        let joined = flag
            .choices
            .iter()
            .map(|c| c.as_str().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let rendered = format!("[{joined}]");
        description_text = Some(match description_text {
            Some(d) if !d.is_empty() => format!("{d} {rendered}"),
            _ => rendered,
        });
    }

    if let Some(tag) = &deprecated_tag {
        description_text = Some(match description_text {
            Some(d) => format!("{d}{tag}"),
            None => tag.trim_start().to_string(),
        });
    }

    let Some(description_text) = description_text.filter(|d| !d.is_empty()) else {
        return vec![Line::from(first_line_spans)];
    };

    // One description column for the entire list, not one per flag. The
    // indent used to be `this flag's own width + 2`, so every row started
    // its text somewhere different and the block read as ragged prose. A
    // shared column is what makes a parameter list read as a table — the
    // defining visual element of API documentation.
    //
    // A spelling longer than the column still never gets truncated to
    // force alignment (spec §9.1's rule for the tree applies here too):
    // it simply pushes its own description along, one row out of step
    // rather than one name destroyed.
    let gap = "  ";
    let indent_width = columns.description.max(prefix_width + display_width(gap));
    let available = width.saturating_sub(indent_width).max(1);
    let chunks = wrap_words(&description_text, available, glyphs.ellipsis);

    let mut lines = Vec::new();
    let mut chunks_iter = chunks.into_iter();
    if let Some(first_chunk) = chunks_iter.next() {
        first_line_spans.push(Span::raw(" ".repeat(indent_width - prefix_width)));
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
/// Where a node's data came from, e.g. `help-text + cobra-dunder-complete`.
///
/// Rendered in the status row under the detail pane rather than inside the
/// pane itself: it describes the pane's *subject*, not its content, and
/// inside the pane it pushed the documentation down by a line on every
/// command to say the same thing each time.
pub fn provenance_summary(node: &CommandNode) -> String {
    node.provenance
        .sources
        .iter()
        .map(|s| s.label())
        .collect::<Vec<_>>()
        .join(" + ")
}

/// Confidence below this is a warning; at or above it, silence.
///
/// 0.5 is exactly the cap Tier B applies when no framework was identified
/// but the generic engine parsed cleanly — `git`, `curl`, `apt-get` and
/// `openssl` all sit there and are fine. What is worth warning about is
/// well below it: `find` scores 0.11 and `ip` 0.09, meaning the grammar
/// recognised almost nothing and what is on screen is a guess.
const LOW_CONFIDENCE: f32 = 0.5;

/// A caveat about *this* node, or nothing at all.
///
/// The footer used to read `help-text · structure ✓ · prose ✓` under every
/// command of every tool. Both axes have authority for every tool
/// measured, so the ticks were always ticks; the tier list was the same
/// string on every node. It was decoration, and it crowded out the one
/// thing in this area that carries information — how much of the help text
/// the grammar actually understood.
///
/// So it now appears only when there is a caveat. Silence means "nothing
/// to flag", which is a stronger signal than a tick that is always
/// present, and it is the same reasoning that moved the framework out of
/// here: repeated identical metadata is noise, not provenance.
pub fn provenance_caveat(node: &CommandNode, glyphs: Glyphs) -> Option<String> {
    let confidence = node.provenance.confidence?;
    if confidence >= LOW_CONFIDENCE {
        return None;
    }

    // Terse on purpose: this shares one row with the controls, and the
    // long form ("… understood little of this tool's help text; treat the
    // structure as a guess") ran past the width available and pushed them
    // off. The percentage is the information; the reader can infer the
    // rest, and `--doctor` has the detail.
    let pct = (confidence * 100.0).round() as u32;
    let _ = glyphs;
    Some(format!("low confidence: {pct}% parsed"))
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
        let (lines, _) = flag_lines(&flags, 80, true, None, crate::glyphs::UNICODE);
        let text: Vec<String> = lines.iter().map(text_of).collect();
        let inherited_pos = text.iter().position(|l| l.contains("INHERITED")).unwrap();
        let help_pos = text.iter().position(|l| l.contains("--help")).unwrap();
        assert!(help_pos > inherited_pos);
    }

    #[test]
    fn hidden_flags_suppressed_by_default() {
        let mut node = node_with_flags();
        node.flags[0].hidden = true;
        let built = build_lines(&node, false, 80, true, None, crate::glyphs::UNICODE);
        let joined: String = built.lines.iter().map(text_of).collect();
        assert!(!joined.contains("--interactive"));
    }

    #[test]
    fn hidden_flags_shown_when_toggled() {
        let mut node = node_with_flags();
        node.flags[0].hidden = true;
        let built = build_lines(&node, true, 80, true, None, crate::glyphs::UNICODE);
        let joined: String = built.lines.iter().map(text_of).collect();
        assert!(joined.contains("--interactive"));
    }

    /// Every description starts in the same column, whatever the flag's
    /// spelling is. Descriptions used to be indented by *each flag's own*
    /// width, so a list of options read as ragged prose rather than a
    /// parameter table — the alignment is what makes it look like
    /// documentation.
    #[test]
    fn flag_descriptions_share_one_column() {
        let mk = |short: Option<char>, long: &str, value: Option<&str>, desc: &str| {
            let mut f = mandible_core::Flag::long(long, Provenance::single(Source::HelpText));
            f.short = short;
            f.value_name = value.map(|v| v.to_string());
            if value.is_some() {
                f.value_kind = ValueKind::Required;
            }
            f.description = Some(Text::sanitize(desc));
            f
        };
        let flags = [
            mk(Some('d'), "detach", None, "Detached mode"),
            mk(
                None,
                "detach-keys",
                Some("string"),
                "Override the key sequence",
            ),
            mk(Some('e'), "env", Some("list"), "Set environment variables"),
        ];
        let refs: Vec<&mandible_core::Flag> = flags.iter().collect();
        let (lines, _) = flag_lines(&refs, 80, true, None, crate::glyphs::UNICODE);

        // Column at which each row's description text begins.
        let starts: Vec<usize> = lines
            .iter()
            .filter_map(|line| {
                let text = text_of(line);
                let trimmed = text.trim_start();
                if trimmed.starts_with('-') {
                    // A flag row: find where the description follows the
                    // spelling and its run of padding.
                    let spec_end = text.find("  ")?;
                    let rest = &text[spec_end..];
                    let pad = rest.len() - rest.trim_start().len();
                    Some(spec_end + pad)
                } else {
                    None
                }
            })
            .collect();

        assert!(starts.len() >= 3, "expected a row per flag, got {starts:?}");
        assert!(
            starts.windows(2).all(|w| w[0] == w[1]),
            "descriptions are not column-aligned: {starts:?}"
        );
    }

    /// A confidently-parsed node says nothing. Silence is the signal that
    /// there is nothing to flag, and it is a stronger one than a tick that
    /// was present on every node of every tool measured.
    #[test]
    fn a_confident_node_gets_no_caveat() {
        let mut node = node_with_flags();
        node.provenance = Provenance::with_confidence(Source::HelpText, 0.97);
        assert_eq!(provenance_caveat(&node, crate::glyphs::UNICODE), None);

        // Exactly at the threshold is Tier B's "no framework identified but
        // parsed cleanly" cap, where git, curl and apt-get sit. Not a
        // warning.
        node.provenance = Provenance::with_confidence(Source::HelpText, LOW_CONFIDENCE);
        assert_eq!(provenance_caveat(&node, crate::glyphs::UNICODE), None);
    }

    /// A barely-parsed node says so. `find` scores 0.11 and `ip` 0.09 in
    /// practice, and both used to report `structure ✓ · prose ✓`.
    #[test]
    fn a_barely_parsed_node_warns_with_its_score() {
        let mut node = node_with_flags();
        node.provenance = Provenance::with_confidence(Source::HelpText, 0.11);
        let caveat = provenance_caveat(&node, crate::glyphs::UNICODE)
            .expect("low confidence must be surfaced");
        assert!(caveat.contains("11%"), "{caveat:?}");
        assert!(caveat.contains("low confidence"), "{caveat:?}");
        // Short enough to share a row with the controls.
        assert!(caveat.chars().count() <= 32, "too long: {caveat:?}");
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
        let lines = flag_line(
            &flag,
            false,
            40,
            true,
            crate::glyphs::UNICODE,
            FlagColumns {
                value: 18,
                description: 20,
            },
        );
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
        let lines = flag_line(
            &flag,
            false,
            80,
            true,
            crate::glyphs::UNICODE,
            FlagColumns {
                value: 18,
                description: 20,
            },
        );
        let spans = &lines[0].spans;
        assert!(spans.len() >= 3, "{spans:?}");
        // Spelling span carries the accent color.
        assert_eq!(spans[0].style.fg, Some(style::ACCENT));
        // The value sits in its own column, so the padding between the two
        // is its own (unstyled) span and the value follows it.
        let value = spans
            .iter()
            .find(|s| s.content.as_ref() == "FILE")
            .expect("value should be its own span");
        assert_ne!(
            value.style, spans[0].style,
            "value must not read as a spelling"
        );
    }

    #[test]
    fn deprecated_flag_gets_a_tag() {
        let mut flag = Flag::long("old-flag", Provenance::single(Source::HelpText));
        flag.deprecated = Some(Text::sanitize("use --new-flag instead"));
        flag.description = Some(Text::sanitize("Old behavior"));
        let lines = flag_line(
            &flag,
            false,
            80,
            true,
            crate::glyphs::UNICODE,
            FlagColumns {
                value: 18,
                description: 20,
            },
        );
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
            crate::glyphs::UNICODE,
        );
        let idx = built.target_flag_line.expect("flag should be found");
        let line_text = text_of(&built.lines[idx]);
        assert!(line_text.contains("--interactive"), "{line_text:?}");
    }

    #[test]
    fn no_target_flag_means_no_scroll_override() {
        let node = node_with_flags();
        let built = build_lines(&node, false, 80, true, None, crate::glyphs::UNICODE);
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
