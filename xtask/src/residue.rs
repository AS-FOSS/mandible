//! Residue ranking — the complement of [`crate::existence`], and the only
//! instrument in this workspace deliberately forbidden from ever becoming
//! a number anyone quotes.
//!
//! [`crate::existence`] asks "is everything in the tree attested by the
//! text?" (catches invention). This module asks "what in the text did the
//! tree never account for?" (catches omission — most of the seed-2
//! audit's non-K1 defect backlog).
//!
//! Per-line attribution over the raw `--help` capture. Each physical line
//! is classified by shape alone, never by tool name or heading wording
//! (spec §1): a flag row (first token dash-shaped, not a prose bullet), a
//! name row (indented, lowercase, [`is_command_name_shaped`] first token
//! followed by a 2+ space gutter and more text), or uncounted. A row is
//! accounted for when the tree carries a matching spelling/name;
//! otherwise it is residue.
//!
//! Independent of the parser's own line classification deliberately: the
//! parser's largest omission class (rule 1, `sections/mod.rs`) would
//! agree with itself that a dropped bare-word block was never an entry.
//!
//! Never a gate or quotable number: not an accuracy metric (only the 94
//! human verdicts in `audit/submissions/sadigaxund/2.toml`, spec §13.1c,
//! touch ground truth; residue has a large, unmeasured false-positive
//! rate); not a pass/fail check (nothing reads it —
//! [`residue_is_reachable_from_no_gate`] fails the build if that changes).
//! A wrong residue candidate costs review time and cannot produce a wrong
//! parse; treating it as a measurement would be a metric-design incident
//! in advance rather than in hindsight (spec §13.1b, §13.1f).
//!
//! No new probes: reads bytes already captured, replaying
//! `corpus/`-shaped fixtures with zero subprocesses, exactly as `xtask
//! corpus` does. No `PATH` scan, ever.

use crate::corpus::{discover_fixtures, extract_tree, Fixture};
use mandible_core::{is_command_name_shaped, CommandNode};
use mandible_extract::{default_tiers_with_probe, Runner};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::Arc;

/// Hard cap on physical lines examined from one capture. The coverage
/// sweep found a tool (`instmodsh`, a Perl REPL that ignores `--help` and
/// free-runs its own banner) that produced 8 MiB of output; a discovery
/// tool that spends minutes on it is a discovery tool nobody runs. Past
/// this point a document's shape is established many times over.
const MAX_LINES: usize = 20_000;

/// A block must leave at least this many rows unaccounted before its rows
/// count toward the ranking key. One stray unmatched row in a table is the
/// overwhelmingly common shape of *noise* — a hand-formatted note between
/// entries, a heading the classifier mistook for a row, a glossary line —
/// whereas the omission class this exists to find drops rows in bulk,
/// because it drops whole blocks. This trades recall for precision at the
/// top of the list on purpose: the output is a human's reading queue, and
/// a queue whose first page is noise does not get read a second time.
const MIN_BLOCK_RESIDUE: usize = 2;

// ---------------------------------------------------------------------
// Row shapes
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RowShape {
    /// A dash-led option row (`  -v, --verbose   be verbose`).
    Flag,
    /// An indented `name<gutter>description` row — the physical shape of a
    /// command-list entry.
    Name,
}

impl RowShape {
    fn label(self) -> &'static str {
        match self {
            RowShape::Flag => "flag-row",
            RowShape::Name => "name-row",
        }
    }
}

/// One classified, structurally-interesting row of the raw text.
#[derive(Debug, Clone)]
struct Row {
    /// 1-indexed physical line number in the raw capture.
    line: usize,
    /// Display width of the leading whitespace, tabs expanded to 8.
    indent: usize,
    shape: RowShape,
    /// Every candidate spelling/name this row could be attested by. A flag
    /// row commonly carries several (`-v, --verbose`) and is accounted for
    /// if *any* of them is in the tree — a tool that documents a short and
    /// long spelling on one line and whose parser recovered only one of
    /// them has a real but different defect (alias pairing), not an
    /// omission of the row.
    keys: Vec<String>,
    /// The raw line, trimmed and length-capped, for the evidence listing.
    text: String,
}

/// Leading-whitespace width, tabs expanded to the conventional 8. GCC's
/// option tables are tab-separated, so treating a tab as one column would
/// put its rows in a different "column" from space-indented ones in the
/// same block.
fn indent_width(line: &str) -> usize {
    let mut width = 0usize;
    for c in line.chars() {
        match c {
            ' ' => width += 1,
            '\t' => width = width.next_multiple_of(8),
            _ => break,
        }
    }
    width
}

/// Split a trimmed line at its first *gutter* — two-or-more spaces, or a
/// tab — into `(first cell, remainder)`. A single space never splits: a
/// help-text column gutter is always at least two spaces wide, and
/// treating one space as a gutter would make every prose sentence a
/// two-column row.
fn split_cell(trimmed: &str) -> (&str, &str) {
    let chars: Vec<char> = trimmed.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\t' {
            break;
        }
        if chars[i] == ' ' && i + 1 < chars.len() && chars[i + 1] == ' ' {
            break;
        }
        i += 1;
    }
    if i == chars.len() {
        return (trimmed, "");
    }
    // Recover the two borrowed halves from the *char* prefix's own encoded
    // length, never from a raw byte index computed against the input
    // (AGENTS.md's rule: this is arbitrary tool output and routinely
    // non-ASCII). `get` returns `None` rather than panicking if that ever
    // stops holding.
    let cell_bytes: usize = chars[..i].iter().map(|c| c.len_utf8()).sum();
    match (trimmed.get(..cell_bytes), trimmed.get(cell_bytes..)) {
        (Some(a), Some(b)) => (a, b),
        _ => (trimmed, ""),
    }
}

