//! Layout-driven parsing of `--help` output: the `Usage:` block, and
//! indentation-delimited sections (`Options:`, `Flags:`, git's headingless
//! command groups, ...). Recognized purely by layout — a column-0 line
//! followed by more-indented lines — never by specific heading text;
//! within a block, entries are flags or subcommands by whether they start
//! with `-` (spec §1).
//!
//! **Never invent subcommands (spec §7 Tier B, [M-10]).** Four binding
//! rules gate every bare-word block before it becomes subcommands:
//!
//! 1. The heading must be *recognized* (mentions "command(s)"/
//!    "subcommand(s)" as a whole word, or continues a run that started
//!    under one — git's own leading blurb, not its group headings).
//! 2. A line at the description column with nothing at the name column is
//!    a continuation, never a new row.
//! 3. A candidate name must match `^[a-z][a-z0-9_.-]*$`; failing entries
//!    are dropped, not emitted.
//! 4. An unrecognized block nested under a flag becomes that flag's
//!    [`mandible_core::Entity::choices`], not subcommands; with no owning
//!    flag either, it is dropped. See S-013.

use super::grammar::{
    bracket_flag_row_content, is_bare_flag_spelling, is_bare_flag_token, is_dash_underline_token,
    looks_like_bracket_flag_row, looks_like_flag_start, looks_like_paren_alternation_open,
    looks_like_stanza_head_flag, paren_alternation_member_content, paren_depth_delta,
    parse_bundled_shorts, parse_flag_alternation, parse_flag_spec, split_alternatives, FlagSpec,
};
use super::profile::{heading_matches_markers, FrameworkProfile};
use mandible_core::{
    is_command_name_shaped, strip_escapes, Choice, CommandNode, Dashes, Entity, EntityKind,
    Provenance, Source, Spelling, Text, ValueKind,
};

mod backfill;
mod emit;
mod entry;
mod flag_rows;
mod heading;
mod layout;
mod preamble;
mod repair;
mod scan;
mod spelling;
#[cfg(test)]
mod test_support;
mod usage;

use backfill::*;
pub use emit::*;
pub use entry::*;
use flag_rows::*;
pub use heading::*;
pub use layout::*;
use preamble::*;
use repair::*;
use scan::*;
use spelling::*;
#[cfg(test)]
use test_support::*;
pub use usage::*;

/// Hard cap on distinct entries (subcommands, flags, or choices) accepted
/// from a single probe's output. Real `--help` output never remotely
/// approaches this. Defends against a degenerate input: instmodsh's
/// free-running REPL banner parsed into 58,663 duplicate "subcommands"
/// before this cap. Capping (and deduplicating) at the point of recovery,
/// rather than bounding cost after the fact, keeps one pathological tool
/// from slowing the whole pipeline. See S-072.
const MAX_RECOVERED_ENTRIES: usize = 4096;

/// Everything recovered from one `--help` invocation's output.
#[derive(Debug, Default)]
pub struct ParsedHelp {
    /// Leading prose before the `Usage:` line or the first section,
    /// if any (e.g. tar's "GNU 'tar' saves many files together...").
    pub description: Option<String>,
    /// The `Usage:` line(s), verbatim (joined).
    pub usage: Vec<String>,
    /// Positional placeholders pulled out of the usage line
    /// (`<value>`/`FILE`-shaped tokens not preceded by `-`).
    pub positionals: Vec<Entity>,
    /// Flags recovered from dash-led blocks.
    pub flags: Vec<Entity>,
    /// Modifier letters recovered from a modifier table — `ar`'s `[a]`,
    /// `[D]`, `[l <text> ]` rows (spec §7 Tier B, "Modifier tables"). See
    /// S-020.
    pub modifiers: Vec<Entity>,
    /// Environment variables recovered from a row under an explicitly
    /// labeled environment heading (spec §7 Tier B, "Environment
    /// sections"). Never scavenged from `ALL_CAPS` prose. See S-023.
    pub env_vars: Vec<Entity>,
    /// Subcommand stubs recovered from bare-word blocks under a
    /// recognized command heading (not yet extracted themselves —
    /// `children_filled: false`).
    pub subcommands: Vec<CommandNode>,
    /// Fraction of recognized entry lines the grammar fully understood,
    /// in `[0.0, 1.0]`.
    pub confidence: f32,
    /// True if at least one block yielded a name failing the candidate
    /// regex, or was dropped for lack of an owning heading/flag. Surfaced
    /// so `extract_node` can mark the node's provenance as a guess.
    pub saw_unattributable_content: bool,
    /// Names already accepted into `subcommands`, tracked alongside it so
    /// [`ParsedHelp::try_push_subcommand`] can reject duplicates in O(1)
    /// instead of an O(n) scan of `subcommands` per candidate (which would
    /// itself become O(n^2) on the exact degenerate input this whole cap
    /// exists for).
    subcommand_names_seen: std::collections::HashSet<String>,
}

impl ParsedHelp {
    /// Accept `node` into `subcommands` unless it's a duplicate name (an
    /// entry recovered twice, e.g. because a heading's block repeats
    /// verbatim in genuinely broken output) or the recovery cap has
    /// already been hit. Returns whether it was accepted.
    fn try_push_subcommand(&mut self, node: CommandNode) -> bool {
        if self.subcommands.len() >= MAX_RECOVERED_ENTRIES {
            return false;
        }
        if !self.subcommand_names_seen.insert(node.name.clone()) {
            return false;
        }
        self.subcommands.push(node);
        true
    }
}

/// Minimum count of independently parsed flag rows a same-indent (or
/// deeper) block must produce before it is trusted as real structure over
/// worked-example prose. Shared by [`starts_attested_flag_section`] (a
/// headed block) and `heading::starts_attested_headingless_flag_block` (a
/// headingless one): both read this as the same evidence bar, just applied
/// to different shapes of the same question — "is this row run cheap for
/// an unrelated document to have produced, or does it need a real flag
/// table to explain it."
pub(super) const MIN_ATTESTED_SECTION_FLAGS: usize = 2;

/// Section headings that introduce worked examples or prose, not
/// structure — tar's `Examples:` block contains lines starting with the
/// bare word `tar`, which would otherwise look like a subcommand entry.
/// See S-071.
fn is_ignorable_heading(heading: &str) -> bool {
    // Deliberately not matching "see also": git's own command group
    // headings legitimately carry that phrase as a parenthetical aside.
    let lower = heading.to_lowercase();
    lower.starts_with("example") || lower.contains("report bugs")
}

/// True when `heading` positively names a section whose rows describe CLI
/// flags. Used only to leave an otherwise-contained examples/reporting
/// region at the *same* indentation — same-indent text inside a worked
/// example is ambiguous (`Input:` can govern flag-shaped sample data just
/// as `Options:` governs real flags), so both this vocabulary and a real
/// flag-block shape below are required before the boundary reopens. See
/// S-071.
fn names_flag_section(heading: &str) -> bool {
    if !is_section_heading_line(heading) {
        return false;
    }
    let lower = heading.to_lowercase();
    if lower.contains("example") {
        return false;
    }
    lower.split(|c: char| !c.is_alphanumeric()).any(|word| {
        matches!(
            word,
            "option"
                | "options"
                | "flag"
                | "flags"
                | "switch"
                | "switches"
                | "modifier"
                | "modifiers"
                | "mode"
                | "modes"
                | "operation"
                | "operations"
                | "argument"
                | "arguments"
        )
    })
}

/// Whether the line at `heading_idx` is strong enough to end a hidden
/// ignorable region without a dedent. Heading wording alone is not
/// enough: its content must be more indented and independently satisfy
/// the flag-block recognizer. See S-071.
fn starts_attested_flag_section(lines: &[&str], heading_idx: usize) -> bool {
    let heading = lines[heading_idx];
    if !names_flag_section(heading.trim()) {
        return false;
    }
    let heading_indent = leading_whitespace(heading);
    let mut content_idx = heading_idx + 1;
    while content_idx < lines.len() && lines[content_idx].trim().is_empty() {
        content_idx += 1;
    }
    if content_idx >= lines.len() || leading_whitespace(lines[content_idx]) <= heading_indent {
        return false;
    }
    let Some(flags_start) = flags_block_start(lines, content_idx) else {
        return false;
    };
    // One flag-shaped sample row is still cheap inside a worked example;
    // at least two independently parsed rows plus the heading vocabulary
    // above is the minimum evidence to reopen a same-indent section. See
    // S-071.
    let (_, entries, _, _) = scan_flags_block(lines, flags_start, false);
    entries.len() >= MIN_ATTESTED_SECTION_FLAGS
}

/// True if `heading` mentions "command"/"commands"/"subcommand"/
/// "subcommands" as a whole word (case-insensitive) — spec §7 Tier B rule
/// 1's literal recognized-heading test. Matches `"Commands:"`,
/// `"Available Commands:"`, `"SUBCOMMANDS"`, and prose like `"These are
/// common Git commands used in various situations:"`.
fn mentions_commands_word(s: &str) -> bool {
    s.split(|c: char| !c.is_alphanumeric())
        .map(|w| w.to_lowercase())
        .any(|w| {
            matches!(
                w.as_str(),
                "command" | "commands" | "subcommand" | "subcommands"
            )
        })
}

/// True if `heading` mentions "operation"/"operations" as a whole word —
/// llvm-ar's `OPERATIONS:` table, an invocation verb the same class of
/// evidence as "command(s)". Deliberately narrower than
/// [`mentions_commands_word`]: feeds only [`is_recognized_command_heading`],
/// never [`command_mode_seed`]. See S-022.
fn mentions_operations_word(s: &str) -> bool {
    s.split(|c: char| !c.is_alphanumeric())
        .map(|w| w.to_lowercase())
        .any(|w| matches!(w.as_str(), "operation" | "operations"))
}

