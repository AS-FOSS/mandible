//! A row of three or more long spellings, most of which the same help
//! documents again as rows of their own, names independent options, not
//! aliases. The spellings documented elsewhere leave the row; the row keeps
//! the rest and its description. `docs/shapes.md` S-207,
//! `corpus/ld/2.42`.

use mandible_core::{Dashes, Entity};

/// Fewest spellings a row needs before it can be read as a list of
/// independent options.
const MIN_SPELLINGS: usize = 3;
/// Fewest spellings of the row that must be documented elsewhere.
const MIN_DOCUMENTED_ELSEWHERE: usize = 2;

fn all_long(e: &Entity) -> bool {
    e.spellings.iter().all(|s| s.dashes == Dashes::Double)
}

/// Drop from each qualifying multi-spelling flag the spellings that another
/// flag of the same node carries, keeping at least one.
pub(super) fn split_spellings_documented_elsewhere(flags: &mut [Entity]) {
    for i in 0..flags.len() {
        if flags[i].spellings.len() < MIN_SPELLINGS || !all_long(&flags[i]) {
            continue;
        }
        let elsewhere: Vec<bool> = flags[i]
            .spellings
            .iter()
            .map(|s| {
                flags
                    .iter()
                    .enumerate()
                    .any(|(j, other)| j != i && other.spellings.contains(s))
            })
            .collect();
        let count = elsewhere.iter().filter(|b| **b).count();
        if count < MIN_DOCUMENTED_ELSEWHERE || count == elsewhere.len() {
            continue;
        }
        let mut keep = elsewhere.iter().map(|b| !*b);
        flags[i].spellings.retain(|_| keep.next().unwrap_or(true));
    }
}
