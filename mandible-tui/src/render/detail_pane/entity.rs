//! One entity rendered as its own row, plus the choice rows beneath it.

use super::*;

/// An entity's spellings, e.g. `-i, --interactive` for a flag or
/// `pathspec` for a positional — with a repeatable positional's `...`
/// (spec §9.3).
pub(super) fn entity_name_spec(flag: &Entity) -> String {
    // Every documented spelling, comma-separated, each reconstructed by
    // `Spelling::render`: the dashes and the getopt_long `--[no-]foo`
    // convention are display metadata, because the IR stores every name
    // bare (`"help"`, never `"-help"`; `"foo"`, never `"[no-]foo"`) so
    // that what a user searches and copies has no punctuation smuggled
    // into it. This is the one place that spelling comes back together —
    // and it is a *list*, so a row documenting four spellings
    // (`-h, -?, -help, --help`) renders all four rather than the two a
    // short/long pair could hold.
    let spellings = flag
        .spellings
        .iter()
        .map(Spelling::render)
        .collect::<Vec<_>>()
        .join(", ");
    // `repeatable` is one fact spelled two ways by the two kinds'
    // notation (`Entity::repeatable`): a flag says it by being accepted
    // again (`-v -v -v`), a positional by the POSIX synopsis ellipsis the
    // parser read it from (`FILE...`). Only the positional's notation is
    // a suffix on the name, so only POSITIONALS gets one back — putting
    // `...` on `-v` would invent a spelling nobody can type. Required/no
    // marker is untouched; the ellipsis says "more than one", not
    // "at least one".
    if flag.kind == EntityKind::Positional && flag.repeatable {
        return format!("{spellings}...");
    }
    spellings
}

/// How wide one entity's head runs: from the pane's left edge, through the
/// section's indent and the column its shape starts it at, to the end of
/// its value placeholder.
///
/// The single width the section's shared column is fitted to (spec §9.3).
/// It has to be one number, because a placeholder measured separately is a
/// placeholder the row is not charged for: `grep`'s `-e, --regexp
/// PATTERNS` fits a column measured over `-e, --regexp` and overruns the
/// one it is rendered against.
pub(super) fn entity_head_width(entity: &Entity, indent: usize) -> usize {
    let mut width = spelling_column(entity, indent) + display_width(&entity_name_spec(entity));
    if let Some(v) = entity_value_text(entity) {
        let gap = if spelling_is_sigil(entity) { 0 } else { 1 };
        width += gap + display_width(&v);
    }
    width
}

/// An entity's value placeholder, e.g. `FILE` or `[FILE]` when optional.
/// `None` when it takes no value.
pub(super) fn entity_value_text(flag: &Entity) -> Option<String> {
    flag.value_name
        .as_ref()
        .and_then(|name| match flag.value_kind {
            ValueKind::Required => Some(name.clone()),
            ValueKind::Optional => Some(format!("[{name}]")),
            ValueKind::None => None,
        })
}

/// True when this entity's value placeholder glues directly onto its
/// spelling with no space — the argfile sigil flag's row-verbatim shape,
/// `@<file>` (spec §4.5), rather than the ordinary `--output FILE` gap
/// (spec §9.3). Decided by shape (a single dashless spelling whose first
/// character is not alphanumeric), not by the literal `"@"`: a dashed
/// short option like `-?` must not match, since it does take a value
/// (`ffplay`'s `-? topic`) with the ordinary space.
pub(super) fn spelling_is_sigil(flag: &Entity) -> bool {
    flag.spellings.len() == 1
        && matches!(flag.spellings[0].dashes, Dashes::None)
        && flag.spellings[0]
            .name
            .chars()
            .next()
            .is_some_and(|c| !c.is_alphanumeric())
}