/// True when a trimmed line opens with something dash-shaped enough to be
/// an option row rather than a prose bullet (`- like this`), an option
/// terminator (`--`), or an em-dash-led aside. `--[no-]foo` counts: it is
/// getopt_long's own inline-negation spelling, not a bracket group.
fn opens_like_a_flag(trimmed: &str) -> bool {
    let mut chars = trimmed.chars();
    if chars.next() != Some('-') {
        return false;
    }
    match chars.next() {
        Some('-') => matches!(chars.next(), Some(c) if c.is_alphanumeric() || c == '['),
        Some(c) => c.is_alphanumeric(),
        None => false,
    }
}

/// Strip a value spec glued onto a flag token (`--gpg-sign[=<keyid>]`,
/// `--param=lazy-modules=`, `--format={json|yaml}`) down to the bare
/// spelling the IR stores, and unwrap getopt_long's inline negation
/// bracket (`--[no-]source` -> `--source`). Returns every form worth
/// testing against the tree's vocabulary, most specific first — the
/// unstripped token matters for the GCC single-dash convention
/// (`-fdump-scos` is one whole spelling, not `-f` plus a value).
fn flag_token_candidates(token: &str) -> Vec<String> {
    let token = token.trim_matches(|c: char| matches!(c, '.' | ',' | ';' | ':' | ')' | '"' | '\''));
    if token.len() < 2 || !token.starts_with('-') {
        return Vec::new();
    }
    let mut out = vec![token.to_string()];
    let cut = token
        .char_indices()
        .find(|(i, c)| *i > 0 && matches!(c, '=' | '[' | '<' | '{' | '('))
        .map(|(i, _)| i);
    if let Some(i) = cut {
        if let Some(base) = token.get(..i) {
            if !base.trim_start_matches('-').is_empty() {
                out.push(base.to_string());
            }
        }
    }
    if let Some(rest) = token
        .strip_prefix("--[no-]")
        .or_else(|| token.strip_prefix("--[no]"))
    {
        out.push(format!("--{rest}"));
    }
    out
}

/// Classify one physical line, or `None` when it carries no structure.
///
/// `root_name` suppresses a tool's own `Examples:` block, where every line
/// begins with the tool's own name and looks exactly like a command-list
/// entry — `tar`'s examples are the type specimen, and the parser has its
/// own (heading-keyed) guard for the same shape.
fn classify(line: &str, number: usize, root_name: &str, in_leading_run: bool) -> Option<Row> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    let indent = indent_width(line);
    let (cell, rest) = split_cell(trimmed);

    // --- flag row --------------------------------------------------------
    if trimmed.starts_with('-') {
        if !opens_like_a_flag(trimmed) {
            return None;
        }
        // Whitespace tokens first, *then* their comma/pipe-separated
        // pieces — both, never only the pieces. `-v, --verbose` needs the
        // split (the comma is a separator); gcc's `-Wa,<options>` needs
        // the whole token (the comma is part of one spelling the grammar
        // stores as `short='W'` + `value_name="a,<options>"`). Splitting
        // unconditionally reported all three of gcc's `-W{a,p,l},` options
        // as residue when every one of them is in the tree — the same
        // "compare against the pre-normalization spelling" trap
        // `existence.rs` documents at length, met from the other side.
        let keys: Vec<String> = cell
            .split_whitespace()
            .flat_map(|tok| {
                std::iter::once(tok).chain(tok.split([',', '|', '/']).filter(|p| !p.is_empty()))
            })
            .flat_map(flag_token_candidates)
            .collect();
        if keys.is_empty() {
            return None;
        }
        return Some(Row {
            line: number,
            indent,
            shape: RowShape::Flag,
            keys,
            text: capped(trimmed),
        });
    }

    // --- name row --------------------------------------------------------
    // Indented, lowercase identifier, a real gutter, and description text
    // after it. The lowercase requirement is doing heavy lifting for free:
    // environment-variable tables (`GIT_DIR   ...`), exit-status tables
    // (`0   success`) and path tables (`~/.gitconfig   ...`) all fail it,
    // and none of them should ever become tree structure.
    if indent == 0 || rest.trim().is_empty() {
        return None;
    }
    let name = cell.trim_end_matches([':', ',', ';']);
    if !is_command_name_shaped(name) || name == root_name {
        return None;
    }
    // A usage alternative (`  or:  du [OPTION]... --files0-from=F`) has the
    // exact two-column shape of a command-list entry, second column
    // opening with the tool's own name. Compared on the file-name
    // component (absolute-path invocations print `/usr/bin/chgrp ...`).
    //
    // Scoped to the leading run (before the document's first blank line):
    // "description opens with the tool's own name" is also how a real
    // command table reads (`clone    git clone a repository`), and an
    // unscoped predicate would swallow it.
    if in_leading_run {
        let opens_with_tool = rest
            .split_whitespace()
            .next()
            .and_then(|w| w.rsplit('/').next())
            == Some(root_name);
        if opens_with_tool {
            return None;
        }
    }
    Some(Row {
        line: number,
        indent,
        shape: RowShape::Name,
        keys: vec![name.to_string()],
        text: capped(trimmed),
    })
}

fn capped(s: &str) -> String {
    let mut out: String = s.chars().take(100).collect();
    if s.chars().count() > 100 {
        out.push('…');
    }
    out
}

// ---------------------------------------------------------------------
// The tree's vocabulary
// ---------------------------------------------------------------------

