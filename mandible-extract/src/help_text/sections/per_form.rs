//! Per-usage-form reading (S-197). A synopsis that prints several
//! invocation forms (`ssh-keygen`'s thirty) gives one flag a different
//! placeholder in each, selects a form by a fixed word after one flag, and
//! ends each form with its own operands. The first-seen reading in
//! `usage.rs` keeps one placeholder per flag; this pass reads every form
//! and adds what that reading dropped. See corpus/ssh-keygen/9.6p1-forms.

use super::entry::{is_command_placeholder, is_option_list_placeholder};
use mandible_core::{is_literal_choice_value, Choice, Entity, Provenance, Source, ValueKind};

enum Tok {
    Word(String),
    Group(String),
}

struct Occ {
    key: String,
    value: Option<String>,
    alts: Vec<String>,
    head: bool,
}

struct Op {
    name: String,
    required: bool,
    repeatable: bool,
    admit: bool,
    at: usize,
    word: bool,
}

/// `line` cut at its first run of three spaces outside brackets: the
/// column gap a trailing description sits behind.
fn cut_at_gap(line: &str) -> &str {
    let (mut depth, mut run) = (0i32, 0usize);
    for (i, c) in line.char_indices() {
        depth += i32::from(c == '[') - i32::from(c == ']');
        run = if c == ' ' { run + 1 } else { 0 };
        if run >= 3 && depth <= 0 {
            return &line[..i + 1 - run];
        }
    }
    line
}

/// Split the usage lines into one text per invocation form, program name
/// removed.
fn split_forms(usage_lines: &[String]) -> Vec<String> {
    let mut forms: Vec<String> = Vec::new();
    let usage_lines: Vec<String> = usage_lines
        .iter()
        .map(|l| cut_at_gap(l.trim()).to_string())
        .collect();
    let mut prog: Option<String> = None;
    let mut fresh = true;
    for line in &usage_lines {
        let mut t = line.trim();
        let lower = t.to_ascii_lowercase();
        if lower.starts_with("usage:") || lower.starts_with("or:") {
            t = t.split_once(':').map_or("", |(_, r)| r).trim();
            fresh = true;
        } else if lower == "or" {
            fresh = true;
            continue;
        }
        let Some(first) = t.split_whitespace().next() else {
            continue;
        };
        let p = prog.get_or_insert_with(|| first.to_string());
        let rest = t.strip_prefix(first).unwrap_or("").trim();
        if first == p.as_str() && (fresh || !forms.is_empty()) {
            forms.push(rest.to_string());
            fresh = false;
        } else if let Some(last) = forms.last_mut() {
            last.push(' ');
            last.push_str(t);
        }
    }
    forms
}

fn tokenize(form: &str) -> Vec<Tok> {
    let chars: Vec<char> = form.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
        } else if chars[i] == '[' {
            let (mut depth, mut j) = (0i32, i);
            while j < chars.len() {
                depth += i32::from(chars[j] == '[') - i32::from(chars[j] == ']');
                j += 1;
                if depth == 0 {
                    break;
                }
            }
            let text: String = chars[i + 1..j.saturating_sub(1).max(i + 1)]
                .iter()
                .collect();
            if depth == 0 {
                toks.push(Tok::Group(text.trim().to_string()));
            } else {
                toks.push(Tok::Word(chars[i..j].iter().collect()));
            }
            i = j;
        } else {
            let start = i;
            while i < chars.len() && !chars[i].is_whitespace() {
                i += 1;
            }
            toks.push(Tok::Word(chars[start..i].iter().collect()));
        }
    }
    toks
}

