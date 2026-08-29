//! Spec §9.3: the detail pane is one scrollable document of sections.
//!
//! Rendered through the whole frame (`TestBackend`) rather than through
//! `build_lines`, because the properties under test are properties of what
//! reaches the screen: which sections appear, in what order, what their
//! headers say, and whether a divider actually runs to the pane's edge.
//! A section that builds correctly and is then clipped by the border, or a
//! rule that stops two cells short, is invisible to a line-level test.
//!
//! `MODIFIERS` and `ENVIRONMENT` have no producer yet — no extraction tier
//! emits `EntityKind::Modifier` or `EntityKind::EnvVar`. They are rendered
//! by the same kind-keyed loop as the two kinds that do have producers, and
//! the entities here are constructed directly to prove it: the day a parser
//! emits one, the section is already on screen.

use mandible_core::{
    CommandNode, Entity, EntityKind, Provenance, Source, Spelling, Text, ValueKind,
};
use mandible_tui::app::App;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// The detail pane's content area, one string per row, trailing space
/// trimmed.
///
/// Strips the border *and* the pane's one-column padding either side, so
/// column 0 here is column 0 of the pane's own layout — the coordinate
/// every width and indent in `detail_pane.rs` is computed in.
fn detail_rows(app: &App, width: u16, height: u16) -> Vec<String> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| mandible_tui::render::render(frame, app))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let regions =
        mandible_tui::layout::compute(ratatui::layout::Rect::new(0, 0, width, height), app.focus);
    let rect = regions.detail.expect("wide enough for a detail pane");

    let mut rows = Vec::new();
    for y in (rect.y + 1)..(rect.y + rect.height - 1) {
        let mut line = String::new();
        for x in (rect.x + 2)..(rect.x + rect.width - 2) {
            line.push_str(buffer[(x, y)].symbol());
        }
        rows.push(line.trim_end().to_string());
    }
    rows
}

fn entity(kind: EntityKind, spelling: Spelling, description: &str) -> Entity {
    let mut e = Entity::new(kind, Provenance::single(Source::HelpText));
    e.spellings.push(spelling);
    e.description = Some(Text::sanitize(description));
    e
}

/// A node carrying every one of spec §9.3's six sections, deliberately
/// **not** in the order they must render in: the entity vector is built
/// environment-variables-first, so an order assertion cannot pass by the
/// pane happening to echo its input.
fn node_with_every_section() -> CommandNode {
    let mut node = CommandNode::new("tool", Provenance::single(Source::HelpText));
    node.description = Some(Text::sanitize("Does a thing to some other thing."));
    node.usage = vec![Text::sanitize("tool [OPTIONS] <target>")];
    node.entities = vec![
        entity(
            EntityKind::EnvVar,
            Spelling::bare("TOOL_CONFIG"),
            "path to the configuration file",
        ),
        entity(
            EntityKind::Modifier,
            Spelling::bare("d"),
            "delete members from the archive",
        ),
        entity(
            EntityKind::Flag,
            Spelling::long("verbose"),
            "explain what is being done",
        ),
        entity(
            EntityKind::Positional,
            Spelling::bare("target"),
            "the thing to operate on",
        ),
    ];
    node
}

fn app_for(node: CommandNode) -> App {
    let mut app = App::new("tool".to_string(), node);
    app.focus = mandible_tui::Focus::Detail;
    app
}

/// Every section renders, in spec §9.3's order, from a node whose entities
/// are stored in a different one.
#[test]
fn sections_render_in_the_specified_order() {
    let rows = detail_rows(&app_for(node_with_every_section()), 90, 30);
    let joined = rows.join("\n");
    let mut seen = Vec::new();
    for row in &rows {
        for section in [
            "DESCRIPTION",
            "USAGE",
            "POSITIONALS",
            "FLAGS",
            "MODIFIERS",
            "ENVIRONMENT",
        ] {
            if row.starts_with(section) {
                seen.push(section);
            }
        }
    }
    assert_eq!(
        seen,
        vec![
            "DESCRIPTION",
            "USAGE",
            "POSITIONALS",
            "FLAGS",
            "MODIFIERS",
            "ENVIRONMENT"
        ],
        "sections out of order:\n{joined}"
    );
}

