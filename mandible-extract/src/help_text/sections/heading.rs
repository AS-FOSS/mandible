//! Section headings: telling one from prose or a wrapped continuation,
//! splitting a heading that shares its line with a row, recognizing a
//! word grid or a man page, and naming the group a block's entries carry.

use super::*;

/// Rewrite every line that carries a section heading **and** the first row
/// of that section's own table into the two lines it means.
///
/// # The defect
///
/// `uconv --help` runs its heading straight into its first option row:
///
/// ```text
/// Options:  -h, --help                    print this message
///           -V, --version                 print the program version
/// ```
///
/// The section scanner promotes a line to a heading whole, so the entire
/// first line became the heading — `-h, --help` was never a flag under any
/// spelling a user could type, and every other flag in the block inherited
/// `group: "Options:  -h, --help                    print this message"`.
/// The audit reviewer's own words: "since the flag `-h` was in front of
/// `Options:` it got swallowed into the section header".
///
/// Measured over the 2,301 frozen captures in `audit/queue-captures/`:
/// **2 tools** (`uconv`, and `zipinfo`'s `main listing-format options:
/// -s  short Unix "ls -l" format (def.)`), each losing exactly the one row
/// that shares its heading's line. Small, and reported as measured rather
/// than rounded up — the *broad* shape (a heading label, a column gap, and
/// then anything at all) is 12 tools, but the other ten are second heading
/// columns (`awk`'s `POSIX options:\t\tGNU long options: (standard)`) or
/// wrapped prose, and rewriting those would invent rows rather than
/// recover them.
///
/// # A second shape: the BNF-grammar heading (`ip` and its iproute2 siblings)
///
/// `ip --help` writes its whole synopsis as a BNF grammar, and its `OPTIONS`
/// production opens the same way `uconv`'s did — heading and first row on
/// one physical line — except the label is glued to the row by `:=`, not by
/// a column of spaces:
///
/// ```text
/// where  OBJECT := { address | addrlabel | amt | fou | help | ila | ioam | l2tp |
///                    ...
///        OPTIONS := { -V[ersion] | -s[tatistics] | -d[etails] | -r[esolve] |
///                     -h[uman-readable] | -iec | -j[son] | -p[retty] |
///                     ...
/// ```
///
/// The original clause 3 (`MIN_COLUMN_GAP_SPACES` spaces right after the
/// colon) never fires here: the character right after `:` is `=`, not a
/// space, so the gap is zero and the whole line — `-V`, `-s`, `-d`, `-r`
/// included — was promoted to the heading string. `mandible --doctor ip`
/// read 8 flags before this fix; the group label was literally `OPTIONS :=
/// { -V[ERSION] | -S[TATISTICS] | -D[ETAILS] | -R[ESOLVE] |`.
///
/// Measured the same way as the shape above, over the same 2,301 captures:
/// **6 tools** gain their first `OPTIONS` row back — `bridge`, `dcb`,
/// `devlink`, `ip`, `rdma`, `vdpa` (all iproute2-family binaries; `dcb`'s
/// row opens `[ -V | --Version | ...`, the rest open `{ -V[ersion] | ...`).
/// A 7th tool, `ss`, matches the raw `:=\s*[{[]` grep this was measured
/// with but recovers nothing: every one of its BNF productions
/// (`FAMILY := {inet|inet6|...}`, `QUERY := {...}`, `STATE-FILTER := {...}`,
/// `connected := {...}`, `synchronized := {...}`, `bucket := {...}`,
/// `big := {...}`) opens on a bare word, not a flag, so clause 4 rejects
/// all of them — correctly: `ss` writes its actual flags one per line
/// already (`-h, --help          this message`), never sharing a heading.
/// `ip`'s own sibling `OBJECT := { address | addrlabel | ... }` production
/// is excluded the same way, by the same clause, for the same reason.
///
/// The *broader* version of this shape — drop the operator requirement and
/// accept any `label:` immediately followed by an opening bracket — matches
/// 36 tools, and the extra ones are exactly the false positives the operator
/// requirement exists to keep out. `pkgdata`'s `modes: (-m option)` is the
/// sharpest case: strip the label, the gap, and the `(` the same way the
/// bracket-only rule would, and the remainder is `-m option)` — which
/// *does* satisfy [`looks_like_flag_start`], so clause 4 does not save it.
/// The rest of the 36 are the same family of near-miss: usage-line
/// continuations (`lsof`'s own `usage:` line, already excluded elsewhere),
/// stack-trace fragments that happen to contain `[Errno 13]` (`dnf`, `ua`,
/// `pro`, `swift-recon-cron`), and parenthetical asides after a real heading
/// (`whiptail`'s `Options: (depend on box-option)`, `pkgdata`'s `modes: (-m
/// option)`, `mariadb-admin`'s `Where command is a one or more of: (Commands
/// may be shortened)`). Requiring the `=` is what tells a BNF assignment
/// apart from a colon that merely happens to be followed by a parenthesis.
///
/// # The rule
///
/// A line is split when **all** of these hold:
///
/// 1. its indentation is spaces only — a tab's width is a terminal
///    setting, so the recovered row's column could not be reproduced;
/// 2. the text up to and including its first `:` is a
///    [`is_section_heading_line`] label (short, plain words, colon-
///    terminated) and is not a `usage:` marker;
/// 3. **either** at least [`MIN_COLUMN_GAP_SPACES`] spaces follow the colon
///    (the `uconv`/`zipinfo` shape), **or** the colon is immediately
///    followed by a BNF `=` (making it read as `:=`), at least one space,
///    and optionally a single opening bracket (`{`/`[`/`(`) followed by at
///    least one more space (the `ip`-family shape) — see the section above
///    for why the operator, not the bracket, is the discriminator;
/// 4. what follows that gap [`looks_like_flag_start`].
///
/// Clause 4 is the safety argument. Clauses 1-3 alone are satisfied by
/// every "label, then a value" line in the fleet (`ntfs-3g`'s `Options:
/// ro (read-only mount), windows_names, uid=, gid=,`, `delv`'s `Where:
/// domain\t  is in the Domain Name System`), and splitting one of those
/// would hand the flags block a row that is not a flag. Requiring the
/// remainder to open like a flag spelling is what confines this to the
/// case where a real row is demonstrably being lost.
///
/// Returns `None` when no line matched, so the overwhelmingly common
/// document is parsed from its own borrowed `&str` with no allocation.
///
/// The returned [`HashSet`] names, by 0-indexed line number in the
/// *rewritten* text, every **row** line (never the heading line beside it)
/// this function recovered via the `:=` operator clause — never the plain
/// column-gap clause. This is the one piece of evidence
/// [`split_bnf_alternation_row`] is gated on: a BNF `:=` production and an
/// ordinary options table can both write a short/long pair joined by a
/// bare `|` (`btrfsck`'s own `-E|--subvol-extents <subvolid>` uses `|` as a
/// plain alias separator, no grammar involved), and only the *document* —
/// not the row's own text — says which is which. By the point
/// [`scan_flags_block`] sees a row, the operator itself is already gone
/// from both the heading and the row (this function's own job), so the set
/// is the only way that fact survives to reach it.
///
/// **Keyed on the row, not the heading**, because the engine does not
/// always recognize the heading `split_shared_heading_row` produced as a
/// heading in its own right before handing a block to
/// [`scan_flags_block`]. A `where OBJECT := { ... }` production that fits
/// on one physical line (`dcb`, `vdpa`) reads, to the general section
/// loop, as a heading of its own whose "content" is merely whatever is
/// indented more than column 0 — which the very next `OPTIONS :` heading
/// line always is, coincidentally, regardless of what it actually is. The
/// loop never revisits that line as a heading in its own right; it reaches
/// [`scan_flags_block`] straight from the *headingless* call site once the
/// bare-block scan dedents back out, with no heading index available to
/// check at all. The row itself, though, is always exactly the first line
/// [`scan_flags_block`] is asked to start from — [`flags_block_start`]
/// never skips ahead of an already-flag-shaped first line — so recording
/// row lines is what makes the gate reachable from *either* call site.
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
    // A BNF definition operator: the colon reads as `:=`, not a plain
    // section-heading colon. See this function's doc comment for why the
    // operator itself — not merely a bracket — is what widens clause 3.
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