fn is_flag_word(w: &str) -> bool {
    let body = w.trim_start_matches('-');
    let dashes = w.len() - body.len();
    (1..=2).contains(&dashes)
        && body
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn is_plain_word(w: &str) -> bool {
    !w.is_empty()
        && !w.starts_with(['-', '[', '(', '.'])
        && !w.contains(['|', '[', ']', '=', '{', '}'])
}

const STOP_WORDS: &[&str] = &["or", "and", "to", "from", "is", "the", "with", "for"];

/// The operand a word names: lowercase, ALL-CAPS or `<name>`, never a
/// placeholder for the option list or a command table.
fn operand_name(w: &str) -> Option<String> {
    let w = w.trim_end_matches('.');
    let name = match w.strip_prefix('<').and_then(|r| r.strip_suffix('>')) {
        Some(inner) if is_plain_word(inner) => inner,
        _ if w.starts_with('<') => return None,
        _ => w,
    };
    let mut chars = name.chars();
    let first = chars.next()?;
    let lower = first.is_ascii_lowercase()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    let upper = first.is_ascii_uppercase()
        && name.chars().count() > 1
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    let bracketed = name.len() != w.len();
    let ok = lower || upper || (bracketed && is_plain_word(name));
    (ok && !STOP_WORDS.contains(&name)
        && !is_option_list_placeholder(name)
        && !is_command_placeholder(name))
    .then(|| name.to_string())
}

fn read_group(inner: &str, head: bool, occs: &mut Vec<Occ>, ops: &mut Vec<Op>) {
    if inner.contains('[') || inner.contains('=') {
        return;
    }
    if inner.starts_with('-') {
        let parts: Vec<&str> = inner.split('|').map(str::trim).collect();
        let toks: Vec<&str> = parts[0].split_whitespace().collect();
        let Some(&key) = toks.first().filter(|k| is_flag_word(k)) else {
            return;
        };
        let value = toks.get(1).copied().filter(|v| is_plain_word(v));
        let shape_ok = toks.len() == 1 || (toks.len() == 2 && value.is_some());
        // `[-m module | pyfile]` is a flag value or an operand, not a choice
        // list: a list names at least three words.
        let alts_ok = (parts.len() == 1 || parts.len() >= 3)
            && parts[1..]
                .iter()
                .all(|p| value.is_some_and(is_literal_choice_value) && is_literal_choice_value(p));
        if shape_ok && alts_ok {
            occs.push(Occ {
                key: key.to_string(),
                value: value.map(str::to_string),
                alts: parts[1..].iter().map(|p| p.to_string()).collect(),
                head,
            });
        }
        return;
    }
    if inner.contains('|') {
        return;
    }
    let mut group: Vec<Op> = Vec::new();
    for w in inner.split_whitespace() {
        if w.chars().all(|c| c == '.') {
            if let Some(last) = group.last_mut() {
                last.repeatable = true;
            }
        } else if let Some(name) = operand_name(w) {
            group.push(Op {
                name,
                required: false,
                repeatable: w.ends_with(".."),
                admit: false,
                at: 0,
                word: false,
            });
        } else {
            return;
        }
    }
    ops.extend(group);
}

fn is_single_operand(inner: &str) -> bool {
    !inner.contains(['|', '[', ' ']) && operand_name(inner).is_some()
}

/// A bare lowercase word. Two in a row read as prose (`list partition
/// table(s)`, `to connect to running`), so neither is taken as an operand.
fn name_is_lower(w: &str) -> bool {
    w.chars().next().is_some_and(|c| c.is_ascii_lowercase())
}

fn is_dots(w: &str) -> bool {
    !w.is_empty() && w.chars().all(|c| c == '.')
}

fn read_form(text: &str, takes_value: &dyn Fn(&str) -> bool) -> (Vec<Occ>, Vec<Op>) {
    let toks = tokenize(text);
    let (mut occs, mut ops) = (Vec::new(), Vec::new());
    let (mut last_flag, mut prev_made_op): (Option<usize>, bool) = (None, false);
    let mut lowers: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let before = ops.len();
        match &toks[i] {
            Tok::Group(inner) => {
                if inner.starts_with('-') {
                    last_flag = Some(i);
                }
                read_group(inner, i == 0, &mut occs, &mut ops);
            }
            Tok::Word(w) if is_dots(w) => {
                if let Some(last) = ops.last_mut().filter(|_| prev_made_op) {
                    last.repeatable = true;
                }
            }
            Tok::Word(w) if w.starts_with('-') => {
                let known = is_flag_word(w);
                let takes = if known {
                    takes_value(w)
                } else {
                    w.contains('|')
                };
                let next = toks.get(i + 1).filter(|_| takes);
                let value = match next {
                    Some(Tok::Word(n)) if known && is_plain_word(n) => Some(n.clone()),
                    _ => None,
                };
                let eaten = match next {
                    Some(Tok::Word(n)) => value.is_some() || (!known && operand_name(n).is_some()),
                    Some(Tok::Group(g)) => known && is_single_operand(g),
                    None => false,
                };
                if known {
                    occs.push(Occ {
                        key: w.clone(),
                        value,
                        alts: Vec::new(),
                        head: i == 0,
                    });
                }
                i += usize::from(eaten);
                last_flag = Some(i);
            }
            Tok::Word(w) => {
                if name_is_lower(w) {
                    lowers.push(i);
                }
                if let Some(name) = operand_name(w) {
                    ops.push(Op {
                        name,
                        required: true,
                        repeatable: w.ends_with(".."),
                        admit: false,
                        at: 0,
                        word: name_is_lower(w),
                    });
                }
            }
        }
        for op in &mut ops[before..] {
            op.at = i;
        }
        prev_made_op = ops.len() > before;
        i += 1;
    }
    for op in &mut ops {
        let in_run = op.word && lowers.iter().any(|&a| a + 1 == op.at || op.at + 1 == a);
        op.admit = last_flag.is_some_and(|l| op.at > l) && !in_run;
    }
    (occs, ops)
}