/// The two kinds no parser emits yet reach the screen with their content,
/// not just their headers — driven purely by `EntityKind`.
#[test]
fn modifiers_and_environment_render_from_constructed_entities() {
    let rows = detail_rows(&app_for(node_with_every_section()), 90, 30);
    let joined = rows.join("\n");
    // Read across the wrap. Whether a given description fits on its row is
    // the layout's decision and changes with the pane's own arithmetic;
    // what this test is about is that the content reaches the screen at
    // all, so it looks for the words rather than for the line breaks.
    let flat = rows
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    for expected in [
        "MODIFIERS (1)",
        "delete members from the archive",
        "ENVIRONMENT (1)",
        "TOOL_CONFIG",
        "path to the configuration file",
        "POSITIONALS (1)",
        "the thing to operate on",
    ] {
        assert!(flat.contains(expected), "missing {expected:?}:\n{joined}");
    }
}

/// Spec §9.3: an empty section does not render. A node with only a
/// description and flags looks exactly as it did before sections existed —
/// no heading over blank space, and nothing at all for the four kinds it
/// has no entities of.
#[test]
fn empty_sections_do_not_render() {
    let mut node = CommandNode::new("tool", Provenance::single(Source::HelpText));
    node.description = Some(Text::sanitize("Does a thing."));
    node.entities.push(entity(
        EntityKind::Flag,
        Spelling::long("verbose"),
        "explain what is being done",
    ));
    let rows = detail_rows(&app_for(node), 90, 30);
    let joined = rows.join("\n");
    assert!(joined.contains("DESCRIPTION"), "{joined}");
    assert!(joined.contains("FLAGS (1)"), "{joined}");
    for absent in ["USAGE", "POSITIONALS", "MODIFIERS", "ENVIRONMENT"] {
        assert!(
            !joined.contains(absent),
            "an empty section rendered anyway: {absent}\n{joined}"
        );
    }
}

/// Spec §9.3: POSITIONALS rows are inset to the long-flag column, and the
/// flag-shaped sections stay flush against the content edge.
///
/// Compared between sections rather than against a number: the positional,
/// the modifier and the environment variable in this fixture are all bare
/// names of the same shape, so the only thing that can put them in
/// different columns is the section each is in. Read off the frame, since
/// the inset only exists to be seen.
///
/// Supersedes the pin that allowed any inset of four columns or fewer.
/// That bound admitted three different layouts and could not distinguish
/// the one the design asks for: the positional's name must land on exactly
/// the column `--verbose` lands on in FLAGS, so the reader follows one
/// edge down the whole document. `<= 4` passes with the positionals one
/// column shy of that edge, which is the failure worth catching — a near
/// miss reads worse than a plain inset, because two almost-aligned columns
/// look like a mistake rather than a choice.
#[test]
fn positionals_are_inset_to_the_long_column_and_the_flag_sections_stay_flush() {
    let rows = detail_rows(&app_for(node_with_every_section()), 90, 30);
    let joined = rows.join("\n");
    let column_of = |needle: &str| {
        let row = rows
            .iter()
            .find(|r| r.trim_start().starts_with(needle))
            .unwrap_or_else(|| panic!("no row for {needle}:\n{joined}"));
        row.len() - row.trim_start().len()
    };

    let positional = column_of("target");
    assert!(
        positional > 0,
        "a bare positional name sits flush against the pane border:\n{joined}"
    );
    for flush in ["d ", "TOOL_CONFIG"] {
        assert_eq!(
            column_of(flush),
            0,
            "{flush:?} left the content edge the tight sections align on:\n{joined}"
        );
    }
    // The one number in the assertion is read off the page, not written
    // down here: `--verbose` is a long with no short in front of it, so
    // FLAGS preindents it to the long column, and that is the column the
    // positionals must share.
    assert_eq!(
        positional,
        column_of("--verbose"),
        "a positional must start where a preindented long starts:\n{joined}"
    );
}

/// Counts are the section's own entity count, and they follow what is
/// actually rendered rather than what the node holds: a hidden flag is
/// suppressed by default (spec §9), so counting it would advertise rows
/// the reader cannot see.
#[test]
fn section_headers_carry_the_count_of_what_they_render() {
    let mut node = CommandNode::new("tool", Provenance::single(Source::HelpText));
    for i in 0..3 {
        node.entities.push(entity(
            EntityKind::Flag,
            Spelling::long(format!("flag-{i}")),
            "does something",
        ));
    }
    let mut hidden = entity(EntityKind::Flag, Spelling::long("secret"), "internal");
    hidden.hidden = true;
    node.entities.push(hidden);

    let joined = detail_rows(&app_for(node), 90, 30).join("\n");
    assert!(
        joined.contains("FLAGS (3)"),
        "the count must exclude the hidden flag:\n{joined}"
    );
}

