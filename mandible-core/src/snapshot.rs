//! Stable, human-reviewable snapshot serialization of [`CommandNode`] trees.
//!
//! This is the format `corpus/README.md`'s `expected.snap` fixtures are
//! written in (spec §13.2), and the format the (not-yet-built) `cargo xtask
//! corpus` runner will diff against. It lives here rather than in `xtask` or
//! `mandible-extract` because this crate owns the IR, and both a
//! workspace-level `xtask` and crate-level tests (`mandible-extract`'s own
//! pipeline tests) need to agree on exactly one definition of "what a
//! snapshot looks like" — two independent definitions could silently drift.
//!
//! # Why this is a *separate* serialization from `CommandNode`'s own derive
//!
//! `CommandNode` (and `Entity`, `Positional`, `Provenance`, ...) already derive
//! `Serialize`/`Deserialize` for round-tripping — e.g. the `Transcript`
//! replay seam (`mandible-extract/src/exec/probe.rs`). That derive is
//! full-fidelity by design: every field, every `None`, every empty `Vec`,
//! exactly as stored, because a round trip must be lossless.
//!
//! A snapshot has a different job: it exists to be *read* by a human running
//! `cargo insta review`, and reviewability trades off against completeness.
//! A 23-node `git` tree with every `None` and every empty `Vec` spelled out
//! buries the handful of fields a reviewer actually needs to look at — which
//! is exactly the condition under which a diff gets accepted blind, defeating
//! the review step and therefore the regression net it exists to build. So
//! this module normalizes two things, deliberately no more:
//!
//! - **Omits empty collections and `None` fields**, via `NodeSnapshot` and
//!   friends mirroring `CommandNode`'s shape but with
//!   `skip_serializing_if` on every `Option`/`Vec` field. This is safe in
//!   the direction that matters: a field going `Some(x)` -> `None`, or a
//!   `Vec` losing its last element, still shows up in a diff as a *removed
//!   key* — the loss stays visible, just spelled as an absence rather than a
//!   changed value.
//! - **Rounds `Provenance::confidence` to 2 decimal places** (see
//!   [`round_confidence`]). A heuristic tier's float noise — the same parse,
//!   differing in the seventh bit of an `f32` between two runs — would
//!   otherwise churn the snapshot with no signal for a reviewer to act on. A
//!   confidence change large enough to round to a different value still
//!   moves the snapshot, so a genuine confidence regression stays visible.
//! - **Omits `bool` fields when `false`** (via [`is_false`]), extending the
//!   same "loss is still visible as a removed key" reasoning to booleans.
//!   This one isn't in the brief verbatim, but the evidence for it is
//!   concrete: a generated snapshot of a two-flag synthetic tree, before
//!   this rule existed, spent 3 lines per node (`hidden`/`children_filled`/
//!   `heading_attested`, all `false`) and 4 lines per flag
//!   (`repeatable`/`required`/`hidden`/`inherited`, all `false`) restating
//!   the default — for `tar`'s real 171-flag fixture that's ~800 lines of
//!   pure noise before a reviewer reaches anything that varies. `false`
//!   staying implicit and only `true` appearing is exactly [`ValueKind`]'s
//!   own existing precedent in this format (below): the common case is
//!   silent, the notable case is a visible key.
//!
//! # What this module deliberately does *not* normalize
//!
//! **`subcommands` order is untouched.** [`NodeSnapshot::from`] does not
//! sort it, does not dedupe it beyond what the IR itself already guarantees,
//! and does not otherwise reorder it for tidiness. Order is a meaningful
//! structural fact — `git --help` groups its commands ("start a working
//! area", "work on the current change", ...) in an order the source chose,
//! not alphabetically — and a grammar change that silently reordered them
//! would be exactly the class of regression this snapshot format exists to
//! catch. Sorting it away would make that regression permanently invisible,
//! which is strictly worse than the extra review noise a stable-but-not-
//! alphabetical order occasionally costs.
//!
//! **There is nothing else to normalize.** Every field `CommandNode` (and
//! `Entity`, `Positional`, `Example`, `Provenance`) exposes already reaches
//! serialization through a `Vec`/`SmallVec` in source order — an audit of
//! `mandible-core` and the extraction pipeline in `mandible-extract` found
//! no `HashMap`/`HashSet` whose iteration order reaches an emitted
//! `CommandNode`; `mandible-core::merge`'s internal `HashMap` buckets are
//! read back out through a separately tracked first-seen-order `Vec`, never
//! iterated directly. And there is no timing field on `CommandNode` to
//! strip — elapsed time lives on `mandible-extract::ExtractionResult`, one
//! layer above the IR this module snapshots, so it never reaches here.

