//! `mandible-search`: fuzzy search over commands and every entity a node
//! carries — flags, positionals, modifiers, and environment variables
//! (spec §10).
//!
//! Backed by `nucleo` (the matcher behind Helix). Index entries are
//! [`NodeRef`]s — including [`NodeRef::Flag`], whose `key` addresses any
//! entity kind (a dashed flag, or a dashless positional/modifier/env-var
//! by its bare name — see [`mandible_core::FlagKey::Name`]), so every
//! entity is a first-class, independently addressable search result
//! rather than something folded into its parent command's haystack.
//! Matching runs on `nucleo`'s own
//! background thread pool; [`SearchIndex::tick`] must be driven from the
//! caller's event-loop poll timeout, never a blocking spin inside a
//! keystroke handler, so typing never blocks (spec §10 "Threading").

#![forbid(unsafe_code)]
// Size ceilings. Both lints are opt-in; the thresholds live in the
// workspace `clippy.toml`, and CI's `-D warnings` makes them gates.
#![warn(clippy::too_many_lines)]
#![warn(clippy::cognitive_complexity)]
#![warn(missing_docs)]

use mandible_core::{CommandNode, NodeRef};
use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config, Matcher, Nucleo, Utf32Str};
use std::sync::Arc;

/// One indexed item: a command or an entity (flag, positional, modifier, or
/// environment variable), addressable via its [`NodeRef`].
#[derive(Debug, Clone)]
struct Entry {
    node_ref: NodeRef,
    /// The name used for the exact-prefix ranking boost (spec §10
    /// "Ranking"): a command's own name, or an entity's first documented
    /// spelling's bare name ([`mandible_core::Entity::primary_name`]).
    primary_name: String,
}

/// A live, incrementally-updatable fuzzy search index over one tool's
/// command tree.
pub struct SearchIndex {
    nucleo: Nucleo<Entry>,
    current_query: String,
    /// The text `nucleo` was last given, which in wide mode is the query
    /// rewritten as substring atoms.
    pattern_text: String,
    wide: bool,
    populate_count: u64,
    settled: bool,
}

