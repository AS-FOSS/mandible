//! Section headings: telling one from prose or a wrapped continuation,
//! splitting a heading that shares its line with a row, recognizing a
//! word grid or a man page, and naming the group a block's entries carry.

use super::*;

/// Rewrite a line carrying a section heading **and** its first row into
/// two lines: `uconv`/`zipinfo`'s heading-then-row column shape (S-062),
/// and `ip`/iproute2's BNF `label := { ... }` shape (S-042), where `:=`
/// substitutes for the column gap. Requires spaces-only indentation, an
/// [`is_section_heading_line`]-shaped label, a gap or `:=`+bracket after
/// it, and the remainder [`looks_like_flag_start`] — that last clause
/// keeps an ordinary "label, then a value" line intact.
///
/// The returned [`HashSet`] names row lines recovered via the `:=`
/// clause only — the signal [`split_bnf_alternation_row`] needs once the
/// operator itself is stripped. Keyed on the row, not the heading, since
/// a one-line production is never revisited as its own heading. See
/// docs/shapes.md S-062, S-042.
pub(super) fn split_shared_heading_rows(
    raw: &str,
) -> Option<(String, std::collections::HashSet<usize>)> {
    let mut out = String::new();
    let mut split_any = false;
    let mut bnf_row_lines = std::collections::HashSet::new();
    let mut out_line_no = 0usize;
    for line in raw.lines() {
        match split_shared_heading_row(line) {
            Some((heading, row, is_bnf)) => {
                split_any = true;
                if is_bnf {
                    bnf_row_lines.insert(out_line_no + 1);
                }
                out.push_str(&heading);
                out.push('\n');
                out.push_str(&row);
                out.push('\n');
                out_line_no += 2;
            }
            None => {
                out.push_str(line);
                out.push('\n');
                out_line_no += 1;
            }
        }
    }
    split_any.then_some((out, bnf_row_lines))
}

/// One line's worth of [`split_shared_heading_rows`]: the heading line and
/// the row line it was carrying, the row re-indented to the column it
/// occupied in the original so the block below reads the same alignment it
/// always did.
///
/// Char-indexed throughout, never a byte-offset `&str` slice — AGENTS.md's
/// rule against slicing captured tool output at a raw byte offset.
pub(super) fn split_shared_heading_row(line: &str) -> Option<(String, String, bool)> {
    let chars: Vec<char> = line.chars().collect();
    let indent = chars.iter().take_while(|c| c.is_whitespace()).count();
    if chars[..indent].iter().any(|c| *c != ' ') {
        return None;
    }
    let colon = chars.iter().position(|c| *c == ':')?;
    if colon <= indent {
        return None;
    }
    let label: String = chars[indent..=colon].iter().collect();
    if !is_section_heading_line(&label) || starts_with_usage_prefix(&label) {
        return None;
    }
    let mut row_start = colon + 1;
    // A BNF definition operator: the colon reads as `:=`. See this
    // function's doc comment (S-042) for why the operator, not merely a
    // bracket, is what widens clause 3.
    let has_bnf_operator = chars.get(row_start) == Some(&'=');
    if has_bnf_operator {
        row_start += 1;
    }
    let gap_start = row_start;
    while row_start < chars.len() && chars[row_start] == ' ' {
        row_start += 1;
    }
    let gap_spaces = row_start - gap_start;
    if has_bnf_operator {
        if gap_spaces == 0 || row_start >= chars.len() {
            return None;
        }
        // An optional opening bracket the grammar wraps its row in
        // (`ip`'s `{`, `dcb`'s `[`), skipped along with the space after it.
        if matches!(chars.get(row_start), Some('{') | Some('[') | Some('(')) {
            row_start += 1;
            let bracket_gap_start = row_start;
            while row_start < chars.len() && chars[row_start] == ' ' {
                row_start += 1;
            }
            if row_start - bracket_gap_start == 0 || row_start >= chars.len() {
                return None;
            }
        }
    } else if gap_spaces < MIN_COLUMN_GAP_SPACES || row_start >= chars.len() {
        return None;
    }
    let row: String = chars[row_start..].iter().collect();
    if !looks_like_flag_start(&row) {
        return None;
    }
    let heading: String = chars[..=colon].iter().collect();
    let mut row_line = " ".repeat(row_start);
    row_line.push_str(&row);
    Some((heading, row_line, has_bnf_operator))
}

/// Fewest whitespace-separated words a period-terminated single-field
/// line must carry before [`is_prose_sentence`] reads it as a sentence.
/// Five: the shortest real specimen is `getent`'s five-word sentence,
/// while the shortest heading this must never claim is two or three
/// words. See docs/shapes.md S-011.
pub(super) const MIN_PROSE_SENTENCE_WORDS: usize = 5;

/// True when `heading` is an English sentence rather than a section
/// heading: no column gap, terminated by a full stop (not a colon,
/// so a prose-shaped heading like `gcc`'s is left alone), at least
/// [`MIN_PROSE_SENTENCE_WORDS`] words, and not a trailing ellipsis
/// (docopt repetition notation, not a terminator). Prevents a preamble
/// sentence before an indented option table from being misread as the
/// block's heading. See docs/shapes.md S-011.
///
/// Also fences a prose line followed by `is_obscured_fence_marker`
/// (`obscured_ignorable_indent`): its close restores
/// `in_ignorable_section` to what it held before opening, rather than
/// clearing it, so it can't cancel a suppression a real `EXAMPLES:`
/// heading established earlier.
pub(super) fn is_prose_sentence(heading: &str) -> bool {
    let trimmed = heading.trim_end();
    if !trimmed.ends_with('.') || trimmed.ends_with("...") {
        return false;
    }
    if trimmed.split_whitespace().count() < MIN_PROSE_SENTENCE_WORDS {
        return false;
    }
    find_multi_space_gap(heading).is_none()
}

