//! Rows that only regrouped their spellings: `-a` and `--extract-all` on
//! two rows before, `-a,--extract-all` on one row after. An entity id is
//! `path::kind::spelling,spelling`, so a fold or a split changes every id it
//! touches and would otherwise read as rows removed plus rows added. Here
//! removed and added ids that share a spelling under one `path::kind` are
//! paired into one [`Regroup`]; what a regroup did lose is named on it: a
//! spelling no added row carries, or a description, values list or value
//! name that no added row carries any more.

use std::collections::{BTreeMap, BTreeSet};

use super::fingerprint::{ParsedFingerprint, ParsedFlagFingerprint};

/// Removed and added rows of one tool linked by shared spellings.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Regroup<'a> {
    pub(super) before: Vec<&'a str>,
    pub(super) after: Vec<&'a str>,
    /// Spellings on a removed row that no added row carries.
    pub(super) spellings_lost: Vec<String>,
    /// Spellings on an added row that no removed row carried.
    pub(super) spellings_gained: Vec<String>,
    /// Fields some removed row had and no added row has: `description`,
    /// `values`, `value name`.
    pub(super) dropped: Vec<&'static str>,
}

/// `path::kind` and the spellings of one entity id.
fn split_id(id: &str) -> (&str, Vec<&str>) {
    match id.rsplit_once("::") {
        Some((prefix, spellings)) => (prefix, spellings.split(',').collect()),
        None => ("", id.split(',').collect()),
    }
}

/// Every `(path::kind, spelling)` a fingerprint's non-positional ids carry.
pub(super) fn flag_spellings(fp: &ParsedFingerprint) -> BTreeSet<(String, String)> {
    fp.flags
        .keys()
        .filter(|id| !super::fingerprint::is_positional_id(id))
        .flat_map(|id| {
            let (prefix, spellings) = split_id(id);
            spellings
                .into_iter()
                .map(move |s| (prefix.to_string(), s.to_string()))
        })
        .collect()
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// Pairs `removed` and `added` ids sharing a spelling into regroups and
/// takes those ids out of both lists; ids that share nothing stay where
/// they are, as plain removals and additions.
pub(super) fn extract<'a>(
    before: &ParsedFingerprint,
    after: &ParsedFingerprint,
    removed: &mut Vec<&'a str>,
    added: &mut Vec<&'a str>,
) -> Vec<Regroup<'a>> {
    let ids: Vec<&'a str> = removed.iter().chain(added.iter()).copied().collect();
    let n_removed = removed.len();
    let mut parent: Vec<usize> = (0..ids.len()).collect();
    let mut owner: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for (i, id) in ids.iter().enumerate() {
        let (prefix, spellings) = split_id(id);
        for s in spellings {
            match owner.get(&(prefix, s)) {
                Some(&j) => {
                    let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                    parent[a] = b;
                }
                None => {
                    owner.insert((prefix, s), i);
                }
            }
        }
    }
    let mut groups: BTreeMap<usize, (Vec<usize>, Vec<usize>)> = BTreeMap::new();
    for i in 0..ids.len() {
        let root = find(&mut parent, i);
        let g = groups.entry(root).or_default();
        if i < n_removed {
            g.0.push(i);
        } else {
            g.1.push(i);
        }
    }
    let mut out = Vec::new();
    let mut taken = BTreeSet::new();
    for (rs, adds) in groups.into_values() {
        if rs.is_empty() || adds.is_empty() {
            continue;
        }
        taken.extend(rs.iter().chain(adds.iter()).copied());
        let before_ids: Vec<&'a str> = rs.iter().map(|&i| ids[i]).collect();
        let after_ids: Vec<&'a str> = adds.iter().map(|&i| ids[i]).collect();
        out.push(regroup(before, after, before_ids, after_ids));
    }
    let mut k = 0;
    removed.retain(|_| {
        k += 1;
        !taken.contains(&(k - 1))
    });
    added.retain(|_| {
        k += 1;
        !taken.contains(&(k - 1))
    });
    out
}

fn spelling_set<'a>(ids: &[&'a str]) -> BTreeSet<&'a str> {
    ids.iter().flat_map(|id| split_id(id).1).collect()
}

