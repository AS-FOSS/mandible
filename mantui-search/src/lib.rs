//! `mantui-search`: fuzzy search over commands and flags (spec §10).
//!
//! Backed by `nucleo` (the matcher behind Helix). Index entries are
//! [`NodeRef`]s — including [`NodeRef::Flag`], so a flag is a first-class,
//! independently addressable search result rather than something folded
//! into its parent command's haystack. Matching runs on `nucleo`'s own
//! background thread pool; [`SearchIndex::tick`] must be driven from the
//! caller's event-loop poll timeout, never a blocking spin inside a
//! keystroke handler, so typing never blocks (spec §10 "Threading").

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use mantui_core::{CommandNode, NodeRef};
use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config, Nucleo};
use std::sync::Arc;

/// One indexed item: a command or a flag, addressable via its [`NodeRef`].
#[derive(Debug, Clone)]
struct Entry {
    node_ref: NodeRef,
    /// The name used for the exact-prefix ranking boost (spec §10
    /// "Ranking"): a command's own name, or a flag's long spelling (short,
    /// if there's no long one).
    primary_name: String,
}

/// A live, incrementally-updatable fuzzy search index over one tool's
/// command tree.
pub struct SearchIndex {
    nucleo: Nucleo<Entry>,
    current_query: String,
}

impl Default for SearchIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchIndex {
    /// Build an empty index. Call [`SearchIndex::populate`] to fill it.
    pub fn new() -> SearchIndex {
        // `notify` exists for callers that want to wake a redraw the
        // moment new results are available; this crate's contract is
        // instead "the caller drives `tick` from its own poll timeout"
        // (spec §10), so no wakeup callback is needed here.
        let notify: Arc<dyn Fn() + Sync + Send> = Arc::new(|| {});
        let nucleo = Nucleo::new(Config::DEFAULT, notify, None, 1);
        SearchIndex {
            nucleo,
            current_query: String::new(),
        }
    }

    /// Rebuild the index from `root`. Call whenever the tree's structure
    /// changes (initial load, or after a lazy fill splices in new nodes) —
    /// cheap enough to call on every such change; `nucleo` streams items in
    /// lock-free, and re-populating from scratch is simpler and less
    /// error-prone than trying to diff two trees.
    pub fn populate(&mut self, root: &CommandNode) {
        self.nucleo.restart(true);
        let injector = self.nucleo.injector();
        push_node(&injector, root, vec![root.name.clone()]);
    }

    /// Set the live query text. Matching happens asynchronously on
    /// `nucleo`'s background pool; call [`SearchIndex::tick`] to let it
    /// make progress and [`SearchIndex::results`] to read the latest
    /// available ranking.
    pub fn set_query(&mut self, query: &str) {
        if query == self.current_query {
            return;
        }
        let append = query.starts_with(&self.current_query);
        self.nucleo
            .pattern
            .reparse(0, query, CaseMatching::Ignore, Normalization::Smart, append);
        self.current_query = query.to_string();
    }

    /// Drive the background matcher forward, waiting at most `timeout_ms`.
    /// Returns `true` if the result snapshot changed (the caller should
    /// treat this as "results may need re-reading, tree may need
    /// rebuilding"). Must be called regularly from the event loop's own
    /// poll timeout — never as a blocking spin inside a keystroke handler
    /// (spec §10 "Threading").
    pub fn tick(&mut self, timeout_ms: u64) -> bool {
        self.nucleo.tick(timeout_ms).changed
    }