/// Every spelling and name the parsed tree could attest a row with.
///
/// Built the same way [`crate::existence`] builds its candidates, in the
/// opposite direction: that module asks whether a stored spelling occurs
/// in the text, this one whether a text token occurs in the store, so
/// both need the pre-normalization forms. The GCC-family split
/// (`-fdump-scos` stored as `short='f'` + `value_name="dump-scos"`) is
/// reconstructed here too, or `lto-dump`'s real options would read as
/// residue.
#[derive(Default)]
struct Vocabulary {
    flags: HashSet<String>,
    names: HashSet<String>,
}

impl Vocabulary {
    /// Absorb every entity a node carries, keyed on the **shape of what it
    /// carries** — a dashed spelling or a bare one — never on
    /// [`mandible_core::EntityKind`]. This is what lets a kind added later
    /// (an `EnvVar` producer lands on a sibling branch) reach the
    /// vocabulary with no further edit here: it is covered the moment it
    /// exists, because it already has one of the two shapes below.
    fn absorb(&mut self, node: &CommandNode) {
        self.names.insert(node.name.clone());
        for alias in &node.aliases {
            self.names.insert(alias.clone());
        }
        for entity in &node.entities {
            if entity.short().is_some() || entity.long().is_some() {
                // A dashed spelling: contributes flag keys, including the
                // GCC-family reconstruction and `--[no-]long` negation.
                if let Some(short) = entity.short() {
                    self.flags.insert(format!("-{short}"));
                    if let Some(v) = &entity.value_name {
                        self.flags.insert(format!("-{short}{v}"));
                        self.flags.insert(format!("-{short}={v}"));
                    }
                }
                if let Some(long) = entity.long() {
                    self.flags.insert(format!("--{long}"));
                    if entity.negatable() {
                        self.flags.insert(format!("--[no-]{long}"));
                        self.flags.insert(format!("--[no]{long}"));
                    }
                    if let Some(v) = &entity.value_name {
                        self.flags.insert(format!("--{long}={v}"));
                    }
                }
            } else {
                // A bare, dashless spelling (positional, modifier letter,
                // env-var name): contributes its primary name, lowercased
                // to match `is_command_name_shaped`'s all-lowercase
                // requirement (`classify` never emits a name row for an
                // uppercase table, so an env var's ALL_CAPS name is
                // unaffected).
                let name = entity.primary_name();
                if !name.is_empty() {
                    self.names.insert(name.to_lowercase());
                }
            }
            // Choices are attested the same way for every kind: a
            // bare-word block the parser attached as an entity's
            // enumerated choices (`sections/mod.rs` rule 4) was
            // *consumed* — counting those rows as residue would report
            // the rule working as if it had failed.
            for choice in &entity.choices {
                self.names.insert(choice.name.clone());
            }
        }
        for child in &node.subcommands {
            self.absorb(child);
        }
    }

    fn attests(&self, row: &Row) -> bool {
        match row.shape {
            RowShape::Flag => row.keys.iter().any(|k| self.flags.contains(k)),
            RowShape::Name => row
                .keys
                .iter()
                .any(|k| self.names.contains(k) || self.flags.contains(k)),
        }
    }
}

// ---------------------------------------------------------------------
// Report types
// ---------------------------------------------------------------------

/// One contiguous run of non-blank lines that produced residue.
pub struct ResidueBlock {
    /// The nearest column-0 line above this block, capped — the document's
    /// own heading for it, shown so a reviewer can tell an options table
    /// from a glossary at a glance without opening the capture.
    pub heading: Option<String>,
    pub shape: RowShape,
    /// Rows of this shape the block contains.
    pub rows: usize,
    /// `(line number, text)` for each row nothing in the tree accounts for.
    pub unaccounted: Vec<(usize, String)>,
}

/// One tool's residue analysis.
pub struct ResidueReport {
    /// Structurally-interesting rows found in the raw text.
    pub rows: usize,
    /// Rows nothing in the tree accounts for, across all blocks.
    pub unaccounted: usize,
    /// Unaccounted rows sitting in a block that lost at least
    /// [`MIN_BLOCK_RESIDUE`] rows — the ranking key. See that constant.
    pub signal: usize,
    /// True when the tree is the verbatim fallback
    /// (`CommandNode::unparsed` non-empty): no structure was extracted at
    /// all, so *every* row is trivially residue. This is a status the
    /// coverage scoreboard already counts (`verbatim_count`), not a
    /// discovery, and it would otherwise occupy the whole top of the list.
    pub verbatim: bool,
    /// Blocks with residue, most-residue first.
    pub blocks: Vec<ResidueBlock>,
}

