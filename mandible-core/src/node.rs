//! The intermediate representation itself: [`CommandNode`], [`Example`].
//! See spec §4; the documented items a node carries are
//! [`Entity`](crate::Entity), in `entity.rs`.

use crate::entity::{Entity, EntityKind};
use crate::provenance::Provenance;
use crate::text::Text;
use serde::{Deserialize, Serialize};

/// One command or subcommand in the tree: `git`, `git rebase`,
/// `git rebase --onto`'s parent, and so on.
///
/// `#[non_exhaustive]`: build one with [`CommandNode::new`] and mutate the
/// public fields. Cross-crate struct literals are forbidden by design, so
/// 0.5.x can add fields — the modifier and env-var stages of spec §4.5
/// each do — without a breaking release.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CommandNode {
    /// The command's own name, e.g. `"rebase"` (not the full path).
    pub name: String,
    /// Alternate names this command is also invoked as, e.g. `["stage"]`
    /// for `git add`, or a cobra alias like `"co"` for `"checkout"`.
    pub aliases: Vec<String>,
    /// A one-line hint, shown in tree rows and as the detail pane's
    /// headline.
    pub summary: Option<Text>,
    /// Long-form prose, shown in the detail pane body.
    pub description: Option<Text>,
    /// Raw usage patterns, kept verbatim (not re-flowed).
    pub usage: Vec<Text>,
    /// Every documented item this node carries — flags, positionals,
    /// modifiers, environment variables — as one kind-tagged vector
    /// (spec §4.5), in document order within each kind. Read one kind
    /// through [`CommandNode::flags`], [`CommandNode::positionals`] or
    /// [`CommandNode::entities_of`].
    ///
    /// Flags here are the node's *own*, in the sense that an inherited
    /// one carries [`Entity::inherited`](crate::Entity) rather than being
    /// filed anywhere else.
    pub entities: Vec<Entity>,
    /// Direct subcommands.
    pub subcommands: Vec<CommandNode>,
    /// Worked examples.
    pub examples: Vec<Example>,
    /// True if this command should be hidden from the tree by default.
    pub hidden: bool,
    /// `Some(reason)` when this command is deprecated.
    pub deprecated: Option<Text>,
    /// True when this node's `subcommands` list is known-complete. False
    /// means the subtree has not been extracted yet (spec §5, lazy
    /// extraction) and the runner should request it on expand.
    pub children_filled: bool,
    /// Display grouping from the source, e.g. carapace's `group: "main"` for
    /// `git`'s porcelain commands. Extension beyond the spec's base schema,
    /// permitted by spec §4 (carapace's `group` is a real display grouping).
    pub group: Option<String>,
    /// The tool's raw `--help` output, one sanitized [`Text`] per line, set
    /// **only** when no parse produced anything structurally plausible
    /// (spec §7 Tier B step 3 / batch 6 part 4: no flags, no subcommands,
    /// no usage). Non-empty means "we are showing you the author's own
    /// text untouched because inventing structure would be worse" — when
    /// this is non-empty, `flags`/`subcommands`/`usage`/`description` are
    /// all empty by construction, `provenance.confidence` is `0.0`, and a
    /// consumer (the TUI's detail pane) must render this as a preformatted
    /// block, not re-wrap or markdown-treat it. One `Text` per line
    /// deliberately: it reuses `Text`'s own single-line invariant instead
    /// of introducing a second, weaker sanitizer for a raw blob.
    pub unparsed: Vec<Text>,
    /// The framework Tier A′ identified for this node's `--help` output
    /// (`Framework::name()`, e.g. `"clap (v3/v4)"`), if any — for display
    /// only (`--doctor`, the detail pane's provenance footer), never for
    /// parsing decisions on the consumer side. `mandible-core` cannot
    /// depend on `mandible-extract::framework::Framework` (that would be a
    /// cyclic crate dependency), so this is carried as the already-
    /// rendered short name rather than the enum itself.
    pub detected_framework: Option<String>,
    /// Which source(s) contributed this node's own fields (not its
    /// children's — each child has its own `Provenance`).
    pub provenance: Provenance,
    /// True when this node was recovered from a bare-word block sitting
    /// under a **recognized** command heading (spec §7 Tier B rule 1: a
    /// literal heading-vocabulary match, or a chain started by one, e.g.
    /// git's group headings) — as opposed to being conjured from layout
    /// alone. This is *positive evidence the node names a real command*,
    /// independent of whether the source `--help` text bothered to
    /// describe it: `openssl --help`'s `Standard commands:` grid lists
    /// `asn1parse`, `ca`, `ciphers`, ... with no per-entry description at
    /// all, and every one is a real subcommand.
    ///
    /// Set only at the handful of call sites already gated on a recognized
    /// heading (`help_text::sections::emit_subcommands`,
    /// `help_text::sections::process_word_grid`); every other constructor
    /// (`CommandNode::new`, Tier A/E's own node-building) leaves this
    /// `false`. That is what lets the coverage harness's structure-sanity
    /// check (spec §13.1, `xtask::coverage::structure_sanity`) stop
    /// flagging openssl's 151 genuinely empty-but-real nodes as
    /// `suspicious` while still flagging an empty node with no such
    /// evidence — [M-10]'s exact shape — regardless of whether its name
    /// happens to look plausible.
    pub heading_attested: bool,
    /// True when this node was recovered from a **headingless invocation
    /// table** — a repetition-shaped run of rows the tool printed of its
    /// own invocation forms (`btrfs balance start [options] <path>`, one
    /// level deeper description beneath each), every row anchored on the
    /// tool's own name, with no governing heading at all (spec §7 Tier B's
    /// headingless-command-table subsection).
    ///
    /// This is a *second*, distinct attestation bit from
    /// [`Self::heading_attested`] — existence-attested (every emitted name
    /// is checked to occur literally in the raw text) but **deliberately
    /// not probe-eligible**: spec §6's `--help` probe gate reads
    /// `heading_attested` only, and this field must never be added to that
    /// gate. A table row is layout evidence about a *document*, not a
    /// heading declaring "here is the command list" — see spec §6's
    /// closing paragraphs for the reasoning kept there, not duplicated
    /// here.
    ///
    /// The sanity/audit detectors (`xtask::status::count_suspicious`,
    /// `xtask::audit::is_attestation_gated_stub`) accept *either* bit as
    /// evidence a node names a real command, so a headingless-table node is
    /// never flagged as a fabricated phantom subcommand merely for not
    /// being probe-eligible.
    pub invocation_attested: bool,
    /// The binary this node was discovered as, when it was found by the
    /// `<parent>-<sub>` PATH convention rather than documented by its
    /// parent's own help text (spec §5.4) — e.g. `Some("cargo-clippy")` on
    /// the `clippy` child of `cargo`. `None` for every node any extraction
    /// tier produced, which is all of them: only the tree assembly above
    /// this crate (`mandible/src/discovery.rs`) ever sets it.
    ///
    /// `Some` is the *unverified* state, and the TUI says so (spec §9.2): a
    /// file on `PATH` whose name starts with the parent's own name plus a
    /// dash is a convention, not a claim the parent made, so the node is
    /// real evidence about the filesystem and a guess about the tool. It is
    /// also the redirect this node's own probing follows — everything at or
    /// below it is probed against *that* binary, so the guessed word never
    /// becomes argv for the parent (spec §6).
    pub discovered_binary: Option<String>,
    /// What this node's own `--help` text said about being an incomplete
    /// document, if anything (spec §6 rule 2b: the "truncation confession"
    /// convention — curl's `--help` ending "For all options use the manual
    /// or \"--help all\"."). `None` means the tool's text printed no such
    /// confession at all, which is the overwhelming common case and is
    /// never treated as evidence of anything.
    pub confession: Option<Confession>,
    /// True when this node's own selected `--help` text fingerprinted as
    /// byte-identical to a strict ancestor's (spec [M-19], the self-similar
    /// fan-out guard) — see docs/design.md §16's ruling. Replaces the
    /// `unparsed` fill the same guard used before that ruling: such a node
    /// carries no repeated text at all, and the detail
    /// pane renders its own `summary`, a fixed notice, and
    /// `accepted_modifiers` instead of usage/children/flags. The `t` key
    /// still fetches this node's own live text regardless of this field.
    pub same_as_ancestor: bool,
    /// The accepted-modifier letters a parent's own command-table row
    /// documented for this subcommand, e.g. `['a', 'b', 'f', 'u']` for
    /// binutils `ar`'s `r[ab][f][u]` row (docs/shapes.md S-020). Filled
    /// only from that exact row shape — a command letter immediately
    /// followed by one or more bracketed single letters, no space — never
    /// from prose, a usage line, or the tool's own name. Each letter's own
    /// description is looked up in the *parent's* `Modifier` entities by
    /// name; this field itself carries only the letters.
    pub accepted_modifiers: Vec<char>,
    /// The row's own source spelling, kept for display only, when it
    /// differs from `name` — `ar`'s `r[ab][f][u]` row names the command
    /// `r` (spec §7 Tier B rule 3's name-shape test rejects the bracketed
    /// form outright), but the commands pane shows the row as the tool
    /// itself printed it. `None` when the row's own spelling is `name`
    /// verbatim, which is the overwhelming common case.
    pub display_name: Option<String>,
}