/// Upper bound on `nucleo`'s background matcher threads.
///
/// `nucleo` defaults to `available_parallelism()`, i.e. one matcher thread
/// per core. Every [`SearchIndex::populate`] restarts the match, so each
/// rebuild hands the whole item set back to all of those threads at once;
/// on a 64-core machine that is 64 threads re-scoring the same few
/// thousand short haystacks. The item set here is bounded by spec §5.2's
/// 4,096-node cap (a few thousand entries of a dozen-odd characters), so
/// past a handful of threads the coordination costs more than the parallel
/// scoring saves — and pegging every core on a machine whose owner is
/// reading a man page is the failure this cap exists to prevent. Not the
/// primary fix for that (see [`SearchIndex::populate`]'s note on rebuild
/// frequency); a second, cheap bound on the blast radius of each rebuild.
const MAX_MATCHER_THREADS: usize = 4;

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
        let threads = std::thread::available_parallelism()
            .map_or(MAX_MATCHER_THREADS, |n| n.get().min(MAX_MATCHER_THREADS));
        let nucleo = Nucleo::new(Config::DEFAULT, notify, Some(threads), 1);
        SearchIndex {
            nucleo,
            current_query: String::new(),
            pattern_text: String::new(),
            wide: false,
            populate_count: 0,
            settled: true,
        }
    }

    /// Rebuild the index from `root`, from scratch: `restart` drops the old
    /// item set and every entry is re-injected.
    ///
    /// **This is O(tree), not O(change), and it is not free — do not call it
    /// once per spliced node.** Re-populating from scratch is still the
    /// right shape (diffing two trees is much harder to get right, and
    /// `nucleo` streams items in lock-free), but the cost has to be paid
    /// per *batch* of structural changes rather than per change. Calling it
    /// per node made `mandible docker` peg every core for its whole
    /// 22-second warm: the background warmer delivers one splice per filled
    /// node (~255 for docker, up to spec §5.2's 4,096-node cap), each
    /// splice restarted the match and re-injected an ever-growing item set,
    /// and `nucleo`'s matcher threads re-scored all of it each time — O(n²)
    /// with a large constant. `mandible-tui`'s `App` therefore coalesces:
    /// it marks the index stale and rebuilds at most once per event-loop
    /// iteration (see `App::flush_search_index`).
    ///
    /// Leaves the current query untouched — only the item set changes — so
    /// an active search simply re-matches against the fresh data on the
    /// next [`SearchIndex::tick`].
    pub fn populate(&mut self, root: &CommandNode) {
        self.populate_count += 1;
        self.settled = false;
        self.nucleo.restart(true);
        let injector = self.nucleo.injector();
        push_node(&injector, root, vec![root.name.clone()]);
    }

    /// How many times [`SearchIndex::populate`] has run on this index.
    ///
    /// A seam, not a statistic: "the index is rebuilt per batch of warmed
    /// nodes, not per node" is a performance property with no visible
    /// effect on any result, so nothing else a test can read distinguishes
    /// the fixed code from the O(n²) version it replaced. Counting the
    /// rebuilds is what makes that property assertable — see
    /// `mandible-tui`'s `splicing_many_nodes_rebuilds_the_index_once`.
    pub fn populate_count(&self) -> u64 {
        self.populate_count
    }

    /// Set the live query text. Matching happens asynchronously on
    /// `nucleo`'s background pool; call [`SearchIndex::tick`] to let it
    /// make progress and [`SearchIndex::results`] to read the latest
    /// available ranking.
    pub fn set_query(&mut self, query: &str) {
        if query == self.current_query {
            return;
        }
        self.current_query = query.to_string();
        self.reparse();
    }

    /// Choose how the query matches. Name mode (`false`) hands `nucleo` the
    /// raw query, a fuzzy superset that the caller narrows to word starts.
    /// Wide mode (`true`) requires every space-separated word of the query
    /// to occur as connected text, and ranks the hits where every word
    /// starts a word first (spec §10).
    pub fn set_wide(&mut self, wide: bool) {
        if wide == self.wide {
            return;
        }
        self.wide = wide;
        self.reparse();
    }

    fn reparse(&mut self) {
        let text = if self.wide {
            substring_atoms(&self.current_query)
        } else {
            self.current_query.clone()
        };
        if text == self.pattern_text {
            return;
        }
        self.settled = false;
        let append = text.starts_with(&self.pattern_text);
        self.nucleo
            .pattern
            .reparse(0, &text, CaseMatching::Ignore, Normalization::Smart, append);
        self.pattern_text = text;
    }

    /// Drive the background matcher forward, waiting at most `timeout_ms`.
    /// Returns `true` if the result snapshot changed (the caller should
    /// treat this as "results may need re-reading, tree may need
    /// rebuilding"). Must be called regularly from the event loop's own
    /// poll timeout — never as a blocking spin inside a keystroke handler
    /// (spec §10 "Threading").
    pub fn tick(&mut self, timeout_ms: u64) -> bool {
        let status = self.nucleo.tick(timeout_ms);
        self.settled = !status.running;
        status.changed
    }

    /// Whether the snapshot [`SearchIndex::results`] reads is the finished
    /// match of the current items and query.
    ///
    /// `false` from the moment [`SearchIndex::populate`] or a changed
    /// [`SearchIndex::set_query`] runs until a [`SearchIndex::tick`] reports
    /// that `nucleo`'s matcher is no longer running (`Status::running` is
    /// false). Until then the snapshot is empty or partial, so an empty
    /// result list says nothing about whether anything matches.
    pub fn is_settled(&self) -> bool {
        self.settled
    }

    /// The current query's matches, ranked: items whose name starts with
    /// the query (case-insensitive) first (spec §10 "Ranking": "Boost exact
    /// prefix matches on names above description matches"), then, in wide
    /// mode, items where every word of the query starts a word of the
    /// haystack, and within each group `nucleo`'s own score order.
    pub fn results(&self) -> Vec<NodeRef> {
        let snapshot = self.nucleo.snapshot();
        let n = snapshot.matched_item_count();
        let query_lower = self.current_query.to_lowercase();
        let words: Vec<Vec<char>> = query_lower
            .split_whitespace()
            .map(|w| w.chars().collect())
            .collect();

        let mut ranked: Vec<(bool, bool, usize, NodeRef)> = snapshot
            .matched_items(0..n)
            .enumerate()
            .map(|(rank, item)| {
                let is_prefix = !query_lower.is_empty()
                    && item
                        .data
                        .primary_name
                        .to_lowercase()
                        .starts_with(&query_lower);
                let at_word_start = self.wide
                    && item
                        .matcher_columns
                        .first()
                        .is_some_and(|col| every_word_starts_a_word(col, &words));
                // `!` so the better group (true) sorts first in the
                // ascending sort below.
                (!is_prefix, !at_word_start, rank, item.data.node_ref.clone())
            })
            .collect();
        ranked.sort_by_key(|(not_prefix, not_word, rank, _)| (*not_prefix, *not_word, *rank));
        ranked
            .into_iter()
            .map(|(_, _, _, node_ref)| node_ref)
            .collect()
    }
}

