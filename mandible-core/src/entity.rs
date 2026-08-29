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
//! Migration is staged (spec §4.5). The Flag stage is complete:
//! `CommandNode::flags` is `Vec<Entity>` and the pre-0.5.0 `Flag` survives
//! only as this module's test-local parity reference, against which
//! [`Entity::spelling`], [`Entity::key`] and the `short`/`long`/
//! `negatable`/`single_dash` accessors are pinned — corpus snapshots stay
//! byte-identical across the migration.

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
    /// The bare name: `"i"`, `"interactive"`, `"?"`, `"help"`, `"FFREPORT"`.
    pub name: String,
    /// How many dashes [`Spelling::render`] puts in front of `name`.
    pub dashes: Dashes,
    /// True when the tool documents the `--[no-]name` negation convention
    /// for this spelling (the pre-0.5.0 `Flag::negatable`).
    pub negatable: bool,
}

impl Spelling {
    /// A short flag spelling: `-c`.
    pub fn short(c: char) -> Spelling {
        Spelling {
            name: c.to_string(),
            dashes: Dashes::Single,
            negatable: false,
        }
    }

    /// An ordinary long spelling: `--name`.
    pub fn long(name: impl Into<String>) -> Spelling {
        Spelling {
            name: name.into(),
            dashes: Dashes::Double,
            negatable: false,
        }
    }

    /// A single-dash long spelling: `-name` (qemu/find/gcc convention).
    pub fn single_dash(name: impl Into<String>) -> Spelling {
        Spelling {
            name: name.into(),
            dashes: Dashes::Single,
            negatable: false,
        }
    }

    /// A dashless spelling: an `ar`-style modifier letter or an
    /// environment variable name.
    pub fn bare(name: impl Into<String>) -> Spelling {
        Spelling {
            name: name.into(),
            dashes: Dashes::None,
            negatable: false,
        }
    }

    /// The user-visible form: dashes, then `[no-]` if negatable, then the
    /// name — exactly the reconstruction the pre-0.5.0 `Flag::spelling` performed.
    pub fn render(&self) -> String {
        let dashes = match self.dashes {
            Dashes::None => "",
            Dashes::Single => "-",
            Dashes::Double => "--",
        };
        let no = if self.negatable { "[no-]" } else { "" };
        format!("{dashes}{no}{}", self.name)
    }
}

/// Which kind of documented item an [`Entity`] is. Decides which detail
/// pane section it renders under (spec §9.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Enumerated choices, e.g. `{json|yaml|table}` for `--format`.
    pub choices: Vec<Text>,
    /// True if this entity may be given more than once.
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

    /// True if `key` addresses this entity, checking every spelling
    /// regardless of which one is considered canonical.
    pub fn matches_key(&self, key: &crate::noderef::FlagKey) -> bool {
        match key {
            crate::noderef::FlagKey::Long(l) => self.long() == Some(l.as_str()),
            crate::noderef::FlagKey::Short(s) => self.short() == Some(*s),
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

    /// The canonical identity key for cross-source matching, with the same
    /// preference order as the pre-0.5.0 `Flag::key`: the long-like spelling wins, a
    /// lone short letter is the fallback, and an entity with no spellings
    /// (or a dashless kind) has no key.
    pub fn key(&self) -> Option<crate::noderef::FlagKey> {
        use crate::noderef::FlagKey;
        if let Some(l) = self.long_spelling() {
            return Some(FlagKey::Long(l.name.clone()));
        }
        self.short().map(FlagKey::Short)
    }
}

/// The pre-0.5.0 `Flag`, kept **only** as this module's parity reference.
///
/// It is not the IR any more — `CommandNode::flags` is `Vec<Entity>` and no
/// crate outside these tests can name this type. It survives because the
/// migration's success condition is behavioural, not structural: every
/// corpus snapshot stays byte-identical, which is a claim about
/// [`Entity::spelling`], [`Entity::key`] and the `short`/`long`/`negatable`/
/// `single_dash` accessors reproducing what this struct's methods produced.
/// Deleting it would delete the only independent statement of what they are
/// supposed to agree with, leaving the tests asserting `Entity` equals
/// itself.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Flag {
    pub short: Option<char>,
    pub long: Option<String>,
    pub value_name: Option<String>,
    pub value_kind: ValueKind,
    pub choices: Vec<Text>,
    pub repeatable: bool,
    pub required: bool,
    pub negatable: bool,
    pub single_dash: bool,
    pub hidden: bool,
    pub deprecated: Option<Text>,
    pub inherited: bool,
    pub group: Option<String>,
    pub description: Option<Text>,
    pub default: Option<Text>,
    pub env_var: Option<String>,
    pub provenance: Provenance,
}

