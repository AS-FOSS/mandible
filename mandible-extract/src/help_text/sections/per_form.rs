//! Per-usage-form reading (S-197). A synopsis that prints several
//! invocation forms (`ssh-keygen`'s thirty) gives one flag a different
//! placeholder in each, selects a form by a fixed word after one flag, and
//! ends each form with its own operands. The first-seen reading in
//! `usage.rs` keeps one placeholder per flag; this pass reads every form
//! and adds what that reading dropped. See corpus/ssh-keygen/9.6p1-forms.

use super::usage::cut_before_description_gap;
use mandible_core::{is_literal_choice_value, Choice, Entity, Source, ValueKind};

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

/// Split the usage lines into one text per invocation form, program name
/// removed.
fn split_forms(usage_lines: &[String]) -> Vec<String> {
    let mut forms: Vec<String> = Vec::new();
    let usage_lines: Vec<String> = usage_lines
        .iter()
        .map(|l| cut_before_description_gap(l.trim()).to_string())
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

fn read_group(inner: &str, head: bool, occs: &mut Vec<Occ>) {
    if inner.contains('[') || inner.contains('=') || !inner.starts_with('-') {
        return;
    }
    let parts: Vec<&str> = inner.split('|').map(str::trim).collect();
    let toks: Vec<&str> = parts[0].split_whitespace().collect();
    let Some(&key) = toks.first().filter(|k| is_flag_word(k)) else {
        return;
    };
    let value = toks.get(1).copied().filter(|v| is_plain_word(v));
    let shape_ok = toks.len() == 1 || (toks.len() == 2 && value.is_some());
    let alts_ok = parts[1..]
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
}

fn read_form(text: &str, takes_value: &dyn Fn(&str) -> bool) -> Vec<Occ> {
    let toks = tokenize(text);
    let mut occs = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        match &toks[i] {
            Tok::Group(inner) => read_group(inner, i == 0, &mut occs),
            Tok::Word(w) if w.starts_with('-') && is_flag_word(w) => {
                let value = match toks.get(i + 1) {
                    Some(Tok::Word(n)) if is_plain_word(n) && takes_value(w) => Some(n.clone()),
                    _ => None,
                };
                occs.push(Occ {
                    key: w.clone(),
                    value: value.clone(),
                    alts: Vec::new(),
                    head: i == 0,
                });
                i += usize::from(value.is_some());
            }
            Tok::Word(_) => {}
        }
        i += 1;
    }
    occs
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

/// Read every usage form of `usage_lines` and fold what the first-seen
/// reading dropped into `flags`. See S-197.
pub(super) fn apply(usage_lines: &[String], flags: &mut [Entity]) {
    let texts = split_forms(usage_lines);
    let multi = texts.len() >= 2;
    let takes_value = |key: &str| {
        flags
            .iter()
            .any(|f| spells(f, key) && f.value_kind != ValueKind::None)
    };
    let occs: Vec<Vec<Occ>> = texts.iter().map(|t| read_form(t, &takes_value)).collect();
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
}
