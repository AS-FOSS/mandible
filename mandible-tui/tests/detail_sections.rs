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
    for expected in [
        "MODIFIERS (1)",
        "delete members from the archive",
        "ENVIRONMENT (1)",
        "TOOL_CONFIG",
        "path to the configuration file",
        "POSITIONALS (1)",
        "the thing to operate on",
    ] {
        assert!(joined.contains(expected), "missing {expected:?}:\n{joined}");
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

/// Spec §9.3 and §9.2: a section header and a group divider must stay
/// distinguishable with every attribute stripped, because several
/// terminals ignore dimming. `TestBackend`'s symbol grid *is* that
/// terminal — it carries no styling at all — so this reads the two shapes
/// exactly as such a terminal would.
///
/// The header is CAPS with a count and its name at column 0; the divider
/// is mixed case with no count behind a leading rule. Both run to the
/// pane's edge, and the rows beneath the divider sit at the section's
/// normal margin — grouping is drawn, not indented.
#[test]
fn a_group_divider_is_shaped_differently_from_a_section_header() {
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

    let header = rows
        .iter()
        .find(|r| r.starts_with("FLAGS"))
        .unwrap_or_else(|| panic!("no FLAGS header:\n{joined}"));
    assert!(header.starts_with("FLAGS (3) "), "{header:?}");

    // The tool shouted one heading and set the other in its own casing;
    // both render as dividers, and neither reads as a section header.
    for label in ["Main operation mode", "Device selection and switching"] {
        let divider = rows
            .iter()
            .find(|r| r.contains(label))
            .unwrap_or_else(|| panic!("no divider for {label:?}:\n{joined}"));
        assert!(
            divider.starts_with('─'),
            "a group divider leads with the rule: {divider:?}"
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
    // same two columns an ungrouped row gets, not an extra level in.
    let row = rows
        .iter()
        .find(|r| r.contains("--create"))
        .unwrap_or_else(|| panic!("no --create row:\n{joined}"));
    assert_eq!(
        row.len() - row.trim_start().len(),
        2,
        "grouping must cost no width: {row:?}"
    );
}

/// The same frame with the ASCII glyph set: a divider degrades to `-`
/// like every other rule in the pane, and the shape that carries its
/// meaning survives (spec §9.2 — a glyph may only be used if there is
/// something legible to fall back to).
#[test]
fn a_group_divider_degrades_to_ascii() {
    let mut node = CommandNode::new("tar", Provenance::single(Source::HelpText));
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
    assert!(
        divider.starts_with("- Main operation mode -"),
        "{divider:?}"
    );
    assert!(
        divider.is_ascii(),
        "the ASCII frame must stay ASCII: {divider:?}"
    );
}

/// Spec §9.3's capped column, on screen: the majority of a section's
/// entities share one description column, and an entity past the column
/// puts its description on the next line at the small fixed hanging
/// indent — four columns, well inside the column it could not reach.
#[test]
fn a_wide_entity_hangs_its_description_at_the_fixed_indent() {
    let mut node = CommandNode::new("tool", Provenance::single(Source::HelpText));
    for i in 0..9 {
        node.entities.push(entity(
            EntityKind::Flag,
            Spelling::long(format!("opt-{i}")),
            "zzz an ordinary description",
        ));
    }
    let mut wide = entity(
        EntityKind::Flag,
        Spelling::long("an-extremely-long-option-name-nobody-would-type"),
        "zzz the outlier's description",
    );
    wide.value_kind = ValueKind::None;
    node.entities.push(wide);

    let rows = detail_rows(&app_for(node), 90, 30);
    let joined = rows.join("\n");

    let mut shared = Vec::new();
    let mut hanging = Vec::new();
    for row in &rows {
        let Some(at) = row.find("zzz") else { continue };
        if row.trim_start().starts_with("zzz") {
            hanging.push(at);
        } else {
            shared.push(at);
        }
    }
    assert_eq!(shared.len(), 9, "nine rows share the column:\n{joined}");
    assert!(
        shared.windows(2).all(|w| w[0] == w[1]),
        "the shared column is not shared: {shared:?}\n{joined}"
    );
    assert_eq!(
        hanging,
        vec![4],
        "the outlier hangs at four columns:\n{joined}"
    );
    assert!(
        hanging[0] < shared[0],
        "a hanging description must not be indented past the column it missed"
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
