//! A usage flag followed by a run of bracketed bare words
//! (`--groups [eth-phy] [eth-mac] [eth-ctrl] [rmon]`) takes those words as
//! its literal values. The first word was read as the flag's value name.
//! Needs two or more lowercase words. See docs/shapes.md S-244 and
//! corpus/ethtool/6.7.

use super::*;

fn bracketed_word(tok: &str) -> Option<&str> {
    let word = tok.strip_prefix('[')?.strip_suffix(']')?;
    mandible_core::is_literal_choice_value(word).then_some(word)
}

fn word_run<'a>(usage_lines: &'a [String], typed: &str) -> Vec<&'a str> {
    for line in usage_lines {
        let toks: Vec<&str> = line.split_whitespace().collect();
        let Some(at) = toks.iter().position(|t| *t == typed) else {
            continue;
        };
        let run: Vec<&str> = toks[at + 1..]
            .iter()
            .map_while(|t| bracketed_word(t))
            .collect();
        if run.len() >= 2 {
            return run;
        }
    }
    Vec::new()
}

pub(super) fn repair(usage_lines: &[String], flags: &mut [Entity]) {
    for flag in flags.iter_mut() {
        let sole_synopsis = flag
            .provenance
            .sources
            .iter()
            .all(|s| matches!(s, Source::HelpTextSynopsis));
        let (Some(long), Some(name)) = (flag.long_spelling(), flag.value_name.as_deref()) else {
            continue;
        };
        if !sole_synopsis || flag.value_kind != ValueKind::Optional || !flag.choices.is_empty() {
            continue;
        }
        let run = word_run(usage_lines, &long.typed());
        if run.first() == Some(&name) {
            flag.choices = run.iter().map(|w| Choice::bare(*w)).collect();
            flag.value_name = None;
        }
    }
}
