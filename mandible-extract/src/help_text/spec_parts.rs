//! Parts of a flag-spec parse kept beside the main grammar: the joiner of
//! an optional value, a `<flag> .. <flag>` range head, and the copy of a
//! parsed spec into an entity.

use super::grammar::FlagSpec;
use mandible_core::{Dashes, Entity, OptionalJoin, Spelling, SpellingRange};

/// How an optional value spelled `value` joins the flag whose spelling
/// ends where `after_spellings` begins: `[=V]` is `=`, a bracket touching
/// the spelling is glued, anything else is a separate word.
pub(super) fn optional_join_of(after_spellings: &str, value: &str) -> OptionalJoin {
    if value.starts_with("[=") || value.starts_with("=[") {
        OptionalJoin::Equals
    } else if after_spellings.starts_with('[') {
        OptionalJoin::Glued
    } else {
        OptionalJoin::Separate
    }
}

/// One plain flag name at the front of `input` (`-9`, `-11`, `--fast`):
/// dashes, then letters, digits, `_` or `-`, ending at whitespace, a comma,
/// a dot or the end of the text.
fn take_plain_flag(input: &str) -> Option<(Spelling, &str)> {
    let (dashes, body) = match input.strip_prefix("--") {
        Some(body) => (Dashes::Double, body),
        None => (Dashes::Single, input.strip_prefix('-')?),
    };
    let end = body
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(body.len());
    let (name, tail) = body.split_at(end);
    if name.is_empty() || !(tail.is_empty() || tail.starts_with([' ', '\t', ',', '.', '…'])) {
        return None;
    }
    let spelling = Spelling {
        name: name.to_string(),
        dashes,
        negatable: false,
        abbrev: None,
    };
    Some((spelling, tail))
}

/// A head that is exactly `<flag> <sep> <flag>`, `sep` being `..`, `...`,
/// `…` or the word `to`, optionally followed by `, <flag>` items
/// (`-0 to -9, -11`). Anything else on the head, a value, a bracket or an
/// `=`, is not a range. Nothing between the two ends is computed.
/// See docs/shapes.md S-181.
pub(super) fn parse_range_head(head: &str) -> Option<FlagSpec> {
    let (start, rest) = take_plain_flag(head)?;
    let rest = rest.trim_start();
    let rest = if let Some(r) = rest.strip_prefix("...").or_else(|| rest.strip_prefix("..")) {
        r
    } else if let Some(r) = rest.strip_prefix('…') {
        r
    } else {
        let r = rest.strip_prefix("to")?;
        r.starts_with([' ', '\t']).then_some(r)?
    };
    let (end, mut rest) = take_plain_flag(rest.trim_start())?;
    let mut spellings = vec![start.clone()];
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        let (name, after) = take_plain_flag(rest.strip_prefix(',')?.trim_start())?;
        spellings.push(name);
        rest = after;
    }
    Some(FlagSpec {
        spellings,
        range: Some(SpellingRange { start, end }),
        fully_consumed: true,
        ..FlagSpec::default()
    })
}

impl FlagSpec {
    /// Take the value, joiner and further spellings of `tail`, the parse of
    /// the text that follows a sigil this spec already holds.
    pub(super) fn take_tail(&mut self, tail: FlagSpec) {
        self.value_name = tail.value_name;
        self.value_kind = tail.value_kind;
        self.optional_join = tail.optional_join;
        self.spellings.extend(tail.spellings);
        self.fully_consumed = tail.fully_consumed;
    }

    /// Copy the spellings, value, joiner and range of this parse into
    /// `flag`. The start of a range is always among the spellings, so the
    /// setter's refusal cannot happen for a spec this grammar built.
    pub(super) fn fill_entity(&mut self, flag: &mut Entity) {
        flag.spellings = std::mem::take(&mut self.spellings);
        flag.value_name = self.value_name.take();
        flag.value_kind = self.value_kind;
        flag.optional_join = self.optional_join;
        if let Some(range) = self.range.take() {
            let _ = flag.set_range(range);
        }
    }
}
