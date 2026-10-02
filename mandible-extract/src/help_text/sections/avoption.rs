//! The `<type>` and capability columns of an AVOptions row (atlas S-015).

use super::*;

/// A row `-name  <type>  ED.VAS.....  description` reads its description as
/// everything after the gap. Take `<type>` as the value placeholder and the
/// fixed-position capability bitmask as column noise, not as prose.
/// Fixture: `corpus/ffplay/6.1.1-3ubuntu5-avoptions`.
pub(super) fn recover_avoption_type_column(flags: &mut [Entity]) {
    for flag in flags.iter_mut() {
        let Some(desc) = flag.description.as_ref().map(|t| t.as_str().to_string()) else {
            continue;
        };
        let mut words = desc.split_whitespace().peekable();
        // The type is either still the description's first word, or the
        // spaced-placeholder repair already took it as `<type>`.
        let ty = match flag.value_name.as_deref() {
            None if flag.value_kind == ValueKind::None => words.next().map(str::to_string),
            Some(v) if flag.value_kind == ValueKind::Required => Some(v.to_string()),
            _ => None,
        };
        let Some(name) = ty
            .as_deref()
            .and_then(|t| t.strip_prefix('<'))
            .and_then(|t| t.strip_suffix('>'))
            .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        else {
            continue;
        };
        if !words.next().is_some_and(is_capability_column) {
            continue;
        }
        let rest: Vec<&str> = words.collect();
        flag.value_name = Some(name.to_string());
        flag.value_kind = ValueKind::Required;
        flag.description = non_empty_text(&rest.join(" "));
    }
}

/// `ED.VAS.....`: at least eight characters, only capability letters and
/// dots, at least three dots.
fn is_capability_column(token: &str) -> bool {
    token.len() >= 8
        && token.chars().all(|c| "EDFVASXBRTP.".contains(c))
        && token.chars().filter(|&c| c == '.').count() >= 3
}