/// The query as `nucleo` pattern text with every word an exact substring
/// atom: a leading `'` makes the atom a substring match, and a trailing
/// `$` or backslash is made literal so it cannot turn the atom into
/// another kind or glue two words together.
fn substring_atoms(query: &str) -> String {
    let words: Vec<String> = query
        .split_whitespace()
        .map(|w| w.trim_end_matches('\\'))
        .filter(|w| !w.is_empty())
        .map(|w| match w.strip_suffix('$') {
            Some(rest) => format!("'{rest}\\$"),
            None => format!("'{w}"),
        })
        .collect();
    words.join(" ")
}

/// Whether each of `words` (lowercase) occurs in the haystack at the start
/// of a word: the first character, or one after a character that is not a
/// letter or digit.
fn every_word_starts_a_word(haystack: &nucleo::Utf32String, words: &[Vec<char>]) -> bool {
    let text: Vec<char> = haystack
        .slice(..)
        .chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect();
    words.iter().all(|word| {
        !word.is_empty()
            && (0..text.len()).any(|at| {
                (at == 0 || !text[at - 1].is_alphanumeric()) && text[at..].starts_with(word)
            })
    })
}

/// The character indices within `name` the words of `query` occupy as
/// connected text, for highlighting in wide mode (spec §10). Each word
/// marks its first occurrence at the start of a word, else its first
/// occurrence anywhere. Case-insensitive; a word that is not in `name`
/// marks nothing.
pub fn substring_indices(name: &str, query: &str) -> Vec<u32> {
    let text: Vec<char> = name
        .chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect();
    let mut out = std::collections::BTreeSet::new();
    for word in query.to_lowercase().split_whitespace() {
        let word: Vec<char> = word.chars().collect();
        if word.is_empty() || word.len() > text.len() {
            continue;
        }
        let at = |start: &usize| text[*start..].starts_with(&word);
        let starts = 0..=text.len() - word.len();
        let boundary = |s: &usize| *s == 0 || !text[*s - 1].is_alphanumeric();
        let found = starts
            .clone()
            .filter(boundary)
            .find(at)
            .or_else(|| starts.clone().find(at));
        if let Some(start) = found {
            out.extend((start..start + word.len()).map(|i| i as u32));
        }
    }
    out.into_iter().collect()
}

