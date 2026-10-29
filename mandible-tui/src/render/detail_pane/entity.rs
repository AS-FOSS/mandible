//! One entity rendered as its own row, plus the choice rows beneath it.

use super::*;

/// True for a short spelling (one dash, one character) — the same shape
/// test `Entity::short_spelling` uses. Display-order only (spec §16, "short
/// spellings before long ones"): the IR keeps source order, this function
/// decides only how [`entity_name_spec`] reassembles it for the screen.
pub(super) fn is_short_spelling(s: &Spelling) -> bool {
    matches!(s.dashes, Dashes::Single) && s.name.chars().count() == 1
}

/// True for the spelling a range row starts at; see
/// [`Entity::is_range_start`].
pub(super) fn is_range_start(entity: &Entity, s: &Spelling) -> bool {
    entity.is_range_start(s)
}

/// True for a short spelling that is drawn as a short; see
/// [`Entity::is_drawn_short`].
pub(super) fn is_drawn_short(entity: &Entity, s: &Spelling) -> bool {
    entity.is_drawn_short(s)
}

/// How spelling `i` is drawn: its own rendering, or the whole range when
/// it is the range's start.
pub(super) fn spelling_text(entity: &Entity, i: usize) -> String {
    let s = &entity.spellings[i];
    match entity.range() {
        Some(range) if is_range_start(entity, s) => range.render(),
        _ => s.render(),
    }
}

/// The order spellings are drawn in; see [`Entity::display_order`].
pub(super) fn display_order(entity: &Entity) -> Vec<usize> {
    entity.display_order()
}

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
    // Shorts before longs, always (spec §16): display-side only, a stable
    // sort so each group keeps its own source order and the IR's
    // `spellings` vector is untouched.
    let spellings = display_order(flag)
        .into_iter()
        .map(|i| spelling_text(flag, i))
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
        width += display_width(value_gap(entity)) + display_width(&v);
    }
    width
}

/// An entity's value placeholder as drawn; see [`Entity::value_text`].
pub(super) fn entity_value_text(flag: &Entity) -> Option<String> {
    flag.value_text()
}

/// The gap between the spellings and the placeholder; see
/// [`Entity::value_gap`].
pub(super) fn value_gap(flag: &Entity) -> &'static str {
    flag.value_gap()
}