/// True when `heading` is the first half of a backslash-continued
/// logical line, and so cannot be a heading of anything — the same
/// indentation-alone misreading [`is_prose_sentence`] documents, reached
/// via a synopsis wrapped with a shell continuation marker instead of a
/// sentence (`update-xmlcatalog`'s synopsis). Only suppresses `group`,
/// same as [`is_prose_sentence`]. See docs/shapes.md S-011 and
/// corpus/update-xmlcatalog.
pub(super) fn is_line_continuation_fragment(heading: &str) -> bool {
    heading.trim_end().ends_with('\\')
}

/// True when `heading` may be copied into a recovered entry's `group`.
/// The one predicate the three group-assigning call sites share.
/// Subtractive only: a line either reads as positively not a heading, or
/// is left exactly as it was.
pub(super) fn heading_can_name_a_group(heading: &str) -> bool {
    !is_prose_sentence(heading)
        && !is_line_continuation_fragment(heading)
        && !is_dash_underline_row(heading)
}

/// True when `line`, trimmed, is nothing but dash characters and
/// whitespace — a table's own column-underline decoration (jmod's
/// `Option`/`Description` header row: `------  -----------`), not a real
/// heading. Applies [`super::grammar::is_dash_underline_token`] to every
/// whitespace-delimited run, since a two-column underline row is two
/// such runs; keeps the row's literal dashes out of
/// `Flag::group`/`CommandNode::group` once it stops being read as a
/// flag. See docs/shapes.md S-063.
pub(super) fn is_dash_underline_row(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty() && trimmed.split_whitespace().all(is_dash_underline_token)
}

/// True when `line`, trimmed, is a decorative section-divider heading
/// with no trailing colon: a dash run, then a plain-word label, then
/// another dash run — `tree --help`'s `------- Listing options -------`.
/// [`is_section_heading_line`] requires a trailing colon and cannot see
/// this shape; without this guard the row instead folds into the usage
/// synopsis's continuation and its `-------` token gets mined as a
/// fabricated flag. The label must be non-empty and plain-word shaped
/// (same class as [`is_section_heading_line`]'s), so a synopsis fragment
/// merely starting and ending with a dash isn't mistaken for it. See
/// docs/shapes.md S-064.
pub(super) fn looks_like_dash_bracketed_heading(line: &str) -> bool {
    let trimmed = line.trim();
    let Some(rest) = trimmed
        .split_once(char::is_whitespace)
        .map(|(head, tail)| (head, tail.trim()))
        .filter(|(head, _)| is_dash_underline_token(head))
    else {
        return false;
    };
    let (_, tail) = rest;
    let Some(label) = tail
        .rsplit_once(char::is_whitespace)
        .filter(|(_, last)| is_dash_underline_token(last))
        .map(|(head, _)| head.trim())
    else {
        return false;
    };
    !label.is_empty()
        && label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_')
}

/// Index just past a hard-wrapped prose sentence opening at `head`, or
/// `None` when that line does not open one. A paragraph that hard-wraps
/// with a hanging indent puts a more-indented line beneath an ordinary
/// sentence, so the scanner reads the sentence as a heading and the rest
/// as its flags block when the wrap lands on a dash-led word (`dpkg`'s
/// cross-reference sentence naming `dpkg-deb`'s own flags). Neither
/// [`is_prose_sentence`] (needs a full stop; the line isn't finished
/// yet) nor [`is_line_continuation_fragment`] (needs a backslash) can
/// see this, and suppressing `group` alone would leave the fabricated
/// flag behind — so this fences the whole region instead.
///
/// The head line must end with a comma, be a single field
/// ([`find_multi_space_gap`]), and be at least
/// [`MIN_PROSE_SENTENCE_WORDS`] words; the immediately next line (no
/// intervening blank) must be indented further and also a single field.
/// The region then runs until a blank line, a dedent, or an aligned
/// column — which keeps a real option table's rows from ever being
/// swallowed. See docs/shapes.md S-011 and corpus/dpkg.
pub(super) fn wrapped_prose_region_end(lines: &[&str], head: usize) -> Option<usize> {
    let head_line = lines.get(head)?;
    let trimmed = head_line.trim_end();
    if !trimmed.ends_with(',') {
        return None;
    }
    if trimmed.split_whitespace().count() < MIN_PROSE_SENTENCE_WORDS {
        return None;
    }
    if find_multi_space_gap(head_line).is_some() {
        return None;
    }
    let head_indent = leading_whitespace(head_line);
    let mut end = head + 1;
    while let Some(line) = lines.get(end) {
        if line.trim().is_empty()
            || leading_whitespace(line) <= head_indent
            || find_multi_space_gap(line).is_some()
        {
            break;
        }
        end += 1;
    }
    (end > head + 1).then_some(end)
}