// Rule 3's name-shape test (`^[a-z][a-z0-9_.-]*$`) lives in
// `mandible_core::is_command_name_shaped` (imported above) — it's also half
// of the coverage harness's structure-sanity check (spec §13.1), so there
// is exactly one definition of "looks like a name, not a fabricated
// fragment" rather than two that could drift apart.

/// Parse raw `--help` text into structured pieces, with no framework
/// knowledge — the generic layout engine alone (spec §7 Tier B step 2).
/// Equivalent to `parse_with_profile(raw, None, None)`. `#[cfg(test)]`:
/// the one production caller always calls [`parse_with_profile`] directly
/// with a known name.
#[cfg(test)]
pub fn parse(raw: &str) -> ParsedHelp {
    parse_with_profile(raw, None, None)
}

/// [`parse`], but naming the tool whose `--help` this is — see
/// [`parse_with_profile`]'s `tool_name` parameter.
#[cfg(test)]
fn parse_named(raw: &str, tool_name: &str) -> ParsedHelp {
    parse_with_profile(raw, None, Some(tool_name))
}

/// Same engine as [`parse`], but consulting `profile`'s framework-specific
/// heading vocabulary when present (spec §7 Tier B step 1). `None`
/// reproduces [`parse`]'s generic behavior exactly, so both degradation
/// levels share one engine instead of forking into two.
///
/// `tool_name` is the probed tool's own root name when known. It feeds
/// the usage-block scanner's "starts a new entry" test alongside the
/// `usage:`/`or:` markers. `None` is always safe: it only makes the
/// name-based half of that test inert, never wrong.
pub fn parse_with_profile(
    raw: &str,
    profile: Option<&FrameworkProfile>,
    tool_name: Option<&str>,
) -> ParsedHelp {
    // Strip terminal escape sequences over the whole document, once,
    // before any layout analysis: headings, indentation and column gaps
    // are all measured on this raw string, so an escape sequence left in
    // place corrupts every one of those measurements. systemd-creds
    // glues a reset code onto its own heading (`[0mCommands:`), which
    // fuses into one alphanumeric run that matches no recognized heading
    // word. See S-002.
    let raw = strip_escapes(raw);
    // A heading that shares its physical line with the first row of its
    // own table is rewritten into the two lines it means before the
    // engine below ever sees it. Doing it here, once, keeps the recovered
    // row subject to every block-level decision alongside the rows
    // beneath it, rather than bolted on afterwards. See S-018.
    match split_shared_heading_rows(&raw) {
        Some((rewritten, bnf_row_lines)) => {
            parse_body(&rewritten, profile, tool_name, &bnf_row_lines)
        }
        None => parse_body(&raw, profile, tool_name, &std::collections::HashSet::new()),
    }
}

/// The recovered usage block: where scanning stopped, the synopsis
/// entries, and the operands read out of them.
struct UsageScan {
    next_index: usize,
    entries: Vec<String>,
    positionals: Vec<Entity>,
}

/// Walk the usage block starting at `start`, folding wrapped continuation
/// lines into one entry each. See docs/shapes.md S-001 and S-037.
fn scan_usage_section(
    lines: &[&str],
    start: usize,
    labelled_usage_start: Option<usize>,
    tool_name: Option<&str>,
    usage_lines: &mut Vec<String>,
) -> UsageScan {
    let mut i = start;
    let base_indent = leading_whitespace(lines[i]);
    usage_lines.push(lines[i].trim().to_string());
    let mut usage_entries = vec![lines[i].trim().to_string()];
    // Parallel to `usage_lines`: which `usage_entries` index each
    // physical line was folded into — a wrapped entry (sg_sanitize's
    // five-line synopsis) spans several lines but is one entry, and
    // [`primary_synopsis_lines`] needs every one of them.
    let mut line_entry_index = vec![0usize];
    // Running depth of an open parenthesized alternation group (LVM's
    // "any one is required" convention), tracked only for an
    // unlabelled synopsis: a member row routinely opens with `-`
    // itself, which the "a continuation line that reads as a flag
    // entry ends the block" check just below would otherwise
    // misread as ending the block. See S-088.
    let mut paren_group_depth: i32 = 0;
    // True for exactly the one loop iteration right after
    // `paren_group_depth` returns to zero — tells "a blank line right
    // after the group's own closing `)`" (still the same stanza's
    // trailing bracket-row flags) apart from an ordinary between-
    // stanza blank line. Reset unconditionally elsewhere.
    let mut just_closed_paren_group = false;
    i += 1;
    while i < lines.len() {
        let l = lines[i];
        if l.trim().is_empty() {
            if paren_group_depth > 0 {
                // A blank line inside an unclosed group is not a shape
                // any real specimen produces; refuse to guess at what it
                // means rather than fabricate a continuation across it.
                paren_group_depth = 0;
                just_closed_paren_group = false;
            }
            if just_closed_paren_group {
                just_closed_paren_group = false;
                if let Some(next) = lines.get(i + 1) {
                    let t = next.trim_start();
                    if !t.is_empty() && looks_like_bracket_flag_row(t) {
                        // The group's trailing bracket-row flags
                        // continue after exactly one blank line —
                        // vgchange's `( ... )` then a blank line then
                        // `[ -A|--autobackup y|n ]`, still the same
                        // stanza. See S-088.
                        i += 1;
                        continue;
                    }
                }
            }
            // Some tools write their unlabelled synopsis as one stanza
            // per operation mode: a description line, an own-name
            // head, continuation rows, with a blank line between
            // stanzas. LVM's own emitter is the specimen; adduser and
            // pydoc3 hit the identical shape with unrelated
            // formatters, so the predicates below key on structure,
            // never a tool's name. A blank line ended the usage block
            // unconditionally before this fix, so only vgck's first
            // stanza was ever read (`vgck --updatemetadata`, a flag,
            // was absent; lvconvert hides 26 more stanzas this way).
            //
            // Deliberately not "any blank line continues the block"
            // ([M-10]): it fires only for the unlabelled-synopsis
            // entry point, and only when the next non-consumed line is
            // itself unambiguous synopsis-head evidence. At most one
            // line in between may be skipped, and only when it reads
            // as a full sentence — the stanza's own description,
            // consumed here so it lands in neither the synopsis nor
            // the tool's description. See S-005.
            if labelled_usage_start.is_none() {
                if let Some(name) = tool_name {
                    let mut j = i + 1;
                    // Deliberately not `looks_like_unlabeled_synopsis_line`
                    // here: that test alone would also admit corepack's
                    // headingless invocation-table rows (`corepack
                    // enable [--install-directory #0] ...`), demoting a
                    // real subcommand into fabricated usage text. See
                    // S-016.
                    let is_head = |lines: &[&str], j: usize| {
                        j < lines.len() && looks_like_stanza_continuation_head(lines, j, name)
                    };
                    if !is_head(lines, j) {
                        if let Some(next) = lines.get(j) {
                            let t = next.trim_start();
                            if !t.is_empty() && is_prose_sentence(t) {
                                j += 1;
                            }
                        }
                    }
                    if is_head(lines, j) {
                        let trimmed = lines[j].trim().to_string();
                        usage_lines.push(trimmed.clone());
                        usage_entries.push(trimmed);
                        line_entry_index.push(usage_entries.len() - 1);
                        i = j + 1;
                        continue;
                    }
                }
            }
            break;
        }
        let trimmed_start = l.trim_start();
        if labelled_usage_start.is_none() && paren_group_depth > 0 {
            // Already inside an open parenthesized alternation group:
            // every line up to the matching close is a member,
            // regardless of shape. A member row routinely opens with
            // `-` itself, which the flag-entry-ends-the-block check
            // below would otherwise misread — depth, not content, is
            // what says this line still belongs. See S-088.
            paren_group_depth += paren_depth_delta(trimmed_start);
            if paren_group_depth <= 0 {
                paren_group_depth = 0;
                just_closed_paren_group = true;
            } else {
                just_closed_paren_group = false;
            }
            let trimmed = l.trim().to_string();
            usage_lines.push(trimmed.clone());
            if let Some(last) = usage_entries.last_mut() {
                last.push(' ');
                last.push_str(&trimmed);
            }
            line_entry_index.push(usage_entries.len() - 1);
            i += 1;
            continue;
        }
        if labelled_usage_start.is_none()
            && paren_group_depth == 0
            && looks_like_paren_alternation_open(trimmed_start)
        {
            // Opens the group: hand off to the depth-tracking branch
            // above for every later line, but this opening row itself
            // still falls through to the ordinary flow just below,
            // which already appends it correctly (it is more indented
            // than the stanza head and, starting with `(` rather than
            // `-`, trips none of the content checks that would end the
            // block).
            paren_group_depth = paren_depth_delta(trimmed_start);
        }
        just_closed_paren_group = false;
        let is_marker =
            starts_with_usage_prefix(trimmed_start) || starts_with_or_marker(trimmed_start);
        let is_own_name = tool_name.is_some_and(|name| starts_with_tool_name(trimmed_start, name));
        let starts_new_entry = is_marker || is_own_name;

        // A line the one above it ended with a backslash is a
        // continuation by the tool's own explicit statement, and no
        // content test may overrule that. update-xmlcatalog's
        // backslash-wrapped synopsis tail begins with `--id`, which
        // the flag-start guard below would otherwise end the block
        // on, dropping `--del`/`--root`/`--type`. See S-011.
        let continues_previous_line = i > 0 && lines[i - 1].trim_end().ends_with('\\');
        if !starts_new_entry && !continues_previous_line {
            // A continuation line that itself reads as a flag entry
            // ends the usage block, even indented with no blank
            // separator: a usage continuation is an alternative
            // invocation form and never begins with a dash. curl's 13
            // flag rows sit one space under `Usage:` with no `Options:`
            // heading, and all 13 used to land in `usage` with zero
            // flags parsed. See S-074.
            if looks_like_flag_start(trimmed_start) {
                break;
            }
            // A section heading ends the usage block no matter how
            // far it is indented. Binutils ar indents its whole body,
            // including its heading, under the synopsis; indentation
            // alone would join the heading and all eight command rows
            // into one usage string and yield zero subcommands. See
            // S-075.
            if is_section_heading_line(trimmed_start) {
                break;
            }
            // A decorative dash-bracketed divider (tree's own
            // `------- Listing options -------`) is never a usage
            // continuation either. See S-064.
            if looks_like_dash_bracketed_heading(trimmed_start) {
                break;
            }
            // A line more indented than the base is not always a
            // continuation — only when it still reads as usage
            // grammar. sg_emc_trespass follows its usage line with two
            // ordinary sentences indented under it; the old rule
            // joined them onto the synopsis, and `extract_positionals`
            // mined their bare uppercase words as fabricated operands.
            // Drops the one prose line rather than ending the block:
            // mdadm interleaves a description under each of several
            // alternative `mdadm --mode ...` forms, so breaking on the
            // first would drop every later one. Guarded to lines
            // strictly more indented than the base — du's own
            // trailing sentence sits at the base indent and must keep
            // taking the base-indent fallback below. See S-003.
            if leading_whitespace(l) > base_indent && is_prose_sentence(trimmed_start) {
                i += 1;
                continue;
            }
            // Below the base indent (never above it: `leading_whitespace`
            // is unsigned, so this also covers "equal to"), indentation
            // alone can't distinguish a genuine continuation (lsof) from
            // the block having ended (du) — fall back to content shape.
            if leading_whitespace(l) <= base_indent && !looks_like_usage_fragment(trimmed_start) {
                break;
            }
        }
        let trimmed = l.trim().to_string();
        usage_lines.push(trimmed.clone());
        if starts_new_entry {
            // A form keeps the indentation its author gave it (spec
            // §4.1): ip lines its second form up under the first. Only
            // the display form carries it; `usage_lines` stays trimmed
            // since it reads tokens, never columns.
            usage_entries.push(l.trim_end().to_string());
        } else if let Some(last) = usage_entries.last_mut() {
            // The backslash is the join, the same way a single space
            // is elsewhere: without dropping it, the displayed
            // synopsis reads `--type <type> \ --id <id>`, a
            // continuation marker stranded mid-line.
            if last.ends_with('\\') {
                last.pop();
                let trimmed_tail = last.trim_end().len();
                last.truncate(trimmed_tail);
            }
            last.push(' ');
            last.push_str(&trimmed);
        }
        line_entry_index.push(usage_entries.len() - 1);
        i += 1;
    }
    // Scoped to a labelled block, never an unlabelled synopsis
    // (`dbus-cleanup-sockets`, `lvreduce`): the existence oracle's own
    // synopsis scanner has no unlabelled-synopsis support yet, so any
    // operand recovered from one reports as invented today. See
    // S-001.
    let primary_lines = if labelled_usage_start.is_some() {
        primary_synopsis_lines(&usage_entries, &line_entry_index, usage_lines.len())
    } else {
        std::collections::HashSet::new()
    };
    UsageScan {
        next_index: i,
        positionals: extract_positionals(usage_lines, primary_lines),
        entries: usage_entries,
    }
}