/// Fewest whitespace-separated words a period-terminated single-field line
/// must carry before [`is_prose_sentence`] reads it as a sentence.
///
/// Five, chosen against the measured population rather than by taste: the
/// shortest real specimen in the fleet is `[`'s "Exit with the status
/// determined by EXPRESSION." (seven words) and `getent`'s "Get entries
/// from administrative database." (five), while the shortest *heading*
/// this must never claim is a two- or three-word label. Nothing between
/// four and five words was found on either side, so the boundary is not
/// load-bearing in the way a tighter one would be.
pub(super) const MIN_PROSE_SENTENCE_WORDS: usize = 5;

/// True when `heading` is an English sentence rather than a section
/// heading — a single field (no column gap anywhere), several words long,
/// terminated by a full stop.
///
/// # The defect
///
/// The section scanner promotes a line to a heading on **indentation
/// alone**: any line whose next non-blank neighbour is indented further is
/// read as introducing that neighbour's block. A tool that closes its
/// preamble with a sentence and then indents its option table one column
/// therefore hands the scanner a sentence where a heading belongs, and
/// every flag in the block inherits it as [`mandible_core::Entity::group`] —
/// which the flags pane renders, uppercased, as a section header:
///
/// ```text
/// When a filename is '-', nano reads data from standard input.
///
///  Option         Long option             Meaning
///  -A             --smarthome             Enable smart home key
/// ```
///
/// Measured over the 2,301 frozen captures in `audit/queue-captures/`:
/// **205 tools**, 211 distinct (tool, line) pairs. It is overwhelmingly
/// the GNU convention — 56 tools inherit "Mandatory arguments to long
/// options are mandatory for short options too.", 13 inherit "With no
/// FILE, or when FILE is -, read standard input." — so it is a layout
/// fact about a whole family of `--help` writers, not a quirk of any one
/// tool.
///
/// # The rule, and what each clause keeps out
///
/// - **No column gap** ([`find_multi_space_gap`], deliberately *not*
///   [`find_description_gap`], whose sentence-start and `=`-separator
///   fallbacks would fire on the very prose this is trying to recognize).
///   A two-column line is a table row, not a sentence: it is what keeps
///   `arptables`' `[!] --version\t-V\t\tprint package version.` and
///   `fail2ban-client`'s `set logtarget <TARGET>   sets logging target to
///   <TARGET>.` — both period-terminated, neither prose — out.
/// - **Terminated by a full stop.** Headings are labels; they do not end
///   in a sentence terminator. This is what leaves every colon-terminated
///   heading alone, including the genuinely prose-shaped ones a stricter
///   wording test would have destroyed: `gcc`/`lto-dump` writes "The
///   following options are specific to just the language C:" and
///   `objdump` "At least one of the following switches must be given:",
///   and both are real headings over real blocks.
/// - **At least [`MIN_PROSE_SENTENCE_WORDS`] words**, so a short
///   period-carrying label can never qualify.
/// - **Not an ellipsis.** A trailing `...` is docopt-style usage notation
///   for repetition (`numactl`'s own `[--localalloc | -l] command args
///   ...`, `mkfontscale`'s `[-u] [-U] [-v] [ directory ]...`), never a
///   sentence terminator, but a naive `ends_with('.')` test reads its last
///   character the same way it reads a real full stop. Measured on the
///   usage-block continuation call site added alongside this clause:
///   without it, both lines above were misread as prose and (incorrectly)
///   ended the usage block early, silently dropping every flag the
///   synopsis still had left to name — 19 on `numactl`, 3 on
///   `mkfontscale`. Three dots, not one, so a single mid-notation period
///   (`<v1.0>`) is unaffected.
///
/// # What this does *not* touch
///
/// Two call sites use this to decide whether a line reads as English prose
/// rather than usage/heading notation ([`looks_like_unlabeled_synopsis_line`]
/// and the usage-block's own more-indented-continuation check in
/// [`parse_with_profile`], which stops a synopsis from swallowing
/// `sg_emc_trespass`'s trailing sentences and mining `LUN`/`SP`/`EMC` out of
/// them as fabricated positionals); two more only decide whether a heading
/// may be copied into a flag's `group`. The section loop has one additional
/// use: a prose line followed by a more-indented `is_obscured_fence_marker`
/// opens `obscured_ignorable_indent`, whose whole-region fence can remove
/// entries fabricated from worked examples. It never recognizes a command
/// heading or sets `CommandNode::heading_attested`, and its reopening exit
/// (`obscured_fence_reopens`) admits only an independently attested *flag*
/// section, so it cannot widen the set of nodes eligible to become `<word>
/// --help` probe argv — but calling the whole use "subtractive" overstates
/// what it guarantees: the fence's *close* restores whatever
/// `in_ignorable_section` held immediately before it opened (issue #77 edge
/// 2), rather than clearing it, because clearing unconditionally could
/// cancel a suppression a genuine, non-obscured `EXAMPLES:` heading had
/// already established earlier in the same document — a real restoration,
/// not a pure subtraction. The jar and internal-`Commands:` regression
/// tests beside the section parser pin the argv-eligibility boundary;
/// `mandible-extract/tests/exec_policy.rs` separately pins the older,
/// group-only call sites.
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