/// Spec §9.3: exactly one blank row sits between a section's last row and
/// the next section's header — never none, never two, and never one above
/// the first header on the page.
///
/// The fixture is built so the boundaries differ in every way a boundary
/// can: DESCRIPTION ends on a wrapped paragraph after a paragraph break,
/// USAGE on a verbatim synopsis line, POSITIONALS on a wrapped
/// description, and FLAGS on a group. Each of those used to contribute its
/// own trailing blank independently of the next section's, which is what
/// let the page's rhythm change with the content.
///
/// Read off the frame, because uneven spacing down a page is a failure a
/// reader sees and nothing else reports.
#[test]
fn exactly_one_blank_row_separates_every_section() {
    let sections = [
        "DESCRIPTION",
        "USAGE",
        "POSITIONALS",
        "FLAGS",
        "MODIFIERS",
        "ENVIRONMENT",
    ];
    let mut node = CommandNode::new("tool", Provenance::single(Source::HelpText));
    node.description = Some(Text::sanitize(
        "A first paragraph.\n\nA second paragraph, long enough that it has to wrap \
         onto a further line before the section ends, so the boundary beneath it \
         follows a continuation row rather than a heading.",
    ));
    node.usage = vec![Text::sanitize("tool [OPTIONS] <target>")];
    node.entities.push(entity(
        EntityKind::Positional,
        Spelling::bare("target"),
        "the thing to operate on, described at enough length that this row wraps \
         and the section therefore ends on a continuation line",
    ));
    node.entities.push(entity(
        EntityKind::Flag,
        Spelling::long("verbose"),
        "explain what is being done",
    ));
    for name in ["create", "extract"] {
        let mut e = entity(
            EntityKind::Flag,
            Spelling::long(name),
            "one of the things this tool does",
        );
        e.group = Some("Operation:".to_string());
        node.entities.push(e);
    }
    node.entities.push(entity(
        EntityKind::Modifier,
        Spelling::bare("d"),
        "delete members from the archive",
    ));
    node.entities.push(entity(
        EntityKind::EnvVar,
        Spelling::bare("TOOL_CONFIG"),
        "path to the configuration file",
    ));

    let rows = detail_rows(&app_for(node), 120, 44);
    let joined = rows.join("\n");
    let headers: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| sections.iter().any(|s| r.starts_with(s)))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        headers.len(),
        sections.len(),
        "every section must be on screen for the boundaries to be readable:\n{joined}"
    );

    // The fixture is only worth reading if its sections really do end the
    // several different ways the doc comment claims — a boundary after a
    // one-line section proves nothing about one after a wrapped row.
    let wrapped = headers.windows(2).filter(|w| w[1] - w[0] > 3).count();
    assert!(
        wrapped >= 2,
        "the fixture must have sections that wrap, or the boundaries are all alike:\n{joined}"
    );

    assert_eq!(
        headers[0], 0,
        "the first section starts at the top, with no blank above it:\n{joined}"
    );
    for &at in &headers[1..] {
        assert!(
            rows[at - 1].is_empty(),
            "no blank row above {:?}:\n{joined}",
            rows[at]
        );
        assert!(
            !rows[at - 2].is_empty(),
            "two blank rows above {:?}:\n{joined}",
            rows[at]
        );
    }
}

