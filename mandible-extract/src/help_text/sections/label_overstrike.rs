//! Backspace-overstrike in the plain labels of a recovered entity (a value
//! name, a group heading): descriptions are sanitized where they are built,
//! labels are not, so `_\bN` reached the tree whole. Resolved once over the
//! parsed page. See docs/shapes.md S-257 and corpus/less/590.

use super::ParsedHelp;

pub(super) fn resolve(parsed: &mut ParsedHelp) {
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
}