/// True when `heading` is the first half of a backslash-continued logical
/// line, and so cannot be a heading of anything: the tool has said, with
/// the shell's own continuation marker, that the line is not finished.
///
/// # The defect
///
/// The same indentation-alone promotion [`is_prose_sentence`] documents,
/// reached from the other direction. `update-xmlcatalog --help` writes its
/// synopsis as backslash-continued pairs:
///
/// ```text
///     update-xmlcatalog <options> --del --root --type <type> \
///                                                 --id <id>
/// ```
///
/// The second line is indented far past the first, so the first is read as
/// a heading and the second as its block — and the TUI renders
/// `UPDATE-XMLCATALOG <OPTIONS> --DEL --ROOT --TYPE <TYPE> \` as a section
/// header. Measured over the frozen captures: **7 tools**, 16 distinct
/// lines (`update-xmlcatalog`, `wpa_cli`, `zic`, and the four `bpfcc`
/// tracers, whose `EXAMPLES` sections wrap the same way).
///
/// Like [`is_prose_sentence`], this only suppresses the `group`; see that
/// function's "What this does not touch".
pub(super) fn is_line_continuation_fragment(heading: &str) -> bool {
    heading.trim_end().ends_with('\\')
}

/// True when `heading` may be copied into a recovered entry's `group`.
///
/// The one predicate the three group-assigning sites share, so "what
/// counts as a heading for display purposes" is written down once instead
/// of three times. Both clauses are *subtractive*: a line either reads as
/// something that is positively not a heading, or it is left exactly as it
/// was before.
pub(super) fn heading_can_name_a_group(heading: &str) -> bool {
    !is_prose_sentence(heading)
        && !is_line_continuation_fragment(heading)
        && !is_dash_underline_row(heading)
}

/// True when `line`, trimmed, is nothing but dash characters and
/// whitespace — a table's own column-underline decoration (jmod's own
/// `Option`/`Description` header row: `------  -----------`) rather than a
/// real heading. [`super::grammar::is_dash_underline_token`] already keeps
/// this shape from opening a flag entry; this is the same rule applied to
/// every whitespace-delimited run in the line, since a two-column
/// underline row is two such runs, not one — and it exists here because
/// the row can still reach [`meaningful_flag_group`]/[`process_word_grid`]
/// as an ordinary heading candidate once it stops being read as a flag,
/// carrying its own literal dashes into `Flag::group`/`CommandNode::group`
/// otherwise.
pub(super) fn is_dash_underline_row(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty() && trimmed.split_whitespace().all(is_dash_underline_token)
}