/// Spec §9.3 and §9.2: a section header and a group divider must stay
/// distinguishable with every attribute stripped, because several
/// terminals ignore dimming. `TestBackend`'s symbol grid *is* that
/// terminal — it carries no styling at all — so this reads the two shapes
/// exactly as such a terminal would.
///
/// Both are label-first at column 0 with a rule running to the pane's
/// edge; the header is CAPS with a count and the divider mixed case
/// without one. The rows beneath the divider sit at the section's normal
/// margin — grouping is drawn, not indented.
///
/// An ungrouped flag leads the section so that neither divider is the
/// section's own first row: a divider in that position drops its rule
/// (spec §9.3), which is a different shape with its own test below. What
/// this one is about is the ruled divider, so the fixture puts one where a
/// ruled divider belongs.
#[test]
fn a_group_divider_is_shaped_differently_from_a_section_header() {
    let mut node = CommandNode::new("tar", Provenance::single(Source::HelpText));
    node.entities.push(entity(
        EntityKind::Flag,
        Spelling::long("verbose"),
        "verbosely list files processed",
    ));
    for (group, name) in [
        ("Main operation mode:", "create"),
        ("Main operation mode:", "extract"),
        ("DEVICE SELECTION AND SWITCHING:", "file"),
    ] {
        let mut e = entity(
            EntityKind::Flag,
            Spelling::long(name),
            "does one of the things tar does",
        );
        e.group = Some(group.to_string());
        node.entities.push(e);
    }
    let rows = detail_rows(&app_for(node), 90, 30);
    let joined = rows.join("\n");

    let header = rows
        .iter()
        .find(|r| r.starts_with("FLAGS"))
        .unwrap_or_else(|| panic!("no FLAGS header:\n{joined}"));
    assert!(header.starts_with("FLAGS (4) "), "{header:?}");

    // The tool shouted one heading and set the other in its own casing;
    // both render as dividers, and neither reads as a section header.
    for label in ["Main operation mode", "Device selection and switching"] {
        let divider = rows
            .iter()
            .find(|r| r.contains(label))
            .unwrap_or_else(|| panic!("no divider for {label:?}:\n{joined}"));
        assert!(
            divider.starts_with(&format!("{label} ─")),
            "a group divider leads with its label, then the rule: {divider:?}"
        );
        assert!(
            !divider.contains('('),
            "a group divider carries no count: {divider:?}"
        );
        assert!(
            divider.ends_with('─'),
            "a group divider runs to the pane's edge: {divider:?}"
        );
    }

    // The rows beneath a divider are at the section's own margin — the
    // same column an ungrouped row of the same shape gets, not an extra
    // level in. Asserted against that ungrouped row rather than against a
    // literal, because the number is the layout's business (spec §9.3)
    // and this test's claim is only that grouping does not change it.
    let indent = |needle: &str| {
        let row = rows
            .iter()
            .find(|r| r.contains(needle))
            .unwrap_or_else(|| panic!("no {needle} row:\n{joined}"));
        row.len() - row.trim_start().len()
    };
    assert_eq!(
        indent("--create"),
        indent("--verbose"),
        "grouping must cost no width:\n{joined}"
    );
}

/// Spec §9.3: a group divider that **opens** its section renders its label
/// alone at column 0, with no rule at all.
///
/// A section header ends in a full-width rule. A ruled divider on the very
/// next line puts a second full-width rule directly beneath it, and the
/// pair reads as one doubled line rather than as a boundary and a
/// subdivision of it. Read off the symbol grid, which carries no styling —
/// the same terminal spec §9.2 writes the shape rules for.
#[test]
fn a_divider_that_opens_its_section_drops_its_rule() {
    let mut node = CommandNode::new("tar", Provenance::single(Source::HelpText));
    for (group, name) in [
        ("Main operation mode:", "create"),
        ("Main operation mode:", "extract"),
        ("DEVICE SELECTION AND SWITCHING:", "file"),
    ] {
        let mut e = entity(
            EntityKind::Flag,
            Spelling::long(name),
            "does one of the things tar does",
        );
        e.group = Some(group.to_string());
        node.entities.push(e);
    }
    let rows = detail_rows(&app_for(node), 90, 30);
    let joined = rows.join("\n");

    let index = |needle: &str| {
        rows.iter()
            .position(|r| r.contains(needle))
            .unwrap_or_else(|| panic!("no row for {needle:?}:\n{joined}"))
    };
    let header = index("FLAGS (3)");
    let first = index("Main operation mode");
    let second = index("Device selection and switching");

    assert_eq!(
        first,
        header + 1,
        "the first divider must sit directly under the header:\n{joined}"
    );
    assert_eq!(
        rows[first], "Main operation mode",
        "a divider opening its section is its label alone:\n{joined}"
    );
    assert!(
        !rows[first].contains('─'),
        "an opening divider draws no rule at all:\n{joined}"
    );
    // ...and the header above it is still ruled, so what was removed is
    // the *second* rule of the pair, not the boundary itself.
    assert!(
        rows[header].ends_with('─'),
        "the section header keeps its rule:\n{joined}"
    );
    // A later divider in the same section still separates one run of rows
    // from another, and keeps its rule.
    assert!(
        rows[second].starts_with("Device selection and switching ─") && rows[second].ends_with('─'),
        "a later divider stays ruled: {:?}\n{joined}",
        rows[second]
    );
}