fn spells(flag: &Entity, key: &str) -> bool {
    flag.spellings.iter().any(|s| s.render() == key)
}

fn synopsis_only(flag: &Entity) -> bool {
    flag.provenance
        .sources
        .iter()
        .all(|s| matches!(s, Source::HelpTextSynopsis))
}

fn push_unique(list: &mut Vec<String>, name: &str) {
    if !list.iter().any(|n| n == name) {
        list.push(name.to_string());
    }
}

/// The fixed words a flag takes: listed as alternatives in one bracket, or
/// the form-selecting words of a flag that opens two or more forms.
fn fixed_words(occs: &[&Occ], multi: bool) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    for o in occs.iter().filter(|o| !o.alts.is_empty()) {
        o.value
            .iter()
            .chain(&o.alts)
            .for_each(|w| push_unique(&mut words, w));
    }
    if !words.is_empty() || !multi {
        return words;
    }
    let mut heads: Vec<String> = Vec::new();
    for o in occs.iter().filter(|o| o.head) {
        o.value.iter().for_each(|w| push_unique(&mut heads, w));
    }
    let all_words = occs
        .iter()
        .all(|o| o.value.as_deref().is_some_and(is_literal_choice_value));
    if heads.len() >= 2 && all_words && occs.iter().filter(|o| o.head).count() >= 2 {
        return heads;
    }
    Vec::new()
}

fn apply_flag(flag: &mut Entity, occs: &[&Occ], multi: bool) {
    let words = fixed_words(occs, multi);
    if words.is_empty() {
        return;
    }
    if flag.choices.is_empty() {
        flag.choices = words.iter().map(Choice::bare).collect();
    }
    if flag
        .value_name
        .as_deref()
        .is_some_and(|v| words.iter().any(|w| w == v))
    {
        flag.value_name = None;
    }
}

/// (forms holding it, required in every one, repeatable anywhere, admitted)
fn operand_summary(forms: &[Vec<Op>]) -> Vec<(String, usize, bool, bool, bool)> {
    let mut out: Vec<(String, usize, bool, bool, bool)> = Vec::new();
    for ops in forms {
        let mut seen: Vec<&str> = Vec::new();
        for op in ops {
            let at = out.iter().position(|e| e.0 == op.name).unwrap_or_else(|| {
                out.push((op.name.clone(), 0, true, false, false));
                out.len() - 1
            });
            let first_in_form = !seen.contains(&op.name.as_str());
            seen.push(&op.name);
            let e = &mut out[at];
            e.1 += usize::from(first_in_form);
            e.2 &= op.required;
            e.3 |= op.repeatable;
            e.4 |= op.admit;
        }
    }
    out
}

fn apply_operands(forms: &[Vec<Op>], positionals: &mut Vec<Entity>) {
    for (name, count, required, repeatable, admit) in operand_summary(forms) {
        let everywhere = count == forms.len();
        if let Some(p) = positionals.iter_mut().find(|p| p.primary_name() == name) {
            p.required &= everywhere && required;
        } else if admit && count >= 2 {
            let mut p = Entity::positional(name, Provenance::single(Source::HelpText));
            p.required = everywhere && required;
            p.repeatable = repeatable;
            positionals.push(p);
        }
    }
}

/// Read every usage form of `usage_lines` and fold what the first-seen
/// reading dropped into `flags` and `positionals`. See S-197.
pub(super) fn apply(usage_lines: &[String], flags: &mut [Entity], positionals: &mut Vec<Entity>) {
    let texts = split_forms(usage_lines);
    if texts.is_empty() {
        return;
    }
    let multi = texts.len() >= 2;
    let takes_value = |key: &str| {
        flags
            .iter()
            .any(|f| spells(f, key) && f.value_kind != ValueKind::None)
    };
    let read: Vec<(Vec<Occ>, Vec<Op>)> = texts.iter().map(|t| read_form(t, &takes_value)).collect();
    let (occs, ops): (Vec<_>, Vec<_>) = read.into_iter().unzip();
    for flag in flags.iter_mut().filter(|f| synopsis_only(f)) {
        let mine: Vec<&Occ> = occs
            .iter()
            .flatten()
            .filter(|o| spells(flag, &o.key))
            .collect();
        if !mine.is_empty() {
            apply_flag(flag, &mine, multi);
        }
    }
    if multi {
        apply_operands(&ops, positionals);
    }
}
