//! Field-level fingerprinting (WS2 part 2): a per-tool hash of every
//! flag/positional/modifier/env-var entity's description, choices, and
//! value_name, serialized as the scoreboard's `#fp2` footer lines so
//! [`crate::transition`] can diff two sweeps at field granularity instead
//! of by count alone.

use super::Row;
use std::collections::BTreeMap;

/// One entity's field-level fingerprint (flag, positional, modifier, or
/// env-var item — see [`ToolFingerprint::flags`]): whether it has a
/// description, a hash of the description text, a hash of its choices
/// list, and `value_name` verbatim. Named `FlagFingerprint` from when
/// flags were the only kind fingerprinted; every field applies unchanged
/// to every `EntityKind`.
///
/// Hashes, not full text, for description/choices — keeps the checked-in
/// scoreboard small; it only needs to know "did this change," and the
/// scoreboard files being diffed are on disk for a human to read if so.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FlagFingerprint {
    has_description: bool,
    description_hash: Option<u64>,
    choices_hash: Option<u64>,
    value_name: Option<String>,
}

/// One tool's full field-level fingerprint: every entity — flag,
/// positional, modifier, env-var item — keyed by a stable per-node
/// identity ([`entity_identity`]; never `Entity::spelling`, which folds
/// the value placeholder in), plus the full set of subcommand paths.
///
/// Field still named `flags` from when flags were the only kind
/// fingerprinted; now holds every kind.
///
/// A size comparison against the scoreboard's `flags` column must filter
/// ids to `EntityKind::Flag` first, or it silently reports zero
/// duplicate-carrying tools instead of erroring.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ToolFingerprint {
    flags: BTreeMap<String, FlagFingerprint>,
    subcommands: std::collections::BTreeSet<String>,
}

