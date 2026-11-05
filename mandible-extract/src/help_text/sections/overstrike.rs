//! Backspace-overstrike in a page. Underline (`_\bX`) is resolved over the
//! whole text before any row is split, so spellings and values see the
//! plain placeholder (`-_\b<_\bf...>` is `-<flag>`). Bold (`X\bX`) heading
//! lines are left as written: resolved, they would read as headings and
//! turn key tables into commands. Labels still carrying bold are resolved
//! after the parse. See docs/shapes.md S-257 and corpus/less/590.

use super::ParsedHelp;
use std::borrow::Cow;

pub(super) fn resolve_underline(raw: &str) -> Cow<'_, str> {
    if raw.contains("_\u{8}") {
        Cow::Owned(raw.replace("_\u{8}", ""))
    } else {
        Cow::Borrowed(raw)
    }
}

pub(super) fn resolve_labels(parsed: &mut ParsedHelp) {
    let clean = |s: &mut Option<String>| {
        if let Some(v) = s.as_mut() {
            *v = mandible_core::Text::resolve_overstrike(v);
        }
    };
    for e in parsed
        .flags
        .iter_mut()
        .chain(&mut parsed.positionals)
        .chain(&mut parsed.modifiers)
        .chain(&mut parsed.env_vars)
    {
        clean(&mut e.value_name);
        clean(&mut e.group);
    }
    for node in &mut parsed.subcommands {
        clean(&mut node.group);
    }
    parsed.flags.retain(|f| !is_key_placeholder_row(f));
}

/// True for a row of a commands table that names a key pressed inside the
/// tool by a dash and an angle placeholder (`-<flag>`, read as the spelling
/// `-<` with the value `flag>`): it is no flag. See docs/shapes.md S-261.
fn is_key_placeholder_row(flag: &mandible_core::Entity) -> bool {
    let key = matches!(flag.spellings.as_slice(), [s] if s.dashes == mandible_core::Dashes::Single && s.name == "<");
    key && flag
        .group
        .as_deref()
        .is_some_and(super::mentions_commands_word)
        && flag
            .value_name
            .as_deref()
            .is_some_and(|v| v.ends_with('>') && !v.contains('<'))
}