#[cfg(test)]
impl Flag {
    /// A minimal flag with only a long spelling.
    pub fn long(name: impl Into<String>, provenance: Provenance) -> Flag {
        Flag {
            short: None,
            long: Some(name.into()),
            value_name: None,
            value_kind: ValueKind::None,
            choices: Vec::new(),
            repeatable: false,
            required: false,
            negatable: false,
            single_dash: false,
            hidden: false,
            deprecated: None,
            inherited: false,
            group: None,
            description: None,
            default: None,
            env_var: None,
            provenance,
        }
    }

    /// The pre-0.5.0 identity key: prefer the long name, fall back to the
    /// short letter.
    pub fn key(&self) -> Option<crate::noderef::FlagKey> {
        if let Some(long) = &self.long {
            Some(crate::noderef::FlagKey::Long(long.clone()))
        } else {
            self.short.map(crate::noderef::FlagKey::Short)
        }
    }

    /// The pre-0.5.0 display spelling, e.g. `"-i, --interactive"`.
    pub fn spelling(&self) -> String {
        let mut parts = Vec::new();
        if let Some(s) = self.short {
            parts.push(format!("-{s}"));
        }
        if let Some(l) = &self.long {
            let dashes = if self.single_dash { "-" } else { "--" };
            if self.negatable {
                parts.push(format!("{dashes}[no-]{l}"));
            } else {
                parts.push(format!("{dashes}{l}"));
            }
        }
        let mut spelling = parts.join(", ");
        if let Some(name) = &self.value_name {
            match self.value_kind {
                ValueKind::Required => spelling.push_str(&format!(" {name}")),
                ValueKind::Optional => spelling.push_str(&format!("[={name}]")),
                ValueKind::None => {}
            }
        }
        spelling
    }
}

