//! The 0.5.0 entity schema (spec §4.5): one kind-tagged type for every
//! documented item a node carries — flags, positionals, modifiers, and
//! environment variables — with every documented spelling kept, in
//! document order.
//!
//! [`Entity`] replaces the four-parallel-vectors design (`Vec<Flag>`,
//! `Vec<Positional>`, and the never-built modifier and env-var vectors it
//! implied). `spellings: Vec<Spelling>` dissolves the multi-spelling bug:
//! ffplay documents `-h`, `-?`, `-help`, `--help` as one row, and a
//! `short: Option<char>` + `long: Option<String>` pair can hold only two
//! of the four.
//!
//! Migration is staged (spec §4.5). All four stages are complete: a node
//! carries one [`CommandNode::entities`] vector (`CommandNode::flags()`,
//! `CommandNode::positionals()`, `CommandNode::modifiers()` and
//! `CommandNode::env_vars()` filter it by kind). The pre-0.5.0 `Flag` and
//! `Positional` types are gone entirely — including from this module's
//! tests — now that [`crate::FlagSnapshot`] itself writes the honest
//! 0.5.0 shape, one `spellings` key holding every rendered [`Spelling`] in
//! document order, in place of the old frozen `short`/`long`/`negatable`/
//! `single_dash` keys. [`Entity::spelling`], [`Entity::key`] and the
//! spelling accessors are pinned by direct tests against literal expected
//! strings/keys instead.
//!
//! [`CommandNode::entities`]: crate::CommandNode::entities

use serde::{Deserialize, Serialize};

use crate::node::ValueKind;
use crate::provenance::Provenance;
use crate::text::Text;

/// How many dashes a [`Spelling`] renders with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dashes {
    /// No dashes at all: an `ar`-style modifier letter (`d`, `r`, `t`) or
    /// an environment variable name.
    None,
    /// One dash: a short flag (`-i`, `-?`) or a single-dash long option
    /// (`-help`, `-vv`) — the convention `Flag::single_dash` records.
    Single,
    /// Two dashes: an ordinary long option (`--interactive`).
    Double,
}

/// One documented spelling of an [`Entity`].
///
/// `name` never contains dashes or brackets — the same rule
/// the pre-0.5.0 `Flag::negatable` and `Flag::single_dash` documented: what a user
/// searches and copies is the bare name; how many dashes (and any
/// `[no-]` prefix) are *rendering* metadata, reconstructed by
/// [`Spelling::render`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spelling {
    /// The bare, full name — never the abbreviated prefix: `"i"`,
    /// `"interactive"`, `"?"`, `"help"`, `"FFREPORT"`, `"resolve"` (for
    /// `-r[esolve]`).
    pub name: String,
    /// How many dashes [`Spelling::render`] puts in front of `name`.
    pub dashes: Dashes,
    /// True when the tool documents the `--[no-]name` negation convention
    /// for this spelling (the pre-0.5.0 `Flag::negatable`).
    pub negatable: bool,
    /// `Some(n)` when the tool documents an abbreviation bracket — the
    /// minimum accepted prefix length: `-r[esolve]` is `name: "resolve"`,
    /// `abbrev: Some(1)`; `-rc[vbuf]` is `name: "rcvbuf"`, `abbrev:
    /// Some(2)`; `--br[ief]` is `name: "brief"`, `abbrev: Some(2)`.
    /// `None` for every other spelling. [`Spelling::render`] reproduces
    /// the bracket form; [`Spelling::typed`] and [`Entity::key`] address
    /// the full name — a shell doesn't need the tool's documentation
    /// shorthand, and identity should not depend on how much of a name a
    /// particular row happened to abbreviate.
    pub abbrev: Option<usize>,
}

impl Spelling {
    /// A short flag spelling: `-c`.
    pub fn short(c: char) -> Spelling {
        Spelling {
            name: c.to_string(),
            dashes: Dashes::Single,
            negatable: false,
            abbrev: None,
        }
    }

    /// An ordinary long spelling: `--name`.
    pub fn long(name: impl Into<String>) -> Spelling {
        Spelling {
            name: name.into(),
            dashes: Dashes::Double,
            negatable: false,
            abbrev: None,
        }
    }

    /// A single-dash long spelling: `-name` (qemu/find/gcc convention).
    pub fn single_dash(name: impl Into<String>) -> Spelling {
        Spelling {
            name: name.into(),
            dashes: Dashes::Single,
            negatable: false,
            abbrev: None,
        }
    }

    /// A dashless spelling: an `ar`-style modifier letter or an
    /// environment variable name.
    pub fn bare(name: impl Into<String>) -> Spelling {
        Spelling {
            name: name.into(),
            dashes: Dashes::None,
            negatable: false,
            abbrev: None,
        }
    }

    /// The user-visible form: dashes, then `[no-]` if negatable, then the
    /// name (or the abbreviation bracket, `pre[fix]`, when this spelling
    /// carries one) — exactly the reconstruction the pre-0.5.0
    /// `Flag::spelling` performed, extended for abbreviation brackets.
    /// `negatable` and `abbrev` are not observed together by any producer,
    /// and `negatable` wins if they ever were: `--[no-]name` is a rendering
    /// convention with nothing analogous to abbreviate.
    pub fn render(&self) -> String {
        if self.negatable {
            return format!("{}[no-]{}", self.dash_prefix(), self.name);
        }
        match self.abbrev {
            Some(n) if n < self.name.chars().count() => {
                let chars: Vec<char> = self.name.chars().collect();
                let prefix: String = chars[..n].iter().collect();
                let rest: String = chars[n..].iter().collect();
                format!("{}{prefix}[{rest}]", self.dash_prefix())
            }
            _ => format!("{}{}", self.dash_prefix(), self.name),
        }
    }

