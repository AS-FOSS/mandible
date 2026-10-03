//! The `<type>` and capability columns of an AVOptions row (atlas S-015).

use super::*;

/// A row `-name  <type>  ED.VAS.....  description` reads its description as
/// everything after the gap. Take `<type>` as the value placeholder and the
/// fixed-position capability bitmask as column noise, not as prose, unless
/// it is the row's only text, when it stays as the description (AGENTS 3.9).
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
        let Some(cap) = words.next().filter(|w| is_capability_column(w)) else {
            continue;
        };
        let rest: Vec<&str> = words.collect();
        flag.value_name = Some(name.to_string());
        flag.value_kind = ValueKind::Required;
        // A row whose only text is the column keeps it as the description.
        flag.description = non_empty_text(&rest.join(" ")).or_else(|| non_empty_text(cap));
    }
}

/// `ED.VAS.....`: at least eight characters, only capability letters and
/// dots, at least three dots.
pub(super) fn is_capability_column(token: &str) -> bool {
    token.len() >= 8
        && token.chars().all(|c| "EDFVASXBRTP.".contains(c))
        && token.chars().filter(|&c| c == '.').count() >= 3
}

/// True when a bare block row's text opens `<type> ED.VAS.....`: the row is
/// an option of its own AVOptions section, never a value of a flag above.
/// Fixture: `corpus/ffplay/6.1.1-3ubuntu5-section-bound`, atlas S-014.
pub(super) fn is_avoption_row_text(text: &str) -> bool {
    let mut words = text.split_whitespace();
    words
        .next()
        .is_some_and(|t| t.starts_with('<') && t.ends_with('>'))
        && words.next().is_some_and(is_capability_column)
}