/// Analyze one captured document against the tree that was parsed from it.
pub fn analyze(raw: &str, root: &CommandNode) -> ResidueReport {
    let mut vocab = Vocabulary::default();
    vocab.absorb(root);

    let lines: Vec<&str> = raw.lines().take(MAX_LINES).collect();
    let mut blocks: Vec<ResidueBlock> = Vec::new();
    let mut rows_total = 0usize;
    let mut unaccounted_total = 0usize;
    let mut signal_total = 0usize;

    let mut i = 0usize;
    let mut last_heading: Option<String> = None;
    let mut is_leading_run = true;
    while i < lines.len() {
        if lines[i].trim().is_empty() {
            i += 1;
            continue;
        }
        let start = i;
        while i < lines.len() && !lines[i].trim().is_empty() {
            i += 1;
        }
        let run = &lines[start..i];
        // Only the document's *first* run of non-blank lines is the usage
        // block — see `classify`'s `in_leading_run` on why one predicate
        // there is scoped to it.
        let leading = std::mem::take(&mut is_leading_run);

        let mut rows: Vec<Row> = run
            .iter()
            .enumerate()
            .filter_map(|(k, l)| classify(l, start + k + 1, &root.name, leading))
            .collect();

        // Rule 2 of `sections/mod.rs`, rediscovered independently and applied
        // only where it is actually needed: a name row deeper than its
        // block's name column is a wrapped description continuation, not a
        // new entry. Flag rows need no such guard — a wrapped description
        // essentially never begins with a dash glued to a word — and
        // applying one to them would silently drop the nested option
        // tables that some tools really do indent.
        if let Some(name_col) = rows
            .iter()
            .filter(|r| r.shape == RowShape::Name)
            .map(|r| r.indent)
            .min()
        {
            rows.retain(|r| r.shape != RowShape::Name || r.indent == name_col);
        }

        // The block's own heading, for the evidence listing only: the last
        // line of this run that sits *above* the first row and is less
        // indented than it. "Less indented than the rows" rather than
        // "at column 0" because a sub-block (tar's `--format=FORMAT`
        // choices, indented under the flag that owns them) has a real
        // heading of its own that is not at column zero, and labelling it
        // with the last column-0 line seen — several screens earlier — is
        // actively misleading to the person reading the evidence.
        let row_indent = rows.iter().map(|r| r.indent).min().unwrap_or(0);
        let rows_start_at = rows.iter().map(|r| r.line).min().unwrap_or(usize::MAX);
        let mut inline_heading = None;
        for (k, line) in run.iter().enumerate() {
            let number = start + k + 1;
            if number >= rows_start_at {
                break;
            }
            if !line.trim().is_empty() && indent_width(line) < row_indent.max(1) {
                inline_heading = Some(capped(line.trim()));
            }
        }
        let heading = inline_heading.clone().or_else(|| last_heading.clone());
        if let Some(h) = inline_heading {
            last_heading = Some(h);
        }

        for shape in [RowShape::Flag, RowShape::Name] {
            let of_shape: Vec<&Row> = rows.iter().filter(|r| r.shape == shape).collect();
            if of_shape.is_empty() {
                continue;
            }
            rows_total += of_shape.len();
            let missing: Vec<(usize, String)> = of_shape
                .iter()
                .filter(|r| !vocab.attests(r))
                .map(|r| (r.line, r.text.clone()))
                .collect();
            if missing.is_empty() {
                continue;
            }
            unaccounted_total += missing.len();
            if missing.len() >= MIN_BLOCK_RESIDUE {
                signal_total += missing.len();
            }
            blocks.push(ResidueBlock {
                heading: heading.clone(),
                shape,
                rows: of_shape.len(),
                unaccounted: missing,
            });
        }
    }

    blocks.sort_by(|a, b| {
        b.unaccounted
            .len()
            .cmp(&a.unaccounted.len())
            .then_with(|| a.shape.cmp(&b.shape))
    });

    ResidueReport {
        rows: rows_total,
        unaccounted: unaccounted_total,
        signal: signal_total,
        verbatim: !root.unparsed.is_empty(),
        blocks,
    }
}

// ---------------------------------------------------------------------
// The command
// ---------------------------------------------------------------------

/// One ranked entry: a fixture, its report, and the human verdict already
/// recorded for it, if any.
struct Ranked {
    tool: String,
    label: String,
    report: ResidueReport,
    verdict: Option<String>,
}

/// Load `<dir>/<seed>.toml`'s verdicts, keyed by tool name. Absent file is
/// not an error: the ranking stands on its own, the verdicts only add the
/// validation view.
fn load_verdicts(dir: &Path, seed: u64) -> anyhow::Result<BTreeMap<String, String>> {
    let path = mandible_core::audit::verdict_path(dir, seed);
    if !path.is_file() {
        return Ok(BTreeMap::new());
    }
    let file = mandible_core::audit::load(&path)?;
    Ok(file
        .entries
        .into_iter()
        .filter_map(|e| e.verdict.map(|v| (e.tool, v)))
        .collect())
}

fn analyze_fixture(fixture: &Fixture) -> anyhow::Result<Option<ResidueReport>> {
    let Some(raw) = fixture.root_help_text()? else {
        return Ok(None);
    };
    let transcript = fixture.build_transcript()?;
    let runner = Runner::new(default_tiers_with_probe(Arc::new(transcript)));
    let Some(root) = extract_tree(&runner, &fixture.resolved_tool()) else {
        return Ok(None);
    };
    Ok(Some(analyze(&raw, &root)))
}