    /// The form a user types: dashes and the name, with no `[no-]`
    /// notation.
    ///
    /// [`Spelling::render`] is documentation — `--[no-]color` is how the
    /// tool *describes* two spellings on one row, and it is not one of
    /// them. Anything handing a spelling to a shell (spec §2's
    /// `--print-selection`) needs the affirmative form, which is what this
    /// returns; a reader who wants the negation types the `no-` themselves,
    /// exactly as they would have read it off the row.
    pub fn typed(&self) -> String {
        format!("{}{}", self.dash_prefix(), self.name)
    }

    fn dash_prefix(&self) -> &'static str {
        match self.dashes {
            Dashes::None => "",
            Dashes::Single => "-",
            Dashes::Double => "--",
        }
    }
}

/// One enumerated value an [`Entity`] may take, e.g. one row of ffmpeg's
/// AVOption sub-table under `-flags`:
///
/// ```text
/// -flags             <flags>      ED.VAS..... (default 0)
///      unaligned                    .D.V....... allow decoders to produce unaligned output
/// ```
///
/// Most tools document choices as a bare list of names with no per-value
/// explanation (`tar --quoting-style`'s `literal`/`shell`/`c`/...) — those
/// carry `description: None`. A tool whose choice rows carry their own text
/// (ffmpeg's AVOption constants, `tar --format`'s `FORMAT is one of the
/// following:` enum) keeps it here rather than smeared into the owning
/// entity's own `description` (spec §7 Tier B rule 4, §9.3's `values:`
/// line). `description` is `Text` because it originates in the tool's own
/// help output and must pass through the same sanitization boundary as any
/// other prose the IR carries (spec §4.1) — `name` stays a bare `String`,
/// the same choice `Spelling::name` makes for an identifier that is
/// searched and compared rather than displayed as prose.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    /// The enumerated value's own name, e.g. `"unaligned"`, `"gnu"`.
    pub name: String,
    /// The value's own documentation, when the tool writes one per choice.
    /// `None` for the common bare-list case.
    pub description: Option<Text>,
}

impl Choice {
    /// A choice with no documented description — the common case.
    pub fn bare(name: impl Into<String>) -> Choice {
        Choice {
            name: name.into(),
            description: None,
        }
    }

    /// A choice whose own row carries a description.
    pub fn described(name: impl Into<String>, description: Text) -> Choice {
        Choice {
            name: name.into(),
            description: Some(description),
        }
    }
}

/// Which kind of documented item an [`Entity`] is. Decides which detail
/// pane section it renders under (spec §9.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EntityKind {
    /// A dashed option: `-i`, `--interactive`, `-help`.
    Flag,
    /// A positional argument: `<pathspec>...`.
    Positional,
    /// A dashless mode letter (`ar`'s `d`/`r`/`t`) documented in its own
    /// table.
    Modifier,
    /// An environment variable documented under an explicitly labeled
    /// environment heading — never scavenged from prose (spec §4.5).
    EnvVar,
}

/// One documented item on a node: a flag, positional, modifier, or
/// environment variable (spec §4.5).
///
/// `#[non_exhaustive]` from birth: downstream crates construct through
/// [`Entity::new`] and mutate the public fields, never through a struct
/// literal — which is what lets 0.5.x add fields without a breaking
/// release.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Entity {
    /// Which kind of item this is, and therefore which section renders it.
    pub kind: EntityKind,
    /// Every documented spelling, in document order.
    pub spellings: Vec<Spelling>,
    /// The value placeholder, e.g. `"FILE"` in `--output FILE`.
    pub value_name: Option<String>,
    /// Whether this entity takes no value, a required one, or an optional
    /// one.
    pub value_kind: ValueKind,
    /// Enumerated choices, e.g. `{json|yaml|table}` for `--format`. Each
    /// [`Choice`] carries its own description when the tool documents one
    /// per value (spec §7 Tier B rule 4, §9.3).
    pub choices: Vec<Choice>,
    /// True if this entity may be given more than once: a flag the tool
    /// accepts repeatedly (`-v -v -v`), and a positional written with an
    /// ellipsis (`<pathspec>...`) — the pre-0.5.0 `Positional::variadic`.
    /// One field, because "may be given more than once" is the same fact
    /// about both, spelled differently by the two kinds' notation.
    pub repeatable: bool,
    /// True if this entity is required.
    pub required: bool,
    /// True if this entity should be hidden by default.
    pub hidden: bool,
    /// `Some(reason)` when deprecated.
    pub deprecated: Option<Text>,
    /// True when declared on an ancestor node and propagated down (cobra
    /// persistent flags). Rendered in a separate, dimmed group.
    pub inherited: bool,
    /// Display grouping from the source, e.g. tar's `"Main operation
    /// mode"`. Renders as a full-width divider rule (spec §9.3).
    pub group: Option<String>,
    /// The entity's description.
    pub description: Option<Text>,
    /// The documented default value, if any.
    pub default: Option<Text>,
    /// Explicitly documented cross-references only — never inferred
    /// (spec §4.5).
    pub see_also: Vec<Text>,
    /// An environment variable that also sets this entity, when the source
    /// documented the association on the entity's own row (a `[env: FOO]`
    /// annotation, or an override file's `env_var` key).
    ///
    /// Distinct from [`EntityKind::EnvVar`], which is a variable documented
    /// as an item in its own right under an explicit environment heading.
    /// This field is the *cross-reference* a flag row carries, and it is
    /// kept because dropping it would silently discard what an override
    /// file states and would change [`crate::snapshot::FlagSnapshot`].
    pub env_var: Option<String>,
    /// Which source(s) contributed this entity's fields.
    pub provenance: Provenance,
}