/// True when `lines[idx]` is a dash-led line continuing the prose above it
/// rather than opening a new flag row (atlas S-027,
/// `corpus/zgrep/1.12`, `corpus/resolvconf/255.4`). Same-indent counterpart
/// of [`wrapped_prose_region_end`]. Gates: prev line non-blank, not
/// sentence-final, prose (cascades — a predecessor this rule already
/// accepted counts as prose too) and at least [`MIN_PROSE_SENTENCE_WORDS`]
/// long when not itself dash-led (tar's four-word `*This* tar defaults to:`
/// introduces real flag rows, not a wrap), cur has no description column,
/// cur's indent equals prev's.
pub(super) fn is_wrapped_prose_continuation(lines: &[&str], idx: usize) -> bool {
    if idx == 0 {
        return false;
    }
    let cur = lines[idx];
    let prev = lines[idx - 1];
    if prev.trim().is_empty() {
        return false;
    }
    if prev.trim_end().ends_with(['.', '!', '?']) {
        return false;
    }
    if prev.trim_start().starts_with('-') {
        if !is_wrapped_prose_continuation(lines, idx - 1) {
            return false;
        }
    } else if prev.split_whitespace().count() < MIN_PROSE_SENTENCE_WORDS {
        return false;
    }
    if find_multi_space_gap(cur).is_some() {
        return false;
    }
    leading_whitespace(cur) == leading_whitespace(prev)
}

/// The full prose paragraph a rescued wrapped-prose continuation at
/// `flag_line_idx` belongs to (S-027): back to the blank line above it (or
/// the document start), forward through every line
/// [`is_wrapped_prose_continuation`] still accepts, up to the next blank
/// line. Returns the index just past the paragraph and its text, physical
/// lines trimmed and joined by single spaces — so the sentence the
/// fabricated flags were mined from still reaches the description instead
/// of vanishing with them (AGENTS.md §3.9).
pub(super) fn wrapped_prose_paragraph(lines: &[&str], flag_line_idx: usize) -> (usize, String) {
    let mut start = flag_line_idx;
    while start > 0 && !lines[start - 1].trim().is_empty() {
        start -= 1;
    }
    let mut end = flag_line_idx + 1;
    while end < lines.len() {
        let line = lines[end];
        if line.trim().is_empty() {
            break;
        }
        if looks_like_flag_start(line.trim_start()) && !is_wrapped_prose_continuation(lines, end) {
            break;
        }
        end += 1;
    }
    let text = lines[start..end]
        .iter()
        .map(|l| l.trim())
        .collect::<Vec<_>>()
        .join(" ");
    (end, text)
}

/// True when `heading` may open the obscured-marker whole-region fence
/// (`obscured_ignorable_indent`). [`is_ignorable_heading`] alone is too
/// loose for a whole-region trigger: a mid-document `Report bugs to
/// <maintainer@example.com>.` sentence would fence everything after it
/// until the next dedent. Adds [`is_section_heading_line`]'s bar (short,
/// colon-terminated, plain-word label) on top, so only something
/// heading-shaped as well as heading-worded opens the fence.
/// `is_ignorable_heading` itself stays untouched for its other call
/// sites.
pub(super) fn is_obscured_fence_marker(heading: &str) -> bool {
    is_section_heading_line(heading) && is_ignorable_heading(heading)
}

/// Whether the obscured-marker fence (`obscured_ignorable_indent`) may
/// close at `lines[idx]`, given the marker's own indent. Widens which
/// indents may exit while keeping the exit itself evidence-gated: a
/// physical dedent always exits; [`starts_attested_flag_section`] now
/// qualifies at the marker's indent or deeper, not only exactly at it;
/// and a headingless run of at least [`MIN_ATTESTED_SECTION_FLAGS`] flag
/// rows ([`starts_attested_headingless_flag_block`]) is admitted too,
/// since it can never satisfy a heading-vocabulary test in the first
/// place.
pub(super) fn obscured_fence_reopens(lines: &[&str], idx: usize, marker_indent: usize) -> bool {
    let indent = leading_whitespace(lines[idx]);
    if indent < marker_indent {
        return true;
    }
    starts_attested_flag_section(lines, idx) || starts_attested_headingless_flag_block(lines, idx)
}

/// Headingless counterpart of [`starts_attested_flag_section`]:
/// `lines[idx]` looks like a flag row, no heading-shaped line immediately
/// governs it, and the block independently parses at least
/// [`MIN_ATTESTED_SECTION_FLAGS`] rows. See docs/shapes.md S-052.
///
/// The "no governing heading" clause is load-bearing: without it, a
/// worked example's ` Input:`/` Output:` label over dash-led sample rows
/// (`--fake-one VALUE   example input, not a supported option`) would
/// read those rows as headingless and reopen on the same floor. Trusted
/// as headingless only when nothing heading-shaped sits directly above
/// it — `sed --help`'s `Options:`-free block, entries starting on line
/// one, is the scoped shape.
pub(super) fn starts_attested_headingless_flag_block(lines: &[&str], idx: usize) -> bool {
    if !looks_like_flag_start(lines[idx].trim_start()) {
        return false;
    }
    if lines[..idx]
        .iter()
        .rev()
        .find(|l| !l.trim().is_empty())
        .is_some_and(|l| is_section_heading_line(l.trim()))
    {
        return false;
    }
    let (_, entries, _, _) = scan_flags_block(lines, idx, false);
    entries.len() >= MIN_ATTESTED_SECTION_FLAGS
}

/// Longest label accepted before a `:` still counts as a section heading.
/// Real headings are a few words; a long colon-terminated line is prose.
pub(super) const MAX_HEADING_LABEL: usize = 60;

/// True if `t` (already trimmed) is a section heading: a short,
/// colon-terminated label of plain words. Excludes every docopt-style
/// synopsis delimiter (`[`, `<`, `{`, `|`, `=`, `.`) from the label, so
/// a wrapped synopsis fragment never qualifies. The colon must terminate
/// the whole line, so an interior colon (`host:port`) is untouched.
pub(super) fn is_section_heading_line(t: &str) -> bool {
    let trimmed = t.trim_end();
    let Some(label) = trimmed.strip_suffix(':') else {
        return false;
    };
    if label.is_empty() || label.chars().count() > MAX_HEADING_LABEL {
        return false;
    }
    label
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_')
}