/// The character indices within `name` (not the full search haystack)
/// that matched `query`, for highlighting (spec §9.2: "underline matched
/// characters... within the name only"). This deliberately re-runs a
/// fresh, independent match scoped to just the display name, rather than
/// reusing the ranking match against the full haystack (which also scores
/// aliases/summary/description for commands, or short/long/value/
/// description for flags) — that match's indices are offsets into a
/// haystack the caller never sees, and would need adjusting per entry
/// kind to mean anything. Matching just the name means the returned
/// indices are always directly usable against `name` with no offset
/// bookkeeping, at the cost of a second (cheap — names are short) match
/// per row.
///
/// Case-insensitive, matching how the index itself matches
/// (`CaseMatching::Ignore`). Empty if `query` is empty or doesn't match
/// `name` at all (e.g. this row is only showing because a *different*
/// field matched).
pub fn match_indices(name: &str, query: &str) -> Vec<u32> {
    if query.is_empty() || name.is_empty() {
        return Vec::new();
    }
    let mut matcher = Matcher::new(Config::DEFAULT);
    let name_lower = name.to_lowercase();
    let query_lower = query.to_lowercase();
    let mut name_buf = Vec::new();
    let mut query_buf = Vec::new();
    let haystack = Utf32Str::new(&name_lower, &mut name_buf);
    let needle = Utf32Str::new(&query_lower, &mut query_buf);
    let mut indices = Vec::new();
    matcher.fuzzy_indices(haystack, needle, &mut indices);
    indices
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

    // Every documented item on the node — flags, positionals, modifiers,
    // and environment variables alike — gets its own index entry (spec
    // §10: "Index entries are `NodeRef`s, and every entity gets its own
    // entry"). `Entity::key` now returns `Some` for every kind that has at
    // least one spelling, so this one loop covers them all with no
    // per-kind branch.
    for entity in &node.entities {
        push_entity(injector, entity, &path);
    }

    for child in &node.subcommands {
        let mut child_path = path.clone();
        child_path.push(child.name.clone());
        push_node(injector, child, child_path);
    }
}