impl Entity {
    /// A minimal entity of the given kind: no spellings, no value, nothing
    /// documented. The only constructor downstream crates get —
    /// `#[non_exhaustive]` forbids their struct literals by design.
    pub fn new(kind: EntityKind, provenance: Provenance) -> Entity {
        Entity {
            kind,
            spellings: Vec::new(),
            value_name: None,
            value_kind: ValueKind::None,
            choices: Vec::new(),
            repeatable: false,
            required: false,
            hidden: false,
            deprecated: None,
            inherited: false,
            group: None,
            description: None,
            default: None,
            see_also: Vec::new(),
            env_var: None,
            provenance,
        }
    }

    /// A flag with only a long spelling — the direct counterpart of the
    /// pre-0.5.0 `Flag::long`, and the constructor most tiers reach for.
    pub fn flag_long(name: impl Into<String>, provenance: Provenance) -> Entity {
        let mut e = Entity::new(EntityKind::Flag, provenance);
        e.spellings.push(Spelling::long(name));
        e
    }

    /// A flag with only a short spelling: `-i`.
    pub fn flag_short(c: char, provenance: Provenance) -> Entity {
        let mut e = Entity::new(EntityKind::Flag, provenance);
        e.spellings.push(Spelling::short(c));
        e
    }

    /// A positional argument: one dashless spelling holding the name the
    /// tool shows in its usage line (`pathspec`), optional and
    /// non-repeating until the caller says otherwise. The direct
    /// counterpart of the pre-0.5.0 `Positional::new`.
    pub fn positional(name: impl Into<String>, provenance: Provenance) -> Entity {
        let mut e = Entity::new(EntityKind::Positional, provenance);
        e.spellings.push(Spelling::bare(name));
        e
    }

    /// A modifier: one dashless spelling holding the letter a tool's own
    /// modifier table documents (`ar`'s `[a]`, `[D]`, `[l <text> ]`).
    ///
    /// Takes a `char` rather than a name, because one character is the
    /// whole of a modifier's notation — it is typed glued to an operation
    /// letter (`ar rv`), so there is no longer spelling for it to have.
    /// A `String` here would let a producer file a whole word under a kind
    /// that cannot render one.
    pub fn modifier(letter: char, provenance: Provenance) -> Entity {
        let mut e = Entity::new(EntityKind::Modifier, provenance);
        e.spellings.push(Spelling::bare(letter.to_string()));
        e
    }

    /// An environment variable documented as an item in its own right, under
    /// an explicit environment heading in a tool's own help text (spec
    /// §4.5, §7 Tier B "Environment sections") — `bpftrace`'s
    /// `BPFTRACE_BTF`, `node`'s `NODE_DEBUG`.
    ///
    /// Takes a name (a word, not a single character) because a variable's
    /// notation has no dash and no single-letter constraint the way a
    /// modifier does — it is however long the tool spells it.
    ///
    /// **Not the same thing as [`Entity::env_var`]**, an existing field a
    /// *flag* carries: that field is a cross-reference (`[env: FOO]`, or an
    /// override file's `env_var` key) stating that some other, already-kind
    /// `Flag` entity is also settable from an environment variable. An
    /// `EntityKind::EnvVar` entity, built by this constructor, is the
    /// variable itself, documented as its own row under its own heading —
    /// the two are never merged, and a producer must never populate one from
    /// the other.
    pub fn env_var_item(name: impl Into<String>, provenance: Provenance) -> Entity {
        let mut e = Entity::new(EntityKind::EnvVar, provenance);
        e.spellings.push(Spelling::bare(name));
        e
    }

    /// A flag entity from the short/long spelling pair the extraction
    /// tiers' scratch types still work in (`help_text`'s `FlagSpec`,
    /// `completion_script`'s `ParsedArgSpec`, an override file's row).
    ///
    /// This is the IR boundary, and the one place the short-then-long
    /// order is decided — `-i, --interactive`, never the reverse — so that
    /// no producer has to remember it. `single_dash` and `negatable` apply
    /// to the long spelling, which is the only one they were ever able to
    /// describe.
    ///
    /// A tier that genuinely reads more than two spellings off one row
    /// (ffplay's `-h, -?, -help, --help`) builds `spellings` directly and
    /// does not come through here.
    pub fn flag_spelled(
        short: Option<char>,
        long: Option<String>,
        single_dash: bool,
        negatable: bool,
        provenance: Provenance,
    ) -> Entity {
        let mut e = Entity::new(EntityKind::Flag, provenance);
        if let Some(c) = short {
            e.spellings.push(Spelling::short(c));
        }
        if let Some(name) = long {
            e.spellings.push(Spelling {
                name,
                dashes: if single_dash {
                    Dashes::Single
                } else {
                    Dashes::Double
                },
                negatable,
                abbrev: None,
            });
        }
        e
    }

    /// The long-like spelling, if this entity has one.
    ///
    /// "Long-like" is decided by *shape*, which is the whole point of the
    /// entity model: two dashes always, and one dash when the name is more
    /// than a single character (the single-dash long convention — `-help`,
    /// `-vv`, `-CC`). A lone single-dash character is a short flag, because
    /// `-x` is `-x` no matter which slot a previous schema filed it under.
    pub fn long_spelling(&self) -> Option<&Spelling> {
        self.spellings.iter().find(|s| {
            matches!(s.dashes, Dashes::Double)
                || (matches!(s.dashes, Dashes::Single) && s.name.chars().count() > 1)
        })
    }