/// `cargo run -p xtask -- residue`. Ranks every fixture under `dir` by how
/// much structurally-interesting text its parse left behind, prints the
/// top `top` with their evidence, and — when a verdict file is present —
/// prints how the ranking lines up against the human verdicts that already
/// exist.
///
/// Always exits `0`. There is deliberately no `--check`, no threshold and
/// no failure path: see this module's doc comment, and
/// [`residue_is_reachable_from_no_gate`].
pub fn run(
    dir: &Path,
    top: usize,
    detail: usize,
    include_verbatim: bool,
    audit_dir: &Path,
    seed: u64,
) -> anyhow::Result<()> {
    let verdicts = load_verdicts(audit_dir, seed)?;
    let fixtures = discover_fixtures(dir)?;
    if fixtures.is_empty() {
        anyhow::bail!(
            "no fixtures found under {} (expected <tool>/<version>/meta.toml)",
            dir.display()
        );
    }

    let mut ranked: Vec<Ranked> = Vec::new();
    let mut skipped_no_root = 0usize;
    let mut skipped_verbatim = 0usize;
    for fixture in &fixtures {
        let tool = fixture.tool_name().to_string();
        match analyze_fixture(fixture)? {
            None => skipped_no_root += 1,
            Some(report) => {
                if report.verbatim && !include_verbatim {
                    skipped_verbatim += 1;
                    continue;
                }
                let verdict = verdicts.get(&tool).cloned();
                ranked.push(Ranked {
                    tool,
                    label: fixture.label().to_string(),
                    report,
                    verdict,
                });
            }
        }
    }

    ranked.sort_by(|a, b| {
        b.report
            .signal
            .cmp(&a.report.signal)
            .then_with(|| b.report.unaccounted.cmp(&a.report.unaccounted))
            .then_with(|| a.label.cmp(&b.label))
    });

    println!(
        "residue ranking over {} fixture(s) in {} — a review queue, not a measurement.",
        fixtures.len(),
        dir.display()
    );
    println!(
        "{skipped_no_root} produced no root; {skipped_verbatim} were verbatim (no structure \
         extracted at all — already counted by the scoreboard, pass --include-verbatim to rank them)."
    );
    println!(
        "\nNothing below is a score. See xtask/src/residue.rs's doc comment and spec §13.1f.\n"
    );

    println!(
        "{:>4}  {:<34} {:>6} {:>7} {:>7}  verdict",
        "#", "tool", "rows", "unacct", "signal"
    );
    for (i, r) in ranked.iter().take(top).enumerate() {
        println!(
            "{:>4}  {:<34} {:>6} {:>7} {:>7}  {}",
            i + 1,
            r.label,
            r.report.rows,
            r.report.unaccounted,
            r.report.signal,
            r.verdict.as_deref().unwrap_or("-"),
        );
    }

    for r in ranked.iter().take(detail) {
        println!(
            "\n--- {} ({}) ---",
            r.label,
            r.verdict.as_deref().unwrap_or("unlabelled")
        );
        for block in r.report.blocks.iter().take(4) {
            println!(
                "  [{}] {} of {} rows unaccounted under heading: {}",
                block.shape.label(),
                block.unaccounted.len(),
                block.rows,
                block.heading.as_deref().unwrap_or("(none)")
            );
            for (line, text) in block.unaccounted.iter().take(6) {
                println!("      L{line}: {text}");
            }
            if block.unaccounted.len() > 6 {
                println!("      ... {} more", block.unaccounted.len() - 6);
            }
        }
    }

    if !verdicts.is_empty() {
        print_validation(&ranked, top);
    }
    Ok(())
}