fn regroup<'a>(
    before: &ParsedFingerprint,
    after: &ParsedFingerprint,
    before_ids: Vec<&'a str>,
    after_ids: Vec<&'a str>,
) -> Regroup<'a> {
    let (was, now) = (spelling_set(&before_ids), spelling_set(&after_ids));
    fn fields<'f>(fp: &'f ParsedFingerprint, ids: &[&str]) -> Vec<&'f ParsedFlagFingerprint> {
        ids.iter().filter_map(|id| fp.flags.get(*id)).collect()
    }
    let (old, new) = (fields(before, &before_ids), fields(after, &after_ids));
    let lost = |has: fn(&ParsedFlagFingerprint) -> bool| {
        old.iter().any(|f| has(f)) && !new.iter().any(|f| has(f))
    };
    let mut dropped = Vec::new();
    if lost(|f| f.has_description) {
        dropped.push("description");
    }
    if lost(|f| f.choices_hash.is_some()) {
        dropped.push("values");
    }
    if lost(|f| f.value_name.is_some()) {
        dropped.push("value name");
    }
    Regroup {
        spellings_lost: was.difference(&now).map(|s| s.to_string()).collect(),
        spellings_gained: now.difference(&was).map(|s| s.to_string()).collect(),
        before: before_ids,
        after: after_ids,
        dropped,
    }
}

/// One regroup as a report line: `a + b -> a,b`, then what it lost.
pub(super) fn describe(r: &Regroup) -> String {
    let mut line = format!("{} -> {}", r.before.join(" + "), r.after.join(" + "));
    if !r.spellings_lost.is_empty() {
        line.push_str(&format!(
            " [spellings lost: {}]",
            r.spellings_lost.join(" ")
        ));
    }
    if !r.spellings_gained.is_empty() {
        line.push_str(&format!(
            " [spellings gained: {}]",
            r.spellings_gained.join(" ")
        ));
    }
    if !r.dropped.is_empty() {
        line.push_str(&format!(" [dropped: {}]", r.dropped.join(", ")));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(rows: &[(&str, bool)]) -> ParsedFingerprint {
        let mut f = ParsedFingerprint::default();
        for (id, described) in rows {
            f.flags.insert(
                (*id).to_string(),
                ParsedFlagFingerprint {
                    has_description: *described,
                    description_hash: described.then_some(1),
                    choices_hash: None,
                    value_name: None,
                    join: None,
                },
            );
        }
        f
    }

    #[test]
    fn a_fold_is_one_regroup_and_leaves_no_removal() {
        let b = fp(&[("(root)::Flag::-a", false), ("(root)::Flag::--all", true)]);
        let a = fp(&[("(root)::Flag::-a,--all", true)]);
        let mut removed = vec!["(root)::Flag::--all", "(root)::Flag::-a"];
        let mut added = vec!["(root)::Flag::-a,--all"];
        let r = extract(&b, &a, &mut removed, &mut added);
        assert!(removed.is_empty() && added.is_empty());
        assert_eq!(r.len(), 1);
        assert!(r[0].spellings_lost.is_empty() && r[0].dropped.is_empty());
    }

    #[test]
    fn a_fold_that_drops_a_spelling_or_the_text_says_so() {
        let b = fp(&[("(root)::Flag::-a,--all", true)]);
        let a = fp(&[("(root)::Flag::-a,--al", false)]);
        let mut removed = vec!["(root)::Flag::-a,--all"];
        let mut added = vec!["(root)::Flag::-a,--al"];
        let r = extract(&b, &a, &mut removed, &mut added);
        assert_eq!(r[0].spellings_lost, vec!["--all"]);
        assert_eq!(r[0].spellings_gained, vec!["--al"]);
        assert_eq!(r[0].dropped, vec!["description"]);
    }

    #[test]
    fn rows_sharing_no_spelling_stay_plain_removals_and_additions() {
        let b = fp(&[("(root)::Flag::-x", true)]);
        let a = fp(&[("(root)::Flag::-y", true)]);
        let mut removed = vec!["(root)::Flag::-x"];
        let mut added = vec!["(root)::Flag::-y"];
        assert!(extract(&b, &a, &mut removed, &mut added).is_empty());
        assert_eq!((removed.len(), added.len()), (1, 1));
    }

    #[test]
    fn the_same_spelling_on_another_page_or_kind_is_not_shared() {
        let b = fp(&[("sub::Flag::-a", true)]);
        let a = fp(&[("(root)::Flag::-a,--all", true)]);
        let mut removed = vec!["sub::Flag::-a"];
        let mut added = vec!["(root)::Flag::-a,--all"];
        assert!(extract(&b, &a, &mut removed, &mut added).is_empty());
    }
}