/// True when `line`, trimmed, is a decorative section-divider heading with
/// no trailing colon: a dash run, then a plain-word label, then another
/// dash run — `tree --help`'s own `------- Listing options -------`,
/// `------- File options -------`, and five siblings.
///
/// [`is_section_heading_line`] requires a trailing colon and cannot see
/// this shape at all, and before the [`super::grammar::is_dash_underline_token`]
/// guard existed, `looks_like_flag_start` happened to end the usage block
/// on this row anyway — for the wrong reason (it looked like a flag
/// spelling), but it still ended the block. Once that accidental stop
/// disappeared, this row started folding into the usage synopsis's own
/// continuation instead (nothing else in [`super::parse_body`]'s usage-
/// block loop recognized it as a heading), and the usage-derived flag miner
/// then read the embedded `-------` token out of the folded text as a
/// fabricated flag with an invented value name — a different wrong answer
/// than before, not a fix. This closes that gap at the same call site
/// [`is_section_heading_line`] already gates the usage block on, without
/// attempting the larger (and out of scope here) job of turning the row
/// into a real `group` label — it is dropped, honestly, the same way
/// every other unlabelled row this scanner cannot place already is.
///
/// The label between the two dash runs must be non-empty and read as plain
/// words — the same character class [`is_section_heading_line`]'s own
/// label already requires — so a genuine synopsis fragment that merely
/// starts and ends with a dash for unrelated reasons is never mistaken for
/// this decorative shape.
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
/// `None` when that line does not open one.
///
/// # The defect
///
/// The third face of the indentation-alone promotion [`is_prose_sentence`]
/// and [`is_line_continuation_fragment`] each document one face of, and the
/// only one where suppressing the `group` is not enough. A paragraph that
/// hard-wraps with a hanging indent puts a *more-indented* line beneath an
/// ordinary sentence, so the scanner reads the sentence as a heading and
/// the rest of the sentence as its block — and when the wrap happens to
/// land on a dash-led word, that block is a flags block:
///
/// ```text
/// Use dpkg with -b, --build, -c, --contents, -e, --control, -I, --info,
///   -f, --field, -x, --extract, -X, --vextract, --ctrl-tarfile, --fsys-tarfile
/// on archives (type dpkg-deb --help).
/// ```
///
/// One sentence, naming another program's options so a reader knows not to
/// pass them here. `dpkg --help` acquires from it a section divider reading
/// `USE DPKG WITH -B, --BUILD, -C, --CONTENTS, …` and, under it, a `-f,
/// --field` option `dpkg` does not have.
///
/// Neither existing predicate can see this. [`is_prose_sentence`] requires
/// a full stop, and a wrap by definition breaks the line *before* the
/// sentence ends; [`is_line_continuation_fragment`] requires the author to
/// have marked the wrap with a backslash, which prose never does. And
/// suppressing the `group` alone would leave the fabricated flag behind,
/// merely ungrouped — so this one has to fence the region rather than
/// annotate it, the same containment shape the obscured-`Examples:` marker
/// uses (see `obscured_ignorable_indent`).
///
/// # The rule, and what each clause keeps out
///
/// The head line:
///
/// - **Ends with a comma.** The author's own statement that the line is not
///   finished — the wrap-marker prose does write, where a backslash is the
///   one a synopsis writes. A section heading is a label, and no label ends
///   mid-list; a colon-terminated heading ([`is_section_heading_line`]) and
///   a period-terminated sentence ([`is_prose_sentence`]) are both excluded
///   by construction, so this neither overlaps nor widens either of them.
/// - **Is a single field** ([`find_multi_space_gap`], the same test and the
///   same reason as [`is_prose_sentence`]): a line with an aligned column is
///   a table row, not running prose, whatever punctuation ends it.
/// - **Is at least [`MIN_PROSE_SENTENCE_WORDS`] words**, so a short
///   comma-carrying label can never qualify.
///
/// and the continuation must actually exist: the **immediately** next
/// physical line — no blank line may intervene, because a wrapped sentence
/// never contains one — is indented further and is itself a single field.
/// The region then runs while those two conditions keep holding, so it ends
/// at the first blank line, the first dedent to the head's own column, or
/// the first line carrying an aligned column. That last clause is what
/// bounds the fence to prose flow: a real option table's rows have a column
/// gap, so the region can never swallow one even if a comma-terminated line
/// somehow introduced it.
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

/// True when `heading` may open the obscured-marker whole-region fence
/// (`obscured_ignorable_indent`) — issue #77 edge 3.
///
/// [`is_ignorable_heading`] is a per-heading test: at every one of its other
/// call sites, a false positive suppresses at most the one heading's own
/// block. Reused as the trigger for a *whole-region* fence, the same
/// looseness stopped being bounded — a mid-document `Report bugs to
/// <maintainer@example.com>.` line, sitting under a lower-indented prose
/// sentence purely by document layout, fenced everything after it until the
/// next physical dedent. That line is a sentence (period-terminated, with
/// usage-grammar punctuation in the address), not a label, and no fence
/// trigger should treat it as one.
///
/// This adds [`is_section_heading_line`]'s own bar — short, colon-terminated,
/// plain-word label — on top of [`is_ignorable_heading`]'s vocabulary, so the
/// fence only opens on something that is heading-*shaped* as well as
/// heading-*worded*: `Examples:` and `Report bugs:` qualify; `Report bugs to
/// <maintainer@example.com>.` does not. `is_ignorable_heading` itself is left
/// untouched — it is correct at its other ~10 call sites, and this predicate
/// exists precisely so the fence stops borrowing it instead of tightening it
/// out from under them.
pub(super) fn is_obscured_fence_marker(heading: &str) -> bool {
    is_section_heading_line(heading) && is_ignorable_heading(heading)
}

/// Whether the obscured-marker fence (`obscured_ignorable_indent`) may close
/// at `lines[idx]`, given the marker's own indent — issue #77 edge 1.
///
/// The fence's original exits were a physical dedent below the marker's
/// indent, or [`starts_attested_flag_section`] at *exactly* the marker's
/// indent. Both are too narrow: a well-formed, positively-evidenced flag
/// section indented *deeper* than the marker, or a headingless flag block at
/// any indent at or past the marker's, previously had no exit at all and
/// stayed suppressed for the rest of the document.
///
/// The fix widens *which indents* may exit, while keeping the exit itself
/// exactly as evidence-gated as before — a fence any indented line can
/// reopen is not a fence:
///
/// - A physical dedent (`indent < marker_indent`) still exits unconditionally,
///   as it always did.
/// - [`starts_attested_flag_section`] — heading vocabulary plus
///   [`MIN_ATTESTED_SECTION_FLAGS`] independently parsed rows below it — now
///   qualifies at the marker's indent *or deeper*, not only at exactly it.
/// - A headingless run of at least [`MIN_ATTESTED_SECTION_FLAGS`] flag rows
///   ([`starts_attested_headingless_flag_block`]) is admitted as the same
///   evidence, since a headingless block can never satisfy a heading-vocabulary
///   test in the first place.
pub(super) fn obscured_fence_reopens(lines: &[&str], idx: usize, marker_indent: usize) -> bool {
    let indent = leading_whitespace(lines[idx]);
    if indent < marker_indent {
        return true;
    }
    starts_attested_flag_section(lines, idx) || starts_attested_headingless_flag_block(lines, idx)
}