    /// The current query's matches, ranked: `nucleo`'s own fuzzy score,
    /// with items whose name starts with the query (case-insensitive)
    /// stably promoted ahead of items that only matched elsewhere (spec
    /// §10 "Ranking": "Boost exact prefix matches on names above
    /// description matches").
    pub fn results(&self) -> Vec<NodeRef> {
        let snapshot = self.nucleo.snapshot();
        let n = snapshot.matched_item_count();
        let query_lower = self.current_query.to_lowercase();

        let mut ranked: Vec<(bool, usize, NodeRef)> = snapshot
            .matched_items(0..n)
            .enumerate()
            .map(|(rank, item)| {
                let is_prefix = !query_lower.is_empty()
                    && item
                        .data
                        .primary_name
                        .to_lowercase()
                        .starts_with(&query_lower);
                // `!is_prefix` so prefix matches (false) sort before
                // everything else (true) in the ascending sort below.
                (!is_prefix, rank, item.data.node_ref.clone())
            })
            .collect();
        // Stable sort: within "is prefix" / "isn't", nucleo's own
        // score-based order (captured by `rank`) is preserved.
        ranked.sort_by_key(|(is_not_prefix, rank, _)| (*is_not_prefix, *rank));
        ranked
            .into_iter()
            .map(|(_, _, node_ref)| node_ref)
            .collect()
    }
}

fn push_node(injector: &nucleo::Injector<Entry>, node: &CommandNode, path: Vec<String>) {
    let mut haystack = node.name.clone();
    for alias in &node.aliases {
        haystack.push(' ');
        haystack.push_str(alias);
    }
    if let Some(summary) = &node.summary {
        haystack.push(' ');
        haystack.push_str(summary.as_str());
    }
    let entry = Entry {
        node_ref: NodeRef::Command(path.clone()),
        primary_name: node.name.clone(),
    };
    injector.push(entry, |_entry, cols| {
        cols[0] = haystack.as_str().into();
    });

    for flag in &node.flags {
        push_flag(injector, flag, &path);
    }

    for child in &node.subcommands {
        let mut child_path = path.clone();
        child_path.push(child.name.clone());
        push_node(injector, child, child_path);
    }
}