/// The leading prose before the usage block, as the node description.
/// Paragraph-aware so a version or author banner can be dropped.
/// See docs/shapes.md S-001.
fn extract_description(
    lines: &[&str],
    description_bound: usize,
    usage_start: Option<usize>,
    i: usize,
) -> Option<String> {
    // A column-0 line inside the recovered usage block's own line range is
    // never description prose, whichever of the three entry shapes found
    // it — checking the range rather than re-testing
    // `starts_with_usage_prefix` is what keeps this correct for the
    // name-prefixed and unlabelled shapes too, neither of which starts
    // with `usage:` at its own line start. See S-001.
    let in_usage_block = |idx: usize| usage_start.is_some_and(|s| (s..i).contains(&idx));
    // Collected as paragraphs (blank-line-separated runs), not one flat
    // list, so a leading version/author/URL banner can be told apart from
    // the real description — see `is_banner_paragraph` below. A skipped
    // indented line (a usage continuation in this same zone, du's own
    // `  or: ...`) does not break the paragraph it sits inside.
    let mut paragraphs: Vec<Vec<&str>> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut j = 0;
    while j < lines.len() && j < description_bound {
        let l = lines[j];
        if l.trim().is_empty() {
            if !current.is_empty() {
                paragraphs.push(std::mem::take(&mut current));
            }
        } else if leading_whitespace(l) == 0 {
            let t = l.trim_start();
            if !starts_with_usage_prefix(t)
                && !in_usage_block(j)
                && !looks_like_bnf_production_line(t)
            {
                current.push(l);
            }
        }
        j += 1;
    }
    if !current.is_empty() {
        paragraphs.push(current);
    }
    // clap's own template (and every framework that copies it) renders
    // name/version/author/homepage as one paragraph before the real
    // description — zoxide's own banner is the specimen. Only drop the
    // first paragraph, and only when it looks like this banner shape and
    // a later paragraph exists to fall back to. See S-068.
    // The tool's own option-error complaint (ssh-keygen's `unknown option
    // -- -`) is checked first and independently: unlike a banner, it is
    // allowed to drop the only leading paragraph. See S-039.
    let drop_first_paragraph = match paragraphs.first() {
        Some(first) if is_option_error_paragraph(first) => true,
        Some(first) if paragraphs.len() > 1 && is_banner_paragraph(first) => true,
        _ => false,
    };
    // Handed over with its line structure intact — one `\n` per source
    // line, `\n\n` between paragraphs — rather than pre-flattened with
    // spaces: deciding which break is hard-wrap and which is real
    // structure is `Text::sanitize`'s job (spec §4.1), and joining early
    // throws that evidence away first. grep's `Example: grep -i 'hello
    // world' menu.h main.c` needs its own line kept. See S-069.
    let kept: Vec<Vec<&str>> = if drop_first_paragraph {
        paragraphs.into_iter().skip(1).collect()
    } else {
        paragraphs
    };
    let description = kept
        .iter()
        .map(|paragraph| paragraph.join("\n"))
        .filter(|paragraph| !paragraph.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if description.is_empty() {
        None
    } else {
        Some(description)
    }
}

/// Walk the document body after the description, emitting flags,
/// subcommands, modifiers, positionals and environment variables as
/// each recognized section shape is met. Returns the entry counts
/// `compute_confidence` scores. See docs/shapes.md S-013 and S-019.
/// The mutable state the body scan threads through every branch.
/// Bundled so a branch can be lifted into its own function without a
/// ten-argument signature.
/// The command-table tail: a recognized heading, a sticky chain, or the
/// bare-word fallback, whichever the block turns out to be.
/// See docs/shapes.md S-017 and S-053.
fn emit_command_table(inp: &BodyInput, h: &Heading, mut i: usize, st: &mut BodyScan) -> usize {
    let raw = inp.raw;
    let lines = inp.lines;
    let profile = inp.profile;
    let heading = &h.heading;
    let heading_indent = h.heading_indent;
    // A framework-declared non-command heading both refuses this
    // block and breaks the engine's sticky same-indent chain, so
    // nothing after it inherits a `st.command_mode` this heading
    // positively contradicts.
    let is_declared_non_command = profile.is_some_and(|p| {
        heading_matches_markers(&heading.to_lowercase(), p.non_command_heading_markers)
    });
    let recognized = is_recognized_command_heading(heading, profile);
    // Issue #3: ` - ` is accepted as an entry separator alongside the
    // usual column gap, but only when this block is already headed
    // for `emit_subcommands`. Scoping the decision to before the scan
    // is what keeps a bare ` - ` in ordinary prose from manufacturing
    // commands, the same failure class as apt-get's own description
    // paragraph, just via the column-gap rule instead.
    let allow_dash_separator = (recognized || st.command_mode) && !is_declared_non_command;

    // A headed command table whose rows use ` = ` as a separator, or
    // no separator at all — wpa_cli's `=`-separated rows,
    // fail2ban-client's wrapped continuations. See S-019.
    //
    // Gated on `recognized` alone, deliberately narrower than
    // `allow_dash_separator`, which also accepts a `st.command_mode`
    // chain: fail2ban-client's `Command:` block nests rows whose
    // descriptions wrap across further lines, and the engine's own
    // pseudo-heading rewind re-examines each such row once
    // `st.command_mode` is stuck on, satisfying every safeguard with
    // ordinary English words. `recognized` — true only when this
    // exact heading's own text names a command — is false for every
    // such pseudo-heading, so requiring it directly closes this off.
    if recognized && !is_declared_non_command {
        if let Some((end, entries)) = scan_bare_command_table(lines, i) {
            i = end;
            st.command_mode = true;
            st.in_ignorable_section = false;
            let raw_tokens = command_table_token_index(raw);
            let (seen, clean) = emit_headed_command_table(entries, &raw_tokens, st.result);
            st.total_entries += seen;
            st.clean_entries += clean;
            return i;
        }
    }

    // Busybox's applet list is a single flat, comma-separated run
    // under one heading, structurally distinct from every other
    // framework's per-line bare-word block, so it gets first refusal
    // here. Gated on the profile flag and this heading already being
    // recognized or continuing a `st.command_mode` chain. See S-093.
    if profile.is_some_and(|p| p.comma_separated_command_list)
        && (recognized || st.command_mode)
        && !is_declared_non_command
    {
        let (end, entries) = scan_comma_separated_commands(lines, i);
        i = end;
        st.command_mode = true;
        st.in_ignorable_section = false;
        let (seen, clean) = emit_subcommands(heading, entries, st.result);
        st.total_entries += seen;
        st.clean_entries += clean;
        return i;
    }

    let (end, entries) = scan_bare_block(lines, i, heading_indent, allow_dash_separator);
    i = end;
    if is_ignorable_heading(heading) {
        return i;
    }

    if is_declared_non_command {
        st.command_mode = false;
        let (seen, clean) = emit_choices(heading, entries, st.result);
        st.total_entries += seen;
        st.clean_entries += clean;
        return i;
    }

    if recognized || st.command_mode {
        st.command_mode = true;
        // Only `recognized` (this exact heading's own text says
        // "commands") is strong enough to clear the flag here — the
        // `st.command_mode` sticky-chain half of this condition is not:
        // it can still be true from an *inherited* chain rather than
        // this heading's own evidence, which is exactly the kind of
        // indirect signal `st.in_ignorable_section` must not trust (see
        // its own doc comment on why the generic bare-block fallback
        // is deliberately excluded from clearing it).
        if recognized {
            st.in_ignorable_section = false;
        }
        let (seen, clean) = emit_subcommands(heading, entries, st.result);
        st.total_entries += seen;
        st.clean_entries += clean;
    } else {
        st.command_mode = false;
        let (seen, clean) = emit_choices(heading, entries, st.result);
        st.total_entries += seen;
        st.clean_entries += clean;
    }
    i
}

/// The read-only inputs every body-scan branch needs.
struct BodyInput<'a> {
    raw: &'a str,
    lines: &'a [&'a str],
    usage_lines: &'a [String],
    profile: Option<&'a FrameworkProfile>,
    bnf_row_lines: &'a std::collections::HashSet<usize>,
}