/// A truncation confession a tool's own `--help` text printed, and what
/// this extraction did about it (spec §6 rule 2b).
///
/// Two states share this one type deliberately, rather than a bare `bool`:
/// a confession that was *detected* is worth recording even when it
/// couldn't be *followed* (an unrecognised word, a failed or refused
/// follow-up probe) — that is exactly the case the `incomplete` status
/// exists to name honestly, and a reader (`--doctor`, the detail pane's
/// footer) needs the word and flag to explain *why* a tree is capped, not
/// just that it is.
///
/// `#[non_exhaustive]`: build one with [`Confession::new`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Confession {
    /// The directive word, taken verbatim from the tool's own text — e.g.
    /// `"all"` for curl. Never fabricated, never guessed (spec §6 rule 2b).
    pub word: String,
    /// The flag the directive printed alongside `word` — `"--help"` or
    /// `"-h"`.
    pub flag: String,
    /// True when the advertised argv (`<flag> <word>`) was actually
    /// re-probed and this node's own fields were built from *that*
    /// document. False means the confession was detected but not
    /// followed — an unrecognised word/shape, a failed probe, or a rule 0
    /// refusal — and this node still reflects the original, truncated
    /// text; the status ladder caps at `incomplete` for exactly this case.
    pub followed: bool,
}