/// True if `line` looks like a row of a bare-name grid (openssl-style
/// `--help`: `asn1parse   ca   ciphers   cmp`) rather than prose or a
/// flag spec: every column is name-shaped and none starts with `-`.
/// Used to *continue* a grid [`looks_like_word_grid_start`] already
/// opened, so it accepts a lone trailing token (openssl's final `x509`)
/// too. See docs/shapes.md S-065.
pub(super) fn looks_like_word_grid_line(line: &str) -> bool {
    let columns = split_columns(line);
    if columns.is_empty() {
        return false;
    }
    columns.iter().all(|c| is_name_shaped_token(c))
}

/// Stricter version used only to *start* a grid: requires 3+ columns
/// (2+ spaces apart), so a two-word heading above the grid
/// (`"Standard commands"`) is never mistaken for the first row.
/// "Column" means 2+ spaces, not merely whitespace — the whole guard
/// against reading wrapped prose as a command list: without it,
/// `apt-get --help` gained subcommands from every word of its
/// description paragraph past the first line, since the preceding
/// sentence mentioned "command" and passed
/// [`is_recognized_command_heading`] (spec M-10 fabrication). See
/// docs/shapes.md S-065.
pub(super) fn looks_like_word_grid_start(line: &str) -> bool {
    let columns = split_columns(line);
    columns.len() >= 3 && columns.iter().all(|c| is_name_shaped_token(c))
}

/// True if `lines` is a rendered man page rather than `--help` output:
/// the page banner every `man` renderer emits, the same `NAME(section)`
/// title at both left and right margins (`GIT-BISECT(1)  Git Manual
/// GIT-BISECT(1)`). A property of the roff output format, not of any
/// tool. See docs/shapes.md S-066.
pub(super) fn looks_like_man_page(lines: &[&str]) -> bool {
    let Some(first) = lines.iter().find(|l| !l.trim().is_empty()) else {
        return false;
    };
    let trimmed = first.trim();
    let Some(head) = trimmed.split_whitespace().next() else {
        return false;
    };
    let Some(tail) = trimmed.split_whitespace().next_back() else {
        return false;
    };
    // Both margins must carry the identical `NAME(section)` token, and
    // there must be a centred title between them — a single repeated word
    // on its own is not a banner.
    head == tail
        && head.ends_with(')')
        && head.contains('(')
        && trimmed.split_whitespace().count() > 2
}

/// Public wrapper around [`looks_like_man_page`], for the coverage
/// harness (spec §13.1, M-16) to reuse rather than reimplement. M-16's
/// `-h` fallback enumeration must re-run this same detection over
/// already-captured text (`CommandNode::unparsed`) rather than spawn a
/// second probe (spec §6: every invocation is measured). Kept as a thin
/// wrapper so there is exactly one definition of "looks like a man
/// page" gating this safety decision.
pub fn is_man_page_banner(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().collect();
    looks_like_man_page(&lines)
}