/// Headingless counterpart of [`starts_attested_flag_section`]: `lines[idx]`
/// itself already looks like a flag row ([`looks_like_flag_start`]), no
/// heading-shaped line immediately governs it, and the block it opens
/// independently parses at least [`MIN_ATTESTED_SECTION_FLAGS`] rows.
///
/// The "no heading-shaped line immediately governs it" clause is load-
/// bearing, not a stylistic nicety. `labels_inside_indented_examples_do_not_
/// reopen_flag_parsing` (this file's sibling module) pins the shape that
/// requires it: a worked example writes ` Input:`/` Output:` labels — real
/// section headings by every structural test, just not ones naming CLI
/// vocabulary — directly over sample rows that are themselves dash-led
/// (`--fake-one VALUE   example input, not a supported option`). Dropping
/// the "no governing heading" requirement would read every one of those
/// rows as headingless and reopen on the same two-row floor, exactly
/// reproducing the ambiguity [`names_flag_section`]'s own doc comment
/// warns about — a label can govern `--flag`-shaped sample data as
/// plausibly as it can govern real flags. A row is trusted as headingless
/// only when nothing heading-shaped sits directly above it: `sed --help`'s
/// own `Options:`-free block, whose entries start on line one with nothing
/// above them at all, is the shape this clause is scoped to admit.
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

/// Longest label this will accept before a `:` still counts as a section
/// heading. Real headings are a few words (`command specific modifiers:`,
/// `Available Commands:`); a long colon-terminated line is prose.
pub(super) const MAX_HEADING_LABEL: usize = 60;

/// True if `t` (already trimmed of leading whitespace) is a section
/// heading: a short, colon-terminated label of plain words.
///
/// The plain-words test is what keeps usage grammar out. Every delimiter
/// the docopt-style synopsis grammar uses (`[`, `<`, `{`, `|`, `=`, `.`)
/// is excluded from the label, so a wrapped synopsis fragment can never
/// qualify however it is indented, while ` commands:` and ` generic
/// modifiers:` both do. The colon must terminate the whole line: a
/// synopsis carrying an interior colon (`host:port`) is untouched.
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
/// `--help` output: `asn1parse   ca   ciphers   cmp`) rather than prose or
/// a flag spec — every column is name-shaped (starts with a letter,
/// otherwise only alphanumerics/`-`/`_`) and none starts with `-` (which
/// would make it a flag entry instead).
///
/// Used to *continue* a grid already started by
/// [`looks_like_word_grid_start`], so it accepts a lone trailing token
/// (openssl's final `x509` on its own line) as well as a multi-column
/// row. Multi-column rows are held to the same 2+-space column rule as
/// the start line: continuing on single-spaced prose is how a grid that
/// began legitimately would still end up swallowing a paragraph.
pub(super) fn looks_like_word_grid_line(line: &str) -> bool {
    let columns = split_columns(line);
    if columns.is_empty() {
        return false;
    }
    columns.iter().all(|c| is_name_shaped_token(c))
}

/// Stricter version used only to *start* a grid: requires 3+ **columns**,
/// so a two-word heading immediately above the grid (`"Standard commands"`)
/// is never itself mistaken for the first grid row. Once a grid has
/// started, [`looks_like_word_grid_line`] (which allows a trailing
/// single-token row, e.g. openssl's lone `x509` closing out a section) is
/// used to keep consuming it.
///
/// "Column" means a field separated from its neighbours by a run of **two
/// or more** spaces, not merely by whitespace. That distinction is the
/// whole guard against reading a wrapped prose paragraph as a command
/// list: a real grid is laid out in aligned columns
/// (`asn1parse         ca                ciphers`), while prose separates
/// its words with exactly one space. Without it, `apt-get --help` gained
/// the subcommands *"and"*, *"information"*, *"about"*, *"them"*,
/// *"from"*, *"authenticated"* and *"sources"* — every word of its
/// description paragraph past the first line — because the sentence above
/// it ("apt-get is a **command** line interface for retrieval of
/// packages") contains the word "command" and so passed
/// [`is_recognized_command_heading`], and the paragraph's own lines are
/// all name-shaped words at a matching indent. That is [M-10] exactly:
/// fabricated structure a user cannot tell is wrong. Column alignment is
/// a structural property of the layout, so this stays a general rule
/// rather than anything keyed to a tool or a framework.
pub(super) fn looks_like_word_grid_start(line: &str) -> bool {
    let columns = split_columns(line);
    columns.len() >= 3 && columns.iter().all(|c| is_name_shaped_token(c))
}

/// True if `lines` is a rendered man page rather than `--help` output.
///
/// The signal is the page banner every `man` renderer emits: a first line
/// carrying the same `NAME(section)` title at both the left and right
/// margins, e.g. `GIT-BISECT(1)    Git Manual    GIT-BISECT(1)`. That is a
/// property of the roff output format, not of any tool or framework, and
/// no `--help` summary looks like it.
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