/// How the ranking lines up against the human verdicts that already exist
/// — the only honest way to find out whether this signal separates
/// anything. Printed here, in this command's own output, and nowhere else:
/// it is a statement about *the instrument*, not about the parser, and it
/// must never appear next to the audited accuracy figure (spec §13.1c),
/// which is a statement about the parser.
fn print_validation(ranked: &[Ranked], _top: usize) {
    let of_verdict = |want: &[&str]| -> Vec<&Ranked> {
        ranked
            .iter()
            .filter(|r| r.verdict.as_deref().is_some_and(|v| want.contains(&v)))
            .collect()
    };
    let defective = of_verdict(&["wrong", "incomplete"]);
    let correct = of_verdict(&["correct"]);
    if defective.is_empty() || correct.is_empty() {
        return;
    }

    // Mann-Whitney over the ranking *key*, not over list position. Most
    // documents leave nothing behind, so most of this list is one enormous
    // tie broken alphabetically; scoring by position would read that
    // arbitrary alphabetical order as if it were evidence and quietly
    // inflate (or deflate) the separation. Ties score 0.5, which is what
    // they are worth: the instrument declined to order them.
    let key = |r: &Ranked| (r.report.signal, r.report.unaccounted);
    let mut wins = 0.0f64;
    for d in &defective {
        for c in &correct {
            match key(d).cmp(&key(c)) {
                std::cmp::Ordering::Greater => wins += 1.0,
                std::cmp::Ordering::Equal => wins += 0.5,
                std::cmp::Ordering::Less => {}
            }
        }
    }
    let pairs = (defective.len() * correct.len()) as f64;

    println!("\n--- validation against the recorded human verdicts ---");
    println!(
        "  wrong/incomplete: n={}, of which {} leave residue",
        defective.len(),
        defective.iter().filter(|r| r.report.signal > 0).count()
    );
    println!(
        "  correct:          n={}, of which {} leave residue",
        correct.len(),
        correct.iter().filter(|r| r.report.signal > 0).count()
    );
    println!(
        "  pairwise separation over the ranking key, ties counted as ties: {:.3} \
         (0.500 = no information)",
        wins / pairs
    );
    println!(
        "  NOTE: most documents leave nothing behind, so this is a low-recall, \
         high-precision signal by construction — read the head of the list, never the number."
    );
    let flagged: Vec<&Ranked> = ranked.iter().filter(|r| r.report.signal > 0).collect();
    println!(
        "\n  everything the instrument actually flags ({} tool(s)):",
        flagged.len()
    );
    for r in &flagged {
        println!(
            "      {:<32} {:>3} of {:>4} rows unaccounted   verdict: {}",
            r.tool,
            r.report.signal,
            r.report.rows,
            r.verdict.as_deref().unwrap_or("UNLABELLED"),
        );
    }
    println!(
        "  Anything above whose verdict is `correct`, `skip` or `UNLABELLED` is a \
         candidate nobody has looked at — or a false alarm. A human decides which; \
         nothing here decides anything."
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::{Entity, EntityKind, Provenance, Source, Spelling};

    fn node(name: &str) -> CommandNode {
        CommandNode::new(name, Provenance::single(Source::HelpText))
    }

    fn flag(short: Option<char>, long: Option<&str>) -> Entity {
        Entity::flag_spelled(
            short,
            long.map(str::to_string),
            false,
            false,
            Provenance::single(Source::HelpText),
        )
    }

    // --- the guard ------------------------------------------------------

    /// **This module must be reachable from no gate.** The hard constraint
    /// this whole instrument was built under (see the module doc comment
    /// and spec §13.1f) is that no pass/fail path may ever consult a
    /// residue count. Prose cannot enforce that; this can. `coverage.rs`
    /// owns `--check`, `corpus.rs` owns the fixture ratchet, and
    /// `status.rs` owns the per-tool label those two read — if any of them
    /// ever *calls into* this module, the build stops here rather than in
    /// a review nobody scheduled.
    ///
    /// The forbidden token is the path form `residue::`, not the bare
    /// word: `corpus.rs` legitimately names this module in a doc link
    /// (`Fixture::root_help_text` exists for it, and a reader deleting
    /// that method deserves to know who its caller is). Data flowing
    /// *from* a gate module *into* this one is the intended direction and
    /// carries no risk; a gate reading a residue count is the thing being
    /// prevented, and that cannot be written without the path.
    #[test]
    fn residue_is_reachable_from_no_gate() {
        for (name, src) in [
            ("coverage.rs", include_str!("coverage.rs")),
            ("corpus/mod.rs", include_str!("corpus/mod.rs")),
            ("corpus/contract.rs", include_str!("corpus/contract.rs")),
            ("corpus/markdown.rs", include_str!("corpus/markdown.rs")),
            ("corpus/report.rs", include_str!("corpus/report.rs")),
            ("corpus/runner.rs", include_str!("corpus/runner.rs")),
            ("corpus/summary.rs", include_str!("corpus/summary.rs")),
            ("status.rs", include_str!("status.rs")),
        ] {
            assert!(
                !src.contains("residue::"),
                "{name} calls into `residue`: a discovery instrument must never be \
                 reachable from a gate (spec §13.1f)"
            );
        }
    }

    // --- the distinction the whole thing rests on ------------------------

    /// A tool whose help is one prose paragraph has a great deal of
    /// unconsumed *text* and must produce no residue at all — otherwise
    /// the ranking is a document-length ranking wearing a disguise.
    #[test]
    fn prose_only_help_produces_no_residue() {
        let raw = "gzip compresses files using Lempel-Ziv coding. Whenever possible,\n\
                   each file is replaced by one with the extension .gz, while keeping\n\
                   the same ownership modes, access and modification times.\n\n\
                   If no files are specified, or if a file name is \"-\", the standard\n\
                   input is compressed to the standard output.\n";
        let root = node("gzip");
        let report = analyze(raw, &root);
        assert_eq!(report.rows, 0, "prose is not structure");
        assert_eq!(report.signal, 0);
    }

    /// The counterpart: a flag table the tree never read is residue, one
    /// row at a time, with line numbers.
    #[test]
    fn a_dropped_flag_table_is_residue() {
        let raw =
            "Options:\n  -a, --alpha    first\n  -b, --beta     second\n  -c, --gamma    third\n";
        let root = node("t");
        let report = analyze(raw, &root);
        assert_eq!(report.rows, 3);
        assert_eq!(report.unaccounted, 3);
        assert_eq!(report.signal, 3);
        assert_eq!(report.blocks[0].shape, RowShape::Flag);
        assert_eq!(report.blocks[0].heading.as_deref(), Some("Options:"));
        assert_eq!(report.blocks[0].unaccounted[0].0, 2);
    }

    #[test]
    fn a_fully_read_flag_table_is_not_residue() {
        let raw = "Options:\n  -a, --alpha    first\n  -b, --beta     second\n";
        let mut root = node("t");
        root.entities.push(flag(Some('a'), Some("alpha")));
        root.entities.push(flag(Some('b'), Some("beta")));
        let report = analyze(raw, &root);
        assert_eq!(report.rows, 2);
        assert_eq!(report.unaccounted, 0);
    }

    #[test]
    fn one_missing_row_in_a_read_table_does_not_reach_the_ranking_key() {
        let raw =
            "Options:\n  -a, --alpha    first\n  -b, --beta     second\n  -c, --gamma    third\n";
        let mut root = node("t");
        root.entities.push(flag(Some('a'), Some("alpha")));
        root.entities.push(flag(Some('b'), Some("beta")));
        let report = analyze(raw, &root);
        assert_eq!(report.unaccounted, 1, "the missing row is still reported");
        assert_eq!(report.signal, 0, "but one stray row is noise, not signal");
    }

    // --- shapes that must not be mistaken for structure ------------------

    #[test]
    fn an_environment_variable_table_is_not_a_name_row() {
        // Uppercase first tokens fail `is_command_name_shaped`, so an env
        // table never enters the count — nothing in a tree should hold it.
        let raw =
            "Environment:\n  GIT_DIR        the repository location\n  GIT_AUTHOR     the author\n";
        let root = node("git");
        assert_eq!(analyze(raw, &root).rows, 0);
    }

    #[test]
    fn an_exit_status_table_is_not_a_name_row() {
        let raw = "Exit status:\n  0    success\n  1    failure\n  2    usage error\n";
        let root = node("t");
        assert_eq!(analyze(raw, &root).rows, 0);
    }

    #[test]
    fn a_prose_bullet_is_not_a_flag_row() {
        let raw = "Notes:\n  - the first note\n  - the second note\n";
        let root = node("t");
        assert_eq!(analyze(raw, &root).rows, 0);
    }

    #[test]
    fn a_wrapped_description_continuation_is_not_a_second_name_row() {
        let raw = "These are common commands:\n   clone      Clone a repository into a new\n              directory  and set it up\n   init       Create an empty repository\n";
        let mut root = node("git");
        root.subcommands.push(node("clone"));
        root.subcommands.push(node("init"));
        let report = analyze(raw, &root);
        assert_eq!(
            report.unaccounted,
            0,
            "the continuation line sits below the name column and is not a row: {:?}",
            report
                .blocks
                .iter()
                .flat_map(|b| b.unaccounted.iter().map(|(_, t)| t.clone()))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_tools_own_examples_block_is_not_a_command_list() {
        let raw = "Examples:\n  tar -cf archive.tar foo    create an archive\n  tar -xf archive.tar        extract it\n";
        let root = node("tar");
        assert_eq!(analyze(raw, &root).rows, 0);
    }

    // --- pre-normalization spellings (the existence.rs lesson, mirrored) --

    /// GCC's single-dash multi-character convention, byte-exact from
    /// `existence.rs`'s own capture of the same regression. Comparing only
    /// the bare `-f` in the *other* direction would make every real
    /// `-f...` option in `lto-dump`'s table read as residue.
    #[test]
    fn a_gcc_single_dash_flag_is_attested_by_its_reconstructed_spelling() {
        let raw = "Options:\n  -fdump-scos                 \t\t[available in Ada]\n  -fdump-tree                 \t\t[available in C]\n";
        let mut root = node("lto-dump");
        for v in ["dump-scos", "dump-tree"] {
            let mut f = flag(Some('f'), None);
            f.value_name = Some(v.to_string());
            root.entities.push(f);
        }
        assert_eq!(analyze(raw, &root).unaccounted, 0);
    }

    #[test]
    fn a_glued_value_spec_is_attested_by_the_stripped_stored_spelling() {
        let raw = "Options:\n  -S, --gpg-sign[=<keyid>]   GPG-sign commits\n  --param=lazy-modules=      tune it\n";
        let mut root = node("t");
        let mut a = flag(Some('S'), Some("gpg-sign"));
        a.value_name = Some("<keyid>".to_string());
        root.entities.push(a);
        let mut b = flag(None, Some("param"));
        b.value_name = Some("lazy-modules=".to_string());
        root.entities.push(b);
        assert_eq!(analyze(raw, &root).unaccounted, 0);
    }

    #[test]
    fn a_negatable_flags_bracketed_raw_form_is_attested() {
        let raw = "Options:\n  -s, --[no-]source <tree-ish>   use it\n  -q, --[no-]quiet               hush\n";
        let mut root = node("t");
        for (s, l) in [('s', "source"), ('q', "quiet")] {
            let mut f = flag(Some(s), Some(l));
            for spelling in &mut f.spellings {
                spelling.negatable = true;
            }
            root.entities.push(f);
        }
        assert_eq!(analyze(raw, &root).unaccounted, 0);
    }

    /// `sections/mod.rs` rule 4 routes an unrecognized bare-word block under a
    /// flag into that flag's `choices`. Those rows were consumed; counting
    /// them as residue would report a working rule as a failure.
    #[test]
    fn a_block_consumed_as_a_flags_choices_is_not_residue() {
        let raw = "Valid arguments for the --quoting-style option are:\n  literal      as-is\n  shell        shell-quoted\n  c            C-quoted\n";
        let mut root = node("tar");
        let mut f = flag(None, Some("quoting-style"));
        f.choices = ["literal", "shell", "c"]
            .iter()
            .map(|c| mandible_core::Choice::bare(*c))
            .collect();
        root.entities.push(f);
        assert_eq!(analyze(raw, &root).unaccounted, 0);
    }

    // --- the target shape, against real committed bytes -------------------

    /// `tar`'s own real capture: every flag it documents is recovered by
    /// the current grammar, so a hand-built tree carrying tar's real flags
    /// must leave its flag rows accounted for. This is the "does the
    /// classifier understand a real document" check — a classifier that
    /// mis-shapes real rows would rank every tool equally.
    #[test]
    fn tars_real_capture_classifies_a_large_flag_table() {
        let raw = include_str!("../../corpus/tar/1.35/help.txt");
        let root = node("tar");
        let report = analyze(raw, &root);
        assert!(
            report.rows > 100,
            "tar documents 171 flags; the classifier found {} rows",
            report.rows
        );
        assert_eq!(
            report.unaccounted, report.rows,
            "an empty tree accounts for nothing"
        );
    }

    /// gcc's real `-Wa,<options>` line, byte-exact from
    /// `corpus/gcc/13.3.0/help.txt`. The grammar stores it as
    /// `short='W'` + `value_name="a,<options>"`; a row tokenizer that
    /// split on the comma unconditionally would check for `-Wa` alone and
    /// report all three of these real, correctly-parsed options as
    /// residue. Measured against the committed fixture, not hypothesised.
    #[test]
    fn a_comma_bearing_flag_spelling_is_not_split_into_residue() {
        let raw = "Options:\n  -Wa,<options>            Pass comma-separated <options> on to the assembler.\n  -Wl,<options>            Pass comma-separated <options> on to the linker.\n";
        let mut root = node("gcc");
        for v in ["a,<options>", "l,<options>"] {
            let mut f = flag(Some('W'), None);
            f.value_name = Some(v.to_string());
            root.entities.push(f);
        }
        assert_eq!(analyze(raw, &root).unaccounted, 0);
    }

    #[test]
    fn a_comma_separated_alias_pair_still_splits() {
        let raw = "Options:\n  -v, --verbose   be loud\n  -q, --quiet     be quiet\n";
        let mut root = node("t");
        root.entities.push(flag(Some('v'), Some("verbose")));
        root.entities.push(flag(Some('q'), Some("quiet")));
        assert_eq!(analyze(raw, &root).unaccounted, 0);
    }

    /// `du --help`'s real line 2. A usage alternative has the exact
    /// two-column shape of a command-list entry; its second column opens
    /// with the tool's own name, which is what tells them apart.
    #[test]
    fn a_usage_alternative_line_is_not_a_command_row() {
        let raw = "Usage: du [OPTION]... [FILE]...\n  or:  du [OPTION]... --files0-from=F\n";
        let root = node("du");
        assert_eq!(analyze(raw, &root).rows, 0);
    }

    /// The shape the usage-alternative predicate must **not** swallow: a
    /// genuine command table whose descriptions open with the tool's own
    /// name. Both rows here are real command-list entries, and a residue
    /// instrument that suppressed them would be blind to precisely the
    /// omission class it exists to find.
    ///
    /// This is why that predicate is scoped to the document's leading run
    /// rather than applied everywhere. The scoping was chosen from a
    /// measurement, not a guess: across all 94 captured documents this
    /// project holds, the unscoped predicate fires twice, both on an
    /// `  or:` line inside the leading run, and never once outside it.
    #[test]
    fn a_command_table_described_using_the_tools_own_name_survives() {
        let raw = "Usage: git <command>\n\nCommands:\n  clone    git clone a repository\n  push     git push to a remote\n";
        let root = node("git");
        let report = analyze(raw, &root);
        assert_eq!(report.rows, 2, "both rows are real command-list entries");
        assert_eq!(
            report.unaccounted, 2,
            "and the empty tree accounts for neither"
        );
    }

    /// `chgrp`'s real line 2 — the same shape, printed with the absolute
    /// path the tool was invoked by, which a bare-name comparison misses.
    #[test]
    fn a_usage_alternative_printed_by_absolute_path_is_still_not_a_row() {
        let raw = "Usage: /usr/bin/chgrp [OPTION]... GROUP FILE...\n  or:  /usr/bin/chgrp [OPTION]... --reference=RFILE FILE...\n";
        let root = node("chgrp");
        assert_eq!(analyze(raw, &root).rows, 0);
    }

    // --- entity kinds beyond flags and positionals -----------------------
    //
    // `absorb` walks every entity generically, keyed on shape (a dashed
    // spelling vs. a bare one) rather than on `EntityKind`. These tests
    // pin that a `Modifier` and an `EnvVar` entity — the two kinds a
    // `match` over `EntityKind` would have been tempted to special-case —
    // both reach the vocabulary through the same bare-spelling branch a
    // positional already used.

    /// A `Modifier` entity's bare letter reaches the vocabulary and
    /// attests a matching row.
    ///
    /// The table below is deliberately **name-row shaped**
    /// (`d<gutter>description`), not `ar`'s real bracketed modifier row
    /// (`  [D]          - use zero for timestamps...`, see
    /// `corpus/ar/audit-seed2/help.txt`) — a bracketed row is not
    /// name-shaped and `classify` produces no row for it at all, so it
    /// could never exercise this attribution. The name-row shape here is
    /// the only one `classify` can emit a row for, which is also exactly
    /// why the fleet-wide before/after residue diff over `corpus/` is
    /// null today: no committed fixture's modifier table happens to be
    /// name-shaped. `ar rvD archive.a foo.o` is real `ar` usage — modifier
    /// letters glued to an operation letter — kept only as a familiar
    /// example of the notation, not a claim about how `ar` prints its
    /// table.
    #[test]
    fn a_modifier_entitys_letter_reaches_the_vocabulary() {
        let raw = "Modifiers:\n  d            delete members from the archive\n  r            insert with replacement\n";
        let mut root = node("ar");
        root.entities
            .push(Entity::modifier('d', Provenance::single(Source::HelpText)));
        root.entities
            .push(Entity::modifier('r', Provenance::single(Source::HelpText)));
        let report = analyze(raw, &root);
        assert_eq!(report.rows, 2);
        assert_eq!(
            report.unaccounted, 0,
            "a modifier's letter must attest its own row"
        );
    }

    /// The forward-compatibility claim this whole rewrite exists for: an
    /// `EntityKind::EnvVar` entity — a kind that exists on `main` today
    /// even though no tier emits one yet (spec §4.5) — reaches the
    /// vocabulary the moment one is constructed, with no further edit to
    /// this file once a producer lands. Built directly (`Entity::new` +
    /// a bare `Spelling`) rather than through a not-yet-existing
    /// `Entity::env_var` constructor, since none exists on `main`.
    #[test]
    fn an_env_var_entitys_name_reaches_the_vocabulary() {
        let mut e = Entity::new(EntityKind::EnvVar, Provenance::single(Source::HelpText));
        e.spellings.push(Spelling::bare("FFREPORT"));
        let mut root = node("ffmpeg");
        root.entities.push(e);

        let mut vocab = Vocabulary::default();
        vocab.absorb(&root);
        assert!(
            vocab.names.contains("ffreport"),
            "an EnvVar entity's name must reach the vocabulary: {:?}",
            vocab.names
        );
    }

    #[test]
    fn split_cell_handles_a_multibyte_first_cell() {
        // Arbitrary tool output is not ASCII; a byte-offset split here
        // would panic on exactly this shape (AGENTS.md's rule).
        let (cell, rest) = split_cell("--café  a description");
        assert_eq!(cell, "--café");
        assert_eq!(rest.trim(), "a description");
    }

    #[test]
    fn an_empty_document_is_inert() {
        let root = node("t");
        let report = analyze("", &root);
        assert_eq!(report.rows, 0);
        assert_eq!(report.unaccounted, 0);
        assert!(report.blocks.is_empty());
    }
}