/// A heading with content indented beneath it. Each recognized section
/// shape gets first refusal in turn, and the bare-word block is the
/// fallback. See docs/shapes.md S-013, S-019 and S-020.
fn emit_heading_block(
    inp: &BodyInput,
    tool_name: Option<&str>,
    h: &Heading,
    mut i: usize,
    st: &mut BodyScan,
) -> usize {
    let raw = inp.raw;
    let lines = inp.lines;
    let usage_lines = inp.usage_lines;
    let profile = inp.profile;
    let bnf_row_lines = inp.bnf_row_lines;
    let line = h.line;
    let heading = &h.heading;
    let heading_indent = h.heading_indent;
    let heading_idx = h.heading_idx;

    // Reaching here means genuinely more-indented content follows this
    // heading — LVM's own stanza shape, a head line naming a
    // mode-selecting flag followed by that mode's rows. Recovering
    // the flag is independent of what the content turns out to be, so
    // this runs once per heading rather than folded into any later
    // branch.
    //
    // Gated on `!st.in_ignorable_section`: bpftrace's own `EXAMPLES:`
    // block writes each example in the identical shape to a real
    // stanza head, and without this guard it fabricated `-e`/`-l`
    // rows that displaced the real, described ones. See S-071.
    //
    // A stanza with its own description sentence above its head line
    // labels its group with that sentence instead ([`stanza_description_above`]).
    // The head line is not lost: it becomes a usage form
    // (`st.result.usage`, spec §4.5). Pushed here because this is the
    // one place that knows the head line is about to stop being the
    // group label; `extract_positionals` has already run, so nothing
    // is mined out of the added line. See S-012.
    //
    // Capped, not deduplicated: `i` only ever advances, so no
    // physical line becomes `heading` twice; two identical entries
    // mean the tool printed the stanza twice. A scan of `usage` per
    // heading would be the quadratic shape `MAX_RECOVERED_ENTRIES`
    // exists for (instmodsh's repeated banner, S-072).
    let stanza_label = if st.in_ignorable_section {
        None
    } else {
        stanza_description_above(lines, heading_idx, tool_name).map(str::to_string)
    };
    if stanza_label.is_some() && st.result.usage.len() < MAX_RECOVERED_ENTRIES {
        st.result.usage.push(heading.clone());
    }
    if !st.in_ignorable_section {
        if let Some(mut flag) = recover_stanza_head_flag(heading, tool_name) {
            if let Some(label) = stanza_label.clone() {
                flag.group = Some(label);
            }
            if st.result.flags.len() < MAX_RECOVERED_ENTRIES {
                st.result.flags.push(flag);
            }
        }
    }

    // A headed command table whose first row sits on the heading's
    // own physical line (apt-ftparchive's `Commands: packages
    // binarypath [overridefile [pathprefix]]`), with remaining rows
    // column-aligned beneath it. Without this it vanishes whole into
    // the `heading` string and never reaches any scanner as data. See
    // S-018.
    //
    // Gated on `is_recognized_command_heading(label, ...)` for this
    // heading directly, never a `st.command_mode` chain, since a sticky
    // chain can reach an unrelated wrapped-prose block.
    if let Some((label, inline_row)) = split_heading_inline_row(line.trim()) {
        if !is_ignorable_heading(heading)
            && is_recognized_command_heading(label, profile)
            && !profile.is_some_and(|p| {
                heading_matches_markers(&label.to_lowercase(), p.non_command_heading_markers)
            })
        {
            if let Some(first_name) = leading_command_name(inline_row) {
                if let Some((end, mut entries)) = scan_bare_command_table(lines, i) {
                    entries.insert(0, (first_name, None));
                    i = end;
                    st.command_mode = true;
                    st.in_ignorable_section = false;
                    let raw_tokens = command_table_token_index(raw);
                    let (seen, clean) = emit_headed_command_table(entries, &raw_tokens, st.result);
                    st.total_entries += seen;
                    st.clean_entries += clean;
                    return i;
                }
            }
        }
    }

    // A modifier table (ar's ` command specific modifiers:`/
    // ` generic modifiers:`, llvm-ar's `MODIFIERS:`) gets first
    // refusal here: a `[a]` row is not `looks_like_flag_start`, and
    // under a heading containing "command" it went to
    // `emit_subcommands`, where every row failed the name-shape test.
    // See S-020.
    //
    // Falls through rather than `continue`ing when the block still
    // has content past the run: ar's ` generic modifiers:` is seven
    // bracket rows, then `@<file>`, then four real flags that must
    // keep the group they already carry.
    if let Some((end, rows)) = scan_modifier_table(lines, i) {
        i = end;
        if !is_ignorable_heading(heading) {
            // Positively-recognized structure clears the examples flag
            // and contradicts a command-mode sticky chain (ar's own
            // heading contains "command").
            st.in_ignorable_section = false;
            st.command_mode = false;
            let (seen, clean) =
                emit_modifiers(meaningful_flag_group(heading.clone()), rows, st.result);
            st.total_entries += seen;
            st.clean_entries += clean;
        }
        if i >= lines.len() || leading_whitespace(lines[i]) <= heading_indent {
            return i;
        }
    }

    // The argfile sigil row (spec §4.5) sometimes sits directly where
    // a modifier table's rows just ended — ar's `@<file>` row after
    // its modifier table — and `flags_block_start` below never sees
    // it there. Captured here instead, mirroring the modifier
    // branch's "falls through" shape. See S-021.
    if !is_ignorable_heading(heading) && i < lines.len() {
        if let Some(value_name) = argfile_row_value_name(lines[i].trim_start()) {
            let entry = argfile_flag_entry(lines[i], value_name);
            emit_argfile_flag(meaningful_flag_group(heading.clone()), entry, st.result);
            st.total_entries += 1;
            st.clean_entries += 1;
            i += 1;
            if i >= lines.len() || leading_whitespace(lines[i]) <= heading_indent {
                return i;
            }
        }
    }

    // An environment section (bpftrace's `ENVIRONMENT:`, node's
    // `Environment variables:`, mksquashfs's `Environment:`) — spec
    // §4.5's "strict-sections-only" rule made structural: unlike the
    // modifier table above, gated on the heading itself first, never
    // row shape alone, since a bare identifier plus a column gap is
    // indistinguishable from mysqlslap's own flush-left settings
    // table otherwise. See S-023.
    //
    // Falls through rather than `continue`ing, mirroring the modifier
    // branch, so a block running on past its rows into ordinary flags
    // does not lose those flags' group.
    if is_environment_heading(heading) && !is_ignorable_heading(heading) {
        if let Some((end, rows)) = scan_env_var_table(lines, i) {
            i = end;
            st.in_ignorable_section = false;
            st.command_mode = false;
            let (seen, clean) =
                emit_env_vars(meaningful_flag_group(heading.clone()), rows, st.result);
            st.total_entries += seen;
            st.clean_entries += clean;
            if i >= lines.len() || leading_whitespace(lines[i]) <= heading_indent {
                return i;
            }
        }
    }

    // Peek the first content lines to decide flags vs. bare-word. Not
    // just the *first*: some tools document a positional at the top of
    // their options table, and keying the whole decision off row one
    // threw the rest of the block away. See `flags_block_start`.
    if let Some(flags_start) = flags_block_start(lines, i) {
        // `flags_start` — never `heading_idx` — is the evidence: see
        // `split_shared_heading_rows`'s doc comment for why the BNF
        // fact is keyed on the row rather than the heading beside it.
        let heading_is_bnf = bnf_row_lines.contains(&flags_start);
        let (end, entries, packed, argfile_entry) =
            scan_flags_block(lines, flags_start, heading_is_bnf);
        i = end;
        if is_ignorable_heading(heading) {
            st.command_mode = false;
            return i;
        }
        // A real, non-ignorable flags block — structurally strong
        // evidence we are not (or no longer) inside an examples-shaped
        // section. See `st.in_ignorable_section`'s own doc comment.
        st.in_ignorable_section = false;
        // A stanza's own description sentence outranks its head line
        // as the group's label, and only there — every other block
        // still takes `meaningful_flag_group`'s answer unchanged. See
        // S-012.
        let group = stanza_label
            .clone()
            .or_else(|| meaningful_flag_group(heading.clone()));
        if packed {
            let seen = entries.len();
            emit_packed_flags(
                group.clone(),
                entries.into_iter().map(|(s, d, _)| (s, d)).collect(),
                st.result,
            );
            st.total_entries += seen;
            st.clean_entries += seen;
        } else {
            let (seen, clean) = emit_flags(group.clone(), entries, st.result);
            st.total_entries += seen;
            st.clean_entries += clean;
        }
        if let Some(entry) = argfile_entry {
            st.total_entries += 1;
            st.clean_entries += 1;
            emit_argfile_flag(group, entry, st.result);
        }
        st.command_mode = false;
        return i;
    }

    // Argparse's subparser blocks get first refusal here, gated on
    // the profile explicitly opting in. Deliberately not also gated
    // on the heading reading "positional arguments": it was, and that
    // made `add_subparsers(title=...)`'s ordinary heading style
    // collapse the entire command tree — smokecli's fixture rendered
    // one node. The structural evidence the scan demands (a `{...}`
    // pseudo-entry with deeper lines beneath it) is strictly stronger
    // than the heading text was; a plain positional's `{...}` metavar
    // has nothing beneath it and is still never promoted. See S-073.
    if profile.is_some_and(|p| p.argparse_subparser_quirk) {
        if let Some((end, entries)) = scan_argparse_subparsers(lines, i, heading_indent) {
            i = end;
            st.command_mode = false;
            st.in_ignorable_section = false;
            let (seen, clean) = emit_subcommands(heading, entries, st.result);
            st.total_entries += seen;
            st.clean_entries += clean;
            return i;
        }
    }

    // A framework-declared positional-operand heading. Sits directly
    // below the subparser scan since for argparse the two read the
    // same heading, and the subparser scan's `{...}`-with-entries
    // evidence is stronger than heading text alone. Also breaks the
    // sticky `st.command_mode` chain. See S-078.
    if profile.is_some_and(|p| {
        heading_matches_markers(&heading.to_lowercase(), p.positional_heading_markers)
    }) {
        let (end, entries) = scan_bare_block(lines, i, heading_indent, false);
        i = end;
        st.command_mode = false;
        st.in_ignorable_section = false;
        let (block_seen, block_clean) = emit_declared_positionals(entries, usage_lines, st.result);
        st.total_entries += block_seen;
        st.clean_entries += block_clean;
        return i;
    }

    emit_command_table(inp, h, i, st)
}

