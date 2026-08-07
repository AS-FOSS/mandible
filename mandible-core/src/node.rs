//! The intermediate representation itself: [`CommandNode`], [`Flag`],
//! [`Positional`], [`Example`]. See spec §4.

use crate::provenance::Provenance;
use crate::text::Text;
use serde::{Deserialize, Serialize};

/// One command or subcommand in the tree: `git`, `git rebase`,
/// `git rebase --onto`'s parent, and so on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// This node's own flags (not including inherited ones from ancestors —
    /// those are represented by [`Flag::inherited`] on the flags that
    /// originated in an ancestor and were propagated down).
    pub flags: Vec<Flag>,
    /// Positional arguments.
    pub positionals: Vec<Positional>,
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
            flags: Vec::new(),
            positionals: Vec::new(),
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
        }
    }
}

/// A single flag/option, e.g. `-i, --interactive`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Flag {
    /// Short spelling, e.g. `Some('i')` for `-i`.
    pub short: Option<char>,
    /// Long spelling, e.g. `Some("interactive".into())` for `--interactive`.
    pub long: Option<String>,
    /// The value placeholder, e.g. `"FILE"` in `--output FILE`.
    pub value_name: Option<String>,
    /// Whether this flag takes no value, a required value, or an optional
    /// one.
    pub value_kind: ValueKind,
    /// Enumerated choices, e.g. `{json|yaml|table}` for `--format`.
    pub choices: Vec<Text>,
    /// True if this flag may be given more than once.
    pub repeatable: bool,
    /// True if this flag is required.
    pub required: bool,
    /// True if this flag should be hidden by default.
    pub hidden: bool,
    /// `Some(reason)` when this flag is deprecated.
    pub deprecated: Option<Text>,
    /// True when this flag was declared on an ancestor node and propagated
    /// down (cobra "persistent flag" / carapace `persistentflags`).
    /// Rendered in a separate, dimmed group in the detail pane.
    pub inherited: bool,
    /// Display grouping from the source, e.g. tar's `"Main operation mode"`.
    pub group: Option<String>,
    /// The flag's description.
    pub description: Option<Text>,
    /// The flag's default value, if documented.
    pub default: Option<Text>,
    /// An environment variable that also sets this flag, if documented.
    pub env_var: Option<String>,
    /// Which source(s) contributed this flag's fields.
    pub provenance: Provenance,
}

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

    /// The canonical identity key used for cross-source matching and
    /// addressing: prefer the long name, fall back to the short letter.
    /// Returns `None` for a degenerate flag with neither (which cannot be
    /// addressed and is only matched positionally during merge).
    pub fn key(&self) -> Option<crate::noderef::FlagKey> {
        if let Some(long) = &self.long {
            Some(crate::noderef::FlagKey::Long(long.clone()))
        } else {
            self.short.map(crate::noderef::FlagKey::Short)
        }
    }

    /// True if `key` addresses this flag, checking both spellings
    /// regardless of which one is considered canonical.
    pub fn matches_key(&self, key: &crate::noderef::FlagKey) -> bool {
        match key {
            crate::noderef::FlagKey::Long(l) => self.long.as_deref() == Some(l.as_str()),
            crate::noderef::FlagKey::Short(s) => self.short == Some(*s),
        }
    }

    /// A human-readable spelling for display and clipboard copy, e.g.
    /// `"-i, --interactive"`, `"--output FILE"`.
    pub fn spelling(&self) -> String {
        let mut parts = Vec::new();
        if let Some(s) = self.short {
            parts.push(format!("-{s}"));
        }
        if let Some(l) = &self.long {
            parts.push(format!("--{l}"));
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

/// A positional argument, e.g. `<pathspec>...`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Positional {
    /// The argument's name as shown in usage, e.g. `"pathspec"`.
    pub name: String,
    /// True if this positional must be supplied.
    pub required: bool,
    /// True if this positional accepts multiple values (`...`).
    pub variadic: bool,
    /// The positional's description.
    pub description: Option<Text>,
    /// Which source(s) contributed this positional's fields.
    pub provenance: Provenance,
}

/// A worked example: a command line plus an optional explanation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Example {
    /// The example command line, verbatim.
    pub command: Text,
    /// An optional explanation of what the example does.
    pub explanation: Option<Text>,
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
