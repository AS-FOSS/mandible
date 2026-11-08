//! Parts of a flag-spec parse kept beside the main grammar: the joiner of
//! an optional value, a `<flag> .. <flag>` range head, and the copy of a
//! parsed spec into an entity.

use super::grammar::FlagSpec;
use mandible_core::{Dashes, Entity, OptionalJoin, Spelling, SpellingRange};
use std::borrow::Cow;

/// How an optional value spelled `value` joins the flag whose spelling
/// ends where `after_spellings` begins: `[=V]` and `=[V]` are `=` inside
/// and outside the brackets, a bracket touching the spelling is glued,
/// anything else is a separate word.
pub(super) fn optional_join_of(after_spellings: &str, value: &str) -> OptionalJoin {
    if value.starts_with("[=") {
        OptionalJoin::Equals
    } else if value.starts_with("=[") {
        OptionalJoin::EqualsThenBracket
    } else if after_spellings.starts_with('[') {
        OptionalJoin::Glued
    } else {
        OptionalJoin::Separate
    }
}

/// The spelling run of a row as its alias list: the row colon after it is
/// dropped, and `--sync/nosync` reads as `--sync, --nosync`. S-263.
pub(super) fn end_row_spelling(spec: &str) -> Cow<'_, str> {
    let spec = spec
        .strip_suffix(':')
        .filter(|s| !s.is_empty())
        .unwrap_or(spec);
    if let Some(bare) = short_with_repeated_value(spec) {
        return Cow::Owned(bare);
    }
    let Some(long) = spec.strip_prefix("--") else {
        return Cow::Borrowed(spec);
    };
    let name_len = long
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .unwrap_or(long.len());
    let after = &long[name_len..];
    let word_len = after.strip_prefix('/').map(|w| {
        w.find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(w.len())
    });
    match word_len {
        Some(n) if name_len > 0 && n > 0 && !after[1 + n..].starts_with(['-', '/']) => {
            Cow::Owned(format!(
                "--{}, --{}{}",
                &long[..name_len],
                &after[1..=n],
                &after[1 + n..]
            ))
        }
        _ => Cow::Borrowed(spec),
    }
}

/// `-d[n], --debug=[n]` as `-d, --debug=[n]`: the short's glued optional
/// value repeats the long's own `=[n]`, which carries it. S-264.
fn short_with_repeated_value(spec: &str) -> Option<String> {
    let rest = spec.strip_prefix('-').filter(|r| !r.starts_with('-'))?;
    let mut chars = rest.chars();
    let letter = chars.next().filter(char::is_ascii_alphanumeric)?;
    let group = chars.as_str().strip_prefix('[')?;
    let (word, after) = group.split_once(']')?;
    let long = after.strip_prefix(',')?.trim_start();
    let valid = !word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric());
    (valid && long.starts_with("--") && long.contains(&format!("=[{word}]")))
        .then(|| format!("-{letter}, {long}"))
}

/// `:[class|module|gc|jni]`: a colon-glued bracket of plain words is the
/// row's choices, drawn `:[{…}]`. Placeholders (`:<name>`) never match. S-265.
pub(crate) fn colon_bracket_choices(value_name: &str) -> Option<Vec<String>> {
    let inner = value_name.strip_prefix(":[")?.strip_suffix(']')?;
    let members: Vec<&str> = inner.split('|').collect();
    let plain = |m: &str| {
        m.chars().next().is_some_and(|c| c.is_ascii_lowercase())
            && m.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    };
    (members.len() >= 2 && members.iter().all(|m| plain(m)))
        .then(|| members.into_iter().map(str::to_string).collect())
}

/// A required value written straight onto its flag (`-Z<type>`, `-z#`,
/// `+<cmd>`, `-splash:<path>`) is glued: the text after the last spelling
/// is the value itself, opening with a placeholder mark (`<`, `#`, `:`),
/// never a word that may be the rest of the flag's name.
pub(super) fn required_join_of(after_spellings: &str, value: &str) -> Option<OptionalJoin> {
    let glued = after_spellings.len() == value.len() && value.starts_with(['<', '#', ':']);
    glued.then_some(OptionalJoin::Glued)
}

/// A bracketed value followed, after one space, by an angle placeholder that
/// ends the spec (`--ulcolor [bright] <color>`): both stay on the value, as
/// written, which is then required. `head` is the bracket run as `try_value`
/// holds it, `folded` whether it already carries its own brackets. See
/// docs/shapes.md S-260.
pub(super) fn bracket_then_angle<'a>(
    head: &str,
    folded: bool,
    s: &'a str,
) -> Option<(String, &'a str)> {
    let rest = s.strip_prefix(' ')?.strip_prefix('<')?;
    let close = rest.find('>').filter(|&c| c > 0)?;
    let after = &rest[close + 1..];
    after.trim().is_empty().then(|| {
        let head = if folded {
            head.to_string()
        } else {
            format!("[{head}]")
        };
        (format!("{head} <{}>", &rest[..close]), after)
    })
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
        negation: None,
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