    /// The short spelling, if this entity has one: one dash, one character.
    pub fn short_spelling(&self) -> Option<&Spelling> {
        self.spellings
            .iter()
            .find(|s| matches!(s.dashes, Dashes::Single) && s.name.chars().count() == 1)
    }

    /// The short letter, if this entity has one: `Some('i')` for `-i`.
    pub fn short(&self) -> Option<char> {
        self.short_spelling().and_then(|s| s.name.chars().next())
    }

    /// The bare long name, if this entity has a long-like spelling:
    /// `Some("interactive")` for `--interactive`, `Some("vv")` for `-vv`.
    /// Never contains dashes or brackets.
    pub fn long(&self) -> Option<&str> {
        self.long_spelling().map(|s| s.name.as_str())
    }

    /// True when any spelling documents the `--[no-]name` negation
    /// convention.
    pub fn negatable(&self) -> bool {
        self.spellings.iter().any(|s| s.negatable)
    }

    /// True when the long-like spelling is written with one dash rather
    /// than two (`-help`, `-vv`). False when there is no long spelling.
    pub fn single_dash(&self) -> bool {
        self.long_spelling()
            .is_some_and(|s| matches!(s.dashes, Dashes::Single))
    }

    /// The bare name of the first documented spelling, or `""` for an
    /// entity with no spellings at all.
    ///
    /// For the kinds that carry exactly one spelling — positionals,
    /// modifiers, environment variables — this is *the* name, and the one
    /// the pre-0.5.0 `Positional::name` held. For a flag it is whichever
    /// spelling the tool printed first, which is why flag code asks
    /// [`Entity::short`]/[`Entity::long`] instead.
    pub fn primary_name(&self) -> &str {
        self.spellings.first().map_or("", |s| s.name.as_str())
    }

    /// True if `key` addresses this entity, checking every spelling
    /// regardless of which one is considered canonical.
    ///
    /// `Long`/`Short` are answered by [`Entity::long`]/[`Entity::short`],
    /// which only ever return `Some` for a dashed spelling — so a dashless
    /// entity (positional, modifier, env var) can never match either,
    /// with no extra kind check needed. `Name` is answered by checking
    /// every [`Dashes::None`] spelling, which a `Flag` entity never has
    /// (its spellings always carry at least one dash), so `Name` can
    /// never match a `Flag` entity either.
    pub fn matches_key(&self, key: &crate::noderef::FlagKey) -> bool {
        match key {
            crate::noderef::FlagKey::Long(l) => self.long() == Some(l.as_str()),
            crate::noderef::FlagKey::Short(s) => self.short() == Some(*s),
            crate::noderef::FlagKey::Name(n) => self
                .spellings
                .iter()
                .any(|s| matches!(s.dashes, Dashes::None) && s.name == *n),
        }
    }

    /// A human-readable spelling for display and clipboard copy — for an
    /// entity converted from a pre-0.5.0 `Flag`, byte-identical to
    /// that type's own `spelling()` (pinned by tests below).
    pub fn spelling(&self) -> String {
        let mut spelling = self
            .spellings
            .iter()
            .map(Spelling::render)
            .collect::<Vec<_>>()
            .join(", ");
        if let Some(name) = &self.value_name {
            match self.value_kind {
                ValueKind::Required => spelling.push_str(&format!(" {name}")),
                ValueKind::Optional => spelling.push_str(&format!("[={name}]")),
                ValueKind::None => {}
            }
        }
        spelling
    }

    /// The single spelling to put on a command line for this entity: the
    /// long-like one when the tool documents one, otherwise the short
    /// letter. `None` for an entity that is not a flag, and for a flag with
    /// no spellings at all.
    ///
    /// The preference is the same one [`Entity::key`] already applies for
    /// identity, and for the same reason: the long spelling is the one that
    /// still reads as itself in someone's shell history a week later.
    /// Rendered through [`Spelling::typed`], so a negatable flag composes as
    /// `--color`, never as the un-runnable `--[no-]color`.
    ///
    /// No value placeholder is appended. A flag that takes one composes as
    /// the bare flag, and the value is the user's to type — spec §2's
    /// `--print-selection` hands over a line to *edit*, and inventing
    /// `--output FILE` on it would put a literal `FILE` in their history.
    pub fn shell_spelling(&self) -> Option<String> {
        if !matches!(self.kind, EntityKind::Flag) {
            return None;
        }
        self.long_spelling()
            .or_else(|| self.short_spelling())
            .map(Spelling::typed)
    }