fn push_flag(injector: &nucleo::Injector<Entry>, flag: &mantui_core::Flag, path: &[String]) {
    let Some(key) = flag.key() else {
        return; // a flag with neither short nor long spelling can't be addressed
    };
    let mut haystack = String::new();
    if let Some(s) = flag.short {
        haystack.push('-');
        haystack.push(s);
        haystack.push(' ');
    }
    if let Some(l) = &flag.long {
        haystack.push_str("--");
        haystack.push_str(l);
        haystack.push(' ');
    }
    if let Some(v) = &flag.value_name {
        haystack.push_str(v);
        haystack.push(' ');
    }
    if let Some(d) = &flag.description {
        haystack.push_str(d.as_str());
    }
    let primary_name = flag
        .long
        .clone()
        .or_else(|| flag.short.map(|c| c.to_string()))
        .unwrap_or_default();
    let entry = Entry {
        node_ref: NodeRef::Flag {
            path: path.to_vec(),
            key,
        },
        primary_name,
    };
    injector.push(entry, |_entry, cols| {
        cols[0] = haystack.as_str().into();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use mantui_core::{Flag, Provenance, Source, Text, ValueKind};
    use std::time::{Duration, Instant};

    fn flag(short: Option<char>, long: Option<&str>, description: &str) -> Flag {
        Flag {
            short,
            long: long.map(|s| s.to_string()),
            value_name: None,
            value_kind: ValueKind::None,
            choices: Vec::new(),
            repeatable: false,
            required: false,
            hidden: false,
            deprecated: None,
            inherited: false,
            group: None,
            description: Some(Text::sanitize(description)),
            default: None,
            env_var: None,
            provenance: Provenance::single(Source::HelpText),
        }
    }

    fn sample_tree() -> CommandNode {
        let mut root = CommandNode::new("git", Provenance::single(Source::HelpText));
        root.summary = Some(Text::sanitize("the stupid content tracker"));

        let mut rebase = CommandNode::new("rebase", Provenance::single(Source::HelpText));
        rebase.summary = Some(Text::sanitize("Reapply commits on top of another base tip"));
        rebase.flags.push(flag(
            Some('i'),
            Some("interactive"),
            "Make a list of commits",
        ));
        rebase.flags.push(flag(
            None,
            Some("autosquash"),
            "Automatically squash commits",
        ));

        let mut add = CommandNode::new("add", Provenance::single(Source::HelpText));
        add.summary = Some(Text::sanitize("Add file contents to the index"));
        add.flags
            .push(flag(Some('p'), Some("patch"), "Interactively choose hunks"));

        root.subcommands.push(rebase);
        root.subcommands.push(add);
        root
    }

    /// Drive `tick` until the snapshot stops changing for a few
    /// consecutive polls (bounded overall, so a bug can't hang the test
    /// suite) — mirrors how the real event loop is expected to call
    /// `tick` repeatedly rather than assuming one call finishes the
    /// match, but exits promptly once settled instead of always
    /// consuming the full deadline.
    fn settle(index: &mut SearchIndex) {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut quiet_polls = 0;
        while Instant::now() < deadline && quiet_polls < 3 {
            if index.tick(20) {
                quiet_polls = 0;
            } else {
                quiet_polls += 1;
            }
        }
    }

    #[test]
    fn searching_a_flag_spelling_returns_the_flag_not_the_command() {
        let mut index = SearchIndex::new();
        index.populate(&sample_tree());
        index.set_query("autosquash");
        settle(&mut index);

        let results = index.results();
        assert!(!results.is_empty(), "expected at least one match");
        match &results[0] {
            NodeRef::Flag { path, key } => {
                assert_eq!(path, &vec!["git".to_string(), "rebase".to_string()]);
                assert_eq!(key, &mantui_core::FlagKey::Long("autosquash".to_string()));
            }
            other => panic!("expected the top match to be the --autosquash flag, got {other:?}"),
        }
    }

    #[test]
    fn flags_are_independently_addressable_not_folded_into_parent_command() {
        let mut index = SearchIndex::new();
        index.populate(&sample_tree());
        index.set_query("patch");
        settle(&mut index);

        let results = index.results();
        let has_flag_match = results
            .iter()
            .any(|r| matches!(r, NodeRef::Flag { key, .. } if key == &mantui_core::FlagKey::Long("patch".to_string())));
        assert!(
            has_flag_match,
            "expected --patch flag among results: {results:?}"
        );
    }

    #[test]
    fn exact_name_prefix_match_ranks_above_description_only_match() {
        let mut index = SearchIndex::new();
        let mut root = sample_tree();
        // A third command whose *description* (not name) contains "reb",
        // to prove a name-prefix match on `rebase` still wins.
        let mut decoy = CommandNode::new("zzz", Provenance::single(Source::HelpText));
        decoy.summary = Some(Text::sanitize(
            "something about a rebar (not a typo test decoy)",
        ));
        root.subcommands.push(decoy);
        index.populate(&root);

        index.set_query("reb");
        settle(&mut index);

        let results = index.results();
        let rebase_pos = results.iter().position(
            |r| matches!(r, NodeRef::Command(p) if p.last().map(|s| s.as_str()) == Some("rebase")),
        );
        let decoy_pos = results.iter().position(
            |r| matches!(r, NodeRef::Command(p) if p.last().map(|s| s.as_str()) == Some("zzz")),
        );
        if let (Some(rp), Some(dp)) = (rebase_pos, decoy_pos) {
            assert!(rp < dp, "name-prefix match on 'rebase' should rank above a description-only match: {results:?}");
        }
    }

    #[test]
    fn empty_query_matches_are_stable_and_do_not_panic() {
        let mut index = SearchIndex::new();
        index.populate(&sample_tree());
        index.set_query("");
        settle(&mut index);
        // Should not panic; an empty pattern in nucleo matches everything.
        let _ = index.results();
    }

    #[test]
    fn repopulating_after_a_query_still_works() {
        let mut index = SearchIndex::new();
        index.populate(&sample_tree());
        index.set_query("rebase");
        settle(&mut index);
        assert!(!index.results().is_empty());

        // Simulate a lazy fill changing the tree structure.
        let mut updated = sample_tree();
        updated.subcommands[0].summary =
            Some(Text::sanitize("Reapply commits, now with more detail"));
        index.populate(&updated);
        settle(&mut index);
        assert!(
            !index.results().is_empty(),
            "results should still be findable after repopulating"
        );
    }
}