/// One candidate heading line and where it sits.
struct Heading<'a> {
    line: &'a str,
    heading: String,
    heading_indent: usize,
    heading_idx: usize,
}

/// A heading whose next non-blank line is not indented past it. The
/// block is still emitted when the shape is unambiguous on its own,
/// otherwise scanning rewinds past the heading. See docs/shapes.md S-016.
fn emit_flush_heading(
    lines: &[&str],
    profile: Option<&FrameworkProfile>,
    h: &Heading,
    mut i: usize,
    st: &mut BodyScan,
) -> usize {
    let heading = &h.heading;
    let heading_indent = h.heading_indent;
    let heading_idx = h.heading_idx;
    // An environment section flush with its own heading — no
    // indent step between "Environment variables:" and its first
    // row. node's real `--help` writes exactly this shape (both
    // at column 0). Gated on the row sitting at exactly
    // `heading_indent`, the same bar the same-indent command
    // table below requires. See S-023.
    if i < lines.len()
        && leading_whitespace(lines[i]) == heading_indent
        && is_environment_heading(heading)
        && !is_ignorable_heading(heading)
    {
        if let Some((end, rows)) = scan_env_var_table(lines, i) {
            i = end;
            st.in_ignorable_section = false;
            st.command_mode = false;
            let (seen, clean) =
                emit_env_vars(meaningful_flag_group(heading.clone()), rows, st.result);
            st.total_entries += seen;
            st.clean_entries += clean;
            return i;
        }
    }
    // Nothing more-indented follows. openssl and BSD-style
    // listings generally present a command list as a same-indent
    // word grid: a heading followed by lines of several bare
    // identifier-shaped tokens, no descriptions. Starting a grid
    // requires >=3 name-shaped tokens on the trigger line (not
    // the >=2 for continuation rows), so a genuine two-word
    // heading (openssl's "Standard commands") is never itself
    // swallowed as the first grid row — it rewinds and is
    // re-examined as its own heading. See S-065.
    if i < lines.len()
        && leading_whitespace(lines[i]) == heading_indent
        && looks_like_word_grid_start(lines[i])
    {
        let grid_start = i;
        while i < lines.len() {
            if lines[i].trim().is_empty() {
                break;
            }
            if leading_whitespace(lines[i]) != heading_indent
                || !looks_like_word_grid_line(lines[i])
            {
                break;
            }
            i += 1;
        }
        if !is_ignorable_heading(heading) {
            st.in_ignorable_section = false;
            let recognized = is_recognized_command_heading(heading, profile);
            if recognized {
                st.command_mode = true;
            }
            let (seen, clean) = process_word_grid(
                heading,
                &lines[grid_start..i],
                recognized || st.command_mode,
                st.result,
            );
            st.total_entries += seen;
            st.clean_entries += clean;
        }
        return i;
    }
    // A command table that sits at the same indent as its own
    // heading rather than beneath it. dnf's flush command list is
    // the specimen; the engine's "content indented more than its
    // heading" rule cannot see it at all.
    //
    // Guarded much harder than the indented case, since this is
    // the shape [M-10] came in through: apt-get's own prose
    // paragraph fabricated the subcommands "and", "information",
    // "about", "them". The heading must be a recognized command
    // heading (never merely a line ending in a colon), every row
    // must be column-aligned, and there must be at least two such
    // rows. Prose is single-spaced, so it fails the second
    // test on its first line. The heading must also not itself
    // look like one of the rows: at a shared indent there is no
    // structural difference otherwise, and mysqlslap's own
    // `init-command    (No default value)` row was taken as a
    // heading, fabricating 28 subcommands out of MySQL settings. A
    // real heading is a single field; a row is two columns. See
    // S-092.
    let heading_is_itself_a_row = find_description_gap(lines[heading_idx]).is_some();

    if i < lines.len()
        && leading_whitespace(lines[i]) == heading_indent
        && !heading_is_itself_a_row
        && !is_ignorable_heading(heading)
        && is_recognized_command_heading(heading, profile)
    {
        if let Some((end, entries)) = scan_same_indent_entry_table(lines, i, heading_indent) {
            i = end;
            st.command_mode = true;
            st.in_ignorable_section = false;
            let (seen, clean) = emit_subcommands(heading, entries, st.result);
            st.total_entries += seen;
            st.clean_entries += clean;
            return i;
        }
    }

    // Not actually a heading — but if it reads like an
    // introduction to a command list (git's own leading blurb,
    // whose group headings say nothing about "commands"
    // themselves), remember that. See S-070.
    if command_mode_seed(heading, profile) {
        st.command_mode = true;
    }
    // Rewind to just past the original line and continue scanning
    // it as its own candidate.
    i = heading_idx + 1;
    i
}

struct BodyScan<'a> {
    result: &'a mut ParsedHelp,
    command_mode: bool,
    in_ignorable_section: bool,
    obscured_ignorable_indent: Option<(usize, bool)>,
    total_entries: usize,
    clean_entries: usize,
}