use crate::entity::Entity;
use crate::node::{CommandNode, Example, Positional, ValueKind};
use crate::provenance::{Provenance, Source};
use serde::Serialize;

/// Build a [`NodeSnapshot`] from a [`CommandNode`], applying this module's
/// normalization rules (confidence rounding, omission of empty/`None`
/// fields) without touching anything order-sensitive. This is the one
/// function a corpus runner or a snapshot test needs.
pub fn to_snapshot(node: &CommandNode) -> NodeSnapshot {
    NodeSnapshot::from(node)
}

/// Round a confidence score to 2 decimal places.
///
/// 2 decimals is coarse enough to absorb the float noise a heuristic tier's
/// scoring produces between otherwise-identical runs, and fine enough that a
/// real confidence change (a grammar edit that makes a tier genuinely more
/// or less sure) still lands on a different rounded value and therefore
/// still moves the snapshot. See this module's doc comment.
fn round_confidence(c: f32) -> f32 {
    (c * 100.0).round() / 100.0
}

/// Snapshot form of [`Provenance`]: `sources` rendered through
/// [`Source::label`] (already the human-readable form used by `--doctor` and
/// the detail pane's footer, so this introduces no second vocabulary) and
/// `confidence` rounded per [`round_confidence`]. Both fields are omitted
/// when empty/`None`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProvenanceSnapshot {
    /// Contributing source labels, in contribution order (earliest first) —
    /// order preserved, not sorted, same reasoning as `subcommands`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    /// Heuristic confidence, rounded to 2 decimals. Absent for
    /// structured/authoritative sources, which never set it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
}

impl From<&Provenance> for ProvenanceSnapshot {
    fn from(p: &Provenance) -> Self {
        ProvenanceSnapshot {
            sources: p.sources.iter().map(Source::label).collect(),
            confidence: p.confidence.map(round_confidence),
        }
    }
}

/// True when `v` is [`ValueKind::None`] (the default, boolean-switch case) —
/// used to skip the field for the common case so a long list of plain
/// boolean flags (most real flag lists) doesn't repeat `value_kind: None` on
/// every row.
fn is_no_value(v: &ValueKind) -> bool {
    matches!(v, ValueKind::None)
}

/// True when `b` is `false`. Used to skip boolean fields in their (near-
/// universal) default state — see this module's doc comment. A flip from
/// `true` back to `false` still shows up in a diff as the key disappearing,
/// same as `Some` -> `None`.
fn is_false(b: &bool) -> bool {
    !*b
}

/// Snapshot form of a flag [`Entity`]. Field order matches the pre-0.5.0
/// `Flag`'s own declaration, and must keep matching it —
/// see the `From` impl below. Every `Option`/`Vec` field is omitted when
/// empty, every `bool` field is omitted when `false`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FlagSnapshot {
    /// Short spelling, e.g. `'i'` for `-i`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short: Option<char>,
    /// Long spelling, e.g. `"interactive"` for `--interactive`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub long: Option<String>,
    /// The value placeholder, e.g. `"FILE"` in `--output FILE`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_name: Option<String>,
    /// Whether this flag takes no value, a required value, or an optional
    /// one. Omitted for the common no-value case.
    #[serde(skip_serializing_if = "is_no_value")]
    pub value_kind: ValueKind,
    /// Enumerated choices, e.g. `{json|yaml|table}` for `--format`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    /// True if this flag may be given more than once.
    #[serde(skip_serializing_if = "is_false")]
    pub repeatable: bool,
    /// True if this flag is required.
    #[serde(skip_serializing_if = "is_false")]
    pub required: bool,
    /// True if the tool documents this boolean's negation inline
    /// (`--[no-]foo`). `long` holds the base name either way.
    #[serde(skip_serializing_if = "is_false")]
    pub negatable: bool,
    /// True when `long` is spelled with one dash rather than two (`-help`,
    /// `-vv`). `long` holds the bare name either way.
    #[serde(skip_serializing_if = "is_false")]
    pub single_dash: bool,
    /// True if this flag should be hidden by default.
    #[serde(skip_serializing_if = "is_false")]
    pub hidden: bool,
    /// The deprecation reason, when deprecated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<String>,
    /// True when inherited from an ancestor node.
    #[serde(skip_serializing_if = "is_false")]
    pub inherited: bool,
    /// Display grouping from the source, e.g. tar's "Main operation mode".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// The flag's description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The flag's default value, if documented.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// An environment variable that also sets this flag, if documented.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_var: Option<String>,
    /// Which source(s) contributed this flag's fields.
    pub provenance: ProvenanceSnapshot,
}