/// The same frame with the ASCII glyph set: a divider degrades to `-`
/// like every other rule in the pane, and the shape that carries its
/// meaning survives (spec §9.2 — a glyph may only be used if there is
/// something legible to fall back to).
#[test]
fn a_group_divider_degrades_to_ascii() {
    let mut node = CommandNode::new("tar", Provenance::single(Source::HelpText));
    // Ungrouped first, so the divider under test is a ruled one rather
    // than the rule-less divider that opens a section (spec §9.3).
    node.entities.push(entity(
        EntityKind::Flag,
        Spelling::long("verbose"),
        "verbosely list files processed",
    ));
    let mut e = entity(
        EntityKind::Flag,
        Spelling::long("create"),
        "create a new archive",
    );
    e.group = Some("Main operation mode:".to_string());
    node.entities.push(e);

    let mut app = app_for(node);
    app.glyphs = mandible_tui::glyphs::ASCII;
    let rows = detail_rows(&app, 90, 30);
    let joined = rows.join("\n");
    let divider = rows
        .iter()
        .find(|r| r.contains("Main operation mode"))
        .unwrap_or_else(|| panic!("no divider:\n{joined}"));
    assert!(divider.starts_with("Main operation mode -"), "{divider:?}");
    assert!(
        divider.is_ascii(),
        "the ASCII frame must stay ASCII: {divider:?}"
    );
}

/// Spec §9.3's capped column, on screen: the majority of a section's
/// entities share one description column, and an entity whose head runs
/// past that column starts its own first description line one space past
/// its head, with everything else in the section — its own continuation
/// lines included — still on the column.
///
/// This supersedes the hanging-indent pin: the rule it checked (a wide row
/// must not drag the column right for everyone, and must not invent a
/// column of its own) is the same one, but the place the exception lands
/// is now defined by the row's own head rather than by a second fixed
/// number. Both halves are read off the rendered frame, since the column
/// that matters is the one a reader sees.
#[test]
fn a_wide_entity_pushes_only_its_own_first_line() {
    let mut node = CommandNode::new("tool", Provenance::single(Source::HelpText));
    for i in 0..9 {
        node.entities.push(entity(
            EntityKind::Flag,
            Spelling::long(format!("opt-{i}")),
            "zzz zzz zzz",
        ));
    }
    let mut wide = entity(
        EntityKind::Flag,
        Spelling::long("an-extremely-long-option-name-nobody-would-type"),
        // Every word is the marker, so a continuation line can be located
        // as exactly as a first one.
        "zzz zzz zzz zzz zzz zzz zzz zzz",
    );
    wide.value_kind = ValueKind::None;
    node.entities.push(wide);

    // Wide enough that the nine ordinary rows have room to sit beside
    // their descriptions, so the outlier is the only thing under test.
    let rows = detail_rows(&app_for(node), 140, 30);
    let joined = rows.join("\n");

    // Every description line on screen, split by whether it shares its row
    // with a head.
    let mut beside_a_head: Vec<(usize, String)> = Vec::new();
    let mut continuations: Vec<usize> = Vec::new();
    for row in &rows {
        let Some(at) = row.find("zzz") else { continue };
        if row.trim_start().starts_with("zzz") {
            continuations.push(at);
        } else {
            beside_a_head.push((at, row[..at].trim_end().to_string()));
        }
    }

    // Nine rows on the column, and one past it.
    let mut columns: Vec<usize> = beside_a_head.iter().map(|(at, _)| *at).collect();
    columns.sort_unstable();
    let shared = columns[0];
    assert_eq!(
        columns.iter().filter(|c| **c == shared).count(),
        9,
        "nine rows share the column:\n{joined}"
    );
    let (pushed, head) = beside_a_head
        .iter()
        .find(|(at, _)| *at != shared)
        .unwrap_or_else(|| panic!("the outlier must be pushed past the column:\n{joined}"));
    assert!(
        head.contains("an-extremely-long-option-name"),
        "only the outlier may miss the column: {head:?}\n{joined}"
    );
    assert_eq!(
        *pushed,
        head.chars().count() + 1,
        "a pushed description starts one space past its own head:\n{joined}"
    );

    // ...and nothing else in the section left the column, the outlier's
    // own wrapped tail included.
    assert!(
        !continuations.is_empty(),
        "the fixture must wrap, or continuation lines prove nothing:\n{joined}"
    );
    assert!(
        continuations.iter().all(|c| *c == shared),
        "a continuation line left the column: {continuations:?}\n{joined}"
    );
}

