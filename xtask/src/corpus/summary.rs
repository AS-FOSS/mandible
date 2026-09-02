//! Summarizing a fixture's current and previously-blessed trees for the markdown transition report.
use super::*;

/// One side of a fixture's semantic comparison (spec companion work
/// order's requirement: a report must say *what changed*, never present a
/// text diff) — either the tree `expected.snap` currently pins, or the
/// tree this run just extracted. Built once via [`summarize`] so both
/// sides go through identical logic; no separate "previous" rules to drift
/// from "current" ones.
pub(crate) struct TreeSummary {
    /// [`crate::status::compute`]'s label — the *same* function
    /// `check_contract`'s `min_status` check uses, so a status shown here
    /// can never disagree with what actually gated the run (status.rs's
    /// own doc comment: "two independent definitions of 'status' will
    /// drift, and the drift will be discovered at the worst possible
    /// time").
    pub(crate) status: &'static str,
    pub(crate) nodes: usize,
    pub(crate) flags: usize,
    /// Every subcommand's dotted path (`"bisect start"`, not just
    /// `"start"`), so two same-named subcommands under different parents
    /// are never conflated.
    pub(crate) subcommands: BTreeSet<String>,
    /// Every flag's canonical spelling (`--long` if present, else `-x`).
    pub(crate) flag_names: BTreeSet<String>,
}

/// Build a [`TreeSummary`] for `root` (`None` when no tier produced
/// anything, e.g. a "current" side with no root, or an unresolvable
/// `expected.snap`). `tool` only feeds the status stub's `tool` field,
/// which `crate::status::compute` never actually reads.
pub(crate) fn summarize(tool: &str, root: Option<&CommandNode>) -> TreeSummary {
    let stub = mandible_extract::ExtractionResult {
        tool: tool.to_string(),
        root: root.cloned(),
        tier_statuses: Vec::new(),
        elapsed: Duration::ZERO,
    };
    let status = crate::status::compute(&stub);
    let mut subcommands = BTreeSet::new();
    let mut flag_names = BTreeSet::new();
    if let Some(r) = root {
        collect_subcommand_paths(r, "", &mut subcommands);
        collect_flag_names(r, &mut flag_names);
    }
    TreeSummary {
        status: status.label,
        nodes: root.map(count_nodes).unwrap_or(0),
        flags: root.map(count_flags).unwrap_or(0),
        subcommands,
        flag_names,
    }
}

fn count_nodes(node: &CommandNode) -> usize {
    1 + node.subcommands.iter().map(count_nodes).sum::<usize>()
}

fn count_flags(node: &CommandNode) -> usize {
    node.flags().count() + node.subcommands.iter().map(count_flags).sum::<usize>()
}

fn collect_flag_names(node: &CommandNode, out: &mut BTreeSet<String>) {
    for f in node.flags() {
        if let Some(long) = f.long() {
            out.insert(format!("--{long}"));
        } else if let Some(short) = f.short() {
            out.insert(format!("-{short}"));
        }
    }
    for child in &node.subcommands {
        collect_flag_names(child, out);
    }
}

fn collect_subcommand_paths(node: &CommandNode, prefix: &str, out: &mut BTreeSet<String>) {
    for child in &node.subcommands {
        let path = if prefix.is_empty() {
            child.name.clone()
        } else {
            format!("{prefix} {}", child.name)
        };
        out.insert(path.clone());
        collect_subcommand_paths(child, &path, out);
    }
}