/// An entity's identity for fingerprinting: every documented spelling's
/// dash-prefixed name, excluding `value_name`/`choices`/description (the
/// fields this fingerprint exists to detect changes in — folding them into
/// the key would turn a `value_name` edit into a remove-then-add).
/// Prefixed with the owning node's dotted path and the entity's
/// `EntityKind` (via `{:?}`) so same-spelled entities on different
/// subcommands or of different kinds never collide.
///
/// Generic over `EntityKind` by construction (derived `{:?}`), not a match
/// arm — AGENTS.md §1: no per-kind branching to grow.
fn entity_identity(path: &str, entity: &mandible_core::Entity) -> String {
    let spelling = entity
        .spellings
        .iter()
        .map(|s| {
            let dash = match s.dashes {
                mandible_core::Dashes::None => "",
                mandible_core::Dashes::Single => "-",
                mandible_core::Dashes::Double => "--",
            };
            format!("{dash}{}", s.name)
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("{path}::{:?}::{spelling}", entity.kind)
}

/// FNV-1a over raw bytes — deterministic across processes and Rust std
/// versions (unlike `DefaultHasher`), needed because hashes from separate
/// `xtask` invocations are compared across a sweep.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Walk `root`'s tree and build its field-level [`ToolFingerprint`] — the
/// data [`Row::fingerprint`] carries and [`fingerprint_lines`] serializes
/// into the scoreboard's `#fp` footer, which [`crate::transition`] reads
/// back to diff at field granularity, since a count column alone cannot
/// distinguish "description text changed" from "it didn't."
///
/// Walks `node.entities` (every `EntityKind`), not `node.flags()` alone,
/// so a fingerprint isn't blind to env-var/modifier/positional changes.
/// See [`entity_identity`] for the no-per-kind-branching discipline.
pub(super) fn build_fingerprint(root: Option<&mandible_core::CommandNode>) -> ToolFingerprint {
    let mut fp = ToolFingerprint::default();
    let Some(root) = root else {
        return fp;
    };
    fn walk(node: &mandible_core::CommandNode, path: &str, fp: &mut ToolFingerprint) {
        for entity in &node.entities {
            let id = entity_identity(path, entity);
            let description_hash = entity
                .description
                .as_ref()
                .map(|t| fnv1a(t.as_str().as_bytes()));
            let choices_hash = if entity.choices.is_empty() {
                None
            } else {
                // Name and per-choice description (ffmpeg's AVOption
                // constants) both feed the hash, so a description-only
                // edit still moves the fingerprint.
                let joined = entity
                    .choices
                    .iter()
                    .map(|c| match &c.description {
                        Some(d) => format!("{}\u{1e}{}", c.name, d.as_str()),
                        None => c.name.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join("\u{1f}");
                Some(fnv1a(joined.as_bytes()))
            };
            fp.flags.insert(
                id,
                FlagFingerprint {
                    has_description: entity.description.is_some(),
                    description_hash,
                    choices_hash,
                    value_name: entity.value_name.clone(),
                },
            );
        }
        for sub in &node.subcommands {
            let sub_path = if path == "(root)" {
                sub.name.clone()
            } else {
                format!("{path}.{}", sub.name)
            };
            fp.subcommands.insert(sub_path.clone());
            walk(sub, &sub_path, fp);
        }
    }
    walk(root, "(root)", &mut fp);
    fp
}

/// Backslash-escape every character the `#fp` wire format uses as
/// structure, so the escaped output contains no raw separator character —
/// the read side (`crate::transition`'s `fp_unescape`) keeps its plain
/// `split`/`splitn` calls and only needs an unescape pass per field.
///
/// Escapes every separator, not just [`FP_FIELD_SEP`]: `value_name` is
/// free-form text lifted verbatim from a tool's own help output and can
/// contain any of them, e.g. `awk`'s `-L` flag value_name
/// `"fatal|invalid|no-ext"` collides with [`FP_FLAG_SEP`].
///
/// Escapes, per character: `\` -> `\\`, tab -> `\t`, newline -> `\n`,
/// [`FP_FLAG_SEP`] (`|`) -> `\p`, [`FP_SUBCOMMAND_SEP`] (`,`) -> `\c`,
/// [`FP_ID_SEP`] (`=`) -> `\e`, [`FP_ENTRY_SEP`] (`:`) -> `\s`.
///
/// Fixture: `corpus/awk/*/help.txt`, `corpus/gawk/*/help.txt`.
fn fp_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            FP_FIELD_SEP => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            FP_FLAG_SEP => out.push_str("\\p"),
            FP_SUBCOMMAND_SEP => out.push_str("\\c"),
            FP_ID_SEP => out.push_str("\\e"),
            FP_ENTRY_SEP => out.push_str("\\s"),
            _ => out.push(c),
        }
    }
    out
}

/// Top-level field separator inside one `#fp2` line (`#fp2 <tool>\t<subs>\t<entities>`)
/// — duplicated into `crate::transition` as its own `FP_FIELD_SEP` for the
/// same reason [`EXTRACT_TIMEOUT_MS`] is duplicated rather than imported: a
/// single well-known, stable character, re-measured in the same commit as
/// the other side if it ever changes. Escaped out of every emitted piece by
/// [`fp_escape`], same as the other three separators below.
const FP_FIELD_SEP: char = '\t';

/// Separator between entity entries inside one `#fp2` line's entity-entry
/// list (`<entity1>|<entity2>|...`) — mirrored in `crate::transition`.
const FP_FLAG_SEP: char = '|';

/// Separator between subcommand paths inside one `#fp2` line's subcommand
/// list (`<sub1>,<sub2>,...`) — mirrored in `crate::transition`.
const FP_SUBCOMMAND_SEP: char = ',';

/// Separator between one entity entry's id and its fields (`<id>=<fields>`) —
/// mirrored in `crate::transition`.
const FP_ID_SEP: char = '=';

/// Separator between one entity entry's fields
/// (`<has_desc>:<desc_hash>:<choices_hash>:<value_name>`) — mirrored in
/// `crate::transition`.
const FP_ENTRY_SEP: char = ':';

/// The `#fp2` line prefix ([`fingerprint_lines`]'s wire format, version 2).
/// A different literal from the pre-generalization `"#fp "` prefix
/// (`crate::transition::FP_LINE_PREFIX_V1`), not a bumped suffix: a v1
/// reader's `strip_prefix("#fp ")` doesn't match `"#fp2 ..."`, so it falls
/// back to the existing "predates the footer" path rather than misreading
/// v2 entity ids. See `crate::transition::FingerprintFormat` for why a
/// v1/v2 pair is refused outright rather than diffed.
const FP_LINE_PREFIX_V2: &str = "#fp2 ";

/// Render every row's [`ToolFingerprint`] as `#fp2` footer lines, one per
/// tool, in `rows`' existing sorted order ([`run_over`]).
///
/// One line per row unconditionally, even for empty `flags`/`subcommands`:
/// [`crate::transition`] tells "predates the footer" from "measured clean"
/// by whether a line exists, so skipping empty rows would hide a total
/// wipeout (entities on one side, none on the other) as "unmeasured"
/// instead of "every entity removed."
///
/// Line shape: `#fp2 <tool>\t<sub1>,<sub2>,...\t<entity1>|<entity2>|...`,
/// each entity `<id>=<has_desc:0/1>:<desc_hash-or-->:<choices_hash-or-->:<value_name-or-->`
/// (hex hashes, `id` carries its `EntityKind` tag). Every field individually
/// run through [`fp_escape`] first. Format version 2 — see
/// [`FP_LINE_PREFIX_V2`].
pub(super) fn fingerprint_lines(rows: &[Row]) -> String {
    let mut out = String::new();
    for row in rows {
        let subs = row
            .fingerprint
            .subcommands
            .iter()
            .map(|s| fp_escape(s))
            .collect::<Vec<_>>()
            .join(",");
        let flags = row
            .fingerprint
            .flags
            .iter()
            .map(|(id, f)| {
                format!(
                    "{}={}:{}:{}:{}",
                    fp_escape(id),
                    if f.has_description { 1 } else { 0 },
                    f.description_hash
                        .map(|h| format!("{h:x}"))
                        .unwrap_or_else(|| "-".to_string()),
                    f.choices_hash
                        .map(|h| format!("{h:x}"))
                        .unwrap_or_else(|| "-".to_string()),
                    f.value_name
                        .as_deref()
                        .map(fp_escape)
                        .unwrap_or_else(|| "-".to_string()),
                )
            })
            .collect::<Vec<_>>()
            .join("|");
        out.push_str(&format!(
            "{FP_LINE_PREFIX_V2}{}{FP_FIELD_SEP}{subs}{FP_FIELD_SEP}{flags}\n",
            fp_escape(&row.tool),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coverage::aggregate::compute_aggregate;
    use crate::coverage::render_text::render_text;
    use crate::coverage::row;
    use crate::coverage::score::score_one;

    /// **The round trip this whole `#fp` footer exists for, on a synthetic
    /// tree — never a host binary.** An earlier version of this test drove
    /// it from a real `grep --help` probe and asserted "at least one flag
    /// has a description," which is a fact about the host's `grep`
    /// (GNU grep's `--help` documents its options; BSD grep's, on macOS,
    /// is a bare usage synopsis with none) — exactly the class of failure
    /// AGENTS.md §4 warns about ("macOS breaks in ways Linux CI cannot
    /// see") and a real red `test (macos-latest)` job on this branch. A
    /// hand-built [`mandible_core::CommandNode`] carrying a described flag,
    /// a flag with choices and a `value_name`, and one subcommand makes the
    /// description-carrying case true by construction, on every platform,
    /// with no process spawned at all.
    #[test]
    fn fingerprint_footer_round_trips_a_synthetic_tree() {
        use mandible_core::{Choice, CommandNode, Entity, Provenance, Source, Text, ValueKind};

        let mut root = CommandNode::new("demo", Provenance::single(Source::HelpText));
        let mut flag = Entity::flag_spelled(
            Some('v'),
            Some("verbose".to_string()),
            false,
            false,
            Provenance::single(Source::HelpText),
        );
        flag.description = Some(Text::sanitize("increase verbosity"));
        flag.choices = vec![Choice::bare("low"), Choice::bare("high")];
        flag.value_name = Some("LEVEL".to_string());
        flag.value_kind = ValueKind::Required;
        root.entities.push(flag);
        root.subcommands.push(CommandNode::new(
            "child",
            Provenance::single(Source::HelpText),
        ));

        let mut r = row("demo", 1, Some(100.0), "ok");
        r.fingerprint = build_fingerprint(Some(&root));
        let rows = vec![r];
        let agg = compute_aggregate(&rows);
        let text = render_text(&rows, &agg);

        let parsed = crate::transition::parse_scoreboard(&text);
        let fp = parsed
            .fingerprints
            .get("demo")
            .expect("demo fingerprint present in the #fp footer");
        assert_eq!(fp.flags.len(), 1);
        let flag = fp.flags.values().next().unwrap();
        assert!(flag.has_description, "description presence round-trips");
        assert!(
            flag.description_hash.is_some(),
            "description hash round-trips"
        );
        assert!(flag.choices_hash.is_some(), "choices hash round-trips");
        assert_eq!(flag.value_name.as_deref(), Some("LEVEL"));
        assert_eq!(fp.subcommands.len(), 1);
        assert!(fp.subcommands.contains("child"));
    }

    /// A real-binary smoke check (spec §3.1: "at least one test exercising
    /// real argv construction," not just the parser behind it), but —
    /// unlike the synthetic test above — asserting only what is true of
    /// *any* host's `grep`: that the round trip through
    /// [`fingerprint_lines`]/[`crate::transition::parse_scoreboard`] loses
    /// nothing, whatever `grep --help` on this machine actually said. Never
    /// a claim about grep's own content (that's the synthetic test's job;
    /// this one would stay green against BSD grep's flagless usage synopsis
    /// just as it does against GNU grep's described option table).
    #[test]
    fn fingerprint_footer_round_trips_whatever_a_real_grep_produced() {
        let live = score_one("grep");
        let rows = vec![live];
        let agg = compute_aggregate(&rows);
        let text = render_text(&rows, &agg);

        let parsed = crate::transition::parse_scoreboard(&text);
        let fp = parsed
            .fingerprints
            .get("grep")
            .expect("a #fp line is emitted unconditionally, even for an empty fingerprint");

        let live_fingerprint = &rows[0].fingerprint;
        assert_eq!(
            fp.flags.len(),
            live_fingerprint.flags.len(),
            "flag count must round-trip losslessly regardless of what this host's grep documents"
        );
        assert_eq!(fp.subcommands.len(), live_fingerprint.subcommands.len());
        for (id, live_flag) in &live_fingerprint.flags {
            let parsed_flag = fp.flags.get(id).unwrap_or_else(|| {
                panic!("flag {id:?} present before rendering must survive the round trip")
            });
            assert_eq!(parsed_flag.has_description, live_flag.has_description);
            assert_eq!(parsed_flag.description_hash, live_flag.description_hash);
            assert_eq!(parsed_flag.choices_hash, live_flag.choices_hash);
            assert_eq!(parsed_flag.value_name, live_flag.value_name);
        }
    }

    /// **The awk regression, reproduced.** PR #22's real finding: `awk`'s
    /// `-L` flag has `value_name` `"fatal|invalid|no-ext"` — free-form text
    /// lifted verbatim from `awk --help`, not something this codebase
    /// invents. `fingerprint_lines`'s flag-list separator is also `|`, so
    /// pre-fix (`fp_escape` only scrubbing tab/newline) the rendered `#fp`
    /// line contains three unescaped pipes where only one flag-list
    /// separator was intended; `transition::parse_fingerprint_line` splits
    /// on every `|` it sees, so `"invalid"` and `"no-ext"` become their own
    /// bogus flag entries with no `=`, `split_once('=')` returns `None`, and
    /// the `?` on that line discards the *entire* `#fp awk` line — every
    /// flag on it, not just `-L`. `awk`/`gawk`/`nawk` silently vanish from
    /// every field-level `sweep-diff` comparison. This test drives the exact
    /// shape through the real pipeline (`build_fingerprint` ->
    /// `fingerprint_lines` -> `transition::parse_scoreboard`) and asserts
    /// the line survives and the value_name comes back byte-for-byte.
    #[test]
    fn fingerprint_footer_round_trips_a_value_name_containing_the_flag_list_separator() {
        use mandible_core::{CommandNode, Entity, Provenance, Source};

        let mut root = CommandNode::new("awk", Provenance::single(Source::HelpText));
        let mut flag = Entity::flag_short('L', Provenance::single(Source::HelpText));
        flag.value_name = Some("fatal|invalid|no-ext".to_string());
        root.entities.push(flag);

        let mut r = row("awk", 1, Some(0.0), "ok");
        r.fingerprint = build_fingerprint(Some(&root));
        let rows = vec![r];
        let agg = compute_aggregate(&rows);
        let text = render_text(&rows, &agg);

        let parsed = crate::transition::parse_scoreboard(&text);
        let fp = parsed.fingerprints.get("awk").expect(
            "pre-fix this whole line is dropped by parse_fingerprint_line \
             because value_name's unescaped `|`s are mistaken for extra \
             flag-list entries",
        );
        assert_eq!(fp.flags.len(), 1, "the only flag on the line must survive");
        let flag = fp.flags.values().next().unwrap();
        assert_eq!(
            flag.value_name.as_deref(),
            Some("fatal|invalid|no-ext"),
            "value_name must round-trip byte-for-byte, not be mangled or dropped"
        );
    }

    /// **Every separator the `#fp` wire format uses, in one sweep**, plus
    /// two defensive cases beyond `value_name`: a subcommand name carrying
    /// the subcommand-list separator (`,`), and a flag long spelling
    /// carrying the flag-list separator (`|`) — a badly-parsed flag can, in
    /// principle, carry anything, and the escaping scheme is supposed to be
    /// blind to *which* piece of text needs it, not special-cased to
    /// `value_name` alone. Each value_name below embeds exactly one
    /// character the wire format would otherwise misread as structure:
    /// `,` (subcommand-list sep), `=` (id/fields sep), `:` (intra-entry
    /// sep), a literal tab (top-level field sep), and a literal backslash
    /// (the escape character itself, which must round-trip too).
    #[test]
    fn fingerprint_footer_round_trips_every_separator_character() {
        use mandible_core::{CommandNode, Entity, Provenance, Source};

        let mut root = CommandNode::new("demo", Provenance::single(Source::HelpText));

        let mut comma_flag = Entity::flag_long("comma-value", Provenance::single(Source::HelpText));
        comma_flag.value_name = Some("a,b".to_string());
        root.entities.push(comma_flag);

        let mut equals_flag =
            Entity::flag_long("equals-value", Provenance::single(Source::HelpText));
        equals_flag.value_name = Some("a=b".to_string());
        root.entities.push(equals_flag);

        let mut colon_flag = Entity::flag_long("colon-value", Provenance::single(Source::HelpText));
        colon_flag.value_name = Some("a:b".to_string());
        root.entities.push(colon_flag);

        let mut tab_flag = Entity::flag_long("tab-value", Provenance::single(Source::HelpText));
        tab_flag.value_name = Some("a\tb".to_string());
        root.entities.push(tab_flag);

        let mut backslash_flag =
            Entity::flag_long("backslash-value", Provenance::single(Source::HelpText));
        backslash_flag.value_name = Some("a\\b".to_string());
        root.entities.push(backslash_flag);

        // Defensive: a flag whose own long spelling (not just its
        // value_name) carries the flag-list separator.
        let pipe_id_flag = Entity::flag_long("weird|name", Provenance::single(Source::HelpText));
        root.entities.push(pipe_id_flag);

        // Defensive: a subcommand name carrying the subcommand-list
        // separator.
        root.subcommands.push(CommandNode::new(
            "sub,with,comma",
            Provenance::single(Source::HelpText),
        ));

        let mut r = row("demo", 1, Some(0.0), "ok");
        r.fingerprint = build_fingerprint(Some(&root));
        let rows = vec![r];
        let agg = compute_aggregate(&rows);
        let text = render_text(&rows, &agg);

        let parsed = crate::transition::parse_scoreboard(&text);
        let fp = parsed
            .fingerprints
            .get("demo")
            .expect("the #fp line must survive with every flag intact");
        assert_eq!(fp.flags.len(), 6, "no flag entry may be lost or merged");

        assert_eq!(
            fp.flags
                .get("(root)::Flag::--comma-value")
                .and_then(|f| f.value_name.clone()),
            Some("a,b".to_string())
        );
        assert_eq!(
            fp.flags
                .get("(root)::Flag::--equals-value")
                .and_then(|f| f.value_name.clone()),
            Some("a=b".to_string())
        );
        assert_eq!(
            fp.flags
                .get("(root)::Flag::--colon-value")
                .and_then(|f| f.value_name.clone()),
            Some("a:b".to_string())
        );
        assert_eq!(
            fp.flags
                .get("(root)::Flag::--tab-value")
                .and_then(|f| f.value_name.clone()),
            Some("a\tb".to_string())
        );
        assert_eq!(
            fp.flags
                .get("(root)::Flag::--backslash-value")
                .and_then(|f| f.value_name.clone()),
            Some("a\\b".to_string())
        );
        assert!(
            fp.flags.contains_key("(root)::Flag::--weird|name"),
            "a flag id carrying the flag-list separator must survive under its own key"
        );

        assert_eq!(fp.subcommands.len(), 1);
        assert!(
            fp.subcommands.contains("sub,with,comma"),
            "a subcommand name carrying the subcommand-list separator must round-trip whole"
        );
    }

    /// **The positive-signal proof this task exists for.** A node carrying
    /// one entity of every `EntityKind` — a flag, a positional, a modifier,
    /// and an env-var item — all with the *same* bare spelling (`"x"`),
    /// must fingerprint as four distinct entries, not collapse into one:
    /// `entity_identity`'s `EntityKind` tag is what tells `ar`'s `x`
    /// modifier apart from a hypothetical `x` flag on the same node, and is
    /// exactly what the pre-generalization fingerprint (flags only, no kind
    /// tag) could never have expressed even by accident, since it only ever
    /// saw the flag.
    #[test]
    fn every_entity_kind_fingerprints_as_a_distinct_entry() {
        use mandible_core::{CommandNode, Entity, EntityKind, Provenance, Source};

        let mut root = CommandNode::new("demo", Provenance::single(Source::HelpText));
        root.entities.push(Entity::flag_short(
            'x',
            Provenance::single(Source::HelpText),
        ));
        root.entities.push(Entity::positional(
            "x",
            Provenance::single(Source::HelpText),
        ));
        root.entities
            .push(Entity::modifier('x', Provenance::single(Source::HelpText)));
        root.entities.push(Entity::env_var_item(
            "x",
            Provenance::single(Source::HelpText),
        ));

        let fp = build_fingerprint(Some(&root));
        assert_eq!(
            fp.flags.len(),
            4,
            "four different EntityKinds sharing one bare spelling must not collide: {:?}",
            fp.flags.keys().collect::<Vec<_>>()
        );
        assert!(fp.flags.contains_key("(root)::Flag::-x"));
        assert!(fp.flags.contains_key("(root)::Positional::x"));
        assert!(fp.flags.contains_key("(root)::Modifier::x"));
        assert!(fp.flags.contains_key("(root)::EnvVar::x"));

        // Round-trips through the real wire format too, not just the
        // in-memory ToolFingerprint.
        let mut r = row("demo", 4, Some(0.0), "ok");
        r.fingerprint = fp;
        let rows = vec![r];
        let agg = compute_aggregate(&rows);
        let text = render_text(&rows, &agg);
        assert!(
            text.contains("#fp2 "),
            "the emitted footer must use the v2 line prefix"
        );
        let parsed = crate::transition::parse_scoreboard(&text);
        let round_tripped = parsed
            .fingerprints
            .get("demo")
            .expect("demo fingerprint present in the #fp2 footer");
        assert_eq!(round_tripped.flags.len(), 4);
        for kind in [
            EntityKind::Flag,
            EntityKind::Positional,
            EntityKind::Modifier,
            EntityKind::EnvVar,
        ] {
            let id = format!(
                "(root)::{kind:?}::{}",
                if kind == EntityKind::Flag { "-x" } else { "x" }
            );
            assert!(
                round_tripped.flags.contains_key(&id),
                "{id} must survive the round trip"
            );
        }
    }

    // `structure_sanity`'s own unit tests (fabricated names, empty nodes,
    // the root-name exclusion, `heading_attested` provenance, a clean
    // tree) now live in `status.rs`'s test module, alongside the function
    // itself — see that module's doc comment for why it moved.
}
