//! A flag description that ends in a `Possible values:` label followed by
//! `* name: text` or `* name (text)` bullets (bat's `--style`,
//! `--nonprintable-notation`) names that flag's `choices`, each bullet's
//! text the choice's description. The label and bullets leave the
//! description; the `values:` lines show them. All bullets must be of the
//! shape or nothing changes. See docs/shapes.md S-234.

use mandible_core::{Choice, Entity, Text};

const LABEL: &str = "possible values:";

/// `(name, text)` for one bullet body (`name: text` or `name (text)`).
fn bullet(seg: &str) -> Option<(&str, String)> {
    let seg = seg.trim();
    let name_end = seg
        .find(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_' || c == '.'))
        .unwrap_or(seg.len());
    let (name, rest) = seg.split_at(name_end);
    if name.is_empty() {
        return None;
    }
    if let Some(text) = rest.strip_prefix(':') {
        let text = text.trim();
        return (text.is_empty() || rest[1..].starts_with(' ')).then(|| (name, text.to_string()));
    }
    let text = rest.trim_start();
    (rest.starts_with(' ') && text.starts_with('(') && text.ends_with(')'))
        .then(|| (name, text.to_string()))
}

/// The description's head and the bullets after its last label.
fn split_bullets(desc: &str) -> Option<(String, Vec<(String, String)>)> {
    let at = desc.to_ascii_lowercase().rfind(LABEL)?;
    let rest = desc[at + LABEL.len()..].trim().strip_prefix("* ")?;
    let mut items: Vec<(String, String)> = Vec::new();
    for seg in rest.split(" * ") {
        match bullet(seg) {
            Some((n, t)) => items.push((n.to_string(), t)),
            // A segment that is no bullet continues the previous one.
            None => {
                let last = items.last_mut()?;
                last.1.push_str(" * ");
                last.1.push_str(seg.trim());
            }
        }
    }
    (items.len() >= 2).then(|| (desc[..at].trim_end().to_string(), items))
}

/// Move each flag's bullet list into its `choices`, for a flag that has none.
pub(super) fn attach_bullet_value_choices(flags: &mut [Entity]) {
    for flag in flags {
        if !flag.choices.is_empty() {
            continue;
        }
        let Some(desc) = flag.description.as_ref() else {
            continue;
        };
        let Some((head, items)) = split_bullets(desc.as_str()) else {
            continue;
        };
        if head.is_empty() {
            continue;
        }
        flag.description = Some(Text::sanitize(&head));
        flag.choices = items
            .into_iter()
            .map(|(name, text)| Choice {
                name,
                description: (!text.is_empty()).then(|| Text::sanitize(&text)),
            })
            .collect();
    }
}