impl From<&Entity> for FlagSnapshot {
    /// **The field layout is frozen, deliberately.** This struct's shape,
    /// field order and `skip_serializing_if` rules are what 105 committed
    /// `expected.snap` fixtures are written in, so it stays the pre-0.5.0
    /// `Flag`'s shape even though the IR behind it is now [`Entity`]: the
    /// four spelling keys are recovered through `Entity`'s accessors
    /// (`short`/`long`/`negatable`/`single_dash`, pinned against the old
    /// `Flag` fields by `entity.rs`'s parity tests) rather than read from
    /// stored fields.
    ///
    /// Rendering `spellings` as a list here would be the honest 0.5.0
    /// shape and would move every fixture at once, which is exactly what
    /// the migration's success condition forbids — a snapshot diff must
    /// mean a *parse* changed. The reshape belongs with the stage that
    /// actually emits multi-spelling entities.
    fn from(e: &Entity) -> Self {
        FlagSnapshot {
            short: e.short(),
            long: e.long().map(str::to_string),
            value_name: e.value_name.clone(),
            value_kind: e.value_kind,
            choices: e.choices.iter().map(|t| t.as_str().to_string()).collect(),
            repeatable: e.repeatable,
            required: e.required,
            negatable: e.negatable(),
            single_dash: e.single_dash(),
            hidden: e.hidden,
            deprecated: e.deprecated.as_ref().map(|t| t.as_str().to_string()),
            inherited: e.inherited,
            group: e.group.clone(),
            description: e.description.as_ref().map(|t| t.as_str().to_string()),
            default: e.default.as_ref().map(|t| t.as_str().to_string()),
            env_var: e.env_var.clone(),
            provenance: ProvenanceSnapshot::from(&e.provenance),
        }
    }
}

/// Snapshot form of [`Positional`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PositionalSnapshot {
    /// The argument's name as shown in usage, e.g. `"pathspec"`.
    pub name: String,
    /// True if this positional must be supplied.
    #[serde(skip_serializing_if = "is_false")]
    pub required: bool,
    /// True if this positional accepts multiple values (`...`).
    #[serde(skip_serializing_if = "is_false")]
    pub variadic: bool,
    /// The positional's description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Which source(s) contributed this positional's fields.
    pub provenance: ProvenanceSnapshot,
}

impl From<&Positional> for PositionalSnapshot {
    fn from(p: &Positional) -> Self {
        PositionalSnapshot {
            name: p.name.clone(),
            required: p.required,
            variadic: p.variadic,
            description: p.description.as_ref().map(|t| t.as_str().to_string()),
            provenance: ProvenanceSnapshot::from(&p.provenance),
        }
    }
}

/// Snapshot form of [`Example`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExampleSnapshot {
    /// The example command line, verbatim.
    pub command: String,
    /// An optional explanation of what the example does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
}

impl From<&Example> for ExampleSnapshot {
    fn from(e: &Example) -> Self {
        ExampleSnapshot {
            command: e.command.as_str().to_string(),
            explanation: e.explanation.as_ref().map(|t| t.as_str().to_string()),
        }
    }
}

/// Snapshot form of [`CommandNode`]. See this module's doc comment for the
/// normalization rules; in short, `Option`/`Vec` fields are omitted when
/// empty, `provenance.confidence` is rounded, and `subcommands` order is
/// preserved exactly as `CommandNode` stored it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NodeSnapshot {
    /// The command's own name, e.g. `"rebase"` (not the full path).
    pub name: String,
    /// Alternate names this command is also invoked as.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// A one-line hint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Long-form prose.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Raw usage patterns, kept verbatim.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub usage: Vec<String>,
    /// Positional arguments.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub positionals: Vec<PositionalSnapshot>,
    /// This node's own flags.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<FlagSnapshot>,
    /// Worked examples.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<ExampleSnapshot>,
    /// Display grouping from the source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// The deprecation reason, when deprecated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<String>,
    /// The framework Tier A′ identified for this node, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detected_framework: Option<String>,
    /// Which source(s) contributed this node's own fields.
    pub provenance: ProvenanceSnapshot,
    /// True if this command should be hidden from the tree by default.
    #[serde(skip_serializing_if = "is_false")]
    pub hidden: bool,
    /// True when this node's `subcommands` list is known-complete.
    #[serde(skip_serializing_if = "is_false")]
    pub children_filled: bool,
    /// True when this node was recovered from a bare-word block under a
    /// recognized command heading (spec §7 Tier B rule 1) rather than
    /// conjured from layout alone.
    #[serde(skip_serializing_if = "is_false")]
    pub heading_attested: bool,
    /// True when this node was recovered from a headingless invocation
    /// table (spec §7 Tier B) — existence-attested but not probe-eligible.
    /// See [`crate::CommandNode::invocation_attested`].
    #[serde(skip_serializing_if = "is_false")]
    pub invocation_attested: bool,
    /// The tool's raw `--help` output, one line per entry, set only when no
    /// parse produced anything structurally plausible.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unparsed: Vec<String>,
    /// Direct subcommands, in exactly the order `CommandNode` stored them —
    /// **never** reordered. See this module's doc comment.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subcommands: Vec<NodeSnapshot>,
    /// What this node's own `--help` text said about being an incomplete
    /// document, if anything (spec §6 rule 2b). Omitted entirely for the
    /// overwhelmingly common case, no confession printed at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confession: Option<ConfessionSnapshot>,
}

