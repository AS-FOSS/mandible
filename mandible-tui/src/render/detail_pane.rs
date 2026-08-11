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
use crate::sanitize::{defensive_single_line, display_width};
use crate::style;
use mandible_core::{CommandNode, Flag, FlagKey, ValueKind};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Wrap};
use ratatui::Frame;
use std::collections::HashMap;
use unicode_width::UnicodeWidthChar;

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

    // The user asked to see the tool's own bytes (`t`). Checked before the
    // parsed rendering and before the degradation check below, because it
    // is an override of both: the whole point is to see past whatever
    // mandible decided.
    if let Some(raw) = app.raw_help_for_selected() {
        render_raw_mode(frame, inner, app, raw);
        return;
    }

    // Level 3 of spec §7 Tier B's staged degradation (batch 6 part 4): no
    // parse produced anything structurally plausible for this node, so it
    // carries the tool's own raw `--help` text instead of invented
    // structure. This is a fundamentally different rendering, not a
    // variant of the structured one below — see `render_verbatim`.
    if !node.unparsed.is_empty() {
        render_verbatim(
            frame,
            inner,
            app,
            &format!("unparsed {} showing raw --help output", app.glyphs.absent),
            node.unparsed.iter().map(|t| t.as_str().to_string()),
        );
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

/// Render the verbatim view (`t`): the tool's own `--help` output for the
/// selected node, whatever mandible made of it.
///
/// The three states are all rendered, not just the successful one. A view
/// whose purpose is "show me what you were actually given" cannot answer a
/// refused or failed probe with a blank pane, because blank is also what a
/// tool that prints nothing looks like, and telling those apart is the
/// entire reason someone pressed the key.
fn render_raw_mode(frame: &mut Frame, inner: Rect, app: &App, raw: &crate::app::RawHelp) {
    // Named from the argv actually run, never a hardcoded spelling — see
    // `RawHelp::Ready`. Only `Ready` knows it; the other two states have no
    // output to attribute, so they stay generic.
    let heading = match raw {
        crate::app::RawHelp::Ready(_, argv) => {
            format!("verbatim {} output of `{argv}`", app.glyphs.absent)
        }
        _ => format!("verbatim {} the tool's own help output", app.glyphs.absent),
    };
    match raw {
        crate::app::RawHelp::Pending => {
            render_verbatim(
                frame,
                inner,
                app,
                &heading,
                std::iter::once("running the probe…".to_string()),
            );
        }
        crate::app::RawHelp::Ready(lines, _) => {
            render_verbatim(
                frame,
                inner,
                app,
                &heading,
                lines.iter().map(|t| t.as_str().to_string()),
            );
        }
        crate::app::RawHelp::Failed(reason) => {
            render_verbatim(frame, inner, app, &heading, std::iter::once(reason.clone()));
        }
    }
}

/// Render preformatted text under a muted heading: the tool's own bytes,
/// never re-flowed.
///
/// Shared by the verbatim view (`t`) and by level-3 degradation, which want
/// the same treatment for the same reason and differ only in their label.
///
/// Originally written for a node whose parse degraded to level 3 (spec §7
/// Tier B step 3, batch 6 part 4): `node.unparsed`, one preformatted line
/// per entry, labelled so it reads as "the author's own text", not a
/// mandible parse.
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
fn render_verbatim(
    frame: &mut Frame,
    inner: Rect,
    app: &App,
    heading: &str,
    body: impl Iterator<Item = String>,
) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(Span::styled(
        heading.to_string(),
        style::muted_bold(app.color_enabled),
    )));
    lines.push(Line::default());
    for text in body {
        lines.push(Line::from(text));
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
        for chunk in wrap_words(summary.as_str(), width) {
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
            for chunk in wrap_words(paragraph_text, width) {
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
            for chunk in wrap_words(&full, avail) {
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
/// tool's own command path — `tar --help` yields `Usage: tar [OPTION...]`,
/// and prepending the node name to that produced `tar Usage: tar
/// [OPTION...]`, with the name twice and a label the `USAGE` heading
/// directly above already supplies.
///
/// The old guard only checked the usage text's *first* word, which is why
/// `docker import --help`'s `Usage:  docker import [OPTIONS] file|URL|-
/// [REPOSITORY[:TAG]]` rendered as `import docker import [OPTIONS]
/// file|URL|- [REPOSITORY[:TAG]]`: cobra prints the *full* command path
/// (`docker import`), not just the leaf name, so the first word is
/// `docker` and the check missed. `smokecli columns outlier` (argparse,
/// which does the same thing) has the identical shape: `usage: smokecli
/// columns outlier [-h] ...`.
///
/// So the check now scans the whole run of bare, word-shaped tokens at the
/// front of the usage text — stopping at the first token that looks like
/// an option or placeholder (`-...`, `[...`, `<...`, or a bare ALL-CAPS
/// metavar like `FILE`) — and prepends the name only when it is absent
/// from that whole run, not just its first entry. That run *is* the
/// tool's own command-path prefix; if the node's name shows up anywhere in
/// it the line already names the command. Tools that print no command name
/// at all still work: `Usage: [OPTIONS] FILE` has an empty leading run (the
/// very first token is a placeholder), so nothing is found there and the
/// name still gets prepended — which is what keeps a bare pattern like
/// `[OPTIONS] <url>` a complete, copy-pasteable invocation.
fn usage_signature(node_name: &str, usage: &str) -> String {
    let name = defensive_single_line(node_name);
    let mut text = defensive_single_line(usage);

    // Drop a leading `usage:` label, case-insensitively — the heading says
    // it.
    let trimmed = text.trim_start();
    if trimmed.len() >= 6 && trimmed[..6].eq_ignore_ascii_case("usage:") {
        text = trimmed[6..].trim_start().to_string();
    }

    if name.is_empty() || usage_names_the_node(&text, &name) {
        text
    } else {
        format!("{name} {text}")
    }
}

/// Whether `name` already appears among `text`'s leading run of bare
/// command-path words — see [`usage_signature`] for why the search covers
/// the whole run rather than only the first token.
fn usage_names_the_node(text: &str, name: &str) -> bool {
    text.split_whitespace()
        .take_while(|word| !looks_like_option_or_placeholder(word))
        .any(|word| word == name)
}

/// A token that ends a usage line's leading command-path run: an option
/// (`-v`, `--verbose`), a bracketed/angled placeholder (`[OPTIONS]`,
/// `<url>`), or a bare ALL-CAPS metavar (`FILE`, `URL`) — docopt-style
/// convention for "this is a slot to fill in", never a literal word of the
/// command path.
fn looks_like_option_or_placeholder(word: &str) -> bool {
    if word.starts_with(['-', '[', '<']) {
        return true;
    }
    let has_letter = word.chars().any(|c| c.is_alphabetic());
    has_letter && !word.chars().any(|c| c.is_lowercase())
}

/// Greedy word-wrap of `text` to at most `width` display columns per
/// line, never breaking a word unless it alone exceeds `width` — in which
/// case it is broken across as many lines as it takes (see
/// [`break_overlong_word`]) rather than truncated. A token that is lost
/// once truncated is unrecoverable from the parsed view: `smokecli
/// unbreakable url` prints a ~150-character URL that used to render as
/// `https://registry.example.com/v2/org…` in a 46-column pane, with
/// everything past `/v2/org` gone. Always returns at least one (possibly
/// empty) chunk.
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
            lines.extend(break_overlong_word(word, width));
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

/// Break a single token wider than `width` display columns into as many
/// width-limited chunks as it takes, so the token survives intact across
/// multiple lines instead of being lost to an ellipsis truncation.
///
/// Splits are placed between characters, chosen by summing each
/// character's [`unicode_width`] — never by byte index (a raw byte offset
/// can land mid-character and panic, the exact failure AGENTS.md's
/// byte-slicing rule documents for parsed tool output) and never by
/// `char` count (a `char`-count split can put a double-width CJK or emoji
/// character right at the boundary and let it overflow the line by one
/// cell, the same border-overflow failure display-width truncation exists
/// to prevent elsewhere in this pane). A lone character wider than `width`
/// itself (a 2-wide emoji in a 1-column budget) still cannot be split —
/// it gets its own chunk and that chunk is allowed to exceed `width` by
/// the unavoidable minimum, since no cut point inside a character exists.
fn break_overlong_word(word: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for c in word.chars() {
        let c_width = UnicodeWidthChar::width(c).unwrap_or(0);
        if current_width + c_width > width && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
            current_width = 0;
        }
        current.push(c);
        current_width += c_width;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
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
/// A spelling wider than this fraction of the pane does not get to set the
/// shared column — it hangs instead (see [`FlagLayout::Table`]). One
/// 40-character flag name in a list of short ones used to push every
/// description in the list against the right-hand edge. Mirrors the tree
/// pane's summary-column rule (spec §9.1).
const DESC_COLUMN_CAP_PERCENT: usize = 45;

/// Prose narrower than this reads as a shredded column rather than a
/// sentence, so a table that cannot leave this much room becomes a
/// [`FlagLayout::Stacked`] list instead.
///
/// Measured against real output rather than picked: at 20 columns
/// `docker pull`'s `--platform` description breaks as "Set / platform /
/// if server / is / multi-pla… / capable" — six lines, one of them
/// truncated mid-word, for six words of text. 28 is the point either side
/// of which the table and the stacked list swap places on legibility.
const MIN_DESC_WIDTH: usize = 28;

/// Leading indent for every flag row, and (in stacked mode) the extra
/// indent that subordinates a description to the spelling above it.
const FLAG_INDENT: &str = "  ";
const STACKED_DESC_INDENT: usize = 6;

/// How a whole flag list is arranged. Chosen once for the list, never per
/// row — a per-row decision is exactly what made this ragged.
///
/// The pane is not wide enough for a three-column table at every terminal
/// size, and the previous code did not admit that. It computed one shared
/// description column, capped it at 45% of the pane, and then let any row
/// too wide for the cap start its description wherever its own text
/// happened to end. At 120 columns almost nothing exceeded the cap and the
/// table looked right; at 90 columns `docker`'s global flags rendered with
/// descriptions starting at three different columns (19, 24 and 28), which
/// is not a table at all. The cap was silently setting a target that most
/// rows then missed individually.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlagLayout {
    /// Spelling, value placeholder and description in three aligned
    /// columns.
    ///
    /// Three rather than two, because a value placeholder is a different
    /// *kind* of thing from a spelling — `--env` and `list` answer "what do
    /// I type" and "what does it take". Run together as `--env list` they
    /// read as one token; in their own columns the whole list can be
    /// scanned down either one, which is what a parameter table in API
    /// documentation is for.
    ///
    /// Both columns are invariant for the list: a row too wide for them
    /// hangs its description onto the next line rather than pushing the
    /// column right for itself alone.
    Table { value: usize, description: usize },
    /// Spelling and value on one line, description indented underneath.
    ///
    /// What every narrow-terminal help renderer falls back to, and for the
    /// same reason: it gives prose the full width of the pane and keeps a
    /// perfectly straight left edge, neither of which a table can do once
    /// the columns eat more than half the room.
    Stacked,
}

impl FlagLayout {
    /// Where descriptions begin under this layout.
    fn description_column(self) -> usize {
        match self {
            FlagLayout::Table { description, .. } => description,
            FlagLayout::Stacked => STACKED_DESC_INDENT,
        }
    }
}

/// Choose the layout for `flags` in a pane `width` columns wide.
fn flag_layout(flags: &[&Flag], width: usize) -> FlagLayout {
    let cap = width * DESC_COLUMN_CAP_PERCENT / 100;
    let lead = display_width(FLAG_INDENT);
    let gap = 2;

    // Outliers are excluded from the measurement rather than clamped. A
    // clamped column is a column the outlier still misses; an excluded one
    // is a column it can hang below while every other row stays aligned.
    let fits = |w: usize| lead + w + gap <= cap;
    let widest_spec = flags
        .iter()
        .map(|f| display_width(&flag_name_spec(f)))
        .filter(|w| fits(*w))
        .max()
        .unwrap_or(0);
    let widest_value = flags
        .iter()
        .filter_map(|f| flag_value_text(f))
        .map(|v| display_width(&v))
        .filter(|w| fits(*w))
        .max()
        .unwrap_or(0);

    let value = lead + widest_spec + gap;
    // When nothing in this list takes a value the column collapses, rather
    // than leaving a blank strip down the pane.
    let description = value
        + if widest_value == 0 {
            0
        } else {
            widest_value + gap
        };

    if width.saturating_sub(description) < MIN_DESC_WIDTH {
        return FlagLayout::Stacked;
    }
    FlagLayout::Table { value, description }
}

fn flag_lines(
    flags: &[&Flag],
    width: usize,
    color_enabled: bool,
    target_flag: Option<&FlagKey>,
    glyphs: Glyphs,
) -> (Vec<Line<'static>>, Option<usize>) {
    let layout = flag_layout(flags, width);
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
            out.extend(flag_line(f, false, width, color_enabled, layout));
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
            out.extend(flag_line(f, false, width, color_enabled, layout));
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
            out.extend(flag_line(f, true, width, color_enabled, layout));
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
        // Reconstruct the getopt_long `--[no-]foo` convention for display
        // from `negatable` — the IR's `long` is always the base name
        // (never `[no-]foo`/`no-foo`), so this is the one place that
        // spelling comes back together.
        if flag.negatable {
            spec.push_str("[no-]");
        }
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
    // Where the value and description columns begin, shared across the
    // whole flag list — see `flag_columns`.
    layout: FlagLayout,
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
        // than sitting wherever each spelling happens to end. In stacked
        // mode there is no column to reach, so a single space separates
        // them — the description below is what carries the alignment.
        let pad = match layout {
            FlagLayout::Table { value, .. } => value.saturating_sub(prefix_width).max(1),
            FlagLayout::Stacked => 1,
        };
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

    // One description column for the entire list, not one per flag. That
    // is what makes a parameter list read as a table — the defining visual
    // element of API documentation — and it only holds if it is *always*
    // the same number. It previously wasn't: the column was a target, and
    // any row too wide for it silently started its description at its own
    // width instead, so a list could show three different "columns" at
    // once.
    //
    // So a row that does not fit hangs: its description starts on the next
    // line, at the shared column. The spelling is never truncated to force
    // alignment (spec §9.1's rule for the tree applies here too) and the
    // column never moves — the row costs one extra line, which is the only
    // one of the three that nothing else has to pay for.
    let gap = 2;
    let indent_width = layout.description_column();
    let hangs = prefix_width + gap > indent_width;
    let available = width.saturating_sub(indent_width).max(1);
    let chunks = wrap_words(&description_text, available);

    let mut lines = Vec::new();
    let mut chunks_iter = chunks.into_iter();
    if !hangs {
        if let Some(first_chunk) = chunks_iter.next() {
            first_line_spans.push(Span::raw(" ".repeat(indent_width - prefix_width)));
            first_line_spans.push(Span::styled(first_chunk, desc_style));
        }
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
    // A node rendered verbatim is not a bad parse — it is the designed
    // honest fallback (spec §7 Tier B step 3), it carries confidence 0.0
    // by construction, and the pane already says so in its own words. Every
    // `git` subcommand lands here, because `git clone --help` renders
    // GIT-CLONE(1) and the man-page guard correctly refuses to mine roff
    // prose for structure. Reporting that as "0% parsed" made a deliberate
    // outcome read as a failure on every node of the tool.
    if !node.unparsed.is_empty() {
        return None;
    }
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

    /// `docker --help`'s global flags, which is the list the alignment
    /// actually broke on. The test above uses three short synthetic flags
    /// at one comfortable width, and that is exactly why it kept passing
    /// while real panes rendered ragged: nothing in it was wide enough to
    /// exceed the column cap, so the per-row fallback never fired.
    ///
    /// Every description begins with `zzz` so its column can be located
    /// exactly rather than inferred from runs of whitespace (the value
    /// placeholder is also preceded by a run of whitespace, which is what
    /// makes the inference ambiguous).
    fn docker_global_flags() -> Vec<mandible_core::Flag> {
        let mk = |short: Option<char>, long: &str, value: Option<&str>| {
            let mut f = mandible_core::Flag::long(long, Provenance::single(Source::HelpText));
            f.short = short;
            f.value_name = value.map(|v| v.to_string());
            if value.is_some() {
                f.value_kind = ValueKind::Required;
            }
            f.description = Some(Text::sanitize(
                "zzz set the thing to the other thing and then keep going for a while",
            ));
            f
        };
        vec![
            mk(None, "config", Some("string")),
            mk(Some('c'), "context", Some("string")),
            mk(Some('D'), "debug", None),
            mk(Some('H'), "host", Some("string")),
            mk(Some('l'), "log-level", Some("string")),
            mk(None, "tls", None),
            mk(None, "tlscacert", Some("string")),
        ]
    }

    /// The column that every description line in `lines` starts at.
    fn description_columns(lines: &[Line<'static>]) -> Vec<usize> {
        lines
            .iter()
            .filter_map(|line| {
                let text = text_of(line);
                if let Some(at) = text.find("zzz") {
                    return Some(display_width(&text[..at]));
                }
                // A continuation line: prose with no spelling on it.
                let trimmed = text.trim_start();
                if trimmed.is_empty() || trimmed.starts_with('-') {
                    return None;
                }
                Some(text.len() - trimmed.len())
            })
            .collect()
    }

    /// The reported defect, at every width rather than one.
    ///
    /// A shared column is only shared if it is the same number for every
    /// row. It was not: the column was capped at 45% of the pane and any
    /// row too wide for the cap started its description at its own width
    /// instead, so `docker`'s global flags rendered descriptions at three
    /// different columns (19, 24 and 28) in a 90-column terminal — with
    /// `--log-level string` also losing the gap that separates a spelling
    /// from its value, so the two ran together as one token.
    #[test]
    fn descriptions_share_one_column_at_every_width() {
        let flags = docker_global_flags();
        let refs: Vec<&mandible_core::Flag> = flags.iter().collect();

        for width in 20..=160 {
            let (lines, _) = flag_lines(&refs, width, true, None, crate::glyphs::UNICODE);
            let starts = description_columns(&lines);
            assert!(
                !starts.is_empty(),
                "width {width}: no descriptions rendered"
            );
            let distinct: std::collections::BTreeSet<usize> = starts.iter().copied().collect();
            assert_eq!(
                distinct.len(),
                1,
                "width {width}: descriptions start at {distinct:?}, not one shared column"
            );
        }
    }

    /// Below the point where a table can leave prose a readable width, the
    /// list stacks rather than shredding descriptions into a narrow strip.
    ///
    /// At 90 columns `docker pull`'s `--platform` description used to
    /// break as "Set / platform / if server / is / multi-pla… / capable" —
    /// six lines for six words, one truncated mid-word, because the
    /// columns had eaten everything but 9 cells of the pane.
    #[test]
    fn a_narrow_pane_stacks_instead_of_shredding_prose() {
        let flags = docker_global_flags();
        let refs: Vec<&mandible_core::Flag> = flags.iter().collect();

        assert_eq!(flag_layout(&refs, 38), FlagLayout::Stacked);
        let (lines, _) = flag_lines(&refs, 38, true, None, crate::glyphs::UNICODE);
        for start in description_columns(&lines) {
            assert_eq!(start, STACKED_DESC_INDENT, "stacked prose must be flush");
        }
        // The whole point of stacking: prose gets the pane, not a strip.
        // Measured on the rendered lines rather than asserted against the
        // constants, which would only restate the arithmetic above.
        let widest_prose = lines
            .iter()
            .map(text_of)
            .filter(|t| !t.trim_start().starts_with('-'))
            .map(|t| display_width(t.trim()))
            .max()
            .unwrap_or(0);
        assert!(
            widest_prose >= MIN_DESC_WIDTH,
            "stacked prose still shredded: widest line was {widest_prose}"
        );
    }

    /// One very long spelling must not drag every other row's description
    /// against the right-hand edge — the reason a cap existed at all. It
    /// now hangs instead of widening the column, so the cap's original job
    /// is done without the raggedness it used to cause.
    #[test]
    fn one_overlong_spelling_hangs_rather_than_moving_the_column() {
        let mut flags = docker_global_flags();
        // Past the 45% cap at 120 columns, which is the point of the test.
        // A spelling that merely *looks* long is not an outlier: a 49-char
        // name at this width still leaves 59 columns for prose, and the
        // cap admits it deliberately rather than spending a line on it.
        let mut monster = mandible_core::Flag::long(
            "an-extremely-long-option-name-that-nobody-would-ever-type-by-hand",
            Provenance::single(Source::HelpText),
        );
        monster.description = Some(Text::sanitize("zzz does something"));
        flags.push(monster);
        let refs: Vec<&mandible_core::Flag> = flags.iter().collect();

        let without: Vec<&mandible_core::Flag> = refs[..refs.len() - 1].to_vec();
        assert_eq!(
            flag_layout(&refs, 120),
            flag_layout(&without, 120),
            "an outlier spelling must not set the column for the list"
        );

        let (lines, _) = flag_lines(&refs, 120, true, None, crate::glyphs::UNICODE);
        let distinct: std::collections::BTreeSet<usize> =
            description_columns(&lines).into_iter().collect();
        assert_eq!(distinct.len(), 1, "outlier broke the column: {distinct:?}");

        // ...and it hangs: its spelling occupies a line of its own.
        let joined: Vec<String> = lines.iter().map(text_of).collect();
        let row = joined
            .iter()
            .find(|l| l.contains("an-extremely-long-option-name"))
            .expect("outlier row missing");
        assert!(
            !row.contains("zzz"),
            "an over-long spelling should hang its description, not push the column: {row:?}"
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

    /// A node shown verbatim says nothing here: it is the designed
    /// fallback, not a failed parse, and the pane already labels itself
    /// `unparsed`. Every `git` subcommand is one, since `git clone --help`
    /// renders a man page.
    #[test]
    fn a_verbatim_node_gets_no_caveat() {
        let mut node = node_with_flags();
        node.provenance = Provenance::with_confidence(Source::HelpText, 0.0);
        node.unparsed = vec![Text::sanitize("GIT-CLONE(1) Git Manual GIT-CLONE(1)")];
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
            FlagLayout::Table {
                value: 18,
                description: 20,
            },
        );
        assert!(lines.len() >= 2, "expected wrapping: {lines:?}");
        let first_text = text_of(&lines[0]);
        // Every description line — the first as well as the continuations
        // — sits at the column the list agreed on, never at column 0 and
        // never at this row's own width.
        //
        // This row's spelling plus value runs to 24, past the column, so
        // it hangs: line 0 is the spelling alone and the description
        // starts on line 1. The earlier assertion here demanded the
        // continuation clear *this row's* prefix, which is precisely the
        // per-row indent that made a list of flags render with three
        // different "columns" at once.
        for line in &lines[1..] {
            let text = text_of(line);
            let indent_len = text.len() - text.trim_start().len();
            assert_eq!(
                indent_len, 20,
                "first={first_text:?} line={text:?} must start at the shared column"
            );
        }
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
            FlagLayout::Table {
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
            FlagLayout::Table {
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

    /// Render the whole frame in each of the verbatim view's three states.
    ///
    /// The state machine for `t` is unit-tested in `app`, but that proves
    /// only that the right variant is *selected*; this proves it reaches
    /// the screen. The `Failed` case matters most: a refusal that rendered
    /// as an empty pane would be indistinguishable from a tool that prints
    /// nothing, which is the exact confusion the view exists to remove.
    #[test]
    fn raw_mode_renders_each_state_to_the_screen() {
        use crate::app::{App, RawHelp};
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        fn screen(app: &App) -> String {
            let backend = TestBackend::new(80, 24);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| {
                    let area = frame.area();
                    render(frame, area, app);
                })
                .unwrap();
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect::<Vec<_>>()
                .join("")
        }

        // A node that parses perfectly well, so anything verbatim on
        // screen can only have come from the raw path overriding it.
        let mut root = CommandNode::new("tool", Provenance::single(Source::HelpText));
        let mut flag = Flag::long("verbose", Provenance::single(Source::HelpText));
        flag.description = Some(Text::sanitize("PARSED-FLAG-DESCRIPTION"));
        root.flags.push(flag);
        let mut app = App::new("tool".to_string(), root);
        let path = vec!["tool".to_string()];

        let parsed = screen(&app);
        assert!(parsed.contains("PARSED-FLAG-DESCRIPTION"), "{parsed}");

        app.toggle_raw_mode();

        app.mark_raw_pending(path.clone());
        let pending = screen(&app);
        assert!(pending.contains("verbatim"), "{pending}");
        assert!(pending.contains("running the probe"), "{pending}");

        app.set_raw_help(
            path.clone(),
            RawHelp::Ready(
                vec![Text::sanitize("RAW-HELP-LINE-FROM-THE-TOOL")],
                "git --help".to_string(),
            ),
        );
        let ready = screen(&app);
        assert!(ready.contains("RAW-HELP-LINE-FROM-THE-TOOL"), "{ready}");
        assert!(
            !ready.contains("PARSED-FLAG-DESCRIPTION"),
            "the parse must be replaced, not appended: {ready}"
        );

        app.set_raw_help(
            path.clone(),
            RawHelp::Failed("refused: REASON-SHOWN-TO-THE-USER".to_string()),
        );
        let failed = screen(&app);
        assert!(failed.contains("REASON-SHOWN-TO-THE-USER"), "{failed}");
        assert!(
            !failed.contains("PARSED-FLAG-DESCRIPTION"),
            "a refusal must not silently fall back to the parse: {failed}"
        );

        // And back, to prove the override is not one-way.
        app.toggle_raw_mode();
        let restored = screen(&app);
        assert!(restored.contains("PARSED-FLAG-DESCRIPTION"), "{restored}");
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

    /// The reported defect: cobra prints the *full* command path in its
    /// usage line, not just the leaf node's own name — `docker import
    /// --help` yields `Usage:  docker import [OPTIONS] file|URL|-
    /// [REPOSITORY[:TAG]]`. The old guard only checked the usage text's
    /// first word ("docker" ≠ "import"), so it prepended the leaf name
    /// anyway and produced `import docker import [OPTIONS] file|URL|-
    /// [REPOSITORY[:TAG]]` — the name doubled and the real command path
    /// pushed off the front. The correct output is the tool's own line,
    /// byte for byte.
    #[test]
    fn usage_signature_does_not_prepend_when_the_full_path_already_names_the_node() {
        assert_eq!(
            usage_signature(
                "import",
                "docker import [OPTIONS] file|URL|- [REPOSITORY[:TAG]]"
            ),
            "docker import [OPTIONS] file|URL|- [REPOSITORY[:TAG]]"
        );
        // Same shape, a second real tool (docker pull), so this isn't
        // one coincidental fixture.
        assert_eq!(
            usage_signature("pull", "docker pull [OPTIONS] NAME[:TAG|@DIGEST]"),
            "docker pull [OPTIONS] NAME[:TAG|@DIGEST]"
        );
        // argparse does the same thing, and for a node three levels deep
        // the leading run is three words wide, not one — the fix has to
        // scan the whole run, not just swap which single word it checks.
        assert_eq!(
            usage_signature("outlier", "smokecli columns outlier [-h] [-v] [-n]"),
            "smokecli columns outlier [-h] [-v] [-n]"
        );
    }

    /// The other direction, which is why the fix can't just delete the
    /// prepending: some tools print usage with no command name in it at
    /// all (`Usage: [OPTIONS] FILE`), and mandible adds the name so the
    /// line reads as a complete, copy-pasteable invocation. Here the
    /// node's name genuinely does not appear anywhere in the usage text,
    /// so it must still be prepended.
    #[test]
    fn usage_signature_still_prepends_when_the_name_is_truly_absent() {
        assert_eq!(
            usage_signature("mytool", "[OPTIONS] FILE"),
            "mytool [OPTIONS] FILE"
        );
        assert_eq!(usage_signature("cat", "<url>"), "cat <url>");
    }

    /// A single over-long token must survive wrapping intact — broken
    /// across as many lines as it takes, never truncated. Concatenating
    /// every chunk this function returns must reconstruct the original
    /// word exactly; losing a suffix here is exactly what shipped as
    /// `smokecli unbreakable url` rendering a ~150-character URL as
    /// `https://registry.example.com/v2/org…` with everything past
    /// `/v2/org` gone from the parsed view.
    #[test]
    fn wrap_words_breaks_an_overlong_token_instead_of_losing_it() {
        let url = "https://registry.example.com/v2/org/repo/blobs/uploads/deadbeefcafefeed0123456789abcdef0123456789abcdef0123456789abcd?query=value&more=stuff";
        let chunks = wrap_words(url, 20);
        assert!(chunks.len() > 1, "expected multiple chunks: {chunks:?}");
        let rejoined: String = chunks.concat();
        assert_eq!(rejoined, url, "the token must survive intact");
        for chunk in &chunks {
            assert!(
                display_width(chunk) <= 20,
                "chunk exceeds the budget: {chunk:?}"
            );
        }
        // Nothing here is a hard-truncation ellipsis marker.
        assert!(!rejoined.contains('…'));
    }

    /// [`break_overlong_word`] must split only at character boundaries —
    /// never mid-character — even when the word is wide/emoji text, so
    /// display-width accounting (not byte or `char` count) is what decides
    /// where a line ends.
    #[test]
    fn break_overlong_word_never_splits_a_multibyte_character() {
        // Each CJK character is 2 columns wide; a budget of 3 must place
        // exactly one character per chunk; the whole string must survive.
        let word = "日本語文字列長い";
        let chunks = break_overlong_word(word, 3);
        let rejoined: String = chunks.concat();
        assert_eq!(rejoined, word);
        for chunk in &chunks {
            // Every chunk parses as valid UTF-8 chars by construction
            // (`String` guarantees it), so the real assertion is the
            // width budget: no chunk may smuggle a whole extra character
            // past it.
            assert!(display_width(chunk) <= 3, "chunk too wide: {chunk:?}");
        }
    }

    /// The end-to-end path for the reported repro: a node whose `usage`
    /// carries an over-long token must still show the whole token
    /// somewhere in the rendered lines, and never emit an ellipsis in its
    /// place.
    #[test]
    fn build_lines_wraps_rather_than_truncates_a_long_usage_token() {
        let mut node = CommandNode::new("url", Provenance::single(Source::HelpText));
        let long_url = "https://registry.example.com/v2/org/repo/blobs/uploads/deadbeefcafefeed0123456789abcdef0123456789abcdef0123456789abcd";
        node.usage = vec![Text::sanitize(long_url)];

        let built = build_lines(&node, false, 46, true, None, crate::glyphs::UNICODE);
        // Every usage line carries its own 2-space block indent (see the
        // USAGE section of `build_lines`) — strip it per line before
        // rejoining so adjacent chunks of the broken token reassemble
        // without a spurious gap between them.
        let joined: String = built
            .lines
            .iter()
            .map(text_of)
            .map(|t| t.trim_start().to_string())
            .collect();
        // The chunks concatenate back to the original token exactly, so
        // the whole URL — not just a fragment of it — must appear intact
        // somewhere in the rendered output.
        assert!(
            joined.contains(long_url),
            "token was lost, not wrapped: {joined:?}"
        );
        assert!(
            !joined.contains('…'),
            "an over-long token must never be ellipsis-truncated: {joined:?}"
        );
    }
}