/// One entity's spellings, value placeholder, and description — styled
/// per spec §9.2 (spelling: accent; value: muted; description: default)
/// — laid out against the section's shared column. Every description
/// line starts at that column, except a head that reaches it: it keeps
/// its own line and its first description line starts one space past,
/// with every continuation back at the column (spec §9.3). Returned lines
/// are one logical row; the caller records that as an [`EntryRow`].
pub(super) fn entity_line(
    flag: &Entity,
    dim: bool,
    width: usize,
    color_enabled: bool,
    // Where the value and description columns begin, shared across the
    // whole section — see `section_layout`.
    layout: SectionLayout,
) -> Vec<Line<'static>> {
    let name_spec = entity_name_spec(flag);
    let value_text = entity_value_text(flag);

    // Two columns, chosen by the row's own shape (spec §9.3), behind the
    // section's own indent: shorts at the content edge, longs preindented
    // so every long in the section starts in the same place whether or not
    // a short precedes it.
    let head_column = spelling_column(flag, layout.indent);
    let leading = " ".repeat(head_column);
    let leading = leading.as_str();
    let spelling_style = if dim {
        style::muted(color_enabled)
    } else {
        style::accent(color_enabled)
    };
    // Muted, not italic: many terminals silently ignore italic (spec §9.2).
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
        // One space after the spelling, never a padded slot of its own
        // (spec §9.3), except the argfile sigil flag (spec §4.5, `@<file>`).
        let gap = if spelling_is_sigil(flag) { "" } else { " " };
        first_line_spans.push(Span::raw(gap));
        first_line_spans.push(Span::styled(v.clone(), value_style));
        prefix_width += display_width(gap) + display_width(v);
    }

    // A head wider than the pane is broken across lines here rather than
    // handed to `Paragraph`'s defensive `Wrap` (module doc), which restarts
    // at column 0 with no memory of the row's indent. Regression fixture:
    // `vgchange --alloc` rendered through a real pty (AGENTS.md §3.2).
    let mut head: Vec<Line<'static>> = Vec::new();
    if prefix_width > width {
        // Budget the wrap at the row's own column, so every line of the
        // head sits in the area reserved for heads rather than drifting
        // into the description's.
        let budget = width.saturating_sub(head_column).max(1);
        let indent = leading.to_string();
        for (i, chunk) in wrap_words(&name_spec, budget).into_iter().enumerate() {
            let text = if i == 0 {
                format!("{leading}{chunk}")
            } else {
                format!("{indent}{chunk}")
            };
            head.push(Line::from(Span::styled(text, spelling_style)));
        }
        if let Some(v) = &value_text {
            for chunk in wrap_words(v, budget) {
                head.push(Line::from(Span::styled(
                    format!("{indent}{chunk}"),
                    value_style,
                )));
            }
        }
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
    let description_text = description_text.filter(|d| !d.is_empty());

    // A flag's `choices` (spec §7 Tier B rule 4) never fold into
    // `description_text` or the spelling: they render as their own
    // `values:` line, indented two past the description column (spec
    // §9.3). Choices with no per-value description (e.g. `tar
    // --quoting-style`) get the single summary line below; choices that
    // carry their own text (ffmpeg/ffplay AVOptions) render one indented
    // `name  description` row each instead, via `choice_detail_lines`.
    let has_choice_descriptions = flag.choices.iter().any(|c| c.description.is_some());
    let values_line = (!flag.choices.is_empty() && !has_choice_descriptions).then(|| {
        let joined = flag
            .choices
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        format!("values: {joined}")
    });

    if description_text.is_none() && values_line.is_none() && !has_choice_descriptions {
        if !head.is_empty() {
            return head;
        }
        return vec![Line::from(first_line_spans)];
    }

    // One description column for the entire section (spec §9.3). A head
    // that reaches the column keeps its own line, never truncated (spec
    // §9.1); only its first description line starts one space past it,
    // every later line is back at the column.
    let column = layout.description;
    let rest_width = width.saturating_sub(column).max(1);

    let mut lines = Vec::new();
    let mut remainder = description_text.clone().unwrap_or_default();
    if head.is_empty() {
        if description_text.is_some() {
            // One space past the head, or the column — whichever is
            // further right. They coincide for every row whose head fits.
            let start = column.max(prefix_width + 1);
            let first = width
                .checked_sub(start)
                .and_then(|room| leading_words(&remainder, room));
            if let Some((first_chunk, rest)) = first {
                first_line_spans.push(Span::raw(" ".repeat(start - prefix_width)));
                first_line_spans.push(Span::styled(first_chunk, desc_style));
                remainder = rest;
            }
        }
        // No description: the head is the whole first line, and a
        // choices-only flag's values line still lands on the next one.
        lines.push(Line::from(first_line_spans));
    } else {
        // A head broken across lines has taken the pane's whole width for
        // itself; its description begins on the line after it, at the
        // column like any other.
        lines.extend(head);
    }

    let indent_str = " ".repeat(column);
    if !remainder.is_empty() {
        for chunk in wrap_words(&remainder, rest_width) {
            lines.push(Line::from(Span::styled(
                format!("{indent_str}{chunk}"),
                desc_style,
            )));
        }
    }

    // The values line sits two columns past the description column,
    // marking it subordinate rather than a second row (spec §9.3).
    if let Some(values_line) = values_line {
        let values_column = column + 2;
        let values_width = width.saturating_sub(values_column).max(1);
        let values_indent = " ".repeat(values_column);
        let values_style = style::muted(color_enabled);
        for chunk in wrap_words(&values_line, values_width) {
            lines.push(Line::from(Span::styled(
                format!("{values_indent}{chunk}"),
                values_style,
            )));
        }
    } else if has_choice_descriptions {
        lines.extend(choice_detail_lines(flag, column, width, color_enabled));
    }

    lines
}