/// The description text of a row, with its deprecation tag, or `None`
/// when the row has neither.
pub(super) fn row_description(flag: &Entity) -> Option<String> {
    let mut text = flag.description.as_ref().map(|d| d.single_line());
    if flag.deprecated.is_some() {
        text = Some(match text {
            Some(d) => format!("{d} (deprecated)"),
            None => "(deprecated)".to_string(),
        });
    }
    text.filter(|d| !d.is_empty())
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
    palette: style::Palette,
    // Where the value and description columns begin, shared across the
    // whole section — see `section_layout`.
    layout: SectionLayout,
) -> Vec<Line<'static>> {
    if layout.man {
        return super::man::man_entity_lines(flag, dim, width, palette, layout);
    }
    let text_width = layout.text_width(width);
    let stack = plan(flag, layout, width);
    let name_spec = stack
        .as_ref()
        .map_or_else(|| entity_name_spec(flag), |s| s.main.clone());
    let value_text = entity_value_text(flag);

    // Two columns, chosen by the row's own shape (spec §9.3), behind the
    // section's own indent: shorts at the content edge, longs preindented
    // so every long in the section starts in the same place whether or not
    // a short precedes it.
    let head_column = if stack.is_some() {
        first_line_column(flag, layout.indent)
    } else {
        spelling_column(flag, layout.indent)
    };
    let leading = " ".repeat(head_column);
    let leading = leading.as_str();
    let spelling_style = if dim {
        style::muted(palette)
    } else {
        style::accent(palette)
    };
    // Muted, not italic: many terminals silently ignore italic (spec §9.2).
    let value_style = style::muted(palette);
    let desc_style = if dim {
        style::muted(palette)
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
        // (spec §9.3), except the argfile sigil flag (spec §4.5, `@<file>`)
        // and a value that glues by its own literal character
        // (docs/shapes.md S-116, `-Wa,<options>`).
        let gap = value_gap(flag);
        first_line_spans.push(Span::raw(gap));
        first_line_spans.push(Span::styled(v.clone(), value_style));
        prefix_width += display_width(gap) + display_width(v);
    }

    // A head wider than the pane is broken across lines here rather than
    // handed to `Paragraph`'s defensive `Wrap` (module doc), which restarts
    // at column 0 with no memory of the row's indent. Regression fixture:
    // `vgchange --alloc` rendered through a real pty (AGENTS.md §3.2).
    let mut head: Vec<Line<'static>> = Vec::new();
    if prefix_width > text_width {
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

    let description_text = row_description(flag);

    if description_text.is_none() && flag.choices.is_empty() && flag.env_var.is_none() {
        if !head.is_empty() {
            return head;
        }
        let mut lines = vec![Line::from(first_line_spans)];
        if let Some(stack) = &stack {
            let or_style = style::muted(palette);
            lines.extend(stack.rows(layout, &[], (or_style, spelling_style, desc_style)));
        }
        return lines;
    }

    // One description column for the entire section (spec §9.3). A head
    // that reaches the column keeps its own line, never truncated (spec
    // §9.1); only its first description line starts one space past it,
    // every later line is back at the column.
    let column = layout.description;
    let rest_width = text_width.saturating_sub(column).max(1);

    let mut lines = Vec::new();
    let mut remainder = description_text.clone().unwrap_or_default();
    if head.is_empty() {
        if description_text.is_some() {
            // One space past the head, or the column — whichever is
            // further right. They coincide for every row whose head fits.
            let start = column.max(prefix_width + 1);
            let first = text_width.checked_sub(start).and_then(|room| {
                // Never let the head's own line swallow a later
                // enumerated item's opening token (spec §16): cap the
                // room to the offset of the first enumerator break, if
                // any, so `wrap_description` below still gets to start
                // that item on its own line.
                let room = match enumerator_breaks(&remainder).first() {
                    Some(&cut) => room.min(display_width(&remainder[..cut])),
                    None => room,
                };
                leading_words(&remainder, room)
            });
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
    let chunks = if remainder.is_empty() {
        Vec::new()
    } else {
        wrap_description(&remainder, rest_width)
    };
    if let Some(stack) = &stack {
        let or_style = style::muted(palette);
        lines.extend(stack.rows(layout, &chunks, (or_style, spelling_style, desc_style)));
    } else {
        for chunk in chunks {
            lines.push(Line::from(Span::styled(
                format!("{indent_str}{chunk}"),
                desc_style,
            )));
        }
    }

    lines.extend(subordinate_lines(
        flag, column, text_width, palette, layout.man,
    ));

    lines
}

/// The lines under a row's description: its `values:` summary or described
/// choices, then its `env:` cross-reference (spec §9.3).
pub(super) fn subordinate_lines(
    flag: &Entity,
    column: usize,
    width: usize,
    palette: style::Palette,
    man: bool,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
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

    // A flag's own environment-variable cross-reference (spec §4.5)
    // renders the same way `values:` does: its own line, two columns past
    // the description column, never folded into the description text. See
    // docs/shapes.md S-166.
    let env_line = flag.env_var.as_ref().map(|v| format!("env: {v}"));

    // The values line sits two columns past the description column,
    // marking it subordinate rather than a second row (spec §9.3).
    if let Some(values_line) = values_line {
        let values_column = column + 2;
        let values_width = width.saturating_sub(values_column).max(1);
        let values_indent = " ".repeat(values_column);
        let values_style = style::muted(palette);
        for chunk in wrap_words(&values_line, values_width) {
            lines.push(Line::from(Span::styled(
                format!("{values_indent}{chunk}"),
                values_style,
            )));
        }
    } else if has_choice_descriptions {
        lines.extend(choice_detail_lines(flag, column, width, palette, man));
    }

    if let Some(env_line) = env_line {
        let env_column = column + 2;
        let env_width = width.saturating_sub(env_column).max(1);
        let env_indent = " ".repeat(env_column);
        let env_style = style::muted(palette);
        for chunk in wrap_words(&env_line, env_width) {
            lines.push(Line::from(Span::styled(
                format!("{env_indent}{chunk}"),
                env_style,
            )));
        }
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
    palette: style::Palette,
    man: bool,
) -> Vec<Line<'static>> {
    let values_column = column + 2;
    let values_indent = " ".repeat(values_column);
    let values_style = style::muted(palette);

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

    // A value name is written once, so one too wide for its row breaks
    // across rows instead of running past the pane.
    let push_name = |lines: &mut Vec<Line<'static>>, name: &str| {
        for chunk in wrap_words(name, choice_width) {
            lines.push(Line::from(Span::styled(
                format!("{choice_indent}{chunk}"),
                values_style,
            )));
        }
    };

    for choice in &flag.choices {
        let desc = choice
            .description
            .as_ref()
            .map(|d| d.single_line())
            .filter(|d| !d.is_empty());
        let Some(desc) = desc else {
            push_name(&mut lines, &choice.name);
            continue;
        };
        if man {
            // Man layout: the value alone on its line, its description
            // below it two columns further in.
            push_name(&mut lines, &choice.name);
            let desc_indent = " ".repeat(choice_column + 2);
            let desc_width = width.saturating_sub(choice_column + 2).max(1);
            for chunk in wrap_words(&desc, desc_width) {
                lines.push(Line::from(Span::styled(
                    format!("{desc_indent}{chunk}"),
                    values_style,
                )));
            }
            continue;
        }
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
                push_name(&mut lines, &choice.name);
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