fn scan_entries(
    inp: &BodyInput,
    tool_name: Option<&str>,
    mut i: usize,
    result: &mut ParsedHelp,
) -> (usize, usize) {
    let raw = inp.raw;
    let lines = inp.lines;
    let profile = inp.profile;
    let bnf_row_lines = inp.bnf_row_lines;
    // A run of command-group headings is recognized either by its own
    // wording or by being contiguous with an earlier signal — git's own
    // group headings never say "command" themselves, but the leading
    // blurb introducing them does. Seeding from the leading description
    // only, not the whole document, keeps this from lighting up on tar's
    // `--occurrence` description, which mentions "subcommands" in prose
    // describing something else. Deliberately not also seeded from
    // `st.result.usage`: containerd and ctr both write a docopt-style
    // `USAGE: <tool> ... command ...` synopsis, and seeding from that
    // alone turned their unrelated `VERSION:` block into a fabricated
    // subcommand named their own version string. See S-070.
    let command_mode = result
        .description
        .as_deref()
        .is_some_and(|d| command_mode_seed(d, profile));
    // True from the moment an `is_ignorable_heading` heading is captured
    // until a structurally strong block is recognized under a later,
    // non-ignorable heading. Gates [`recover_stanza_head_flag`]: bpftrace's
    // `EXAMPLES:` invocation lines are, line for line, indistinguishable
    // from a genuine LVM stanza head, and used to fabricate `-e`/`-l` flag
    // rows that displaced the real, described ones. Not reset by the
    // generic bare-block fallback the example lines actually take —
    // resetting there would clear the flag on the first example line and
    // reopen the fabrication on the second. See S-071.
    let in_ignorable_section = false;
    // Some hand-written help indents `Examples:` beneath the prose
    // sentence immediately before it, which would otherwise be promoted
    // to a heading and hide the marker from `is_ignorable_heading`
    // entirely. While this is `Some((indent, _))`, the whole region is
    // fenced before any emission path can see its rows; a physical
    // dedent, or an independently attested flag section at the marker's
    // indent or deeper, closes it. Separate from `st.in_ignorable_section`
    // so a fence opened inside an already-suppressed section restores
    // that suppression rather than clearing it. See S-071.
    let obscured_ignorable_indent: Option<(usize, bool)> = None;

    // 3. Section blocks: scan the rest of the output for a heading line
    // followed by more-indented content — or, if the first content already
    // looks like a flag entry, a headingless flags block (sed has none).
    // "Heading" is relative, not "column 0": tar indents its own headings
    // by one space while entries sit at two, so a block is recognized
    // whenever a line is followed by content indented more than it.
    let mut st = BodyScan {
        result,
        command_mode,
        in_ignorable_section,
        obscured_ignorable_indent,
        total_entries: 0usize,
        clean_entries: 0usize,
    };
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        if let Some((marker_indent, prior_ignorable)) = st.obscured_ignorable_indent {
            if obscured_fence_reopens(lines, i, marker_indent) {
                st.obscured_ignorable_indent = None;
                st.in_ignorable_section = prior_ignorable;
            } else {
                i += 1;
                continue;
            }
        }
        // Headingless flags block: sed has no Options: heading at all; the
        // current line already looks like a flag entry, so it is scanned
        // in place. See S-052.
        if looks_like_flag_start(line.trim_start()) {
            // The row may still be one `split_shared_heading_rows`
            // recovered from a `:=` production the engine never
            // revisited as a heading — dcb and vdpa's `OPTIONS` row.
            // See S-042, noted as `bnf_row_lines`.
            let heading_is_bnf = bnf_row_lines.contains(&i);
            let (end, entries, packed, argfile_entry) = scan_flags_block(lines, i, heading_is_bnf);
            i = end;
            if packed {
                let seen = entries.len();
                emit_packed_flags(
                    None,
                    entries.into_iter().map(|(s, d, _)| (s, d)).collect(),
                    st.result,
                );
                st.total_entries += seen;
                st.clean_entries += seen;
            } else {
                let (seen, clean) = emit_flags(None, entries, st.result);
                st.total_entries += seen;
                st.clean_entries += clean;
            }
            if let Some(entry) = argfile_entry {
                st.total_entries += 1;
                st.clean_entries += 1;
                emit_argfile_flag(None, entry, st.result);
            }
            st.command_mode = false;
            continue;
        }

        // Headingless invocation table (spec §7 Tier B): btrfs's command
        // table sits directly under a blank line, never introduced by a
        // heading. Requires every row to start with the tool's own name
        // at a word boundary as the positive evidence a heading would
        // otherwise supply. See S-016.
        if let Some(tool_name) = tool_name {
            if starts_with_tool_name(line.trim_start(), tool_name) {
                if let Some((end, nodes, seen, clean)) =
                    scan_headingless_invocation_table(lines, i, tool_name, raw)
                {
                    i = end;
                    st.total_entries += seen;
                    st.clean_entries += clean;
                    for node in nodes {
                        st.result.try_push_subcommand(node);
                    }
                    st.command_mode = false;
                    continue;
                }
            }
        }

        let heading_indent = leading_whitespace(line);
        let heading = line.trim().to_string();
        let heading_idx = i;
        if is_prose_sentence(&heading) {
            let mut marker_idx = heading_idx + 1;
            while marker_idx < lines.len() && lines[marker_idx].trim().is_empty() {
                marker_idx += 1;
            }
            if marker_idx < lines.len()
                && leading_whitespace(lines[marker_idx]) > heading_indent
                && is_obscured_fence_marker(lines[marker_idx].trim())
            {
                st.obscured_ignorable_indent = Some((
                    leading_whitespace(lines[marker_idx]),
                    st.in_ignorable_section,
                ));
                st.in_ignorable_section = true;
                st.command_mode = false;
                i = marker_idx + 1;
                continue;
            }
        }
        if is_ignorable_heading(&heading) {
            st.in_ignorable_section = true;
        }
        // A hard-wrapped prose sentence, whose second physical line the
        // indentation-alone heading rule would otherwise hand to the
        // flags scanner. Fenced whole.
        if let Some(end) = wrapped_prose_region_end(lines, heading_idx) {
            i = end;
            continue;
        }
        i += 1;
        while i < lines.len() && lines[i].trim().is_empty() {
            i += 1;
        }
        if i >= lines.len() || leading_whitespace(lines[i]) <= heading_indent {
            let h = Heading {
                line,
                heading,
                heading_indent,
                heading_idx,
            };
            i = emit_flush_heading(lines, profile, &h, i, &mut st);
            continue;
        }
        let h = Heading {
            line,
            heading,
            heading_indent,
            heading_idx,
        };
        i = emit_heading_block(inp, tool_name, &h, i, &mut st);
    }
    (st.total_entries, st.clean_entries)
}

/// [`parse_with_profile`]'s engine, over text whose shared heading rows
/// have already been split out. `bnf_row_lines` records which row lines
/// came from a `:=` BNF production rather than an ordinary column-gap
/// heading, keyed on the row rather than the heading beside it.
fn parse_body(
    raw: &str,
    profile: Option<&FrameworkProfile>,
    tool_name: Option<&str>,
    bnf_row_lines: &std::collections::HashSet<usize>,
) -> ParsedHelp {
    let lines: Vec<&str> = raw.lines().collect();
    let mut result = ParsedHelp::default();

    // Some tools answer `--help` with their man page rather than a help
    // summary — git bisect renders GIT-BISECT(1) in full, and feeding
    // that to this grammar fabricates subcommands from DESCRIPTION prose.
    // Man pages are Tier D's job (not yet implemented), so the honest
    // outcome here is no structure at all, rendered verbatim by the
    // caller (spec §7 Tier B step 3). See S-066.
    if looks_like_man_page(&lines) {
        return result;
    }

    let mut i = 0;
    // Physical usage lines (one string per source line, pre-join), kept
    // alive past the block below so the deferred `extract_usage_flags`
    // call can read the same per-line shape `extract_positionals` does.
    let mut usage_lines: Vec<String> = Vec::new();
    // 1. Usage block: one or more logical entries — each a `usage:`/
    // `or:`/own-name line plus whatever continues it. `usage_lines` stays
    // one string per physical line (feeds the [M-15] synopsis flag
    // grammar); `usage_entries` is the joined display/verbatim form
    // (`result.usage`).
    //
    // A line starts a new entry, regardless of indentation, when it is
    // itself a `usage:`/`Usage:` line, starts with the `or:` marker, or
    // begins with the tool's own name at a word boundary. Anything else
    // is a continuation, unless it ends the block.
    //
    // Indentation alone cannot decide "continuation vs. block end": git's
    // wrap sits more indented than `usage:`, lsof's sits at the same
    // indent as its marker, and du's trailing prose sentence sits there
    // too, yet must still end the block. Only content shape tells these
    // apart: more indented is always a continuation; at or below base
    // indent, a line continues only if it reads as usage grammar (a
    // docopt delimiter `[`, `<`, `{`). See S-037. Joined fragments are
    // separated by a single space, not re-flowed (spec §7).
    //
    // Entry point, tried in this order: (1) an ordinary `usage:`/`Usage:`
    // line anywhere; (2) the C fprintf idiom, nfsidmap's `nfsidmap:
    // Usage: nfsidmap [-vh] ...` (S-001); (3) only when neither appears,
    // an unlabelled synopsis bounded to the lines before the document's
    // real body starts.
    let labelled_usage_start = lines.iter().position(|l| {
        let t = l.trim_start();
        starts_with_usage_prefix(t)
            || tool_name.is_some_and(|name| starts_with_name_prefixed_usage(t, name))
    });
    let unlabelled_synopsis_start = if labelled_usage_start.is_none() {
        tool_name.and_then(|name| {
            let body_start = lines
                .iter()
                .position(|l| {
                    let t = l.trim_start();
                    !t.is_empty() && (looks_like_flag_start(t) || is_section_heading_line(t))
                })
                .unwrap_or(lines.len());
            lines[..body_start].iter().enumerate().position(|(idx, l)| {
                let t = l.trim_start();
                looks_like_unlabeled_synopsis_line(t, name)
                    // LVM's own emitter writes a bare invocation line
                    // (`vgck` alone) with all docopt notation on the rows
                    // that continue it, invisible to
                    // `looks_like_unlabeled_synopsis_line` alone. A bare
                    // own-name line is accepted too, but only when the
                    // next physical line is unambiguous flag-row
                    // evidence. See S-005.
                    || looks_like_bare_synopsis_head(&lines, idx, name)
            })
        })
    } else {
        None
    };
    let usage_start = labelled_usage_start.or(unlabelled_synopsis_start);
    if let Some(start) = usage_start {
        let scan = scan_usage_section(
            &lines,
            start,
            labelled_usage_start,
            tool_name,
            &mut usage_lines,
        );
        i = scan.next_index;
        result.positionals = scan.positionals;
        result.usage = scan.entries;
    }

    // 2. Leading prose before the usage block (or before the first
    // section, if there's no usage block) becomes the description.
    //
    // `leading_prose_bound` is O(lines.len()) and must be computed once
    // here, not inside the loop condition below — re-running it per
    // iteration made this function quadratic, found via the coverage
    // harness (spec §13.1) parsing a degenerate input in minutes instead
    // of milliseconds.
    let description_bound = i.max(leading_prose_bound(&lines));
    if let Some(description) = extract_description(&lines, description_bound, usage_start, i) {
        result.description = Some(description);
    }

    let inp = BodyInput {
        raw,
        lines: &lines,
        usage_lines: &usage_lines,
        profile,
        bnf_row_lines,
    };
    let (total_entries, clean_entries) = scan_entries(&inp, tool_name, i, &mut result);

    // spec [M-15]: mine the usage synopsis for flag spellings too, not just
    // positionals — git's own flags documented only in its usage block
    // (378 of 1,895 `ok` tools fleet-wide had none read at all). Deferred
    // to here so a duplicate spelling can be dropped rather than added a
    // second time. See S-088.
    //
    // Deliberately not `mandible_core::merge_entity_lists`: that function
    // rebuckets every flag in the combined list by identity, which is
    // wrong here since one `Options:`-style block can legitimately list
    // one spelling twice for two forms (du's bare `--time` and valued
    // `--time=WORD`), and rebucketing merged those legitimate pairs and
    // dropped a real description each time. Only a usage-derived flag may
    // be judged redundant; a block-derived flag is never rebucketed,
    // dropped, or altered.
    // Reads `usage_lines` (physical, pre-join), not `result.usage` (the
    // joined display form), deliberately, so the join cannot change what
    // this recovers.
    if !usage_lines.is_empty() {
        for flag in extract_usage_flags(&usage_lines) {
            if result.flags.len() >= MAX_RECOVERED_ENTRIES {
                break;
            }
            if !flag_spelling_already_present(&flag, &result.flags) {
                result.flags.push(flag);
            }
            // else: this spelling already names a flag the block scan
            // recovered, so the usage-derived, always-undescribed
            // duplicate is not added. "Let the described version win"
            // taken literally: the existing entry is never touched.
        }
    }

    // Last, over everything both scans produced: the repeated-character
    // flag repair needs the whole flag list to answer its own question.
    // One pass over the document, shared by both repairs below, since
    // each asks the same glued-and-delimited question per flag. See
    // [`GluedTokenIndex`].
    let glued_tokens = GluedTokenIndex::new(raw);
    repair_repeated_character_flags(&mut result.flags, &glued_tokens);
    // The single-dash long-option repair, ordered after the
    // repeated-character pass: that pass consumes `-vv` and friends, so
    // by the time this one runs the repeated-character family is already
    // gone from the fingerprint the two detectors share.
    repair_single_dash_long_options(&mut result.flags, &glued_tokens);
    // Last because it can only fill what the two above finished naming:
    // descriptions written as free prose paragraphs, not option-table
    // columns.
    backfill_prose_paragraph_descriptions(&mut result.flags, &lines);
    // Last of all: restore a value the single-dash long-option repair
    // cleared, anchored against a run-mate's already-correct value (spec
    // §7's row grammar) — `-help`'s row only qualifies once the repair
    // above has turned it into a single-dash spelling. See S-007.
    result.flags = recover_anchored_values(std::mem::take(&mut result.flags), raw);

    result.confidence = compute_confidence(total_entries, clean_entries, !result.usage.is_empty());
    result
}

