//! The dim suggestion in the search box (spec §10): the rest of the
//! spelling the cursor jumps to, or the closest name when nothing matched.

use crate::search_rank::{best_name, PagePositions};
use mandible_core::{CommandNode, NodeRef};

/// The most edits between the input and a suggested name.
const MAX_EDITS: usize = 2;

/// What `Tab` would put in the box, and what is drawn after the typed text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ghost {
    /// The input this suggestion was worked out for.
    pub(crate) input: String,
    /// Whether it completes the input or replaces it with a close name.
    pub(crate) kind: GhostKind,
    /// The whole suggested spelling or command name.
    pub(crate) accept: String,
}

/// How a suggestion relates to the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GhostKind {
    /// The jump target's spelling starts with the input.
    Completes,
    /// Nothing matched; this is the nearest name.
    Closest,
}

impl Ghost {
    /// The text that follows what is typed: the rest of the spelling, or
    /// the arrow and the closest name.
    pub(crate) fn shown(&self, arrow: &str) -> String {
        match self.kind {
            GhostKind::Completes => self
                .accept
                .chars()
                .skip(self.input.chars().count())
                .collect(),
            GhostKind::Closest => format!("   {arrow} {}", self.accept),
        }
    }
}

/// Whether `name` starts with `input`, ignoring case.
fn starts_with_ignoring_case(name: &str, input: &str) -> bool {
    let mut name = name.chars();
    input.chars().all(|c| {
        name.next()
            .is_some_and(|n| n.to_lowercase().eq(c.to_lowercase()))
    })
}

/// The suggestion for a search whose first jump target is `first`: the
/// rest of its best spelling, when that spelling starts with the input
/// and is longer than it.
pub(crate) fn completion(root: &CommandNode, first: &NodeRef, input: &str) -> Option<Ghost> {
    let name = best_name(root, first, input)?;
    let longer = name.chars().count() > input.chars().count();
    (longer && starts_with_ignoring_case(&name, input)).then(|| Ghost {
        input: input.to_string(),
        kind: GhostKind::Completes,
        accept: name,
    })
}

/// The command name or spelling with the fewest edits from `input`, at
/// most [`MAX_EDITS`]; ties go to the earlier row in the tree, then the
/// earlier line on its page.
pub(crate) fn closest(root: &CommandNode, input: &str) -> Option<Ghost> {
    let wanted = input.to_lowercase();
    let mut best: Option<(usize, usize)> = None;
    let mut found: Vec<(usize, usize, u8, NodeRef, String)> = Vec::new();
    let mut order = 0usize;
    collect(
        root,
        vec![root.name.clone()],
        &wanted,
        &mut order,
        &mut found,
    );
    for (dist, pos, ..) in &found {
        if best.is_none_or(|b| (*dist, *pos) < b) {
            best = Some((*dist, *pos));
        }
    }
    let (dist, pos) = best?;
    let mut pages = PagePositions::default();
    found
        .into_iter()
        .filter(|(d, p, ..)| (*d, *p) == (dist, pos))
        .map(|(_, _, kind, item, name)| ((kind, pages.of(root, &item)), name))
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, accept)| Ghost {
            input: input.to_string(),
            kind: GhostKind::Closest,
            accept,
        })
}

/// Every name within reach of `wanted` under `node`, in tree order, as
/// `(edits, tree position, kind, hit, name)`.
fn collect(
    node: &CommandNode,
    path: Vec<String>,
    wanted: &str,
    order: &mut usize,
    out: &mut Vec<(usize, usize, u8, NodeRef, String)>,
) {
    let pos = *order;
    *order += 1;
    let mut offer = |kind: u8, item: NodeRef, name: String| {
        let dist = strsim::levenshtein(wanted, &name.to_lowercase());
        if dist <= MAX_EDITS {
            out.push((dist, pos, kind, item, name));
        }
    };
    offer(0, NodeRef::Command(path.clone()), node.name.clone());
    for entity in &node.entities {
        let Some(key) = entity.key() else { continue };
        for form in mandible_search::entity_forms(entity) {
            offer(
                1,
                NodeRef::Flag {
                    path: path.clone(),
                    key: key.clone(),
                },
                form,
            );
        }
    }
    for child in &node.subcommands {
        let mut p = path.clone();
        p.push(child.name.clone());
        collect(child, p, wanted, order, out);
    }
}