/// Snapshot form of [`crate::node::Confession`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConfessionSnapshot {
    /// The directive word, verbatim from the tool's own text.
    pub word: String,
    /// The flag printed alongside it (`"--help"` or `"-h"`).
    pub flag: String,
    /// True when the advertised argv was actually re-probed and this
    /// node's fields reflect that document; false when the confession was
    /// detected but not followed (an unrecognised word, a failed probe, a
    /// rule 0 refusal) and the node still reflects the truncated text.
    ///
    /// **Always written, unlike this module's other booleans.** The
    /// omit-when-false rule the rest of the format follows ([`is_false`])
    /// rests on `false` being the unremarkable default, so its absence
    /// says nothing worth reading. Here the polarity is the other way
    /// round: `false` is the *noteworthy* state — it is precisely what
    /// caps a tree at `incomplete` — and encoding the interesting half of
    /// a two-state field as a missing key would make the fixture that
    /// exists to demonstrate that state (`corpus/curl/8.5.0`) show it by
    /// omission, indistinguishable on sight from a snapshot written
    /// before this field existed.
    pub followed: bool,
}

impl From<&crate::node::Confession> for ConfessionSnapshot {
    fn from(c: &crate::node::Confession) -> Self {
        ConfessionSnapshot {
            word: c.word.clone(),
            flag: c.flag.clone(),
            followed: c.followed,
        }
    }
}

