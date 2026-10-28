//! `mandible-search`: search over commands and every entity a node
//! carries — flags, positionals, modifiers, and environment variables
//! (spec §10).
//!
//! A row matches when every space-separated word of the query occurs in its
//! text, case-insensitively and as connected text; a dash is an ordinary
//! character. Index entries are [`NodeRef`]s — including
//! [`NodeRef::Flag`], whose `key` addresses any entity kind (a dashed
//! flag, or a dashless positional/modifier/env-var by its bare name — see
//! [`mandible_core::FlagKey::Name`]), so every entity is a first-class,
//! independently addressable search result rather than something folded
//! into its parent command's haystack. Matching is plain code over the
//! entries and finishes inside [`SearchIndex::set_query`]'s caller's next
//! read, so there is nothing to wait for.

#![forbid(unsafe_code)]
// Size ceilings. Both lints are opt-in; the thresholds live in the
// workspace `clippy.toml`, and CI's `-D warnings` makes them gates.
#![warn(clippy::too_many_lines)]
#![warn(clippy::cognitive_complexity)]
#![warn(missing_docs)]

use mandible_core::{CommandNode, Entity, NodeRef, Spelling};

/// One indexed item: a command or an entity (flag, positional, modifier, or
/// environment variable), addressable via its [`NodeRef`].
#[derive(Debug, Clone)]
struct Entry {
    node_ref: NodeRef,
    /// The name used for the exact-prefix ranking boost (spec §10
    /// "Ranking"): a command's own name, or an entity's first documented
    /// spelling's bare name ([`mandible_core::Entity::primary_name`]).
    primary_name: String,
    /// Everything a query can match, lowercase: names, aliases, value
    /// placeholder, summary or description.
    haystack: String,
}

/// A live search index over one tool's command tree.
pub struct SearchIndex {
    entries: Vec<Entry>,
    current_query: String,
    wide: bool,
    populate_count: u64,
}

impl Default for SearchIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchIndex {
    /// Build an empty index. Call [`SearchIndex::populate`] to fill it.
    pub fn new() -> SearchIndex {
        SearchIndex {
            entries: Vec::new(),
            current_query: String::new(),
            wide: false,
            populate_count: 0,
        }
    }

    /// Rebuild the index from `root`, from scratch.
    ///
    /// This is O(tree), so call it once per batch of structural changes
    /// rather than once per spliced node: `mandible-tui`'s `App` marks the
    /// index stale and rebuilds at most once per event-loop iteration (see
    /// `App::flush_search_index`). The current query is kept, so an active
    /// search simply matches the fresh data on the next read.
    pub fn populate(&mut self, root: &CommandNode) {
        self.populate_count += 1;
        self.entries.clear();
        push_node(&mut self.entries, root, vec![root.name.clone()]);
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

    /// Set the live query text.
    pub fn set_query(&mut self, query: &str) {
        if query != self.current_query {
            self.current_query = query.to_string();
        }
    }

    /// Choose what the results are ordered by. Both modes match the same
    /// way, every word of the query as connected text; wide mode also
    /// orders the hits where every word starts a word first (spec §10).
    pub fn set_wide(&mut self, wide: bool) {
        self.wide = wide;
    }

    /// The current query's matches, ranked: items whose name starts with
    /// the query (case-insensitive) first (spec §10 "Ranking"), then, in
    /// wide mode, items where every word of the query starts a word of the
    /// haystack, and within each group the index's own order.
    pub fn results(&self) -> Vec<NodeRef> {
        self.results_grouped()
            .into_iter()
            .map(|(hit, _)| hit)
            .collect()
    }

    /// [`SearchIndex::results`] with each hit's rank group: 0 when its name
    /// starts with the query, 1 when every word starts a word, else 2. Hits
    /// of one group are the ones the ranking does not tell apart by kind.
    pub fn results_grouped(&self) -> Vec<(NodeRef, u8)> {
        let query_lower = self.current_query.to_lowercase();
        let words: Vec<&str> = query_lower.split_whitespace().collect();
        if words.is_empty() {
            return Vec::new();
        }
        let mut ranked: Vec<(bool, bool, usize, &Entry)> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| words.iter().all(|w| e.haystack.contains(w)))
            .map(|(rank, e)| {
                let is_prefix = e.primary_name.to_lowercase().starts_with(&query_lower);
                let at_word_start = self.wide && every_word_starts_a_word(&e.haystack, &words);
                // `!` so the better group (true) sorts first in the
                // ascending sort below.
                (!is_prefix, !at_word_start, rank, e)
            })
            .collect();
        ranked.sort_by_key(|(not_prefix, not_word, rank, _)| (*not_prefix, *not_word, *rank));
        ranked
            .into_iter()
            .map(|(not_prefix, not_word, _, e)| {
                (
                    e.node_ref.clone(),
                    u8::from(not_prefix) + u8::from(not_word),
                )
            })
            .collect()
    }
}