    /// The canonical addressing key, for search indexing and cross-source
    /// matching.
    ///
    /// For a `Flag`, the same preference order as the pre-0.5.0
    /// `Flag::key`: the long-like spelling wins, a lone short letter is the
    /// fallback, and a flag with no spellings at all has no key. For a
    /// dashless kind (`Positional`, `Modifier`, `EnvVar`), the bare
    /// [`Entity::primary_name`] wrapped in [`crate::noderef::FlagKey::Name`]
    /// — `None` only when the entity has no spellings to name it by.
    pub fn key(&self) -> Option<crate::noderef::FlagKey> {
        use crate::noderef::FlagKey;
        match self.kind {
            EntityKind::Flag => {
                if let Some(l) = self.long_spelling() {
                    return Some(FlagKey::Long(l.name.clone()));
                }
                self.short().map(FlagKey::Short)
            }
            EntityKind::Positional | EntityKind::Modifier | EntityKind::EnvVar => self
                .spellings
                .first()
                .map(|s| FlagKey::Name(s.name.clone())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::noderef::FlagKey;
    use crate::provenance::Provenance;

    /// One documented flag shape and the literal strings/values it must
    /// produce — the same shape matrix the pre-0.5.0 `Flag` parity tests
    /// covered, restated as direct `Entity` construction against literal
    /// expected output rather than a second type built to agree with the
    /// first. Nothing here is self-referential: every `expected*` field is
    /// a literal, never another `Entity` method's return value.
    struct Case {
        spellings: Vec<Spelling>,
        expected_spelling: &'static str,
        expected_key: Option<FlagKey>,
        expected_short: Option<char>,
        expected_long: Option<&'static str>,
        expected_negatable: bool,
        expected_single_dash: bool,
    }

    fn entity_flag(spellings: Vec<Spelling>) -> Entity {
        let mut e = Entity::new(EntityKind::Flag, Provenance::default());
        e.spellings = spellings;
        e
    }

    /// Every flag shape the corpus actually contains, plus the degenerate
    /// and single-dash cases it does not, as one shared matrix.
    fn cases() -> Vec<Case> {
        vec![
            Case {
                spellings: vec![Spelling::short('i'), Spelling::long("interactive")],
                expected_spelling: "-i, --interactive",
                expected_key: Some(FlagKey::Long("interactive".into())),
                expected_short: Some('i'),
                expected_long: Some("interactive"),
                expected_negatable: false,
                expected_single_dash: false,
            },
            Case {
                spellings: vec![Spelling::long("color")],
                expected_spelling: "--color",
                expected_key: Some(FlagKey::Long("color".into())),
                expected_short: None,
                expected_long: Some("color"),
                expected_negatable: false,
                expected_single_dash: false,
            },
            Case {
                spellings: vec![Spelling::short('?')],
                expected_spelling: "-?",
                expected_key: Some(FlagKey::Short('?')),
                expected_short: Some('?'),
                expected_long: None,
                expected_negatable: false,
                expected_single_dash: false,
            },
            Case {
                spellings: vec![],
                expected_spelling: "",
                expected_key: None,
                expected_short: None,
                expected_long: None,
                expected_negatable: false,
                expected_single_dash: false,
            },
            Case {
                spellings: vec![Spelling {
                    name: "staged".into(),
                    dashes: Dashes::Double,
                    negatable: true,
                    abbrev: None,
                }],
                expected_spelling: "--[no-]staged",
                expected_key: Some(FlagKey::Long("staged".into())),
                expected_short: None,
                expected_long: Some("staged"),
                expected_negatable: true,
                expected_single_dash: false,
            },
            Case {
                spellings: vec![
                    Spelling::short('S'),
                    Spelling {
                        name: "staged".into(),
                        dashes: Dashes::Double,
                        negatable: true,
                        abbrev: None,
                    },
                ],
                expected_spelling: "-S, --[no-]staged",
                expected_key: Some(FlagKey::Long("staged".into())),
                expected_short: Some('S'),
                expected_long: Some("staged"),
                expected_negatable: true,
                expected_single_dash: false,
            },
            Case {
                spellings: vec![Spelling::single_dash("vv")],
                expected_spelling: "-vv",
                expected_key: Some(FlagKey::Long("vv".into())),
                expected_short: None,
                expected_long: Some("vv"),
                expected_negatable: false,
                expected_single_dash: true,
            },
            Case {
                spellings: vec![Spelling::single_dash("help")],
                expected_spelling: "-help",
                expected_key: Some(FlagKey::Long("help".into())),
                expected_short: None,
                expected_long: Some("help"),
                expected_negatable: false,
                expected_single_dash: true,
            },
        ]
    }

    #[test]
    fn spelling_matches_the_literal_expected_string() {
        for c in cases() {
            let e = entity_flag(c.spellings.clone());
            assert_eq!(e.spelling(), c.expected_spelling);
        }
    }

    #[test]
    fn key_matches_the_literal_expected_key() {
        for c in cases() {
            let e = entity_flag(c.spellings.clone());
            assert_eq!(e.key(), c.expected_key);
        }
    }

    /// The four spelling accessors match their literal expected values for
    /// every shape in the matrix. This is what makes `FlagSnapshot`'s
    /// `spellings` key serialize correctly: the snapshot renders each
    /// [`Spelling`] directly, and a disagreement here is a moved corpus
    /// fixture.
    #[test]
    fn accessors_match_the_literal_expected_fields() {
        for c in cases() {
            let e = entity_flag(c.spellings.clone());
            assert_eq!(
                e.short(),
                c.expected_short,
                "short() for {}",
                c.expected_spelling
            );
            assert_eq!(
                e.long(),
                c.expected_long,
                "long() for {}",
                c.expected_spelling
            );
            assert_eq!(
                e.negatable(),
                c.expected_negatable,
                "negatable() for {}",
                c.expected_spelling
            );
            assert_eq!(
                e.single_dash(),
                c.expected_single_dash,
                "single_dash() for {}",
                c.expected_spelling
            );
        }
    }

    /// `flag_spelled` — the two-slot adapter every non-multi-spelling tier
    /// emits through — reproduces the same literal shapes.
    #[test]
    fn flag_spelled_matches_literal_shapes() {
        let e = Entity::flag_spelled(
            Some('i'),
            Some("interactive".into()),
            false,
            false,
            Provenance::default(),
        );
        assert_eq!(e.spelling(), "-i, --interactive");
        assert_eq!(e.key(), Some(FlagKey::Long("interactive".into())));

        let e = Entity::flag_spelled(
            None,
            Some("help".into()),
            true,
            false,
            Provenance::default(),
        );
        assert_eq!(e.spelling(), "-help");
        assert!(e.single_dash());

        let e = Entity::flag_spelled(
            Some('S'),
            Some("staged".into()),
            false,
            true,
            Provenance::default(),
        );
        assert_eq!(e.spelling(), "-S, --[no-]staged");
    }

    #[test]
    fn env_var_is_stored_verbatim_on_the_flag() {
        let mut e = Entity::flag_long("color", Provenance::default());
        e.env_var = Some("CLICOLOR".into());
        assert_eq!(e.env_var.as_deref(), Some("CLICOLOR"));
    }

    /// `matches_key` addresses an entity by either spelling, whichever one
    /// `key()` considers canonical.
    #[test]
    fn matches_key_addresses_both_spellings() {
        let e = entity_flag(vec![Spelling::short('i'), Spelling::long("interactive")]);
        assert!(e.matches_key(&FlagKey::Short('i')));
        assert!(e.matches_key(&FlagKey::Long("interactive".into())));
        assert!(!e.matches_key(&FlagKey::Short('x')));
        assert!(!e.matches_key(&FlagKey::Long("other".into())));

        // A single-dash long is addressed by its long key, not as a short.
        let e = entity_flag(vec![Spelling::single_dash("vv")]);
        assert!(e.matches_key(&FlagKey::Long("vv".into())));
        assert!(!e.matches_key(&FlagKey::Short('v')));
    }

    /// `FlagKey::Name` never matches a `Flag` entity, even one whose long
    /// spelling happens to equal the name being searched for — `Name` and
    /// `Long`/`Short` are disjoint key spaces, not two notations for the
    /// same identity. A `Flag`'s spellings always carry a dash, so no
    /// `Dashes::None` spelling is ever there to match against.
    #[test]
    fn name_key_never_matches_a_flag_entity() {
        let e = Entity::flag_long("pathspec", Provenance::default());
        assert!(!e.matches_key(&FlagKey::Name("pathspec".into())));
        assert_ne!(e.key(), Some(FlagKey::Name("pathspec".into())));
    }

    /// `Long`/`Short` never match a dashless entity, even one whose bare
    /// name is a single character that could be mistaken for a short
    /// flag's letter.
    #[test]
    fn long_and_short_keys_never_match_a_dashless_entity() {
        let modifier = Entity::modifier('i', Provenance::default());
        assert!(!modifier.matches_key(&FlagKey::Short('i')));
        assert!(!modifier.matches_key(&FlagKey::Long("i".into())));

        let positional = Entity::positional("interactive", Provenance::default());
        assert!(!positional.matches_key(&FlagKey::Long("interactive".into())));

        let env_var = Entity::env_var_item("i", Provenance::default());
        assert!(!env_var.matches_key(&FlagKey::Short('i')));
    }

    /// Each dashless kind is addressed by `Name` and only by `Name`, and a
    /// `Name` key for one entity's spelling does not leak into matching a
    /// different entity with a different bare name.
    #[test]
    fn name_key_addresses_exactly_the_dashless_entity_it_names() {
        let modifier = Entity::modifier('d', Provenance::default());
        let positional = Entity::positional("pathspec", Provenance::default());
        let env_var = Entity::env_var_item("NODE_DEBUG", Provenance::default());

        assert!(modifier.matches_key(&FlagKey::Name("d".into())));
        assert!(!modifier.matches_key(&FlagKey::Name("pathspec".into())));

        assert!(positional.matches_key(&FlagKey::Name("pathspec".into())));
        assert!(!positional.matches_key(&FlagKey::Name("d".into())));

        assert!(env_var.matches_key(&FlagKey::Name("NODE_DEBUG".into())));
        assert!(!env_var.matches_key(&FlagKey::Name("pathspec".into())));
    }

    /// A lone single-dash *character* is a short flag, not a one-character
    /// single-dash long — the one place the entity model decides by shape
    /// what `Flag` decided by which slot a producer happened to fill.
    ///
    /// No producer emits a one-character single-dash long (both repairs in
    /// `help_text::sections` build names of two characters or more), and no
    /// flag in the 105-fixture corpus has one, so this rule costs nothing
    /// today. It is pinned because the cost of being wrong is a silently
    /// re-keyed flag rather than a loud failure.
    #[test]
    fn a_lone_single_dash_character_is_a_short_flag() {
        let mut e = Entity::new(EntityKind::Flag, Provenance::default());
        e.spellings = vec![Spelling::single_dash("x")];
        assert_eq!(e.short(), Some('x'));
        assert_eq!(e.long(), None);
        assert!(!e.single_dash());
        assert_eq!(e.key(), Some(FlagKey::Short('x')));
        assert_eq!(e.spelling(), "-x");
    }

    /// A short and a single-dash long can coexist (`-v` beside `-vv`) and
    /// the accessors keep them apart by length.
    #[test]
    fn short_and_single_dash_long_coexist() {
        let mut e = Entity::new(EntityKind::Flag, Provenance::default());
        e.spellings = vec![Spelling::short('v'), Spelling::single_dash("vv")];
        assert_eq!(e.short(), Some('v'));
        assert_eq!(e.long(), Some("vv"));
        assert!(e.single_dash());
        assert_eq!(e.spelling(), "-v, -vv");
    }

    #[test]
    fn multi_spelling_renders_every_form_once() {
        // The ffplay row today's Flag cannot hold: -h, -?, -help, --help.
        let mut e = Entity::new(EntityKind::Flag, Provenance::default());
        e.spellings = vec![
            Spelling::short('h'),
            Spelling::short('?'),
            Spelling::single_dash("help"),
            Spelling::long("help"),
        ];
        assert_eq!(e.spelling(), "-h, -?, -help, --help");
        // The long-like spelling wins the identity key.
        assert_eq!(e.key(), Some(FlagKey::Long("help".into())));
    }

    /// `render()` reproduces the abbreviation-bracket form the tool
    /// documented; `typed()` and `key()` address the full name, ignoring
    /// the bracket — a shell doesn't need the tool's documentation
    /// shorthand, and identity should not depend on how much of a name a
    /// particular row happened to abbreviate.
    #[test]
    fn abbrev_renders_the_bracket_but_types_and_keys_the_full_name() {
        let ip_resolve = Spelling {
            name: "resolve".into(),
            dashes: Dashes::Single,
            negatable: false,
            abbrev: Some(1),
        };
        assert_eq!(ip_resolve.render(), "-r[esolve]");
        assert_eq!(ip_resolve.typed(), "-resolve");

        let ip_rcvbuf = Spelling {
            name: "rcvbuf".into(),
            dashes: Dashes::Single,
            negatable: false,
            abbrev: Some(2),
        };
        assert_eq!(ip_rcvbuf.render(), "-rc[vbuf]");
        assert_eq!(ip_rcvbuf.typed(), "-rcvbuf");

        let brief = Spelling {
            name: "brief".into(),
            dashes: Dashes::Double,
            negatable: false,
            abbrev: Some(2),
        };
        assert_eq!(brief.render(), "--br[ief]");
        assert_eq!(brief.typed(), "--brief");

        let mut e = Entity::new(EntityKind::Flag, Provenance::default());
        e.spellings = vec![ip_rcvbuf];
        // Abbreviation doesn't change the key rule: a single-dash spelling
        // longer than one character is still long-like.
        assert_eq!(e.key(), Some(FlagKey::Long("rcvbuf".into())));
        assert_eq!(e.long(), Some("rcvbuf"));
        assert_eq!(e.short(), None);
    }

    /// `shell_spelling` prefers the long-like spelling, falls back to the
    /// short letter, and never emits the `[no-]` notation — the three rules
    /// spec §2's `--print-selection` composes a command line from.
    #[test]
    fn shell_spelling_prefers_long_and_drops_the_no_notation() {
        let both = Entity::flag_spelled(
            Some('i'),
            Some("interactive".into()),
            false,
            false,
            Provenance::default(),
        );
        assert_eq!(both.shell_spelling().as_deref(), Some("--interactive"));

        let short_only = Entity::flag_short('x', Provenance::default());
        assert_eq!(short_only.shell_spelling().as_deref(), Some("-x"));

        // A single-dash long keeps its one dash: `-help`, not `--help`.
        let single = Entity::flag_spelled(
            None,
            Some("help".into()),
            true,
            false,
            Provenance::default(),
        );
        assert_eq!(single.shell_spelling().as_deref(), Some("-help"));

        // `--[no-]color` documents two spellings; only the affirmative one
        // can be typed, and `spelling()` keeps showing the documentation.
        let negatable = Entity::flag_spelled(
            None,
            Some("color".into()),
            false,
            true,
            Provenance::default(),
        );
        assert_eq!(negatable.spelling(), "--[no-]color");
        assert_eq!(negatable.shell_spelling().as_deref(), Some("--color"));
    }

    /// A value placeholder stays out of the composed spelling: the line is
    /// handed over to be edited, and a literal `FILE` in it is worse than
    /// nothing.
    #[test]
    fn shell_spelling_omits_the_value_placeholder() {
        let mut e = Entity::flag_long("output", Provenance::default());
        e.value_name = Some("FILE".into());
        e.value_kind = ValueKind::Required;
        assert_eq!(e.spelling(), "--output FILE");
        assert_eq!(e.shell_spelling().as_deref(), Some("--output"));
    }

    /// Only flags compose. A positional's spelling is a placeholder name
    /// (`pathspec`), and appending it to a command line would put that
    /// literal word on the user's prompt.
    #[test]
    fn shell_spelling_is_none_for_non_flags() {
        let p = Entity::positional("pathspec", Provenance::default());
        assert_eq!(p.shell_spelling(), None);

        let mut m = Entity::new(EntityKind::Modifier, Provenance::default());
        m.spellings = vec![Spelling::bare("d")];
        assert_eq!(m.shell_spelling(), None);

        let empty = Entity::new(EntityKind::Flag, Provenance::default());
        assert_eq!(empty.shell_spelling(), None);
    }

    #[test]
    fn bare_spellings_key_by_name_not_flag_key() {
        let mut e = Entity::new(EntityKind::Modifier, Provenance::default());
        e.spellings = vec![Spelling::bare("d")];
        assert_eq!(e.spelling(), "d");
        assert_eq!(e.key(), Some(FlagKey::Name("d".into())));
    }

    /// Every positional shape the corpus contains: plain, required,
    /// variadic, described, and the combinations — built directly as
    /// `Entity`, asserted against literal expected values rather than a
    /// second type built to agree with `Entity`.
    #[test]
    fn positional_accessors_match_the_literal_expected_fields() {
        let plain = Entity::positional("pathspec", Provenance::default());
        assert_eq!(plain.primary_name(), "pathspec");
        assert!(!plain.required);
        assert!(!plain.repeatable);
        assert_eq!(plain.description, None);

        let mut required = Entity::positional("FILE", Provenance::default());
        required.required = true;
        assert_eq!(required.primary_name(), "FILE");
        assert!(required.required);
        assert!(!required.repeatable);

        let mut variadic = Entity::positional("args", Provenance::default());
        variadic.repeatable = true;
        assert_eq!(variadic.primary_name(), "args");
        assert!(!variadic.required);
        assert!(variadic.repeatable);

        let mut both = Entity::positional("path", Provenance::default());
        both.required = true;
        both.repeatable = true;
        both.description = Some(Text::sanitize("one or more paths to add"));
        assert_eq!(both.primary_name(), "path");
        assert!(both.required);
        assert!(both.repeatable);
        assert_eq!(
            both.description.as_ref().map(Text::as_str),
            Some("one or more paths to add")
        );
    }

    /// `PositionalSnapshot::from` reads the four keys a corpus fixture is
    /// written in straight off the entity, against literal expected
    /// values.
    #[test]
    fn positional_snapshot_matches_literal_expected_fields() {
        use crate::snapshot::PositionalSnapshot;

        let mut both = Entity::positional("path", Provenance::default());
        both.required = true;
        both.repeatable = true;
        both.description = Some(Text::sanitize("one or more paths to add"));

        let snap = PositionalSnapshot::from(&both);
        assert_eq!(snap.name, "path");
        assert!(snap.required);
        assert!(snap.variadic);
        assert_eq!(
            snap.description.as_deref(),
            Some("one or more paths to add")
        );
    }

    /// A modifier is one dashless *letter*: it renders bare, is keyed by
    /// name rather than by [`FlagKey::Short`]/[`FlagKey::Long`], and is not
    /// addressed as a short flag even though its spelling is a single
    /// character — the dash is what makes `-a` a short flag, and a
    /// modifier has none.
    #[test]
    fn a_modifier_is_one_dashless_letter() {
        let e = Entity::modifier('a', Provenance::default());
        assert_eq!(e.kind, EntityKind::Modifier);
        assert_eq!(e.spellings.len(), 1);
        assert_eq!(e.spellings[0].dashes, Dashes::None);
        assert_eq!(e.spelling(), "a");
        assert_eq!(e.primary_name(), "a");
        assert_eq!(e.key(), Some(FlagKey::Name("a".into())));
        assert_eq!(e.short(), None);
        assert_eq!(e.long(), None);
        assert!(!e.matches_key(&FlagKey::Short('a')));
        assert!(e.matches_key(&FlagKey::Name("a".into())));
    }

    /// `ar`'s `[l <text> ]`: a modifier that takes an operand renders it
    /// the same way a flag's is rendered, one space behind the spelling.
    #[test]
    fn a_modifier_can_carry_an_operand() {
        let mut e = Entity::modifier('l', Provenance::default());
        e.value_name = Some("<text>".into());
        e.value_kind = ValueKind::Required;
        assert_eq!(e.spelling(), "l <text>");
    }

    /// A positional is one dashless spelling, so it renders as the bare
    /// name and is keyed by that name rather than a `Long`/`Short` flag key.
    #[test]
    fn a_positional_is_one_dashless_spelling() {
        let e = Entity::positional("pathspec", Provenance::default());
        assert_eq!(e.kind, EntityKind::Positional);
        assert_eq!(e.spellings.len(), 1);
        assert_eq!(e.spellings[0].dashes, Dashes::None);
        assert_eq!(e.spelling(), "pathspec");
        assert_eq!(e.primary_name(), "pathspec");
        assert_eq!(e.key(), Some(FlagKey::Name("pathspec".into())));
        assert_eq!(e.short(), None);
        assert_eq!(e.long(), None);
    }

    /// An env-var item is one dashless *name*: it renders bare, is keyed by
    /// that name, and is not addressed as a short flag — the same shape as
    /// a modifier or a positional, just with a word instead of a letter.
    #[test]
    fn an_env_var_is_one_dashless_name() {
        let e = Entity::env_var_item("NODE_DEBUG", Provenance::default());
        assert_eq!(e.kind, EntityKind::EnvVar);
        assert_eq!(e.spellings.len(), 1);
        assert_eq!(e.spellings[0].dashes, Dashes::None);
        assert_eq!(e.spelling(), "NODE_DEBUG");
        assert_eq!(e.primary_name(), "NODE_DEBUG");
        assert_eq!(e.key(), Some(FlagKey::Name("NODE_DEBUG".into())));
        assert_eq!(e.short(), None);
        assert_eq!(e.long(), None);
        assert!(!e.matches_key(&FlagKey::Short('N')));
    }

    /// `Entity.env_var` (a flag's cross-reference to a variable that also
    /// sets it) and `EntityKind::EnvVar` (the variable documented as its
    /// own item) are different things carried by different entities — a
    /// flag with `env_var = Some("FOO")` is still `EntityKind::Flag`, never
    /// `EntityKind::EnvVar`, and building one never touches the other.
    #[test]
    fn a_flags_env_var_field_is_not_an_env_var_entity() {
        let mut flag = Entity::flag_long("port", Provenance::default());
        flag.env_var = Some("APP_PORT".into());
        assert_eq!(flag.kind, EntityKind::Flag);

        let item = Entity::env_var_item("APP_PORT", Provenance::default());
        assert_eq!(item.kind, EntityKind::EnvVar);
        // The flag's cross-reference is untouched by the item's existence
        // and vice versa: these are two independent entities, not one
        // populated from the other.
        assert_eq!(flag.env_var.as_deref(), Some("APP_PORT"));
        assert_eq!(item.env_var, None);
    }
}
