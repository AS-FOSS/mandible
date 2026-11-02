//! The `{…}` mark for a value that is only its alternatives (spec §9.3
//! rules 6 and 7; docs/shapes.md S-253).

use super::*;
use mandible_core::{Choice, OptionalJoin, Provenance, Source, ValueKind};

fn layout() -> SectionLayout {
    SectionLayout {
        description: 30,
        indent: 0,
        man: false,
        wrap: true,
    }
}

fn text_of(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn render(e: &Entity) -> Vec<String> {
    entity_line(e, false, 100, style::Palette::extended(), layout())
        .iter()
        .map(text_of)
        .collect()
}

fn flag(long: &str, kind: ValueKind, choices: &[&str]) -> Entity {
    let mut e = Entity::flag_long(long, Provenance::single(Source::HelpText));
    e.value_kind = kind;
    e.choices = choices.iter().map(|c| Choice::bare(*c)).collect();
    e
}

#[test]
fn a_nameless_value_draws_the_mark_and_the_values_share_its_line() {
    let e = flag("activate", ValueKind::Required, &["y", "n", "ay"]);
    let rows = render(&e);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].starts_with("    --activate {…}"), "{rows:?}");
    assert!(rows[0].ends_with("values: y, n, ay"), "{rows:?}");
}

#[test]
fn a_described_row_keeps_the_values_line_below() {
    let mut e = flag("compress", ValueKind::Required, &["none", "zlib"]);
    e.description = Some(mandible_core::Text::sanitize("Compress sections"));
    let rows = render(&e);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows[0].contains("--compress {…}"), "{rows:?}");
    assert!(rows[0].ends_with("Compress sections"), "{rows:?}");
    assert!(
        rows[1].trim_start().starts_with("values: none, zlib"),
        "{rows:?}"
    );
}

#[test]
fn an_optional_value_keeps_its_brackets_around_the_mark() {
    let mut e = flag("compress", ValueKind::Optional, &["none", "zlib"]);
    e.optional_join = Some(OptionalJoin::Equals);
    assert_eq!(e.value_text().as_deref(), Some("[={…}]"));
    e.optional_join = Some(OptionalJoin::Separate);
    assert_eq!(e.value_text().as_deref(), Some("[{…}]"));
}

#[test]
fn a_name_the_tool_wrote_is_kept() {
    let mut e = flag("color", ValueKind::Required, &["always", "never"]);
    e.value_name = Some("WHEN".to_string());
    assert_eq!(e.value_text().as_deref(), Some("WHEN"));
}

#[test]
fn a_flag_that_takes_no_value_has_no_mark() {
    let e = flag("block", ValueKind::None, &["a", "b"]);
    assert_eq!(e.value_text(), None);
}

#[test]
fn a_positional_named_by_alternatives_draws_the_mark() {
    let mut p = Entity::positional("{auto|manual}", Provenance::single(Source::HelpText));
    p.choices = vec![Choice::bare("auto"), Choice::bare("manual")];
    let rows = render(&p);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].trim_start().starts_with("{…}"), "{rows:?}");
    assert!(rows[0].ends_with("values: auto, manual"), "{rows:?}");
    p.repeatable = true;
    assert!(render(&p)[0].trim_start().starts_with("{…}..."));
}