/// A value placeholder wider than the whole pane is broken across lines
/// by this module, not left for `Paragraph`'s defensive `Wrap` to reflow.
///
/// The real shape, from `vgchange --alloc`: one 55-column token,
/// `contiguous|cling|cling_by_tags|normal|anywhere|inherit`, against a
/// detail pane around 41 columns wide. `Wrap` restarted it at column 0
/// with no memory of the row's indent, so the placeholder rendered flush
/// against the pane's left edge two rows below the spelling it belongs to.
/// Found through a real pty (AGENTS.md §3.2) — no corpus fixture had a
/// placeholder that wide, and every line-level test passed throughout.
#[test]
fn a_value_placeholder_wider_than_the_pane_is_wrapped_not_reflowed() {
    let mut node = CommandNode::new("vgchange", Provenance::single(Source::HelpText));
    let mut flag = entity(
        EntityKind::Flag,
        Spelling::long("alloc"),
        "set the allocation policy",
    );
    flag.value_name = Some("contiguous|cling|cling_by_tags|normal|anywhere|inherit".to_string());
    flag.value_kind = ValueKind::Required;
    node.entities.push(flag);
    node.entities.push(entity(
        EntityKind::Flag,
        Spelling::long("uuid"),
        "generate a new uuid",
    ));

    let rows = detail_rows(&app_for(node), 90, 30);
    let joined = rows.join("\n");
    let value_rows: Vec<&String> = rows.iter().filter(|r| r.contains("contiguous")).collect();
    assert_eq!(
        value_rows.len(),
        1,
        "the placeholder should start on one row:\n{joined}"
    );
    // Every piece of the placeholder keeps one indent, and that indent is
    // not column 0 — which is the whole failure: `Wrap` restarted the
    // token flush against the pane's left edge with no memory of the row
    // it belonged to. The indent's *value* is the layout's business (spec
    // §9.3) and is not restated here; that it is one number, and not zero,
    // is what this test is for.
    let indents: Vec<usize> = rows
        .iter()
        .filter(|r| {
            r.contains("contiguous") || r.contains("inherit") || r.contains("cling_by_tags")
        })
        .map(|row| row.len() - row.trim_start().len())
        .collect();
    assert!(
        indents.len() > 1,
        "expected a wrapped placeholder:\n{joined}"
    );
    assert!(
        indents.iter().all(|i| *i == indents[0]),
        "a wrapped placeholder must stay on one indent: {indents:?}\n{joined}"
    );
    assert!(
        indents[0] > 0,
        "a wrapped placeholder must not reflow flush left:\n{joined}"
    );
}

/// Spec §9.3, on screen: a placeholder is part of the spelling it belongs
/// to, so `grep`'s `-e, --regexp PATTERNS` reaches its description on its
/// own row instead of overrunning a slot and hanging it.
///
/// The pane is rendered wide, because the failure is about a column being
/// wide enough for the row rather than about a narrow terminal: at the
/// width where the section stacks, everything hangs by design and the
/// defect is invisible.
#[test]
fn a_placeholder_does_not_hang_the_description_it_shares_a_row_with() {
    let mut node = CommandNode::new("grep", Provenance::single(Source::HelpText));
    for (short, long, value) in [
        ('e', "regexp", Some("PATTERNS")),
        ('f', "file", Some("FILE")),
        ('i', "ignore-case", None),
        ('c', "count", None),
        ('v', "invert-match", None),
    ] {
        let mut e = entity(
            EntityKind::Flag,
            Spelling::long(long),
            "zzz what this one does",
        );
        e.spellings.insert(0, Spelling::short(short));
        if let Some(v) = value {
            e.value_name = Some(v.to_string());
            e.value_kind = ValueKind::Required;
        }
        node.entities.push(e);
    }

    let rows = detail_rows(&app_for(node), 140, 30);
    let joined = rows.join("\n");
    let row = rows
        .iter()
        .find(|r| r.contains("PATTERNS"))
        .unwrap_or_else(|| panic!("no --regexp row:\n{joined}"));
    assert!(
        row.starts_with("-e, --regexp PATTERNS"),
        "the placeholder follows its spelling directly: {row:?}"
    );
    assert!(
        row.contains("zzz"),
        "the placeholder must not push the description onto its own line: {row:?}\n{joined}"
    );
    // The placeholders are not aligned with each other — there is no slot.
    let file_row = rows
        .iter()
        .find(|r| r.contains(" FILE"))
        .unwrap_or_else(|| panic!("no --file row:\n{joined}"));
    assert_ne!(
        row.find("PATTERNS"),
        file_row.find("FILE"),
        "placeholders must not share a column:\n{joined}"
    );
}