impl Confession {
    /// A confession detected in a tool's own text: the directive `word`,
    /// the `flag` printed alongside it, and whether the advertised argv was
    /// actually re-probed.
    pub fn new(word: impl Into<String>, flag: impl Into<String>, followed: bool) -> Confession {
        Confession {
            word: word.into(),
            flag: flag.into(),
            followed,
        }
    }
}

/// True if `s` looks like a real command/subcommand name: lowercase,
/// starting with a letter, and otherwise only letters/digits/`_`/`.`/`-`
/// (`^[a-z][a-z0-9_.-]*$`, spec §7 Tier B rule 3).
///
/// This is the shared definition of "looks like a name, not a fabricated
/// fragment" — used by any extraction tier deciding whether a candidate
/// bare-word entry is really a subcommand (rejecting prose fragments like
/// *"treat them as errors"* or placeholder tokens like `BYTES`), and by
/// the coverage harness (spec §13.1) as one half of its structure-sanity
/// check: a tier that starts emitting names failing this test again is
/// exactly the class of regression [M-10] was.
pub fn is_command_name_shaped(s: &str) -> bool {
    // A trailing `.`/`-`/`_` is sentence or hyphenation punctuation, never
    // part of a command name. Interior ones are legitimate (`mount.nfs`,
    // `apt-get`, `foo_bar`), which is why the character class below allows
    // them at all — but allowing them at the end let prose fragments like
    // *"testing."* and *"skipped."* through the name-shape check and into
    // the tree as fabricated subcommands ([M-10]).
    if s.ends_with(['.', '-', '_']) {
        return false;
    }
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.' | '-'))
}

impl CommandNode {
    /// A minimal, empty node with the given name and provenance. Useful as
    /// a starting point for tiers and for tests.
    pub fn new(name: impl Into<String>, provenance: Provenance) -> CommandNode {
        CommandNode {
            name: name.into(),
            aliases: Vec::new(),
            summary: None,
            description: None,
            usage: Vec::new(),
            entities: Vec::new(),
            subcommands: Vec::new(),
            examples: Vec::new(),
            hidden: false,
            deprecated: None,
            children_filled: false,
            group: None,
            unparsed: Vec::new(),
            detected_framework: None,
            provenance,
            heading_attested: false,
            invocation_attested: false,
            discovered_binary: None,
            confession: None,
            same_as_ancestor: false,
            accepted_modifiers: Vec::new(),
            display_name: None,
        }
    }

    /// This node's entities of one kind, in document order.
    ///
    /// The kind-filtered accessors are the whole ergonomic case for one
    /// vector over four: a consumer that only cares about flags reads
    /// [`CommandNode::flags`] and never learns that anything else shares
    /// the storage.
    pub fn entities_of(&self, kind: EntityKind) -> impl Iterator<Item = &Entity> + Clone {
        self.entities.iter().filter(move |e| e.kind == kind)
    }

