//! A flag row whose value placeholder carries text glued straight after it
//! (`--filter <pattern>...`, `--filter <pattern>^...`,
//! `--with=<param>=<constraint>`) names one value, written out whole.
//! `parse_flag_spec` reads the placeholder and leaves the glued tail
//! unconsumed, where nothing took it up and it vanished. The tail must be
//! the row's last token and may not open with a separator. See
//! docs/shapes.md S-245 and corpus/snap/2.77.1-get.

use super::*;

/// True when `tail` can continue a value: non-empty and not an alias
/// separator, a sentence mark or a closing bracket.
fn is_value_tail(tail: &str) -> bool {
    !tail.is_empty() && !tail.starts_with([',', '|', ':', ';', ')', ']', '}', '/'])
}

/// Extends `spec`'s bracketed one-token value to the text glued after it,
/// when the grammar left that text unconsumed at the end of `spec_text`.
pub(super) fn extend_value_tail(spec: &mut FlagSpec, spec_text: &str) {
    if spec.fully_consumed || spec.value_kind != ValueKind::Required {
        return;
    }
    let Some(value) = spec.value_name.as_deref() else {
        return;
    };
    let tokens: Vec<&str> = spec_text.split_whitespace().collect();
    let Some((last, spellings)) = tokens.split_last() else {
        return;
    };
    if spellings.is_empty() && !last.starts_with('-') {
        return;
    }
    if !spellings.iter().all(|t| t.starts_with('-')) {
        return;
    }
    let Some(at) = last.find(value) else {
        return;
    };
    let glue_ok = at == 0 && !last.starts_with('-') || last[..at].ends_with('=');
    let tail = &last[at + value.len()..];
    if glue_ok && value.starts_with('<') && is_value_tail(tail) {
        spec.value_name = Some(last[at..].to_string());
    }
}
