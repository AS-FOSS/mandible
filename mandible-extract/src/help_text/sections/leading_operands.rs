//! A usage form whose operands come first and whose options follow in
//! brackets: `usage: mount.fuse type#[source] destination [-t type]
//! [-o opt[,opts...]]` (S-255). Two or more bare lowercase words straight
//! after the program, then bracketed flag groups only. Each operand keeps
//! the name as written.

use super::*;

/// A bare operand word as the head of a form: lowercase first letter, and
/// only letters, digits, `-`, `_`, `#` and a glued `[..]` suffix.
fn is_head_operand(group: &str) -> bool {
    group.starts_with(|c: char| c.is_ascii_lowercase())
        && group
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '#' | '[' | ']'))
        && group.matches('[').count() == group.matches(']').count()
}

fn is_flag_group(group: &str) -> bool {
    group
        .strip_prefix('[')
        .and_then(|g| g.strip_suffix(']'))
        .is_some_and(|inner| inner.starts_with('-') && inner.len() > 1)
}

/// The leading operands of the only usage form in `usage`, or none.
pub(crate) fn recover(usage: &[String]) -> Vec<Entity> {
    let [form] = usage else { return Vec::new() };
    let lower = form.to_ascii_lowercase();
    let after = match lower.find("usage:") {
        Some(at) => &form[at + "usage:".len()..],
        None => form.as_str(),
    };
    let mut groups = group_synopsis_tokens(cut_before_description_gap(after).trim());
    if groups.is_empty() {
        return Vec::new();
    }
    groups.remove(0); // the program name
    let head = groups.iter().take_while(|g| is_head_operand(g)).count();
    let tail = &groups[head..];
    if head < 2 || tail.is_empty() || !tail.iter().all(|g| is_flag_group(g)) {
        return Vec::new();
    }
    groups[..head]
        .iter()
        .map(|name| {
            let mut p = Entity::positional(name.clone(), Provenance::single(Source::HelpText));
            p.required = true;
            p
        })
        .collect()
}