    /// This node's entities of one kind, mutably, in document order.
    pub fn entities_of_mut(&mut self, kind: EntityKind) -> impl Iterator<Item = &mut Entity> {
        self.entities.iter_mut().filter(move |e| e.kind == kind)
    }

    /// This node's flags, in document order.
    pub fn flags(&self) -> impl Iterator<Item = &Entity> + Clone {
        self.entities_of(EntityKind::Flag)
    }

    /// This node's flags, mutably, in document order.
    pub fn flags_mut(&mut self) -> impl Iterator<Item = &mut Entity> {
        self.entities_of_mut(EntityKind::Flag)
    }

    /// This node's positional arguments, in document order.
    pub fn positionals(&self) -> impl Iterator<Item = &Entity> + Clone {
        self.entities_of(EntityKind::Positional)
    }

    /// This node's modifier letters, in document order.
    pub fn modifiers(&self) -> impl Iterator<Item = &Entity> + Clone {
        self.entities_of(EntityKind::Modifier)
    }

    /// This node's environment variables, in document order.
    pub fn env_vars(&self) -> impl Iterator<Item = &Entity> + Clone {
        self.entities_of(EntityKind::EnvVar)
    }

    /// Replace every entity of one kind, leaving the other kinds and their
    /// relative order untouched. The replacements land at the end of the
    /// vector, which is invisible to every consumer: order is only ever
    /// read within a kind.
    ///
    /// `replacement` is trusted to be of `kind` — it is what a tier just
    /// built for that kind — and mixing kinds in it would be a producer
    /// bug, not something this can repair.
    pub fn set_entities_of(&mut self, kind: EntityKind, replacement: Vec<Entity>) {
        self.entities.retain(|e| e.kind != kind);
        self.entities.extend(replacement);
    }

    /// Replace this node's flags, keeping every other kind.
    pub fn set_flags(&mut self, flags: Vec<Entity>) {
        self.set_entities_of(EntityKind::Flag, flags);
    }

    /// Replace this node's positionals, keeping every other kind.
    pub fn set_positionals(&mut self, positionals: Vec<Entity>) {
        self.set_entities_of(EntityKind::Positional, positionals);
    }

    /// Replace this node's modifiers, keeping every other kind.
    pub fn set_modifiers(&mut self, modifiers: Vec<Entity>) {
        self.set_entities_of(EntityKind::Modifier, modifiers);
    }

    /// Replace this node's environment variables, keeping every other kind.
    pub fn set_env_vars(&mut self, env_vars: Vec<Entity>) {
        self.set_entities_of(EntityKind::EnvVar, env_vars);
    }

    /// Remove every entity of one kind and return them in document order.
    pub fn take_entities_of(&mut self, kind: EntityKind) -> Vec<Entity> {
        let (taken, rest) = std::mem::take(&mut self.entities)
            .into_iter()
            .partition(|e| e.kind == kind);
        self.entities = rest;
        taken
    }
}

/// Whether a flag takes a value, and if so, whether it's required.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ValueKind {
    /// The flag is a boolean switch; it takes no value.
    #[default]
    None,
    /// The flag must be given a value.
    Required,
    /// The flag may optionally be given a value.
    Optional,
}

/// A worked example: a command line plus an optional explanation.
///
/// `#[non_exhaustive]`: build one with [`Example::new`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Example {
    /// The example command line, verbatim.
    pub command: Text,
    /// An optional explanation of what the example does.
    pub explanation: Option<Text>,
}

impl Example {
    /// An example command line with no explanation attached.
    pub fn new(command: Text) -> Example {
        Example {
            command,
            explanation: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_command_name_shaped_accepts_real_names() {
        assert!(is_command_name_shaped("commit"));
        assert!(is_command_name_shaped("http-push"));
        assert!(is_command_name_shaped("sha3-256"));
        assert!(is_command_name_shaped("v7"));
    }

    #[test]
    fn is_command_name_shaped_rejects_prose_and_placeholders() {
        // A wrapped description continuation line — spec [M-10]'s exact
        // phantom-subcommand example.
        assert!(!is_command_name_shaped("treat them as errors"));
        // Uppercase placeholder tokens (`BYTES`, `FORMAT`) are never real
        // command names.
        assert!(!is_command_name_shaped("BYTES"));
        assert!(!is_command_name_shaped(""));
        // Must start with a letter, not a digit.
        assert!(!is_command_name_shaped("42start"));
    }
}
