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
//! Migration is staged (spec §4.5): [`Flag`] converts losslessly via
//! `From<Flag>`, and the conversion's display and addressing parity with
//! [`Flag::spelling`]/[`Flag::key`] is pinned by tests in this module —
//! corpus snapshots must stay byte-identical through the Flag migration.

use serde::{Deserialize, Serialize};

use crate::node::{Flag, ValueKind};
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
/// [`Flag::negatable`] and [`Flag::single_dash`] document: what a user
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
    /// for this spelling (see [`Flag::negatable`]).
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
    /// name — exactly the reconstruction [`Flag::spelling`] performs.
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
            provenance,
        }
    }

    /// A human-readable spelling for display and clipboard copy — for an
    /// entity converted from a [`Flag`], byte-identical to
    /// [`Flag::spelling`] (pinned by tests below).
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
    /// preference order as [`Flag::key`]: the long-like spelling wins, a
    /// lone short letter is the fallback, and an entity with no spellings
    /// (or a dashless kind) has no key.
    pub fn key(&self) -> Option<crate::noderef::FlagKey> {
        use crate::noderef::FlagKey;
        // Long-like: double-dash always; single-dash when the name is more
        // than one character (the single-dash long convention). A lone
        // single-dash character is a short flag.
        let long = self.spellings.iter().find(|s| {
            matches!(s.dashes, Dashes::Double)
                || (matches!(s.dashes, Dashes::Single) && s.name.chars().count() > 1)
        });
        if let Some(l) = long {
            return Some(FlagKey::Long(l.name.clone()));
        }
        self.spellings
            .iter()
            .find(|s| matches!(s.dashes, Dashes::Single))
            .and_then(|s| s.name.chars().next())
            .map(FlagKey::Short)
    }
}

impl From<Flag> for Entity {
    /// Lossless conversion from the pre-0.5.0 `Flag`, preserving the
    /// short-then-long spelling order [`Flag::spelling`] rendered.
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
        // Representative shapes: short+long, long-only negatable,
        // single-dash long, lone `-?`, required value, optional value.
        let mut cases = Vec::new();

        cases.push(flag(Some('i'), Some("interactive")));

        let mut negatable = flag(None, Some("staged"));
        negatable.negatable = true;
        cases.push(negatable);

        let mut vv = flag(None, Some("vv"));
        vv.single_dash = true;
        cases.push(vv);

        cases.push(flag(Some('?'), None));

        let mut valued = flag(Some('o'), Some("output"));
        valued.value_name = Some("FILE".into());
        valued.value_kind = ValueKind::Required;
        cases.push(valued);

        let mut optional = flag(None, Some("color"));
        optional.value_name = Some("WHEN".into());
        optional.value_kind = ValueKind::Optional;
        cases.push(optional);

        for f in cases {
            let expected = f.spelling();
            let entity = Entity::from(f);
            assert_eq!(entity.spelling(), expected);
        }
    }

    #[test]
    fn conversion_preserves_identity_key() {
        let short_and_long = flag(Some('i'), Some("interactive"));
        let long_only = flag(None, Some("color"));
        let short_only = flag(Some('?'), None);
        let mut single_dash = flag(None, Some("vv"));
        single_dash.single_dash = true;
        let degenerate = flag(None, None);

        for f in [
            short_and_long,
            long_only,
            short_only,
            single_dash,
            degenerate,
        ] {
            let expected = f.key();
            let entity = Entity::from(f);
            assert_eq!(entity.key(), expected);
        }
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