/// Whether each of `words` (lowercase) occurs in the haystack at the start
/// of a word: the first character, or one after a character that is not a
/// letter or digit.
fn every_word_starts_a_word(haystack: &str, words: &[&str]) -> bool {
    words.iter().all(|word| {
        haystack.match_indices(word).any(|(at, _)| {
            haystack[..at]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric())
        })
    })
}

/// The character indices within `name` that the words of `query` occupy:
/// every occurrence of every word, as connected text. Case-insensitive; a
/// word that is not in `name` marks nothing.
pub fn substring_indices(name: &str, query: &str) -> Vec<u32> {
    let text: Vec<char> = name
        .chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect();
    let mut out = std::collections::BTreeSet::new();
    for word in query.to_lowercase().split_whitespace() {
        let word: Vec<char> = word.chars().collect();
        if word.len() > text.len() {
            continue;
        }
        for start in 0..=text.len() - word.len() {
            if text[start..].starts_with(&word) {
                out.extend((start..start + word.len()).map(|i| i as u32));
            }
        }
    }
    out.into_iter().collect()
}

/// Every form of `spelling` a user can type: the spelling with its dashes,
/// and for a negatable one also its `no-` form (`--squash`, `--no-squash`).
pub fn spelling_forms(spelling: &Spelling) -> Vec<String> {
    let typed = spelling.typed();
    let mut forms = vec![typed.clone()];
    if spelling.negatable {
        let lead = &typed[..typed.len() - spelling.name.len()];
        forms.push(format!("{lead}no-{}", spelling.name));
    }
    forms
}

/// Every form of every spelling of `entity`, in document order, with the
/// far end of a range last.
pub fn entity_forms(entity: &Entity) -> Vec<String> {
    entity
        .spellings
        .iter()
        .chain(entity.range().map(|r| &r.end))
        .flat_map(spelling_forms)
        .collect()
}

fn push_node(entries: &mut Vec<Entry>, node: &CommandNode, path: Vec<String>) {
    let mut haystack = node.name.clone();
    for alias in &node.aliases {
        haystack.push(' ');
        haystack.push_str(alias);
    }
    if let Some(summary) = &node.summary {
        haystack.push(' ');
        haystack.push_str(summary.as_str());
    }
    entries.push(Entry {
        node_ref: NodeRef::Command(path.clone()),
        primary_name: node.name.clone(),
        haystack: haystack.to_lowercase(),
    });

    // Every documented item on the node — flags, positionals, modifiers,
    // and environment variables alike — gets its own index entry (spec
    // §10: "Index entries are `NodeRef`s, and every entity gets its own
    // entry"). `Entity::key` returns `Some` for every kind that has at
    // least one spelling, so this one loop covers them all with no
    // per-kind branch.
    for entity in &node.entities {
        push_entity(entries, entity, &path);
    }

    for child in &node.subcommands {
        let mut child_path = path.clone();
        child_path.push(child.name.clone());
        push_node(entries, child, child_path);
    }
}