fn push_entity(
    injector: &nucleo::Injector<Entry>,
    entity: &mandible_core::Entity,
    path: &[String],
) {
    let Some(key) = entity.key() else {
        return; // an entity with no addressable spelling can't be indexed
    };
    let mut haystack = String::new();
    for spelling in &entity.spellings {
        // Search on the spelling the user would actually type: a
        // single-dash long option (`-help`, `-vv`) is never `--help`, and
        // indexing it that way would make it unfindable by its own name. A
        // dashless spelling (a modifier letter, an env var name) indexes
        // with no dash prefix, for the same reason.
        //
        // Built from the dashes and the bare name rather than through
        // `Spelling::render`, deliberately: `render` reconstructs the
        // `[no-]` of a negatable boolean for *display*, and nobody
        // searches for `--[no-]staged`. Every documented spelling is
        // indexed, so a row that names four of them is findable by all
        // four.
        haystack.push_str(match spelling.dashes {
            mandible_core::Dashes::None => "",
            mandible_core::Dashes::Single => "-",
            mandible_core::Dashes::Double => "--",
        });
        haystack.push_str(&spelling.name);
        haystack.push(' ');
    }
    // A range row (`-0 ... -9`) is findable by both of its ends.
    if let Some(range) = entity.range() {
        haystack.push_str(match range.end.dashes {
            mandible_core::Dashes::None => "",
            mandible_core::Dashes::Single => "-",
            mandible_core::Dashes::Double => "--",
        });
        haystack.push_str(&range.end.name);
        haystack.push(' ');
    }
    if let Some(v) = &entity.value_name {
        haystack.push_str(v);
        haystack.push(' ');
    }
    if let Some(d) = &entity.description {
        haystack.push_str(d.as_str());
    }
    let entry = Entry {
        node_ref: NodeRef::Flag {
            path: path.to_vec(),
            key,
        },
        primary_name: entity.primary_name().to_string(),
    };
    injector.push(entry, |_entry, cols| {
        cols[0] = haystack.as_str().into();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::{Entity, Provenance, Source, Text};
    use std::time::{Duration, Instant};

    fn flag(short: Option<char>, long: Option<&str>, description: &str) -> Entity {
        let mut f = Entity::flag_spelled(
            short,
            long.map(|s| s.to_string()),
            false,
            false,
            Provenance::single(Source::HelpText),
        );
        f.description = Some(Text::sanitize(description));
        f
    }

    fn sample_tree() -> CommandNode {
        let mut root = CommandNode::new("git", Provenance::single(Source::HelpText));
        root.summary = Some(Text::sanitize("the stupid content tracker"));

        let mut rebase = CommandNode::new("rebase", Provenance::single(Source::HelpText));
        rebase.summary = Some(Text::sanitize("Reapply commits on top of another base tip"));
        rebase.entities.push(flag(
            Some('i'),
            Some("interactive"),
            "Make a list of commits",
        ));
        rebase.entities.push(flag(
            None,
            Some("autosquash"),
            "Automatically squash commits",
        ));

        let mut add = CommandNode::new("add", Provenance::single(Source::HelpText));
        add.summary = Some(Text::sanitize("Add file contents to the index"));
        add.entities
            .push(flag(Some('p'), Some("patch"), "Interactively choose hunks"));

        root.subcommands.push(rebase);
        root.subcommands.push(add);
        root
    }

    /// An `ar`-shaped root: one node whose modifier table carries the `d`
    /// letter (delete a member from the archive), the shape real `ar`
    /// documents under its own un-dashed modifier table.
    fn ar_shaped_tree() -> CommandNode {
        let mut root = CommandNode::new("ar", Provenance::single(Source::HelpText));
        root.summary = Some(Text::sanitize("create, modify, and extract archives"));
        let mut d = Entity::modifier('d', Provenance::single(Source::HelpText));
        d.description = Some(Text::sanitize("delete a member from the archive"));
        root.entities.push(d);
        root
    }

    /// A `bpftrace`-shaped root: one node whose environment section
    /// documents `BPFTRACE_BTF`, the shape real `bpftrace` documents under
    /// an explicit `ENVIRONMENT` heading.
    fn bpftrace_shaped_tree() -> CommandNode {
        let mut root = CommandNode::new("bpftrace", Provenance::single(Source::HelpText));
        root.summary = Some(Text::sanitize("high-level tracing language"));
        let mut btf = Entity::env_var_item("BPFTRACE_BTF", Provenance::single(Source::HelpText));
        btf.description = Some(Text::sanitize("BTF file location"));
        root.entities.push(btf);
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
                assert_eq!(key, &mandible_core::FlagKey::Long("autosquash".to_string()));
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
            .any(|r| matches!(r, NodeRef::Flag { key, .. } if key == &mandible_core::FlagKey::Long("patch".to_string())));
        assert!(
            has_flag_match,
            "expected --patch flag among results: {results:?}"
        );
    }

    /// A dashless modifier is its own index entry, addressable by its bare
    /// letter — the `ar`-shaped fixture's `d`, keyed by [`FlagKey::Name`]
    /// rather than folded into the root command's haystack.
    #[test]
    fn searching_a_modifier_letter_returns_the_modifier() {
        let mut index = SearchIndex::new();
        index.populate(&ar_shaped_tree());
        index.set_query("delete a member");
        settle(&mut index);

        let results = index.results();
        let has_modifier_match = results.iter().any(|r| {
            matches!(r, NodeRef::Flag { path, key }
                if path == &vec!["ar".to_string()]
                && key == &mandible_core::FlagKey::Name("d".to_string()))
        });
        assert!(
            has_modifier_match,
            "expected the 'd' modifier among results: {results:?}"
        );
    }

    /// A range row (`-0 ... -9`) is findable by its end as well as its
    /// start.
    #[test]
    fn searching_a_range_end_returns_the_range_flag() {
        let mut root = CommandNode::new("xz", Provenance::single(Source::HelpText));
        let mut preset = Entity::flag_short('0', Provenance::single(Source::HelpText));
        preset
            .set_range(mandible_core::SpellingRange {
                start: mandible_core::Spelling::short('0'),
                end: mandible_core::Spelling::single_dash("77"),
            })
            .expect("the start is a spelling");
        preset.description = Some(Text::sanitize("compression preset"));
        root.entities.push(preset);

        let mut index = SearchIndex::new();
        index.populate(&root);
        index.set_query("-77");
        settle(&mut index);
        let results = index.results();
        assert!(
            results.iter().any(|r| matches!(r, NodeRef::Flag { key, .. }
                if key == &mandible_core::FlagKey::Short('0'))),
            "{results:?}"
        );
    }

    /// A dashless environment variable is its own index entry, addressable
    /// by its bare name — the `bpftrace`-shaped fixture's `BPFTRACE_BTF`,
    /// keyed by [`FlagKey::Name`].
    #[test]
    fn searching_an_env_var_name_returns_the_env_var() {
        let mut index = SearchIndex::new();
        index.populate(&bpftrace_shaped_tree());
        index.set_query("BPFTRACE_BTF");
        settle(&mut index);

        let results = index.results();
        let has_env_var_match = results.iter().any(|r| {
            matches!(r, NodeRef::Flag { path, key }
                if path == &vec!["bpftrace".to_string()]
                && key == &mandible_core::FlagKey::Name("BPFTRACE_BTF".to_string()))
        });
        assert!(
            has_env_var_match,
            "expected BPFTRACE_BTF among results: {results:?}"
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

    /// The prefix-boost `primary_name` is the first spelling's bare name for
    /// *every* entity kind (spec §10), including a flag whose first
    /// documented spelling is its short one — `-r, --replace` boosts on
    /// `"r"`, not `"replace"`. Proven with a modifier, since a modifier's
    /// one spelling is unambiguous: a query on the modifier's own letter
    /// must rank it above an entity that only matches via its description,
    /// which is exactly what a wrong (or empty) `primary_name` would lose.
    #[test]
    fn prefix_boost_uses_the_first_spellings_bare_name() {
        let mut index = SearchIndex::new();
        let mut root = CommandNode::new("ar", Provenance::single(Source::HelpText));
        root.summary = Some(Text::sanitize("create, modify, and extract archives"));

        let mut modifier =
            mandible_core::Entity::modifier('r', Provenance::single(Source::HelpText));
        modifier.description = Some(Text::sanitize(
            "replace existing or insert new file(s) into the archive",
        ));
        root.entities.push(modifier);

        // A decoy flag whose *name* does not start with "r" but whose
        // description contains a word starting with "r" — findable only via
        // the description, never via a name-prefix boost.
        let mut decoy = flag(None, Some("verbose"), "run with extra reporting output");
        decoy.spellings = vec![mandible_core::Spelling::bare("verbose")];
        decoy.kind = mandible_core::EntityKind::Modifier;
        root.entities.push(decoy);

        index.populate(&root);
        index.set_query("r");
        settle(&mut index);

        let results = index.results();
        let modifier_pos = results.iter().position(|r| {
            matches!(r, NodeRef::Flag { key, .. } if key == &mandible_core::FlagKey::Name("r".to_string()))
        });
        let decoy_pos = results.iter().position(|r| {
            matches!(r, NodeRef::Flag { key, .. } if key == &mandible_core::FlagKey::Name("verbose".to_string()))
        });
        let (mp, dp) = (
            modifier_pos.expect("modifier 'r' should be among results"),
            decoy_pos.expect("decoy 'verbose' should be among results (description match)"),
        );
        assert!(
            mp < dp,
            "the 'r' modifier's own-letter prefix match should rank above the decoy's description-only match: {results:?}"
        );
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

    fn wide_index(tree: &CommandNode, query: &str) -> Vec<NodeRef> {
        let mut index = SearchIndex::new();
        index.set_wide(true);
        index.populate(tree);
        index.set_query(query);
        settle(&mut index);
        index.results()
    }

    fn long_keys(results: &[NodeRef]) -> Vec<String> {
        results
            .iter()
            .filter_map(|r| match r {
                NodeRef::Flag {
                    key: mandible_core::FlagKey::Long(name),
                    ..
                } => Some(name.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn wide_mode_refuses_letters_picked_from_scattered_places() {
        // Fuzzy matching finds `autosquash` for `asq`; connected text does not.
        let tree = sample_tree();
        assert!(wide_index(&tree, "asq").is_empty());
        assert_eq!(long_keys(&wide_index(&tree, "squash")), ["autosquash"]);
    }

    #[test]
    fn wide_mode_wants_every_word_as_connected_text() {
        let tree = sample_tree();
        assert!(!wide_index(&tree, "squash commits").is_empty());
        assert!(wide_index(&tree, "squash zzz").is_empty());
        assert!(wide_index(&tree, "sqush commits").is_empty());
    }

    #[test]
    fn wide_mode_matches_case_insensitively() {
        assert_eq!(
            long_keys(&wide_index(&sample_tree(), "AUTOSQUASH")),
            ["autosquash"]
        );
    }

    #[test]
    fn wide_mode_finds_a_dashed_word_in_a_flag_spelling() {
        let mut root = CommandNode::new("curl", Provenance::single(Source::HelpText));
        root.entities
            .push(flag(None, Some("ssl-reqd"), "Require SSL"));
        root.entities
            .push(flag(None, Some("verbose"), "Talk a lot"));
        assert_eq!(long_keys(&wide_index(&root, "-ssl")), ["ssl-reqd"]);
        assert_eq!(long_keys(&wide_index(&root, "--ssl")), ["ssl-reqd"]);
    }

    #[test]
    fn wide_mode_keeps_special_characters_literal() {
        let mut root = CommandNode::new("tool", Provenance::single(Source::HelpText));
        root.entities.push(flag(None, Some("cost"), "price in $"));
        root.entities
            .push(flag(None, Some("other"), "price in dollars"));
        assert_eq!(long_keys(&wide_index(&root, "in $")), ["cost"]);
        assert_eq!(
            long_keys(&wide_index(&root, "!price")),
            Vec::<String>::new()
        );
        assert_eq!(
            long_keys(&wide_index(&root, "^price")),
            Vec::<String>::new()
        );
        assert_eq!(substring_atoms("a$ b\\ c"), "'a\\$ 'b 'c");
    }

    #[test]
    fn wide_mode_ranks_word_starts_before_mid_word_hits() {
        let mut root = CommandNode::new("tool", Provenance::single(Source::HelpText));
        root.entities.push(flag(None, Some("aaa"), "x nossl x"));
        root.entities.push(flag(None, Some("bbb"), "x ssl x"));
        assert_eq!(long_keys(&wide_index(&root, "ssl")), ["bbb", "aaa"]);
    }

    #[test]
    fn switching_modes_reparses_the_same_query() {
        let mut index = SearchIndex::new();
        index.populate(&sample_tree());
        index.set_query("asq");
        settle(&mut index);
        assert!(!index.results().is_empty(), "fuzzy finds it");
        index.set_wide(true);
        assert!(!index.is_settled());
        settle(&mut index);
        assert!(index.results().is_empty());
        index.set_wide(false);
        settle(&mut index);
        assert!(!index.results().is_empty());
    }

    #[test]
    fn substring_indices_mark_connected_text_at_word_starts_first() {
        assert_eq!(substring_indices("no-ssl", "ssl"), vec![3, 4, 5]);
        assert_eq!(substring_indices("nossl-ssl", "ssl"), vec![6, 7, 8]);
        assert_eq!(substring_indices("autosquash", "sq"), vec![4, 5]);
        assert_eq!(substring_indices("autosquash", "asq"), Vec::<u32>::new());
        assert_eq!(substring_indices("Rebase", "REB"), vec![0, 1, 2]);
        assert_eq!(substring_indices("git commit", "it co"), vec![1, 2, 4, 5]);
    }

    #[test]
    fn match_indices_finds_positions_within_the_name() {
        let idx = match_indices("rebase", "rb");
        // "r" at 0, "b" at 2 in "rebase".
        assert_eq!(idx, vec![0, 2]);
    }

    #[test]
    fn match_indices_is_case_insensitive() {
        let idx = match_indices("Rebase", "REB");
        assert_eq!(idx, vec![0, 1, 2]);
    }

    #[test]
    fn match_indices_empty_query_yields_no_highlight() {
        assert_eq!(match_indices("rebase", ""), Vec::<u32>::new());
    }

    #[test]
    fn match_indices_no_match_in_name_yields_empty() {
        // "rebase" doesn't contain "xyz" at all, fuzzy or otherwise.
        assert_eq!(match_indices("rebase", "xyz"), Vec::<u32>::new());
    }
}