#[cfg(test)]
impl From<Flag> for Entity {
    /// Lossless conversion from the pre-0.5.0 `Flag`, preserving the
    /// short-then-long spelling order `Flag::spelling` rendered.
    fn from(f: Flag) -> Entity {
        let mut spellings = Vec::new();
        if let Some(c) = f.short {
            spellings.push(Spelling::short(c));
        }
        if let Some(l) = f.long {
            spellings.push(Spelling {
                name: l,
                dashes: if f.single_dash {
                    Dashes::Single
                } else {
                    Dashes::Double
                },
                negatable: f.negatable,
            });
        }
        Entity {
            kind: EntityKind::Flag,
            spellings,
            value_name: f.value_name,
            value_kind: f.value_kind,
            choices: f.choices,
            repeatable: f.repeatable,
            required: f.required,
            hidden: f.hidden,
            deprecated: f.deprecated,
            inherited: f.inherited,
            group: f.group,
            description: f.description,
            default: f.default,
            see_also: Vec::new(),
            env_var: f.env_var,
            provenance: f.provenance,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::noderef::FlagKey;
    use crate::provenance::Provenance;

    fn flag(short: Option<char>, long: Option<&str>) -> Flag {
        let mut f = Flag::long("x", Provenance::default());
        f.short = short;
        f.long = long.map(str::to_string);
        f
    }

    #[test]
    fn conversion_preserves_display_spelling() {
        for f in parity_cases() {
            let expected = f.spelling();
            let entity = Entity::from(f);
            assert_eq!(entity.spelling(), expected);
        }
    }

    #[test]
    fn conversion_preserves_identity_key() {
        for f in parity_cases() {
            let expected = f.key();
            let entity = Entity::from(f);
            assert_eq!(entity.key(), expected);
        }
    }

    /// Every flag shape the corpus actually contains, plus the degenerate
    /// and single-dash cases it does not, as one shared matrix. The
    /// accessor-parity test below is the load-bearing one: `snapshot.rs`,
    /// `merge.rs` and every xtask detector now read `short()`/`long()`/
    /// `negatable()`/`single_dash()` where they used to read `Flag`'s
    /// fields, so those four functions are precisely what stands between
    /// the migration and a moved snapshot.
    fn parity_cases() -> Vec<Flag> {
        let mut cases = Vec::new();
        cases.push(flag(Some('i'), Some("interactive")));
        cases.push(flag(None, Some("color")));
        cases.push(flag(Some('?'), None));
        cases.push(flag(None, None));

        let mut negatable = flag(None, Some("staged"));
        negatable.negatable = true;
        cases.push(negatable);

        let mut negatable_pair = flag(Some('S'), Some("staged"));
        negatable_pair.negatable = true;
        cases.push(negatable_pair);

        let mut vv = flag(None, Some("vv"));
        vv.single_dash = true;
        cases.push(vv);

        let mut help = flag(None, Some("help"));
        help.single_dash = true;
        cases.push(help);

        let mut valued = flag(Some('o'), Some("output"));
        valued.value_name = Some("FILE".into());
        valued.value_kind = ValueKind::Required;
        cases.push(valued);

        let mut optional = flag(None, Some("color"));
        optional.value_name = Some("WHEN".into());
        optional.value_kind = ValueKind::Optional;
        cases.push(optional);

        cases
    }

    /// The four spelling accessors reproduce the `Flag` fields they
    /// replaced, for every shape in the matrix.
    ///
    /// This is what makes `FlagSnapshot`'s `short`/`long`/`negatable`/
    /// `single_dash` keys serialize byte-identically after the migration:
    /// the snapshot no longer has stored fields to copy, it asks the
    /// entity, so a disagreement here is a moved corpus fixture.
    #[test]
    fn accessors_reproduce_the_flag_fields_they_replaced() {
        for f in parity_cases() {
            let expected = (f.short, f.long.clone(), f.negatable, f.single_dash);
            let e = Entity::from(f.clone());
            assert_eq!(
                (
                    e.short(),
                    e.long().map(str::to_string),
                    e.negatable(),
                    e.single_dash()
                ),
                expected,
                "accessor parity failed for {}",
                f.spelling()
            );
        }
    }

    /// `matches_key` addresses the same entities `Flag::matches_key` did —
    /// both spellings, whichever one `key()` considers canonical.
    #[test]
    fn matches_key_addresses_both_spellings() {
        let e = Entity::from(flag(Some('i'), Some("interactive")));
        assert!(e.matches_key(&FlagKey::Short('i')));
        assert!(e.matches_key(&FlagKey::Long("interactive".into())));
        assert!(!e.matches_key(&FlagKey::Short('x')));
        assert!(!e.matches_key(&FlagKey::Long("other".into())));

        // A single-dash long is addressed by its long key, not as a short.
        let mut vv = flag(None, Some("vv"));
        vv.single_dash = true;
        let e = Entity::from(vv);
        assert!(e.matches_key(&FlagKey::Long("vv".into())));
        assert!(!e.matches_key(&FlagKey::Short('v')));
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

    /// `flag_spelled` is the adapter every tier now emits through, so it
    /// has to agree with the conversion the parity tests measure against
    /// for every shape in the matrix.
    #[test]
    fn flag_spelled_agrees_with_the_flag_conversion() {
        for f in parity_cases() {
            let mut built = Entity::flag_spelled(
                f.short,
                f.long.clone(),
                f.single_dash,
                f.negatable,
                f.provenance.clone(),
            );
            // `flag_spelled` decides spellings only; the value fields are
            // the caller's, so carry them over before comparing the
            // rendered form.
            built.value_name = f.value_name.clone();
            built.value_kind = f.value_kind;
            let converted = Entity::from(f);
            assert_eq!(built.spellings, converted.spellings);
            assert_eq!(built.spelling(), converted.spelling());
            assert_eq!(built.key(), converted.key());
        }
    }

    #[test]
    fn env_var_survives_conversion() {
        let mut f = flag(None, Some("color"));
        f.env_var = Some("CLICOLOR".into());
        assert_eq!(Entity::from(f).env_var.as_deref(), Some("CLICOLOR"));
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

    #[test]
    fn bare_spellings_have_no_flag_key() {
        let mut e = Entity::new(EntityKind::Modifier, Provenance::default());
        e.spellings = vec![Spelling::bare("d")];
        assert_eq!(e.spelling(), "d");
        assert_eq!(e.key(), None);
    }
}