fn push_entity(entries: &mut Vec<Entry>, entity: &Entity, path: &[String]) {
    let Some(key) = entity.key() else {
        return; // an entity with no addressable spelling can't be indexed
    };
    // Search on the forms the user would actually type: a single-dash long
    // option (`-help`) is never `--help`, a dashless spelling (a modifier
    // letter, an env var name) has no dashes, and a `--[no-]x` spelling is
    // findable as both `--x` and `--no-x`, never as the documentation
    // notation `[no-]`.
    let mut haystack = String::new();
    for form in entity_forms(entity) {
        haystack.push_str(&form);
        haystack.push(' ');
    }
    if let Some(v) = &entity.value_name {
        haystack.push_str(v);
        haystack.push(' ');
    }
    if let Some(d) = &entity.description {
        haystack.push_str(d.as_str());
    }
    entries.push(Entry {
        node_ref: NodeRef::Flag {
            path: path.to_vec(),
            key,
        },
        primary_name: entity.primary_name().to_string(),
        haystack: haystack.to_lowercase(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::{Entity, Provenance, Source, Text};

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

    #[test]
    fn searching_a_flag_spelling_returns_the_flag_not_the_command() {
        let mut index = SearchIndex::new();
        index.populate(&sample_tree());
        index.set_query("autosquash");

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
        // Should not panic; an empty query matches nothing.
        let _ = index.results();
    }

    #[test]
    fn repopulating_after_a_query_still_works() {
        let mut index = SearchIndex::new();
        index.populate(&sample_tree());
        index.set_query("rebase");
        assert!(!index.results().is_empty());

        // Simulate a lazy fill changing the tree structure.
        let mut updated = sample_tree();
        updated.subcommands[0].summary =
            Some(Text::sanitize("Reapply commits, now with more detail"));
        index.populate(&updated);
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
    }

    #[test]
    fn wide_mode_ranks_word_starts_before_mid_word_hits() {
        let mut root = CommandNode::new("tool", Provenance::single(Source::HelpText));
        root.entities.push(flag(None, Some("assl"), ""));
        root.entities.push(flag(None, Some("bbb"), "uses ssl here"));
        assert_eq!(long_keys(&wide_index(&root, "ssl")), ["bbb", "assl"]);
    }

    #[test]
    fn a_word_start_is_the_first_character_or_one_after_a_non_alphanumeric() {
        assert!(every_word_starts_a_word("--ssl-reqd use", &["ssl", "use"]));
        assert!(every_word_starts_a_word("--ssl-reqd", &["-ssl"]));
        assert!(!every_word_starts_a_word("--nossl", &["ssl"]));
        assert!(!every_word_starts_a_word("ssl x", &["ssl", "y"]));
    }

    fn name_index(tree: &CommandNode, query: &str) -> Vec<NodeRef> {
        let mut index = SearchIndex::new();
        index.populate(tree);
        index.set_query(query);
        index.results()
    }

    #[test]
    fn both_modes_refuse_scattered_letters_and_find_the_same_rows() {
        let tree = sample_tree();
        assert!(name_index(&tree, "asq").is_empty());
        assert_eq!(
            long_keys(&name_index(&tree, "squash")),
            long_keys(&wide_index(&tree, "squash"))
        );
        assert_eq!(long_keys(&name_index(&tree, "SQUASH")), ["autosquash"]);
    }

    #[test]
    fn switching_modes_keeps_the_matches() {
        let mut index = SearchIndex::new();
        index.populate(&sample_tree());
        index.set_query("squash");
        let before = index.results();
        index.set_wide(true);
        assert_eq!(index.results(), before);
    }

    #[test]
    fn a_negatable_spelling_is_found_by_both_of_its_forms() {
        let mut root = CommandNode::new("git", Provenance::single(Source::HelpText));
        let mut e = Entity::flag_spelled(
            None,
            Some("squash".to_string()),
            false,
            true,
            Provenance::single(Source::HelpText),
        );
        e.description = Some(Text::sanitize("squash commits"));
        root.entities.push(e);
        assert_eq!(long_keys(&name_index(&root, "--no-squash")), ["squash"]);
        assert_eq!(long_keys(&name_index(&root, "no squash")), ["squash"]);
        assert_eq!(long_keys(&name_index(&root, "--squash")), ["squash"]);
        assert_eq!(entity_forms(&root.entities[0]), ["--squash", "--no-squash"]);
    }

    #[test]
    fn substring_indices_mark_every_occurrence_of_every_word() {
        assert_eq!(substring_indices("no-ssl", "ssl"), vec![3, 4, 5]);
        assert_eq!(
            substring_indices("nossl-ssl", "ssl"),
            vec![2, 3, 4, 6, 7, 8]
        );
        assert_eq!(substring_indices("autosquash", "sq"), vec![4, 5]);
        assert_eq!(substring_indices("autosquash", "asq"), Vec::<u32>::new());
        assert_eq!(substring_indices("Rebase", "REB"), vec![0, 1, 2]);
        assert_eq!(
            substring_indices("git commit", "it co"),
            vec![1, 2, 4, 5, 8, 9]
        );
        assert_eq!(substring_indices("rebase", ""), Vec::<u32>::new());
        assert_eq!(substring_indices("--no-trunc", "run"), vec![6, 7, 8]);
    }
}