/// A ratio computed from an option-table sample of exactly one row is not
/// a measurement. ssh-keygen's final wrapped usage-continuation line
/// (`-n namespace -s signature_file [-r krl_file] [-O option]`) opens
/// with a dash and gets handed to the flags-block scanner the same way
/// curl's real 13-flag table does, but here there is nothing real to
/// find — one unclean row reads as a confident `0 / 1`, indistinguishable
/// from a real bad parse.
///
/// Folded into a dedicated `0.5` fallback, not the zero-row fallback's
/// `had_usage ? 0.5 : 0.15`: that penalty exists because no structure and
/// no usage line is a stronger bad-parse signal than no structure but a
/// usage line present, which does not apply when real structure — just
/// not enough of it to divide by — was found. Not atlas-worthy (confidence
/// calibration, not a text shape); measured against 2,301 frozen captures,
/// not dated.
const SINGLE_ROW_SAMPLE_CONFIDENCE: f32 = 0.5;

fn compute_confidence(total_entries: usize, clean_entries: usize, had_usage: bool) -> f32 {
    if total_entries == 1 {
        return SINGLE_ROW_SAMPLE_CONFIDENCE;
    }
    if total_entries == 0 {
        return if had_usage { 0.5 } else { 0.15 };
    }
    (clean_entries as f32 / total_entries as f32).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- compute_confidence's one-row-sample fallback -------------------

    /// Pins all four `total_entries == 1` combinations. A single row is
    /// uninformative regardless of clean/dirty or usage-line presence, so
    /// all four land at `SINGLE_ROW_SAMPLE_CONFIDENCE`.
    #[test]
    fn a_single_row_sample_is_uninformative_regardless_of_usage_or_cleanliness() {
        assert_eq!(compute_confidence(1, 0, true), SINGLE_ROW_SAMPLE_CONFIDENCE);
        assert_eq!(compute_confidence(1, 1, true), SINGLE_ROW_SAMPLE_CONFIDENCE);
        assert_eq!(
            compute_confidence(1, 0, false),
            SINGLE_ROW_SAMPLE_CONFIDENCE
        );
        assert_eq!(
            compute_confidence(1, 1, false),
            SINGLE_ROW_SAMPLE_CONFIDENCE
        );
    }

    /// The zero-row fallback is untouched by the one-row fix: it still
    /// carries its own usage-line penalty (long-standing, calibrated
    /// behavior, not this fix's to move).
    #[test]
    fn a_zero_row_sample_keeps_its_usage_line_penalty() {
        assert_eq!(compute_confidence(0, 0, true), 0.5);
        assert_eq!(compute_confidence(0, 0, false), 0.15);
    }

    /// A real sample (two or more rows) is untouched: still a plain
    /// ratio, not folded into either fallback. `find`'s real shape (19
    /// rows, 2 clean) must keep dividing.
    #[test]
    fn a_two_or_more_row_sample_still_divides() {
        assert!((compute_confidence(19, 2, true) - (2.0 / 19.0)).abs() < 1e-6);
        assert_eq!(compute_confidence(4, 4, false), 1.0);
        assert_eq!(compute_confidence(4, 0, false), 0.0);
    }

    #[test]
    fn tar_main_operation_mode_group_recovered() {
        let parsed = parse(TAR_HELP);
        let create = parsed.flags.iter().find(|f| f.long() == Some("create"));
        assert!(
            create.is_some(),
            "expected --create among {:?}",
            parsed.flags.iter().map(|f| f.long()).collect::<Vec<_>>()
        );
        assert_eq!(
            create.unwrap().group.as_deref(),
            Some("Main operation mode:")
        );
    }

    #[test]
    fn tar_flag_with_short_and_description() {
        let parsed = parse(TAR_HELP);
        let create = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("create"))
            .unwrap();
        assert_eq!(create.short(), Some('c'));
        assert!(create
            .description
            .as_ref()
            .unwrap()
            .as_str()
            .contains("create a new archive"));
    }

    #[test]
    fn tar_multiline_description_is_joined() {
        let parsed = parse(TAR_HELP);
        let occurrence = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("occurrence"))
            .unwrap();
        let desc = occurrence.description.as_ref().unwrap().as_str();
        assert!(desc.contains("NUMBERth occurrence"), "{desc:?}");
        assert!(
            desc.contains("conjunction with one of the subcommands"),
            "{desc:?}"
        );
    }

    /// A long-only flag indented deeper than its short-form siblings must
    /// still be recovered as its own flag.
    #[test]
    fn tar_long_only_flag_at_deeper_indent_is_its_own_flag() {
        let parsed = parse(TAR_HELP);
        let delete = parsed.flags.iter().find(|f| f.long() == Some("delete"));
        assert!(
            delete.is_some(),
            "expected --delete among {:?}",
            parsed.flags.iter().map(|f| f.long()).collect::<Vec<_>>()
        );
        assert_eq!(
            delete.unwrap().description.as_ref().unwrap().as_str(),
            "delete from the archive (not on mag tapes!)"
        );
    }

    /// A short-form flag after a run of long-only flags at a shallower
    /// indent (tar's `-m, --touch`) must not be misread as a dedent back
    /// to heading level.
    #[test]
    fn tar_short_flag_after_long_only_run_is_recovered() {
        let parsed = parse(TAR_HELP);
        let touch = parsed.flags.iter().find(|f| f.long() == Some("touch"));
        assert!(
            touch.is_some(),
            "expected --touch among {:?}",
            parsed.flags.iter().map(|f| f.long()).collect::<Vec<_>>()
        );
        assert_eq!(touch.unwrap().short(), Some('m'));
    }

    #[test]
    fn tar_examples_section_does_not_produce_fake_subcommands() {
        let parsed = parse(TAR_HELP);
        assert!(
            !parsed.subcommands.iter().any(|c| c.name == "tar"),
            "Examples: section should not produce a fake 'tar' subcommand: {:?}",
            parsed
                .subcommands
                .iter()
                .map(|c| &c.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn tar_has_reasonable_confidence() {
        let parsed = parse(TAR_HELP);
        assert!(
            parsed.confidence > 0.5,
            "confidence was {}",
            parsed.confidence
        );
    }

    /// tar has no `Commands:`-shaped heading anywhere, so the only
    /// correct answer is zero subcommands. See S-013.
    #[test]
    fn tar_produces_zero_subcommands() {
        let parsed = parse(TAR_HELP);
        assert!(
            parsed.subcommands.is_empty(),
            "expected zero subcommands, got {:?}",
            parsed
                .subcommands
                .iter()
                .map(|c| &c.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn empty_input_yields_low_confidence_and_no_panic() {
        let parsed = parse("");
        assert!(parsed.confidence < 0.5);
        assert!(parsed.flags.is_empty());
    }

    /// dd/less/sed/find have no real subcommands; each carries one of the
    /// specific shapes that used to fabricate them. See S-013.
    #[test]
    fn dd_less_sed_find_all_produce_zero_subcommands() {
        for (name, raw) in [
            ("dd", DD_HELP),
            ("less", LESS_HELP),
            ("sed", SED_HELP),
            ("find", FIND_HELP),
        ] {
            let parsed = parse(raw);
            assert!(
                parsed.subcommands.is_empty(),
                "{name}: expected zero subcommands, got {:?}",
                parsed
                    .subcommands
                    .iter()
                    .map(|c| &c.name)
                    .collect::<Vec<_>>()
            );
        }
    }

    /// A multi-byte character positioned so byte offset 6 falls inside it
    /// used to panic `&t[..6]` with "not a char boundary"; must degrade
    /// gracefully instead.
    #[test]
    fn multibyte_characters_near_the_start_of_output_do_not_panic() {
        // U+2588 ('█') spans bytes 5..8, so byte offset 6 falls inside it.
        let raw =
            "12345█ some line\nUsage: weirdtool [OPTIONS]\n\n  -x, --example   an example flag\n";
        let parsed = parse(raw); // must not panic
        assert!(
            !parsed.usage.is_empty(),
            "should still recover the usage line: {parsed:?}"
        );
    }

    #[test]
    fn multibyte_characters_with_no_usage_line_do_not_panic() {
        // Also exercise the path with no "usage:" line at all, and with
        // multi-byte content positioned at various points.
        let raw = "日本語のヘルプ出力\n\n  --flag   description\n";
        let parsed = parse(raw); // must not panic
        let _ = parsed;
    }

    /// instmodsh's own repeated banner recovered 58,663 duplicate-name
    /// subcommands before this cap. Same-named entries must collapse to
    /// one, and the total accepted must stay bounded. See S-072.
    #[test]
    fn repeated_identical_banner_does_not_explode_into_duplicate_subcommands() {
        // Non-blocking timing signal only (docs/design.md decision D3): wall-clock
        // on shared hardware is a statement about the machine, not the
        // parser, so it never flips a correctness gate. `TIMING_BUDGET` is
        // set well above every observed run so only a genuine O(n^2)
        // regression prints a warning. The real assertion is the
        // subcommand count below.
        const TIMING_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);

        let block = "Available commands are:\n   l            - List all installed modules\n   q            - Quit the program\n\n";
        let raw = block.repeat(20_000);
        let start = std::time::Instant::now();
        let parsed = parse(&raw);
        let elapsed = start.elapsed();
        if elapsed > TIMING_BUDGET {
            eprintln!(
                "warning: parsing a repetitive input took {elapsed:?}, exceeding the \
                 {TIMING_BUDGET:?} non-blocking budget — likely a real regression rather \
                 than machine noise; investigate before dismissing"
            );
        }
        assert_eq!(
            parsed.subcommands.len(),
            2,
            "expected exactly one 'l' and one 'q', got {:?}",
            parsed
                .subcommands
                .iter()
                .map(|c| &c.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn entry_recovery_is_capped_even_when_every_entry_is_distinct() {
        let mut raw = String::from("Commands:\n");
        for i in 0..(MAX_RECOVERED_ENTRIES + 500) {
            raw.push_str(&format!("   cmd{i}   does a thing\n"));
        }
        let parsed = parse(&raw);
        assert!(
            parsed.subcommands.len() <= MAX_RECOVERED_ENTRIES,
            "got {} subcommands, expected at most {}",
            parsed.subcommands.len(),
            MAX_RECOVERED_ENTRIES
        );
    }

    #[test]
    fn is_command_name_shaped_rejects_prose_and_placeholders() {
        assert!(is_command_name_shaped("commit"));
        assert!(is_command_name_shaped("http-push"));
        assert!(is_command_name_shaped("sha3-256"));
        assert!(!is_command_name_shaped("treat them as errors"));
        assert!(!is_command_name_shaped("BYTES"));
        assert!(!is_command_name_shaped(""));
        assert!(!is_command_name_shaped("42start"));
    }

    // --- issue #77: obscured-`Examples:` fence edges ---------------------

    /// Edge 1: a positively-attested flag section indented deeper than
    /// the obscured marker must still be recoverable — the fence's only
    /// historical exits were a dedent or a section at exactly the
    /// marker's indent. See S-071.
    #[test]
    fn obscured_fence_admits_a_deeper_attested_flag_section() {
        let raw = "\
This tool does many useful things for the busy user.
  Examples:
    widget run --now
      runs immediately without prompting
    widget stop
      halts all processing at once
    Options:
      --now      run immediately without prompting
      --stop     halt all processing at once
";
        let parsed = parse(raw);
        let names: Vec<_> = parsed.flags.iter().map(|f| f.long()).collect();
        assert!(
            parsed.flags.iter().any(|f| f.long() == Some("now")),
            "expected --now among {names:?}"
        );
        assert!(
            parsed.flags.iter().any(|f| f.long() == Some("stop")),
            "expected --stop among {names:?}"
        );
    }

    /// Edge 1: the same widened exit admits a headingless flag block — no
    /// heading vocabulary is possible, so the row-count floor alone is
    /// the evidence. See S-071.
    #[test]
    fn obscured_fence_admits_a_deeper_headingless_flag_block() {
        let raw = "\
This tool does many useful things for the busy user.
  Examples:
    widget run --now
      runs immediately without prompting
    --now      run immediately without prompting
    --stop     halt all processing at once
";
        let parsed = parse(raw);
        assert!(
            parsed.flags.iter().any(|f| f.long() == Some("now")),
            "expected --now among {:?}",
            parsed.flags.iter().map(|f| f.long()).collect::<Vec<_>>()
        );
        assert!(
            parsed.flags.iter().any(|f| f.long() == Some("stop")),
            "expected --stop among {:?}",
            parsed.flags.iter().map(|f| f.long()).collect::<Vec<_>>()
        );
    }

    /// Edge 2: the fence's close must restore whatever `in_ignorable_section`
    /// held before it opened, not clear it outright — a real outer
    /// `EXAMPLES:` heading's own suppression must survive an inner
    /// obscured fence closing on a dedent. See S-071.
    #[test]
    fn obscured_fence_close_restores_rather_than_clears_suppression() {
        let raw = "\
EXAMPLES:

This tool does many useful things for the busy user.
  Examples:
widget -x
  run example x
";
        let parsed = parse_named(raw, "widget");
        assert!(
            !parsed.flags.iter().any(|f| f.short() == Some('x')),
            "fence close must not fabricate -x from the example: {:?}",
            parsed
                .flags
                .iter()
                .map(|f| (f.short(), f.long()))
                .collect::<Vec<_>>()
        );
    }

    /// Edge 3: the obscured-marker fence trigger must be stricter than
    /// `is_ignorable_heading` — heading-shaped and colon-terminated, not
    /// merely "starts with 'example'". A mid-document `Report bugs to
    /// <address>.` sentence must never open the whole-region fence. See
    /// S-071.
    #[test]
    fn obscured_fence_trigger_requires_heading_shape() {
        let raw = "\
This tool supports many commands for the busy user.
  Report bugs to <maintainer@example.com>.
  start   begin processing
  stop    end processing
";
        let parsed = parse(raw);
        let names: Vec<_> = parsed.subcommands.iter().map(|c| &c.name).collect();
        assert!(
            parsed.subcommands.iter().any(|c| c.name == "start"),
            "expected 'start' among {names:?}"
        );
        assert!(
            parsed.subcommands.iter().any(|c| c.name == "stop"),
            "expected 'stop' among {names:?}"
        );
    }

    #[test]
    fn mentions_commands_word_matches_whole_word_only() {
        assert!(mentions_commands_word("Commands:"));
        assert!(mentions_commands_word("Available Commands:"));
        assert!(mentions_commands_word(
            "These are common Git commands used in various situations:"
        ));
        assert!(mentions_commands_word("SUBCOMMANDS"));
        // "recommends" contains the substring "commands" but is not the
        // word "commands" — must not false-positive on substring match.
        assert!(!mentions_commands_word("This tool recommends caution."));
    }

    // --- the "operations" heading vocabulary (llvm-ar operations table) ---

    #[test]
    fn mentions_operations_word_matches_whole_word_only() {
        assert!(mentions_operations_word("OPERATIONS:"));
        assert!(mentions_operations_word("Main operation modes:"));
        // "operational" contains the substring "operation" but is not the
        // word "operation" — must not false-positive on substring match.
        assert!(!mentions_operations_word("Operational readiness report:"));
    }

    /// llvm-ar's `OPERATIONS:` table, byte-shaped from
    /// corpus/llvm-ar-18/18.1.3. See S-022.
    #[test]
    fn llvm_ar_operations_table_recovered_as_subcommands() {
        let raw = concat!(
            "USAGE: llvm-ar [options] [-]<operation>[modifiers] [relpos] [count] <archive> [files]\n",
            "\n",
            "OPERATIONS:\n",
            "  d - delete [files] from the archive\n",
            "  m - move [files] in the archive\n",
            "  p - print contents of [files] found in the archive\n",
            "  q - quick append [files] to the archive\n",
            "  r - replace or insert [files] into the archive\n",
            "  s - act as ranlib\n",
            "  t - display list of files in archive\n",
            "  x - extract [files] from the archive\n",
        );
        let parsed = parse_named(raw, "llvm-ar");
        let names: Vec<&str> = parsed.subcommands.iter().map(|c| c.name.as_str()).collect();
        for op in ["d", "m", "p", "q", "r", "s", "t", "x"] {
            assert!(names.contains(&op), "missing operation {op:?}: {names:?}");
        }
        let delete = parsed.subcommands.iter().find(|c| c.name == "d").unwrap();
        assert_eq!(
            delete.summary.as_ref().map(|t| t.as_str()),
            Some("delete [files] from the archive")
        );
        // heading_attested since the heading text itself said
        // "OPERATIONS:", which spec §6 rule 0 gates probe eligibility on.
        assert!(
            parsed.subcommands.iter().all(|c| c.heading_attested),
            "an operation letter under a recognized OPERATIONS: heading \
             must be heading_attested: {:?}",
            parsed.subcommands
        );
    }

    /// mount's own `Operations:` heading introduces an ordinary flags
    /// table false positive: `flags_block_start` must claim it before
    /// `is_recognized_command_heading` is ever consulted. See S-022.
    #[test]
    fn flags_shaped_operations_heading_yields_no_subcommands() {
        let raw = concat!(
            "Operations:\n",
            " -B, --bind              mount a subtree somewhere else (same as -o bind)\n",
            " -M, --move              move a subtree to some other place\n",
            " -R, --rbind             mount a subtree and all submounts somewhere else\n",
        );
        let parsed = parse_named(raw, "mount");
        assert!(
            parsed.subcommands.is_empty(),
            "a flags table under an 'Operations:' heading must not become \
             subcommands: {:?}",
            parsed
                .subcommands
                .iter()
                .map(|c| c.name.clone())
                .collect::<Vec<_>>()
        );
        for spelling in ["bind", "move", "rbind"] {
            assert!(
                parsed.flags.iter().any(|f| f.long() == Some(spelling)),
                "missing --{spelling} in {:?}",
                parsed.flags
            );
        }
    }
}