pub(super) fn is_name_shaped_token(t: &str) -> bool {
    let mut chars = t.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// True if `heading` is a recognized command-block introduction: spec §7
/// Tier B rule 1's literal test (mentions "command(s)", "subcommand(s)",
/// or the extension below, "operation(s)"), or a framework's own extra
/// markers ([`FrameworkProfile::command_heading_markers`]).
/// [`FrameworkProfile::no_subcommand_concept`] overrides both, since that
/// framework's help structurally never has subcommands (spec M-10). Not
/// the whole test — a heading can also qualify via a chain started
/// elsewhere; see [`command_mode_seed`].
///
/// The "operations" extension recognizes `llvm-ar`'s `OPERATIONS:` table
/// the same way rule 1 already accepts "command(s)", since an operation
/// letter is an invocation verb. Deliberately *not* folded into
/// [`mentions_commands_word`]/[`command_mode_seed`], since that reads
/// prose (not headings) and seeds a sticky chain — "operation" appears
/// in ordinary English far more often than in a heading. See
/// docs/shapes.md S-022.
pub(super) fn is_recognized_command_heading(
    heading: &str,
    profile: Option<&FrameworkProfile>,
) -> bool {
    if let Some(p) = profile {
        if p.no_subcommand_concept {
            return false;
        }
        if heading_matches_markers(&heading.to_lowercase(), p.command_heading_markers) {
            return true;
        }
    }
    mentions_commands_word(heading) || mentions_operations_word(heading)
}

/// True if `text` (prose introducing a heading chain, e.g. git's "These
/// are common Git commands used in various situations:") should seed
/// `command_mode` — same [`FrameworkProfile::no_subcommand_concept`]
/// override as [`is_recognized_command_heading`]. See docs/shapes.md
/// S-070.
pub(super) fn command_mode_seed(text: &str, profile: Option<&FrameworkProfile>) -> bool {
    if profile.is_some_and(|p| p.no_subcommand_concept) {
        return false;
    }
    mentions_commands_word(text)
}

/// Find the index of the flag in `flags` that `heading` is **provably**
/// "nested under" (spec §7 Tier B rule 4) — two literal proofs, nothing
/// else. `None` means ownership is unproven, and the caller attaches
/// nothing: no names, no descriptions.
///
/// A former third branch guessed "the most recently emitted flag" when
/// neither proof fired. That misattributed `cp`'s trailing
/// `VERSION_CONTROL` enum to `--version` (unrelated prose intervened) and
/// `automake`'s `"Warning categories include:"` block to
/// `-f, --force-missing` instead of `-W, --warnings` ten lines earlier —
/// no adjacency signal tells those two shapes apart, so the fallback is
/// gone. Ownership requires either the heading naming the flag's long
/// spelling literally, or containing one flag's `value_name` verbatim as
/// a whole word (case-insensitive, never a stem/plural match — that's
/// exactly the `automake` false positive). Both proofs scan `flags` in
/// order and take the first hit. See spec §7 Tier B rule 4.
pub(super) fn find_owning_flag_index(heading: &str, flags: &[Entity]) -> Option<usize> {
    let lower = heading.to_lowercase();
    if let Some(idx) = flags.iter().position(|f| {
        f.long()
            .is_some_and(|l| lower.contains(&format!("--{}", l.to_lowercase())))
    }) {
        return Some(idx);
    }
    flags.iter().position(|f| {
        f.value_name.as_ref().is_some_and(|vn| {
            // A one-character value_name is never a real placeholder —
            // it's the signature of an unrelated parser artifact
            // (ffplay's `-fs` misreading as short `-f` plus value_name
            // `"s"`, spec Appendix A). Excluding it costs nothing: every
            // genuine GNU-style placeholder is a whole word already.
            vn.chars().count() > 1 && heading_contains_word(&lower, vn)
        })
    })
}

/// True when `word` appears in `lower_haystack` as a whole token — split
/// on any non-alphanumeric byte, so `FORMAT` matches but not inside
/// `FORMATS`/`REFORMAT`. Case-insensitive since capitalization isn't
/// guaranteed to match between heading and value_name.
fn heading_contains_word(lower_haystack: &str, word: &str) -> bool {
    let word_lower = word.to_lowercase();
    lower_haystack
        .split(|c: char| !c.is_alphanumeric())
        .any(|token| token == word_lower)
}

/// Turn a word-grid block into subcommand stubs (if `treat_as_commands`)
/// or drop it (spec §7 Tier B rule 1 — a word grid is layout, not by
/// itself evidence of a command list). Word grids carry no per-entry
/// description, so there is nothing sensible to route to `choices` here;
/// unattributed grids are simply dropped rather than guessed at.
pub(super) fn process_word_grid(
    heading: &str,
    grid_lines: &[&str],
    treat_as_commands: bool,
    out: &mut ParsedHelp,
) -> (usize, usize) {
    let mut seen = 0usize;
    let mut clean = 0usize;
    for line in grid_lines {
        for token in line.split_whitespace() {
            seen += 1;
            if !is_command_name_shaped(token) {
                out.saw_unattributable_content = true;
                continue;
            }
            clean += 1;
            if treat_as_commands {
                let mut node = CommandNode::new(token, Provenance::single(Source::HelpText));
                node.group = heading_can_name_a_group(heading).then(|| heading.to_string());
                // `treat_as_commands` is only `true` under a recognized
                // heading or an active `command_mode`, the positive
                // evidence spec issue #2 requires even without a
                // per-entry description.
                node.heading_attested = true;
                out.try_push_subcommand(node);
            }
        }
    }
    if !treat_as_commands && seen > 0 {
        out.saw_unattributable_content = true;
    }
    (seen, clean)
}

/// A flags block's heading as a display *group*, or `None` when the
/// heading is just the generic "here are the flags" label.
/// `Flag::group` exists to preserve meaningful subdivisions (tar's 171
/// flags under headings like "Main operation mode"); a heading that only
/// says "Options"/"Flags" subdivides nothing and would otherwise render
/// `FLAGS` twice in a row (`gh`). See docs/shapes.md S-067.
pub(super) fn meaningful_flag_group(heading: String) -> Option<String> {
    const GENERIC: [&str; 6] = [
        "options",
        "flags",
        "option",
        "flag",
        "optional arguments",
        "global flags",
    ];
    let normalized = heading.trim().trim_end_matches(':').to_lowercase();
    if GENERIC.contains(&normalized.as_str()) || !heading_can_name_a_group(&heading) {
        None
    } else {
        Some(heading)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `tree --help`'s own `------- Listing options -------` (and six
    /// siblings) — found via a full-`PATH` sweep run against
    /// fix/row-grammar-jmod-llvm, not a corpus fixture. Must be recognized
    /// as a decorative heading, never mistaken for ordinary usage-synopsis
    /// text or a flag row.
    #[test]
    fn a_dash_bracketed_heading_is_recognized() {
        assert!(looks_like_dash_bracketed_heading(
            "------- Listing options -------"
        ));
        assert!(looks_like_dash_bracketed_heading(
            "  ------- File options -------  "
        ));
        // No label at all: two dash runs glued together with nothing
        // between them is not this shape (and is `is_dash_underline_row`'s
        // job instead).
        assert!(!looks_like_dash_bracketed_heading("----------"));
        // Only one dash run: an ordinary flag-shaped line, not a divider.
        assert!(!looks_like_dash_bracketed_heading("--target-platform"));
    }

    // --- hard-wrapped prose sentences (issue #80) ---

    /// `dpkg --help`'s hard-wrapped cross-reference sentence naming
    /// another program's options must not become a heading and flags
    /// block. See docs/shapes.md S-011 and corpus/dpkg.
    #[test]
    fn wrapped_cross_reference_sentence_yields_no_heading_and_no_flags() {
        let help = "Usage: dpkg [<option>...] <command>\n\
                     \n\
                     Commands:\n\
                     \x20\x20-i|--install       <.deb file name>...\n\
                     \n\
                     Use dpkg with -b, --build, -c, --contents, -e, --control, -I, --info,\n\
                     \x20\x20-f, --field, -x, --extract, -X, --vextract, --ctrl-tarfile, --fsys-tarfile\n\
                     on archives (type dpkg-deb --help).\n\
                     \n\
                     Options:\n\
                     \x20\x20--admindir=<directory>     Use <directory> instead of /var/lib/dpkg.\n\
                     \x20\x20--robot                    Use machine-readable output on some commands.\n";

        let parsed = parse_with_profile(help, None, Some("dpkg"));
        for spelling in [
            "field",
            "extract",
            "vextract",
            "ctrl-tarfile",
            "fsys-tarfile",
        ] {
            assert!(
                parsed.flags.iter().all(|f| f.long() != Some(spelling)),
                "fabricated --{spelling}: {:?}",
                parsed.flags
            );
        }
        assert!(
            parsed
                .flags
                .iter()
                .all(|f| !f.group.as_deref().is_some_and(|g| g.starts_with("Use "))),
            "fabricated group: {:?}",
            parsed.flags
        );
        // The real table beneath the sentence still parses, descriptions
        // and all — containment must not be bought by losing structure.
        for spelling in ["admindir", "robot"] {
            let flag = parsed
                .flags
                .iter()
                .find(|f| f.long() == Some(spelling))
                .unwrap_or_else(|| panic!("missing --{spelling} in {:?}", parsed.flags));
            assert!(
                flag.description.is_some(),
                "--{spelling} lost its description: {:?}",
                parsed.flags
            );
        }
    }

    /// The fence is bounded by shape, not paragraph length: an aligned
    /// table directly beneath a comma-terminated line still parses.
    #[test]
    fn comma_terminated_line_over_an_aligned_table_still_yields_its_flags() {
        let help = "Usage: demo [OPTIONS]\n\
                     \n\
                     The options below accept a size, a duration, or a count,\n\
                     \x20\x20--limit <n>        cap the number of records read\n\
                     \x20\x20--timeout <secs>   give up after this many seconds\n";

        let parsed = parse_with_profile(help, None, Some("demo"));
        for spelling in ["limit", "timeout"] {
            let flag = parsed
                .flags
                .iter()
                .find(|f| f.long() == Some(spelling))
                .unwrap_or_else(|| panic!("missing --{spelling} in {:?}", parsed.flags));
            assert!(
                flag.description.is_some(),
                "--{spelling} lost its description: {:?}",
                parsed.flags
            );
        }
    }

    /// A blank line ends the fence, because a wrapped sentence never
    /// contains one.
    #[test]
    fn blank_line_after_a_comma_terminated_line_ends_the_prose_fence() {
        let help = "Usage: demo [OPTIONS]\n\
                     \n\
                     Accepts a size, a duration, a count, or a ratio,\n\
                     \n\
                     \x20\x20--limit <n>        cap the number of records read\n";

        let parsed = parse_with_profile(help, None, Some("demo"));
        assert!(
            parsed.flags.iter().any(|f| f.long() == Some("limit")),
            "flags: {:?}",
            parsed.flags
        );
    }

    /// Regression for spec M-10: `apt-get --help`'s description prose,
    /// wrapped at a matching indent beneath a "command"-mentioning
    /// sentence, must not be read as a command grid. See docs/shapes.md
    /// S-065.
    #[test]
    fn apt_get_description_prose_is_not_parsed_as_a_command_grid() {
        let parsed = parse(APT_GET_HELP);
        let names: Vec<&str> = parsed.subcommands.iter().map(|c| c.name.as_str()).collect();
        for fabricated in [
            "and",
            "information",
            "about",
            "them",
            "from",
            "authenticated",
            "sources",
        ] {
            assert!(
                !names.contains(&fabricated),
                "prose word {fabricated:?} was parsed as a subcommand: {names:?}"
            );
        }
    }

    /// Regression for spec M-8: `openssl --help` writes only to stderr
    /// with no indentation — commands are a same-indent word grid. See
    /// docs/shapes.md S-065.
    #[test]
    fn openssl_word_grid_recovered_as_subcommands() {
        let parsed = parse(OPENSSL_HELP);
        let names: Vec<&str> = parsed.subcommands.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"asn1parse"), "{names:?}");
        assert!(names.contains(&"ciphers"), "{names:?}");
        assert!(names.contains(&"x509"), "{names:?}");
    }

    #[test]
    fn openssl_word_grid_entries_carry_their_heading_as_group() {
        let parsed = parse(OPENSSL_HELP);
        let asn1parse = parsed
            .subcommands
            .iter()
            .find(|c| c.name == "asn1parse")
            .unwrap();
        assert_eq!(asn1parse.group.as_deref(), Some("Standard commands"));
        let md5 = parsed.subcommands.iter().find(|c| c.name == "md5");
        assert!(md5.is_some(), "expected md5 among digest commands");
        assert!(md5
            .unwrap()
            .group
            .as_deref()
            .unwrap()
            .contains("Message Digest commands"));
    }

    // --- `is_man_page_banner` (spec [M-16] enumeration prerequisite) ---

    /// The exact shape `git bisect --help` renders: `man`'s banner
    /// convention, a true positive. See docs/shapes.md S-066.
    #[test]
    fn is_man_page_banner_true_positive_on_a_real_banner_shape() {
        let rendered = "GIT-BISECT(1)                Git Manual                GIT-BISECT(1)\n\n\
                         NAME\n       git-bisect - Use binary search to find the commit...\n";
        assert!(is_man_page_banner(rendered));
    }

    /// git's root `--help` is conventional help text, not a man page —
    /// this must come back false (spec §7 Tier B step 3 is for
    /// subcommands like `git bisect`, not the root).
    #[test]
    fn is_man_page_banner_is_false_on_gits_own_root_help() {
        assert!(!is_man_page_banner(GIT_HELP));
    }

    /// Ordinary help text and a repeated single word are not a banner —
    /// a centred title must sit between the two margins.
    #[test]
    fn is_man_page_banner_is_false_on_ordinary_help_text() {
        assert!(!is_man_page_banner(TAR_HELP));
        assert!(!is_man_page_banner("USAGE USAGE\n"));
    }

    /// Public wrapper delegates to exactly the rule the parser uses to
    /// decide degradation, not a second, possibly-drifted copy.
    #[test]
    fn is_man_page_banner_agrees_with_the_parsers_own_degradation_decision() {
        let man_page = "FOO(1)   Foo Manual   FOO(1)\n\nNAME\n     foo\n";
        assert!(is_man_page_banner(man_page));
        let parsed = parse(man_page);
        assert!(parsed.flags.is_empty());
        assert!(parsed.subcommands.is_empty());
        assert!(parsed.usage.is_empty());
    }

    // --- over-eager headings: prose, wrapped synopsis, shared rows -------

    /// `nano 7.2`'s preamble and option table head, byte-exact from
    /// corpus/nano/7.2/help.txt.
    const NANO_PREAMBLE: &str = concat!(
        "Usage: nano [OPTIONS] [[+LINE[,COLUMN]] FILE]...\n",
        "\n",
        "To place the cursor on a specific line of a file, put the line number with\n",
        "a '+' before the filename.  The column number can be added after a comma.\n",
        "When a filename is '-', nano reads data from standard input.\n",
        "\n",
        " Option         Long option             Meaning\n",
        " -A             --smarthome             Enable smart home key\n",
        " -B             --backup                Save backups of existing files\n",
    );

    #[test]
    fn a_prose_sentence_above_an_option_table_names_no_group() {
        let parsed = parse_named(NANO_PREAMBLE, "nano");
        for long in ["smarthome", "backup"] {
            let flag = flag_named(&parsed, long);
            assert_eq!(
                flag.group, None,
                "-- {long} inherited nano's preamble sentence as its group"
            );
        }
        // This suppresses a field; it does not decline the block.
        assert_eq!(
            flag_named(&parsed, "smarthome")
                .description
                .as_ref()
                .map(|t| t.as_str()),
            Some("Enable smart home key")
        );
    }

    /// The GNU convention: 56 of 205 affected tools inherit this exact
    /// sentence. See docs/shapes.md S-011.
    #[test]
    fn the_gnu_mandatory_arguments_sentence_names_no_group() {
        let raw = concat!(
            "Usage: head [OPTION]... [FILE]...\n",
            "Print the first 10 lines of each FILE to standard output.\n",
            "\n",
            "Mandatory arguments to long options are mandatory for short options too.\n",
            "  -c, --bytes=[-]NUM       print the first NUM bytes of each file\n",
            "  -n, --lines=[-]NUM       print the first NUM lines instead of the first 10\n",
        );
        let parsed = parse_named(raw, "head");
        assert_eq!(flag_named(&parsed, "bytes").group, None);
        assert_eq!(flag_named(&parsed, "lines").group, None);
    }

    /// The inverse: `gcc`/`lto-dump` writes headings that are complete
    /// English sentences but colon-terminated, so they stay real
    /// headings. Anchors the prose test on the full stop, not wording.
    #[test]
    fn a_prose_shaped_but_colon_terminated_heading_still_names_a_group() {
        let raw = concat!(
            "Usage: lto-dump [OPTION]... FILE\n",
            "\n",
            "The following options are specific to just the language C:\n",
            "  --std=c99                 conform to the C99 standard\n",
            "\n",
            "At least one of the following switches must be given:\n",
            "  --list                    list the objects\n",
        );
        let parsed = parse_named(raw, "lto-dump");
        assert_eq!(
            flag_named(&parsed, "std").group.as_deref(),
            Some("The following options are specific to just the language C:")
        );
        assert_eq!(
            flag_named(&parsed, "list").group.as_deref(),
            Some("At least one of the following switches must be given:")
        );
    }

    /// A period-terminated row is a table row, not a sentence — the
    /// column gap tells them apart. `arptables` writes both in one
    /// document.
    #[test]
    fn a_period_terminated_two_column_row_is_not_read_as_prose() {
        assert!(!is_prose_sentence(
            "[!] --version   -V      print package version."
        ));
        assert!(is_prose_sentence(
            "Either long or short options are allowed."
        ));
        // Too short to be a sentence.
        assert!(!is_prose_sentence("Main modes."));
        // Headings are labels; they do not end in a full stop.
        assert!(!is_prose_sentence("Available Commands:"));
    }

    /// `update-xmlcatalog --help`, byte-exact through its second
    /// invocation form. See docs/shapes.md S-011 and
    /// corpus/update-xmlcatalog.
    const UPDATE_XMLCATALOG_USAGE: &str = concat!(
        "Usage:\n",
        "    update-xmlcatalog <options> --add --root --type <type> \\\n",
        "                                                --id <id> --package <package>\n",
        "    update-xmlcatalog <options> --del --root --type <type> \\\n",
        "                                                --id <id>\n",
    );

    #[test]
    fn a_backslash_wrapped_synopsis_keeps_the_flags_on_its_wrapped_tail() {
        let parsed = parse_named(UPDATE_XMLCATALOG_USAGE, "update-xmlcatalog");
        let spellings: Vec<String> = parsed.flags.iter().map(|f| f.spelling()).collect();
        assert!(
            spellings.iter().any(|s| s == "--del"),
            "--del is documented only on a backslash-continued usage line; \
             got {spellings:?}"
        );
        assert_eq!(
            parsed.usage,
            vec![
                "Usage:".to_string(),
                "    update-xmlcatalog <options> --add --root --type <type> --id <id> --package <package>"
                    .to_string(),
                "    update-xmlcatalog <options> --del --root --type <type> --id <id>".to_string(),
            ],
            "each wrapped form is one usage entry, with the continuation \
             marker consumed by the join it performed"
        );
    }

    #[test]
    fn a_backslash_continued_line_names_no_group() {
        // The same shape reached from the section scanner rather than the
        // usage block: a `bpfcc` tracer's EXAMPLES section.
        let raw = concat!(
            "USAGE message:\n",
            "\n",
            "argdist -p 2780 -z 120 \\\n",
            "        -C 'p:c:write(int fd):int:fd'\n",
        );
        let parsed = parse_named(raw, "argdist");
        for flag in &parsed.flags {
            assert_eq!(
                flag.group,
                None,
                "{} inherited a half-line as its group",
                flag.spelling()
            );
        }
        assert!(!is_line_continuation_fragment("Available Commands:"));
        assert!(is_line_continuation_fragment("argdist -p 2780 -z 120 \\"));
    }

    /// `uconv --help`, byte-exact: heading and first row share one
    /// physical line. See docs/shapes.md S-062.
    const UCONV_OPTIONS: &str = concat!(
        "Options:  -h, --help                    print this message\n",
        "          -V, --version                 print the program version\n",
        "          -s, --silent                  suppress messages\n",
    );

    #[test]
    fn a_heading_sharing_its_line_with_the_first_row_keeps_that_row() {
        let parsed = parse_named(UCONV_OPTIONS, "uconv");
        let help = flag_named(&parsed, "help");
        assert_eq!(help.short(), Some('h'));
        assert_eq!(
            help.description.as_ref().map(|t| t.as_str()),
            Some("print this message")
        );
        // `Options:` is one of `meaningful_flag_group`'s generic labels.
        for flag in &parsed.flags {
            assert_eq!(flag.group, None, "{} kept a group", flag.spelling());
        }
    }

    #[test]
    fn a_heading_line_whose_remainder_is_not_a_flag_is_never_split() {
        // `ntfs-3g`'s real line: label, gap, then a value list, not a row.
        assert_eq!(
            split_shared_heading_row("Options:  ro (read-only mount), windows_names, uid=, gid=,"),
            None
        );
        // `awk`'s second heading column, likewise not a row.
        assert_eq!(
            split_shared_heading_row("POSIX options:\t\tGNU long options: (standard)"),
            None
        );
        // The shape this does claim.
        assert_eq!(
            split_shared_heading_row("Options:  -h, --help    print this message"),
            Some((
                "Options:".to_string(),
                "          -h, --help    print this message".to_string(),
                false
            ))
        );
    }

    #[test]
    fn a_bnf_heading_carrying_its_first_flag_row_is_split() {
        // `ip`'s real line: `:=` reads as the BNF operator. The opening
        // bracket is stripped along with it, matching `ip`'s own
        // continuation lines, which never carry it either.
        assert_eq!(
            split_shared_heading_row(
                "       OPTIONS := { -V[ersion] | -s[tatistics] | -d[etails] | -r[esolve] |"
            ),
            Some((
                "       OPTIONS :".to_string(),
                "                    -V[ersion] | -s[tatistics] | -d[etails] | -r[esolve] |"
                    .to_string(),
                true
            ))
        );
        // `dcb`'s sibling shape: a `[`-bracket instead of `{`.
        assert_eq!(
            split_shared_heading_row("       OPTIONS := [ -V | --Version | -i | --iec ]"),
            Some((
                "       OPTIONS :".to_string(),
                "                    -V | --Version | -i | --iec ]".to_string(),
                true
            ))
        );
    }

    #[test]
    fn a_bnf_heading_whose_row_is_not_a_flag_is_never_split() {
        // `ip`'s `OBJECT` and `ss`'s productions use `:=` but open on a
        // bare word, never a flag — clause 4 rejects them.
        assert_eq!(
            split_shared_heading_row(
                "where  OBJECT := { address | addrlabel | amt | fou | help | ila | ioam | l2tp |"
            ),
            None
        );
        assert_eq!(
            split_shared_heading_row(
                "       FAMILY := {inet|inet6|link|unix|netlink|vsock|tipc|xdp|help}"
            ),
            None
        );
        // `pkgdata`'s `modes: (-m option)`: bracket with no `=` operator
        // — a bracket-only clause 3 would invent this false positive,
        // since `-m option)` alone satisfies `looks_like_flag_start`.
        assert_eq!(split_shared_heading_row("modes: (-m option)"), None);
    }

    /// Group suppression must never touch `heading_attested` (spec §6's
    /// probe-argv gate) in either direction. Same block under a real
    /// command heading (attested) vs. a prose sentence (recovers
    /// nothing, before or after this change).
    #[test]
    fn group_suppression_does_not_widen_probe_eligibility() {
        const BLOCK: &str = concat!(
            "  clone     Clone a repository\n",
            "  init      Create one\n",
        );
        let attested = parse_named(&format!("Commands:\n{BLOCK}"), "prog");
        assert_eq!(attested.subcommands.len(), 2);
        assert!(
            attested.subcommands.iter().all(|c| c.heading_attested),
            "a recognized command heading still attests its entries"
        );

        let prose = parse_named(
            &format!("Copy standard input to each FILE, and also to standard output.\n{BLOCK}"),
            "prog",
        );
        assert!(
            prose.subcommands.is_empty(),
            "a prose sentence attests nothing: {:?}",
            prose
                .subcommands
                .iter()
                .map(|c| c.name.clone())
                .collect::<Vec<_>>()
        );
    }
}