impl From<&CommandNode> for NodeSnapshot {
    fn from(n: &CommandNode) -> Self {
        NodeSnapshot {
            name: n.name.clone(),
            aliases: n.aliases.clone(),
            summary: n.summary.as_ref().map(|t| t.as_str().to_string()),
            description: n.description.as_ref().map(|t| t.as_str().to_string()),
            usage: n.usage.iter().map(|t| t.as_str().to_string()).collect(),
            positionals: n.positionals.iter().map(PositionalSnapshot::from).collect(),
            flags: n.flags.iter().map(FlagSnapshot::from).collect(),
            examples: n.examples.iter().map(ExampleSnapshot::from).collect(),
            group: n.group.clone(),
            deprecated: n.deprecated.as_ref().map(|t| t.as_str().to_string()),
            detected_framework: n.detected_framework.clone(),
            provenance: ProvenanceSnapshot::from(&n.provenance),
            hidden: n.hidden,
            children_filled: n.children_filled,
            heading_attested: n.heading_attested,
            invocation_attested: n.invocation_attested,
            unparsed: n.unparsed.iter().map(|t| t.as_str().to_string()).collect(),
            // The order-preservation this whole module exists to protect:
            // straight `iter().map().collect()` over `n.subcommands`, no
            // sort, no re-grouping.
            subcommands: n.subcommands.iter().map(NodeSnapshot::from).collect(),
            confession: n.confession.as_ref().map(ConfessionSnapshot::from),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::Provenance;
    use crate::text::Text;

    fn node_with_confidence(confidence: f32) -> CommandNode {
        let mut n = CommandNode::new(
            "tool",
            Provenance::with_confidence(Source::HelpText, confidence),
        );
        n.summary = Some(Text::sanitize("does a thing"));
        n.flags.push(Entity::flag_long(
            "verbose",
            Provenance::with_confidence(Source::HelpText, confidence),
        ));
        n
    }

    fn render(node: &CommandNode) -> String {
        serde_yaml::to_string(&to_snapshot(node)).expect("snapshot serializes")
    }

    #[test]
    fn serializing_the_same_node_twice_is_identical() {
        let node = node_with_confidence(0.8734);
        assert_eq!(render(&node), render(&node));
    }

    /// Both halves of the rounding requirement in one test, deliberately: a
    /// test that only checked the "doesn't move" half would pass even if
    /// confidence were rounded to a constant, which would silently delete
    /// the field's entire signal value.
    #[test]
    fn confidence_rounding_absorbs_noise_but_not_real_change() {
        let base = render(&node_with_confidence(0.821));
        // Sub-threshold wobble: both round to 0.82. Must not move the
        // snapshot.
        let wobble = render(&node_with_confidence(0.8199999));
        assert_eq!(
            base, wobble,
            "a sub-hundredth confidence wobble must not change the snapshot"
        );
        assert!(base.contains("0.82"), "rounded value must still appear");

        // A real change: 0.821 -> 0.75 rounds to a different value and must
        // move the snapshot.
        let changed = render(&node_with_confidence(0.75));
        assert_ne!(
            base, changed,
            "a genuine confidence change must still move the snapshot"
        );
        assert!(changed.contains("0.75"));
    }

    #[test]
    fn subcommand_order_is_preserved_not_sorted() {
        let mut root = CommandNode::new("git", Provenance::single(Source::HelpText));
        for name in ["zebra", "apple", "mango"] {
            root.subcommands
                .push(CommandNode::new(name, Provenance::single(Source::HelpText)));
        }
        let out = render(&root);

        let zebra = out.find("zebra").expect("zebra present");
        let apple = out.find("apple").expect("apple present");
        let mango = out.find("mango").expect("mango present");

        // Insertion order (zebra, apple, mango), NOT alphabetical
        // (apple, mango, zebra) and not any other reordering. This is the
        // regression test for a future "tidy-up" that sorts subcommands.
        assert!(
            zebra < apple && apple < mango,
            "subcommand order must be preserved exactly as built, got: {out}"
        );
    }

    #[test]
    fn empty_and_none_fields_are_omitted() {
        let node = CommandNode::new("bare", Provenance::single(Source::HelpText));
        let out = render(&node);
        assert!(!out.contains("aliases"), "empty Vec must be omitted");
        assert!(!out.contains("summary"), "None Option must be omitted");
        assert!(!out.contains("subcommands"), "empty Vec must be omitted");
        assert!(!out.contains("flags"), "empty Vec must be omitted");
    }

    #[test]
    fn a_field_losing_its_value_still_shows_up_as_a_removed_key() {
        let mut with_summary = CommandNode::new("t", Provenance::single(Source::HelpText));
        with_summary.summary = Some(Text::sanitize("hi"));
        let without_summary = CommandNode::new("t", Provenance::single(Source::HelpText));

        assert!(render(&with_summary).contains("summary"));
        assert!(!render(&without_summary).contains("summary"));
    }

    /// A synthetic-but-representative tree, snapshotted through `insta`
    /// directly (rather than the plain `serde_yaml::to_string` the property
    /// tests above use) to prove the crate is actually wired up to `insta`
    /// and to give a reviewer a small, hand-checkable `.snap` file before
    /// any real corpus fixture exists. The real end-to-end proof — the
    /// format surviving contact with genuine `--help` output through the
    /// actual extraction pipeline — lives in `mandible-extract`'s own
    /// tests, since this crate has no tier/parser to run.
    #[test]
    fn snapshot_of_a_representative_synthetic_tree() {
        let mut root =
            CommandNode::new("git", Provenance::with_confidence(Source::HelpText, 0.9123));
        root.summary = Some(Text::sanitize("the stupid content tracker"));

        let mut commit = CommandNode::new("commit", Provenance::single(Source::HelpText));
        commit.summary = Some(Text::sanitize("Record changes to the repository"));
        commit.flags.push({
            let mut f = Entity::flag_long("amend", Provenance::single(Source::HelpText));
            f.description = Some(Text::sanitize("amend the previous commit"));
            f
        });

        let mut status = CommandNode::new("status", Provenance::single(Source::HelpText));
        status.summary = Some(Text::sanitize("Show the working tree status"));

        // Deliberately not alphabetical (commit, status) — matches how
        // real `--help` output groups commands, and this snapshot doubles
        // as a visible example that the order survives untouched.
        root.subcommands.push(commit);
        root.subcommands.push(status);

        insta::assert_yaml_snapshot!(to_snapshot(&root));
    }
}
