//! [S-133] `icupkg`'s `-tl or --type l`, `-tb or --type b`, `-te or
//! --type e` rows: one option, three literal choice values, each keeping
//! its own description, gated on the raw ` or ` row's own literal text so
//! an unrelated same-spelling repeat (S-134's fused `pod2man` regression)
//! is never folded. See docs/shapes.md S-133.

use super::is_choice_token;
use mandible_core::{Choice, Entity};

/// True when `e` is one row of the unfolded shape this fold repairs: both
/// a short and a long spelling, a bare choice-shaped `value_name`, its
/// own description, and no `choices` of its own yet.
fn is_unfolded_or_choice_row(e: &Entity) -> bool {
    e.short().is_some()
        && e.long().is_some()
        && e.choices.is_empty()
        && e.description.is_some()
        && e.value_name.as_deref().is_some_and(is_choice_token)
}

/// The literal row text this fold requires, verbatim in `raw`: the short
/// spelling glued directly to the value, the word ` or `, the long
/// spelling, then the same value again spelled out — `-tl or --type l`.
/// Checking this against the tool's own bytes (not just the already-
/// parsed entity shape) is what keeps this rule from over-generalizing
/// onto a same-spelling repeat that isn't this shape at all.
fn glued_or_choice_row_in_raw(raw: &str, e: &Entity, value: &str) -> bool {
    let (Some(short), Some(long)) = (e.short(), e.long()) else {
        return false;
    };
    let expected = format!("-{short}{value} or --{long} {value}");
    raw.contains(&expected)
}

/// Fold every run of 2+ consecutive [`is_unfolded_or_choice_row`] entities
/// sharing one spelling into a single flag carrying `choices`, each with
/// its own description — but only when every member's own raw row
/// verifies via [`glued_or_choice_row_in_raw`]. A run that fails the raw
/// check (or is shorter than 2) is left exactly as parsed.
pub(super) fn fold_or_joined_choice_rows(raw: &str, flags: &mut Vec<Entity>) {
    let mut i = 0;
    while i < flags.len() {
        if !is_unfolded_or_choice_row(&flags[i]) {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < flags.len()
            && is_unfolded_or_choice_row(&flags[j])
            && flags[j].spellings == flags[i].spellings
        {
            j += 1;
        }
        let run_verified = j - i >= 2
            && (i..j).all(|k| {
                let value = flags[k].value_name.clone().unwrap_or_default();
                glued_or_choice_row_in_raw(raw, &flags[k], &value)
            });
        if run_verified {
            let mut folded = flags[i].clone();
            folded.value_name = None;
            folded.value_kind = mandible_core::ValueKind::Required;
            folded.choices = flags[i..j]
                .iter()
                .map(|e| Choice {
                    name: e.value_name.clone().unwrap_or_default(),
                    description: e.description.clone(),
                })
                .collect();
            folded.description = None;
            flags.splice(i..j, std::iter::once(folded));
            i += 1;
        } else {
            i = j.max(i + 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::parse;

    /// icupkg's own bytes, byte-exact (`corpus/icupkg/74.2/help.txt`).
    const ICUPKG_TYPE_ROWS: &str =
        "\t-tl or --type l   output for little-endian/ASCII charset family\n\
        \t-tb or --type b   output for big-endian/ASCII charset family\n\
        \t-te or --type e   output for big-endian/EBCDIC charset family\n\
        \t                  The output type defaults to the input type.\n";

    #[test]
    fn icupkgs_three_type_rows_fold_into_one_flag_with_choices() {
        let parsed = parse(ICUPKG_TYPE_ROWS);
        let type_flags: Vec<_> = parsed
            .flags
            .iter()
            .filter(|e| e.long() == Some("type"))
            .collect();
        assert_eq!(
            type_flags.len(),
            1,
            "expected one folded -t/--type flag, got {type_flags:?}"
        );
        let choices: Vec<&str> = type_flags[0]
            .choices
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(choices, vec!["l", "b", "e"]);
        assert_eq!(
            type_flags[0]
                .choices
                .iter()
                .map(|c| c.description.as_ref().map(|d| d.as_str()))
                .collect::<Vec<_>>(),
            vec![
                Some("output for little-endian/ASCII charset family"),
                Some("output for big-endian/ASCII charset family"),
                Some(
                    "output for big-endian/EBCDIC charset family The output type defaults to \
                     the input type."
                ),
            ]
        );
    }

    /// pod2man's own two rows (`corpus/pod2man/5.01/help.txt`): two
    /// *different* long spellings, never folded even though each is a
    /// same-shaped value-carrying row — the round-10 regression this rule
    /// must not repeat.
    #[test]
    fn pod2mans_lquote_and_rquote_stay_separate() {
        let help = "  --lquote quote\n                    Sets the string used as a left quote.\n\
                      --rquote quote\n                    Sets the string used as a right quote.\n";
        let parsed = parse(help);
        assert!(
            parsed.flags.iter().any(|e| e.long() == Some("lquote")),
            "expected --lquote to survive on its own"
        );
        assert!(
            parsed.flags.iter().any(|e| e.long() == Some("rquote")),
            "expected --rquote to survive on its own"
        );
    }
}