/// Public wrapper around [`looks_like_man_page`], for the coverage harness
/// (spec §13.1, [M-16]) to reuse rather than reimplement.
///
/// [M-16] proposes falling back to `-h` when `--help` renders a man page
/// (git's subcommands do this; its root does not, and that distinction is
/// exactly what this function exists to get right). Before that fallback
/// can be sent — an argv broadening the maintainer has ruled must be
/// measured first, not assumed — something has to enumerate which tools on
/// `PATH` would newly receive it. That enumeration must not spawn a second
/// probe of its own (spec §6: every invocation is measured, unmeasured
/// broadening is the exact hazard [M-16] is about), so it re-runs this
/// *same* detection over text the pipeline already captured — a tool's
/// `CommandNode::unparsed` line, set by [`super::build_node`] precisely
/// when this check fired (or when nothing else parsed for some other
/// reason; the caller re-checks here to tell those two apart) — instead of
/// touching the tool a second time.
///
/// Kept as a thin wrapper rather than inlined at the call site so there is
/// exactly one definition of "looks like a rendered man page": duplicating
/// the rule for a caller outside this module is how the two copies would
/// eventually drift, and this one is about to gate a safety decision.
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
/// Tier B rule 1's literal generic test (mentions "command(s)" or
/// "subcommand(s)" as a word, or — the extension below — "operation(s)"),
/// or — when a framework was identified — one of that framework's own
/// extra heading markers ([`FrameworkProfile::command_heading_markers`]).
/// A framework profile asserting [`FrameworkProfile::no_subcommand_concept`]
/// overrides both: it means this framework's help output structurally
/// never has subcommands, so no heading of any kind should ever be
/// recognized here — the direct fix for [M-10] (spec §7 Tier B rule 1:
/// "must produce zero subcommands"), made structural instead of incidental
/// to which exact words one tool's heading happens to use.
///
/// This is *not* the whole test — a heading can also qualify by being
/// part of a chain started by such a mention elsewhere (git's group
/// headings) — see [`command_mode_seed`] and `command_mode` in
/// [`parse_with_profile`].
///
/// # The "operations" extension (llvm-ar operations table)
///
/// `llvm-ar --help` documents its single-letter operations (`d`, `m`,
/// `p`, ...) under an `OPERATIONS:` heading — the same class of table as
/// `ar`'s and `llvm-ar`'s own `MODIFIERS:` block, and the same kind of
/// evidence rule 1 already accepts for "command(s)": an operation letter
/// *is* an invocation verb (`llvm-ar d archive.a file.o`), just as a
/// subcommand name is. `binutils ar`'s equivalent table sits under a
/// heading that already says "commands" and needed no change.
///
/// Measured over the 2,301 frozen captures in `audit/queue-captures/`:
/// **22 tools** carry a heading whose text mentions "operation"/
/// "operations". Of those, **20** (`autoconf`, `autom4te`, `automake`,
/// `automake-1.16`, `autoreconf`, `autoupdate`, `btrfsck`, `cpio` ×7,
/// `envsubst`, `jar` ×3, `m4`, `man`, `mount`, `msgcmp`, `msgfmt`,
/// `msgmerge` ×2, `msgunfmt`, `pygmentize` ×2, `tar` ×2, `xgettext` — some
/// tools carry more than one such heading) head an ordinary flags table
/// (`Operation modes:`, `Main operation mode:`, `Operation modifiers
/// valid in copy-in mode:`, `mount`'s `Operations:`, ...): every row is
/// flag-shaped (`-h, --help`, `-B, --bind`), so
/// [`super::flags_block_start`] claims the block as flags *before* this
/// predicate is ever consulted (`parse_with_profile`'s flags-block check
/// runs first and `continue`s the loop) — this extension cannot touch
/// them regardless of what vocabulary it admits. The remaining **2**
/// (`llvm-ar`'s `OPERATIONS:` and `jmod`'s `Main operation modes:`, both
/// `corpus/llvm-ar-18/18.1.3` and a real fixture candidate respectively)
/// are genuine tables of one-word invocation verbs with a ` - `-separated
/// description each — precisely the shape this extension exists to
/// recover, and precisely nothing else in the measured fleet has that
/// shape under this vocabulary. No false positive was found; the only
/// near-miss (`mount`'s bare `Operations:` heading over an actual flags
/// table) is closed structurally by the flags-block gate above, not by
/// narrowing the word list.
///
/// **This vocabulary is deliberately not folded into
/// [`mentions_commands_word`] and does not reach [`command_mode_seed`].**
/// `command_mode_seed` reads a tool's own *description prose*, not a
/// heading, and seeds a sticky chain that later headings inherit; the
/// same 2,301 captures show **141 tools** with the word "operation"/
/// "operations" *somewhere* in their `--help` text (an upper bound on
/// what a shared vocabulary would expose `command_mode_seed` to — most of
/// that is ordinary English, e.g. "This performs a destructive
/// operation"). Seeding a sticky command-list chain from that word in
/// prose, fleet-wide, is a materially different and far riskier claim
/// than recognizing it in a heading that introduces an indented block,
/// so the extension is scoped to heading recognition only.
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
/// override as [`is_recognized_command_heading`]: a framework with no
/// subcommand concept must never have `command_mode` turned on by a prose
/// mention either, since that mention almost certainly isn't about a
/// command list this framework doesn't have (e.g. a GNU-argp tool's
/// `--help` prose mentioning "commands" in an unrelated sentence).
pub(super) fn command_mode_seed(text: &str, profile: Option<&FrameworkProfile>) -> bool {
    if profile.is_some_and(|p| p.no_subcommand_concept) {
        return false;
    }
    mentions_commands_word(text)
}