/// Spec §9.3's two spelling columns, on screen: shorts at the content
/// edge, every long one short-prefix in whether or not a short precedes
/// it.
///
/// The failure this rules out is the one the preindent exists for — a
/// list where `--config` and `-c, --context` start their long names two
/// columns apart, so the eye has to re-find the long on every row instead
/// of running down one column. Read off the rendered frame, because the
/// column that matters is the one a reader sees, not the one the layout
/// computed.
#[test]
fn longs_share_one_column_whether_or_not_a_short_precedes_them() {
    let mut node = CommandNode::new("docker", Provenance::single(Source::HelpText));
    for (short, long) in [
        (None, "config"),
        (Some('c'), "context"),
        (Some('D'), "debug"),
        (None, "tls"),
        (Some('l'), "log-level"),
        (None, "tlscacert"),
    ] {
        let mut e = entity(
            EntityKind::Flag,
            Spelling::long(long),
            "sets one of the things docker sets",
        );
        if let Some(c) = short {
            e.spellings.insert(0, Spelling::short(c));
        }
        node.entities.push(e);
    }
    let rows = detail_rows(&app_for(node), 90, 30);
    let joined = rows.join("\n");

    // Matched on the row's last whitespace-separated spelling token
    // rather than on a substring, so `--tls` cannot be found inside
    // `--tlscacert`'s row.
    let mut columns = Vec::new();
    for long in [
        "--config",
        "--context",
        "--debug",
        "--tls",
        "--log-level",
        "--tlscacert",
    ] {
        let row = rows
            .iter()
            .find(|r| r.split_whitespace().any(|w| w == long))
            .unwrap_or_else(|| panic!("no row for {long:?}:\n{joined}"));
        columns.push(row.find(long).expect("checked above"));
    }
    assert!(
        columns.windows(2).all(|w| w[0] == w[1]),
        "longs start at {columns:?}, not one column:\n{joined}"
    );
    assert!(
        columns[0] > 0,
        "the long column must leave room for a short prefix:\n{joined}"
    );

    // ...and the shorts are at the edge, which is what puts the longs
    // there without any padding of their own.
    let with_short = rows
        .iter()
        .find(|r| r.contains("--context"))
        .expect("checked above");
    assert!(
        with_short.starts_with("-c, --context"),
        "a short leads at the content edge: {with_short:?}"
    );
}

/// Nothing in a section is clipped or horizontally scrolled (spec §9.3):
/// `[ui] horizontal_scroll` governs the raw view and verbatim USAGE lines
/// only. A long description wraps at the pane width whatever the toggle
/// says, and the horizontal-scroll keys leave it alone.
#[test]
fn section_prose_wraps_and_never_scrolls_horizontally() {
    let mut node = CommandNode::new("tool", Provenance::single(Source::HelpText));
    node.entities.push(entity(
        EntityKind::Flag,
        Spelling::long("verbose"),
        "a description far too long for one line of this pane, which must \
         therefore wrap onto the next one rather than run off the edge",
    ));
    let mut app = app_for(node);
    assert!(app.horizontal_scroll_enabled, "default is on");
    let before = detail_rows(&app, 90, 30);
    let body: Vec<&String> = before
        .iter()
        .skip_while(|r| !r.starts_with("FLAGS"))
        .skip(1)
        .take_while(|r| !r.is_empty())
        .collect();
    assert!(
        body.len() > 1,
        "the description must wrap onto further lines: {before:?}"
    );
    // Nothing is lost to the wrap and nothing is clipped: the words come
    // back in order across the rows they were broken over.
    let reflowed = body
        .iter()
        .flat_map(|r| r.split_whitespace())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        reflowed.ends_with("rather than run off the edge"),
        "the description was clipped, not wrapped: {reflowed:?}"
    );

    for _ in 0..5 {
        app.detail_hscroll_right();
    }
    assert_eq!(
        detail_rows(&app, 90, 30),
        before,
        "a section is mandible's own layout and never scrolls horizontally"
    );
}
