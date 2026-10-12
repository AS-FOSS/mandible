//! A glued two-letter bracket group in a usage block that spells every
//! flag value after a space: `ssh-keygen ... [-a rounds] [-f keyfile] [-hU]`.
//!
//! `[-hU]` reads as `-h` with the glued value `U` unless a bundle of three
//! or more members vouches for it (`MIN_CLUSTER_MEMBERS`, S-087: two-letter
//! groups are half real flags elsewhere). The block itself can vouch
//! instead. When it (1) writes at least one flag value after a space,
//! (2) has no flag of two or more letters at all (`-it intable`, `-mdoc`,
//! `-scd`: single-dash words exist there, so `-nt` may be one), and (3) never
//! takes a value for either member on its own (`jstatd`'s `[-nr]` beside
//! `-n rminame`), a glued group of two distinct characters can only be two
//! switches. See docs/shapes.md S-242.

use std::collections::HashSet;

use super::parse_bundled_shorts;

pub(super) type SwitchPairs = HashSet<String>;

/// The switches `token` names: the two of a vouched-for pair, else the
/// members of an ordinary bundle.
pub(super) fn pair_or_bundle(token: &str, pairs: &SwitchPairs) -> Option<Vec<char>> {
    if pairs.contains(token) {
        return token.strip_prefix('-').map(|m| m.chars().collect());
    }
    parse_bundled_shorts(token)
}

fn strip(token: &str) -> &str {
    token.trim_matches(|c| c == '[' || c == ']' || c == ',')
}

/// The characters of `-ab` (a dash and exactly two ASCII alphanumerics).
fn glued_pair(word: &str) -> Option<[char; 2]> {
    let rest = word.strip_prefix('-')?;
    let mut cs = rest.chars();
    let (a, b) = (cs.next()?, cs.next()?);
    (cs.next().is_none() && a != b && a.is_ascii_alphanumeric() && b.is_ascii_alphanumeric())
        .then_some([a, b])
}

fn dash_word(word: &str) -> Option<&str> {
    let rest = word.strip_prefix('-')?;
    (!rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric())).then_some(rest)
}

/// The glued two-letter tokens of `usage_lines` that are two switches.
pub(super) fn switch_pairs(usage_lines: &[String]) -> SwitchPairs {
    let mut valued: HashSet<char> = HashSet::new();
    let mut candidates: Vec<(String, [char; 2])> = Vec::new();
    let mut word_flags = false;
    for line in usage_lines {
        let toks: Vec<&str> = line.split_whitespace().collect();
        for (i, tok) in toks.iter().enumerate() {
            let opened = tok.trim_start_matches('[');
            let closed_here = opened.ends_with(']');
            let bare = opened.trim_end_matches(']');
            let Some(name) = dash_word(bare) else {
                continue;
            };
            if name.chars().count() == 1 {
                let value = toks.get(i + 1).map(|n| n.trim_start_matches('['));
                let spaced = !closed_here
                    && value.is_some_and(|v| {
                        !v.is_empty() && !v.starts_with(['-', '|', '(', ')', ']'])
                    });
                if spaced {
                    valued.extend(name.chars());
                }
            } else if let Some(pair) = glued_pair(bare) {
                let closed = tok.starts_with('[') && tok.ends_with(']');
                if closed {
                    candidates.push((strip(tok).to_string(), pair));
                } else {
                    word_flags = true;
                }
            } else if parse_bundled_shorts(bare).is_none() {
                word_flags = true;
            }
        }
    }
    if valued.is_empty() || word_flags {
        return HashSet::new();
    }
    candidates
        .into_iter()
        .filter(|(_, [a, b])| !valued.contains(a) && !valued.contains(b))
        .map(|(tok, _)| tok)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(l: &[&str]) -> Vec<String> {
        l.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_glued_pair_beside_spaced_values_is_two_switches() {
        let u = lines(&["ssh-keygen -s ca_key [-hU] [-D pkcs11_provider]"]);
        assert!(switch_pairs(&u).contains("-hU"));
    }

    #[test]
    fn no_spaced_value_no_split() {
        assert!(switch_pairs(&lines(&["lessecho [-ox] [-cx] [-a] file"])).is_empty());
    }

    #[test]
    fn a_two_letter_flag_with_a_value_keeps_every_pair_whole() {
        let u = lines(&["psfxtable [-i infont] [-it intable] [-nt]"]);
        assert!(switch_pairs(&u).is_empty());
    }

    #[test]
    fn a_member_that_takes_its_own_value_keeps_the_pair_whole() {
        let u = lines(&["jstatd [-nr] [-p port] [-r rmiport] [-n rminame]"]);
        assert!(switch_pairs(&u).is_empty());
    }
}