/// Find the index of the flag in `flags` that `heading` is **provably**
/// "nested under" (spec §7 Tier B rule 4) — two literal proofs, nothing
/// else. `None` means ownership is unproven, and the caller attaches
/// **nothing**: no names, no descriptions.
///
/// **This function used to guess.** A third branch fell back to
/// `flags.len() - 1` — "the most recently emitted flag" — whenever neither
/// proof fired, on the theory that an unlabeled enum list conventionally
/// follows the flag it enumerates with nothing else in between (tar's
/// `--format=FORMAT` immediately followed by `"FORMAT is one of the
/// following:"`). Measured directly, twice, that theory does not hold:
///
/// - `cp`'s trailing `VERSION_CONTROL` enum (which documents `--backup`)
///   attached to `--version` instead, because several unrelated prose
///   paragraphs sit between the flags table ending and the block — the
///   "most recently emitted flag" was simply whatever printed last, not
///   the block's owner. A proximity check (was this block the very next
///   thing scanned after a flags block ended) fixes this specific case.
/// - `automake`'s `"Warning categories include:"` block documents its own
///   `-W, --warnings=CATEGORY`, ten lines earlier — but attached to
///   `-f, --force-missing`, the *actual* last flag before the block, with
///   only a blank line between them. That is the same tight adjacency
///   shape as tar's correct case, so a proximity check approves it too —
///   confidently, and wrongly. Proximity cannot tell these two shapes
///   apart, because the true axis isn't distance, it's whether the
///   fallback's candidate is the real owner at all.
///
/// No adjacency signal separates "confidently right" (tar) from
/// "confidently wrong" (automake), so the fallback is gone. Ownership is
/// proven exactly two ways:
///
/// 1. The heading names the flag's long spelling literally
///    (`"Valid arguments for the --quoting-style option are:"` names
///    `--quoting-style` directly).
/// 2. The heading contains one candidate flag's `value_name`, verbatim, as
///    a whole word (case-insensitive) — tar's `FORMAT is one of the
///    following:` names `--format=FORMAT`'s own placeholder. This is
///    deliberately a literal word match, not a stem/plural/morphological
///    one: `automake`'s heading says "categories" against a `CATEGORY`
///    value_name, and admitting that match is exactly the false positive
///    that would reproduce the `-f`/`-W` misattribution one level up the
///    stack (see the follow-up issue for what a real fix needs).
///
/// Neither proof favors a "most recent" or "closest" candidate over
/// another that also matches — both scan every flag in `flags`, in order,
/// and take the first hit, same as the original name-match branch always
/// did.
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
            // A one-character value_name is never a real placeholder — no
            // tool author writes a single-letter metavar, since it would
            // be indistinguishable from a short flag on the same row. It
            // is always the signature of an unrelated parser artifact:
            // ffplay's own `-fs` (force full screen) misreads as short
            // `-f` plus value_name `"s"` (a pre-existing, separate
            // single-dash-multi-character defect, spec Appendix A's
            // `as`/`-fdump-*` family — see AGENTS.md's "GCC's single-dash
            // multi-character convention" entry), and across an
            // 1100-flag document a bare one-letter token like `"s"`
            // coincidentally appears as its own word in unrelated
            // headings dozens of times. Measured directly: without this
            // guard, `ffplay`'s corpus fixture moved a described choices
            // block onto `-fs` from a heading that has nothing to do with
            // it. Excluding length-1 value_names costs nothing real —
            // every genuine GNU-style placeholder (`FORMAT`, `CATEGORY`,
            // `CONTROL`, `MODE`) is a whole word already.
            vn.chars().count() > 1 && heading_contains_word(&lower, vn)
        })
    })
}

/// True when `word` (any case) appears in `lower_haystack` (already
/// lowercased) as a whole token — split on any non-alphanumeric byte, so
/// `FORMAT` matches `" FORMAT is one of the following:"` but does not
/// match inside `FORMATS` or `REFORMAT`. Case-insensitive because a
/// heading's own capitalization of a placeholder word is not guaranteed to
/// match the value_name's (both are typically all-caps in practice, but
/// nothing enforces it).
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
                // `treat_as_commands` is only ever `true` when the grid's
                // heading was `recognized` or the parser was already in
                // `command_mode` (see the caller) — i.e. this entry has
                // exactly the positive evidence spec issue #2 asks
                // `structure_sanity` to trust, even though a word-grid
                // entry carries no per-entry description (openssl's
                // `asn1parse`, `ciphers`, ...).
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