/// Render a flag's choices as a `values:` header followed by one indented
/// `name  description` row per choice (spec §9.3/§4.1's render note),
/// dim/derived style matching the bare `values:` line this replaces.
///
/// Only called when at least one choice carries a description — the mixed
/// case (some choices described, some bare, e.g. ffmpeg's `-bug`, whose
/// `autodetect` value has no text of its own) still renders every choice
/// through this path so the list stays one table rather than splitting
/// into two.
pub(super) fn choice_detail_lines(
    flag: &Entity,
    column: usize,
    width: usize,
    color_enabled: bool,
) -> Vec<Line<'static>> {
    let values_column = column + 2;
    let values_indent = " ".repeat(values_column);
    let values_style = style::muted(color_enabled);

    let choice_column = values_column + 2;
    let choice_indent = " ".repeat(choice_column);
    let choice_width = width.saturating_sub(choice_column).max(1);
    let name_width = flag
        .choices
        .iter()
        .map(|c| display_width(&c.name))
        .max()
        .unwrap_or(0);

    let mut lines = vec![Line::from(Span::styled(
        format!("{values_indent}values:"),
        values_style,
    ))];

    for choice in &flag.choices {
        let desc = choice
            .description
            .as_ref()
            .map(|d| d.single_line())
            .filter(|d| !d.is_empty());
        let Some(desc) = desc else {
            lines.push(Line::from(Span::styled(
                format!("{choice_indent}{}", choice.name),
                values_style,
            )));
            continue;
        };
        let head = format!("{:<name_width$}  ", choice.name);
        let head_width = display_width(&head);
        let first_room = choice_width.saturating_sub(head_width);
        match leading_words(&desc, first_room) {
            Some((first_chunk, rest)) => {
                lines.push(Line::from(Span::styled(
                    format!("{choice_indent}{head}{first_chunk}"),
                    values_style,
                )));
                // `leading_words` returns `rest == ""` when the whole
                // description fit on the first line — `wrap_words` always
                // returns at least one (possibly empty) chunk for callers
                // that need one, which is exactly wrong here: an empty
                // `rest` means there is no continuation to render, not one
                // blank line's worth. Skipping the call entirely (rather
                // than filtering its output) is what keeps a choice whose
                // description merely happens to be blank-after-wrapping
                // indistinguishable from one that never had a remainder —
                // there is no such case, `rest` is only ever the literal
                // empty string when nothing is left.
                if !rest.is_empty() {
                    let cont_indent = " ".repeat(choice_column + head_width);
                    let cont_width = width.saturating_sub(choice_column + head_width).max(1);
                    for chunk in wrap_words(&rest, cont_width) {
                        lines.push(Line::from(Span::styled(
                            format!("{cont_indent}{chunk}"),
                            values_style,
                        )));
                    }
                }
            }
            None => {
                lines.push(Line::from(Span::styled(
                    format!("{choice_indent}{}", choice.name),
                    values_style,
                )));
                for chunk in wrap_words(&desc, choice_width) {
                    lines.push(Line::from(Span::styled(
                        format!("{choice_indent}{chunk}"),
                        values_style,
                    )));
                }
            }
        }
    }

    lines
}

/// The words of `text` that fit in `width` display columns, and what is
/// left of it — `None` when not even the first word fits.
///
/// The greedy first line of a description whose head pushed it right
/// (spec §9.3). A word too wide for the room beside the head is not broken
/// there: `None` sends the whole description to the next line, where the
/// section's column gives it the room to break in. Splitting it twice —
/// once against a few leftover cells, once against the column — is how a
/// pushed row ends up less readable than a plain one.
pub(super) fn leading_words(text: &str, width: usize) -> Option<(String, String)> {
    let mut first = String::new();
    let mut used = 0usize;
    let mut words = text.split_whitespace();
    let mut rest: Vec<&str> = Vec::new();

    for word in words.by_ref() {
        let candidate = used + usize::from(used > 0) + display_width(word);
        if candidate > width {
            rest.push(word);
            break;
        }
        if used > 0 {
            first.push(' ');
        }
        first.push_str(word);
        used = candidate;
    }
    if first.is_empty() {
        return None;
    }
    rest.extend(words);
    Some((first, rest.join(" ")))
}