/// Minimal, tolerant mirror of `expected.snap`'s compact YAML shape
/// (`mandible_core::snapshot::NodeSnapshot`), carrying only what
/// [`TreeSummary`] needs. `NodeSnapshot` itself derives only `Serialize`
/// (its own doc comment: the compact form is a one-way review artifact,
/// not a round-trip format — every empty `Vec`/`None` field is omitted
/// entirely), so a direct `Deserialize` of the full IR would fail on
/// "missing field" for nearly every real fixture. `#[serde(default)]` on
/// every field here is what makes an omitted key deserialize back to the
/// same "empty" value its serializer chose not to write, instead of an
/// error.
#[derive(Debug, Clone, Default, Deserialize)]
struct SnapFlag {
    /// Every rendered spelling, in document order — `expected.snap`'s
    /// `spellings` key, exactly as [`mandible_core::FlagSnapshot`] writes
    /// it. Parsed back into [`Spelling`]s by [`parse_rendered_spelling`]
    /// so [`collect_flag_names`] can read `short()`/`long()` off the
    /// reconstructed entity the same way it reads them off a fresh one.
    #[serde(default)]
    spellings: Vec<String>,
    /// Needed for nothing this module names directly, but
    /// [`crate::status::compute`]'s `pct_flags_with_text` (and therefore the
    /// `low-confidence` vs `ok` status boundary) reads it — omitting it
    /// would make every reconstructed "previous" tree look 0% described
    /// regardless of the real fixture, which would desync `TreeSummary`'s
    /// two sides even when `expected.snap` and the fresh extraction are
    /// byte-identical.
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct SnapNode {
    #[serde(default)]
    name: String,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    flags: Vec<SnapFlag>,
    #[serde(default)]
    subcommands: Vec<SnapNode>,
    #[serde(default)]
    unparsed: Vec<String>,
    #[serde(default)]
    heading_attested: bool,
    #[serde(default)]
    invocation_attested: bool,
}

/// Rebuild a real (if synthetic-provenance) [`CommandNode`] from a
/// [`SnapNode`], so [`summarize`] can run the *exact same* status/count/
/// name-collection logic over `expected.snap`'s tree that it runs over a
/// freshly extracted one — one code path for both sides of the
/// comparison, never a second one hand-written against the compact YAML
/// shape. Every field `SnapNode` doesn't carry (provenance detail,
/// descriptions, positionals, ...) is irrelevant to what [`summarize`]
/// reads and is left at `CommandNode::new`'s defaults.
fn snap_to_command_node(n: &SnapNode) -> CommandNode {
    let mut node = CommandNode::new(n.name.clone(), Provenance::single(Source::HelpText));
    node.summary = n.summary.as_deref().map(Text::sanitize);
    node.heading_attested = n.heading_attested;
    node.invocation_attested = n.invocation_attested;
    node.unparsed = n.unparsed.iter().map(|s| Text::sanitize(s)).collect();
    node.set_flags(
        n.flags
            .iter()
            .map(|f| {
                let mut flag = Entity::new(EntityKind::Flag, Provenance::single(Source::HelpText));
                flag.spellings = f
                    .spellings
                    .iter()
                    .map(|s| parse_rendered_spelling(s))
                    .collect();
                flag.description = f.description.as_deref().map(Text::sanitize);
                flag
            })
            .collect(),
    );
    node.subcommands = n.subcommands.iter().map(snap_to_command_node).collect();
    node
}

/// Parse one of [`FlagSnapshot`](mandible_core::FlagSnapshot)'s rendered
/// `spellings` entries (`"-i"`, `"--interactive"`, `"--[no-]color"`,
/// `"-help"`, `"-r[esolve]"`) back into a [`Spelling`], the inverse of
/// [`Spelling::render`]. Good enough for [`snap_to_command_node`]'s
/// purpose — reconstructing an entity whose `short()`/`long()` accessors
/// agree with what a fresh extraction would produce — not a general
/// parser: a name that itself began with a literal `-` would round-trip
/// wrong, and no real flag spelling does.
fn parse_rendered_spelling(rendered: &str) -> Spelling {
    let (dashes, rest) = if let Some(r) = rendered.strip_prefix("--") {
        (Dashes::Double, r)
    } else if let Some(r) = rendered.strip_prefix('-') {
        (Dashes::Single, r)
    } else {
        (Dashes::None, rendered)
    };
    if let Some(base) = rest.strip_prefix("[no-]") {
        return Spelling {
            name: base.to_string(),
            dashes,
            negatable: true,
            abbrev: None,
        };
    }
    // An abbreviation bracket: "r[esolve]" -> name "resolve", abbrev
    // Some(1) — the prefix's character count.
    if let Some(open) = rest.find('[') {
        if let Some(rest_after) = rest.strip_suffix(']') {
            let (prefix, bracketed) = rest_after.split_at(open);
            let bracketed = &bracketed[1..]; // drop the '['
            return Spelling {
                name: format!("{prefix}{bracketed}"),
                dashes,
                negatable: false,
                abbrev: Some(prefix.chars().count()),
            };
        }
    }
    Spelling {
        name: rest.to_string(),
        dashes,
        negatable: false,
        abbrev: None,
    }
}

/// Read and convert a fixture's `expected.snap` into a [`TreeSummary`],
/// `None` when it doesn't exist yet (legal for an unfixed `[xfail]`
/// fixture, `corpus/README.md` step 4) or fails to parse (a corrupt or
/// hand-edited file — degrade to "no baseline" rather than panicking on
/// tool input, per AGENTS.md's `unwrap()` rule).
pub(crate) fn previous_summary(fixture: &Fixture) -> Option<TreeSummary> {
    let snap_path = fixture.expected_snap_path();
    if !snap_path.is_file() {
        return None;
    }
    let raw = std::fs::read_to_string(&snap_path).ok()?;
    let snap: SnapNode = serde_yaml::from_str(&raw).ok()?;
    let converted = snap_to_command_node(&snap);
    Some(summarize(&fixture.meta.tool.name, Some(&converted)))
}

/// One fixture's row in the markdown transition report: the same
/// pass/fail classification and detail lines the text report already
/// computed, plus the two [`TreeSummary`] sides [`render_markdown_report`]
/// diffs.
pub(crate) struct FixtureRow {
    pub(crate) label: String,
    pub(crate) status_word: &'static str,
    /// `contract: ...` / `snapshot: ...` lines, reused to decide which
    /// remedy to name (§ this module's doc comment on `render_markdown_report`).
    pub(crate) detail: Vec<String>,
    pub(crate) current: TreeSummary,
    pub(crate) previous: Option<TreeSummary>,
    /// What a human actually verified before this fixture was blessed
    /// (`ContractMeta::verdict_scope`'s doc comment) — surfaced as the
    /// table's own `scope` column so a reviewer sees, without opening
    /// `meta.toml`, that a green row's descriptions may still be
    /// unreviewed prose.
    pub(crate) verdict_scope: Vec<VerdictScope>,
    /// Who blessed this fixture's `expected.snap` (`BlessMeta`'s doc
    /// comment) — the complement to `verdict_scope`: surfaced as its own
    /// table column so an `ok` row can never be misread as human-verified
    /// when no human has looked.
    pub(crate) provenance: BlessProvenance,
}