/// Emit a flags block's entries as [`Flag`]s. `group` is `None` for a
/// headingless block (spec §7 Tier B rule 2's continuation handling
/// already folded wrapped descriptions in during scanning).
/// A flags block's heading as a display *group*, or `None` when the
/// heading is just the generic "here are the flags" label.
///
/// `Flag::group` exists to preserve meaningful subdivisions — tar's 171
/// flags under headings like "Main operation mode" are the difference
/// between a scannable pane and a wall of text. A heading that only says
/// "Options" or "Flags" subdivides nothing: it names the section the
/// detail pane already prints its own `FLAGS` heading for, so keeping it
/// rendered `FLAGS` twice in a row (visible on `gh`, whose help output
/// titles that section `FLAGS`).
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

    /// `dpkg --help` closes its command list with one sentence, wrapped
    /// across three physical lines, that names *another program's* options
    /// so a reader knows not to pass them to this one. The hanging indent
    /// makes line two more-indented than line one, so the indentation-alone
    /// heading rule reads the sentence as a section heading over a flags
    /// block — and because the wrap lands on `-f,`, that block parses.
    ///
    /// Both halves of the fabrication have to go: the divider the flags
    /// pane renders from the `group`, and the `-f, --field` option `dpkg`
    /// does not have. The real `Options:` table beneath must be untouched.
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

    /// The fence is bounded by shape, not by how far the paragraph runs: a
    /// genuine, column-aligned option table directly beneath a
    /// comma-terminated line is a table row, not sentence flow, so the
    /// region must end before it and the rows must still be recovered.
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
    /// contains one. Without that clause the region would run from a
    /// comma-terminated trailing line straight through the blank separator
    /// and swallow whatever section came next.
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

    /// Regression for [M-10], found by reading the real TUI rather than a
    /// green test suite: `apt-get --help` gained the subcommands *"and"*,
    /// *"information"*, *"about"*, *"them"*, *"from"*, *"authenticated"*
    /// and *"sources"* — the words of its own description paragraph past
    /// the first line. The paragraph's opening sentence ("apt-get is a
    /// **command** line interface for retrieval of packages") satisfied
    /// the recognized-command-heading test, and the wrapped lines beneath
    /// it are all name-shaped words at a matching indent, so the
    /// bare-name grid parser (which exists for openssl's genuinely
    /// column-aligned command grid) consumed the prose.
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

    /// Regression for spec [M-8]: `openssl --help` writes only to stderr,
    /// with no `Usage:` line and no indentation at all — commands are a
    /// same-indent word grid (`asn1parse   ca   ciphers   cmp`). A tier
    /// that only recognized indented blocks produced nothing here.
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

    /// The exact shape `git bisect --help` renders (`man`'s own banner
    /// convention: identical `NAME(section)` token at both margins around
    /// a centred title) — a true positive.
    #[test]
    fn is_man_page_banner_true_positive_on_a_real_banner_shape() {
        let rendered = "GIT-BISECT(1)                Git Manual                GIT-BISECT(1)\n\n\
                         NAME\n       git-bisect - Use binary search to find the commit...\n";
        assert!(is_man_page_banner(rendered));
    }

    /// git's *root* `--help` is conventional help text, not a man page —
    /// [M-16]'s whole subtlety is that this must come back false. If this
    /// ever flips true, the detection is firing in the wrong place (spec
    /// §7 Tier B step 3 is meant for subcommands like `git bisect`, not
    /// the root `git --help`, which parses cleanly today).
    #[test]
    fn is_man_page_banner_is_false_on_gits_own_root_help() {
        assert!(!is_man_page_banner(GIT_HELP));
    }

    /// Ordinary `--help` output — even output that starts with a single
    /// all-caps word — is not a false positive: a repeated *single* word is
    /// not a banner (there must be a centred title between the two
    /// margins), and `tar`'s help doesn't repeat its own name at both ends
    /// of its first line at all.
    #[test]
    fn is_man_page_banner_is_false_on_ordinary_help_text() {
        assert!(!is_man_page_banner(TAR_HELP));
        assert!(!is_man_page_banner("USAGE USAGE\n"));
    }

    /// Public wrapper delegates to exactly the same rule the parser itself
    /// uses to decide whether to degrade to verbatim — not a second,
    /// possibly-drifted copy.
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

    /// `nano 7.2`'s real preamble and the head of its option table,
    /// byte-exact from `corpus/nano/7.2/help.txt`.
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
        // The rows themselves are untouched: this suppresses a field, it
        // does not decline the block.
        assert_eq!(
            flag_named(&parsed, "smarthome")
                .description
                .as_ref()
                .map(|t| t.as_str()),
            Some("Enable smart home key")
        );
    }

    /// The GNU convention, and the largest single share of the family:
    /// 56 of the 205 affected tools in `audit/queue-captures/` inherit
    /// exactly this sentence.
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

    /// The inverse direction, and the reason the prose test is anchored on
    /// the *full stop* rather than on wording: `gcc`/`lto-dump` writes
    /// section headings that are complete English sentences, and they are
    /// real headings over real blocks. A wording- or length-based test
    /// would have destroyed every one of them.
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

    /// A period-terminated *row* is a table row, not a sentence — the
    /// column gap is what tells them apart. `arptables` writes both
    /// shapes in the same document.
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
    /// invocation form. Two defects in one document: the wrapped tail
    /// begins with `--id`, which ended the usage block and lost `--del`
    /// with it, and the backslash-terminated line above it was then read
    /// as a section heading.
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

    /// `uconv --help`, byte-exact: the heading and the first option row
    /// share one physical line, and before the split `-h, --help` was in
    /// the tree under no spelling at all.
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
        // `Options:` is one of `meaningful_flag_group`'s generic labels,
        // so the recovered heading names no group — and neither does the
        // whole line any more.
        for flag in &parsed.flags {
            assert_eq!(flag.group, None, "{} kept a group", flag.spelling());
        }
    }

    #[test]
    fn a_heading_line_whose_remainder_is_not_a_flag_is_never_split() {
        // `ntfs-3g`'s real line: label, column gap, and then a *value*
        // list. Splitting it would hand the block a row that is not a row.
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
        // `ip`'s real line: the colon reads as `:=`, not a plain heading
        // colon, so the original column-gap clause (zero spaces right
        // after `:`) never fired and `-V`/`-s`/`-d`/`-r` were eaten by the
        // heading string. The recovered row is re-indented to column 20,
        // matching the continuation lines `ip` itself wraps to.
        // The opening bracket is stripped along with the operator, not kept
        // in the row: the continuation lines this heading introduces
        // (`-h[uman-readable] | -iec | ...`) never carry it either, and
        // downstream flag-row parsing expects a bare flag at the row's
        // start.
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
        // `ip`'s own `OBJECT` production and `ss`'s grammar productions all
        // use the same `:=` operator but open on a bare word, never a flag
        // spelling — clause 4 must reject every one of them.
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
        // `pkgdata`'s `modes: (-m option)`: a bracket immediately follows
        // the colon, but with no `=` — this is the false positive that a
        // bracket-without-operator version of clause 3 would invent, since
        // the remainder `-m option)` does satisfy `looks_like_flag_start`
        // on its own. Requiring the BNF operator keeps this line intact.
        assert_eq!(split_shared_heading_row("modes: (-m option)"), None);
    }

    /// Spec §6's attestation gate reads `CommandNode::heading_attested`,
    /// which is what decides whether a recovered word may become
    /// `<word> --help` probe argv. Group suppression must not touch it in
    /// either direction, and above all must never make a word probe-
    /// eligible that was not.
    ///
    /// The pair below is the proof: the same block under a real command
    /// heading and under a prose sentence. The real heading attests its
    /// entries; the prose sentence recovers no commands at all, before
    /// this change or after it. Nothing this change does can move a node
    /// from the second document into the first.
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
