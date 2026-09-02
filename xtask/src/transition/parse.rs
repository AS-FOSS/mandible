//! Parsing a rendered [`crate::coverage::ScoreFormat::Text`] scoreboard
//! back into a [`super::ParsedScoreboard`] — the fixed-offset column
//! reader and the `#fp`/`#fp2` footer-line dispatch.

use super::fingerprint::{parse_fingerprint_line, FP_LINE_PREFIX_V1, FP_LINE_PREFIX_V2};
use super::{FingerprintFormat, ParsedRow, ParsedScoreboard};
use crate::coverage::{
    BUNDLE_COL_WIDTH, EXISTENCE_COL_WIDTH, FLAGS_COL_WIDTH, FRAMEWORK_COL_WIDTH, MAN_COL_WIDTH,
    MISATTR_COL_WIDTH, MS_COL_WIDTH, NODES_COL_WIDTH, PCT_COL_WIDTH, SUSPECT_COL_WIDTH,
    TIER_COL_WIDTH, TOOL_COL_WIDTH,
};

/// True when `header` is (or resembles) a scoreboard's own header line,
/// used only to decide whether it carries the `misattr` column added after
/// the misattribution detector shipped — every scoreboard from before that
/// (this task found four real ones, captured during earlier work on this
/// branch) has ten columns instead of eleven (or twelve, with `exist` too)
/// and needs a different offset for the trailing `status` column. See
/// [`row_offsets`].
fn has_misattr_column(header: &str) -> bool {
    header.contains("misattr")
}

/// Same idea as [`has_misattr_column`], for the `exist` column
/// ([`crate::existence`], this task) appended after `misattr` — every
/// scoreboard from before this task has eleven columns (or ten, with no
/// `misattr` either) and needs the shorter offset for `status`.
fn has_existence_column(header: &str) -> bool {
    header.contains("exist")
}

/// Same idea again, for the `bundle` column ([`crate::bundling`]) appended
/// after `exist` — every scoreboard from before it has twelve columns or
/// fewer and needs the shorter offset for `status`.
fn has_bundle_column(header: &str) -> bool {
    header.contains("bundle")
}

/// The exact character offsets [`crate::coverage::render_text`] writes each
/// column at, derived from the same width constants that function uses.
/// The three `with_*` flags select among the four layouts a real
/// scoreboard can have (ten columns, +`misattr`, +`exist`, +`bundle`),
/// since each detector only ever appended a column. The optional three are
/// laid out as a chain, each starting where the last present one ended.
struct RowOffsets {
    tool: (usize, usize),
    tier: (usize, usize),
    framework: (usize, usize),
    nodes: (usize, usize),
    flags: (usize, usize),
    pct: (usize, usize),
    ms: (usize, usize),
    suspect: (usize, usize),
    man: (usize, usize),
    misattr: Option<(usize, usize)>,
    existence: Option<(usize, usize)>,
    bundle: Option<(usize, usize)>,
    status_start: usize,
}

fn row_offsets(with_misattr: bool, with_existence: bool, with_bundle: bool) -> RowOffsets {
    let tool = (0, TOOL_COL_WIDTH);
    let tier = (tool.1 + 1, tool.1 + 1 + TIER_COL_WIDTH);
    let framework = (tier.1 + 1, tier.1 + 1 + FRAMEWORK_COL_WIDTH);
    let nodes = (framework.1 + 1, framework.1 + 1 + NODES_COL_WIDTH);
    let flags = (nodes.1, nodes.1 + FLAGS_COL_WIDTH);
    let pct = (flags.1, flags.1 + PCT_COL_WIDTH);
    let ms = (pct.1, pct.1 + MS_COL_WIDTH);
    let suspect = (ms.1, ms.1 + SUSPECT_COL_WIDTH);
    let man = (suspect.1, suspect.1 + MAN_COL_WIDTH);
    let mut end = man.1;
    let mut append = |present: bool, width: usize| -> Option<(usize, usize)> {
        if !present {
            return None;
        }
        let range = (end, end + width);
        end = range.1;
        Some(range)
    };
    let misattr = append(with_misattr, MISATTR_COL_WIDTH);
    let existence = append(with_existence, EXISTENCE_COL_WIDTH);
    let bundle = append(with_bundle, BUNDLE_COL_WIDTH);
    let status_start = end + 2;
    RowOffsets {
        tool,
        tier,
        framework,
        nodes,
        flags,
        pct,
        ms,
        suspect,
        man,
        misattr,
        existence,
        bundle,
        status_start,
    }
}

/// Slice `chars[start..end]` as a trimmed `String`, or `None` if the line
/// is too short to contain that field at all (a corrupt/truncated line).
fn slice(chars: &[char], range: (usize, usize)) -> Option<String> {
    let (start, end) = range;
    if chars.len() < end {
        return None;
    }
    Some(
        chars[start..end]
            .iter()
            .collect::<String>()
            .trim()
            .to_string(),
    )
}

/// Parse one data line into a [`ParsedRow`], or a reason it was dropped.
///
/// The success variant is boxed because the other two carry nothing at all,
/// and every added scoreboard column grows `ParsedRow` a little further past
/// the point where the whole enum costs a rejected line as much memory as an
/// accepted one (`clippy::large_enum_variant`, which the `bundle` column
/// tipped over).
enum LineResult {
    Row(Box<ParsedRow>),
    Truncated,
    Unparseable,
}

fn parse_line(line: &str, offsets: &RowOffsets) -> LineResult {
    let chars: Vec<char> = line.chars().collect();

    let Some(tool) = slice(&chars, offsets.tool) else {
        return LineResult::Unparseable;
    };
    // Detection mirrors `coverage::truncate_col` exactly: it never leaves a
    // string shorter than `width` chars once it truncates, and it only
    // ever appends `…` in that path. A tool name that happens to be
    // naturally exactly `TOOL_COL_WIDTH` chars long *and* end in a literal
    // `…` is the one false positive this shares with the renderer itself;
    // accepted for the same reason `truncate_col`'s own doc comment
    // accepts it (real tool names are overwhelmingly ASCII).
    if tool.chars().count() == TOOL_COL_WIDTH && tool.ends_with('…') {
        return LineResult::Truncated;
    }
    if tool.is_empty() {
        return LineResult::Unparseable;
    }

    let Some(tiers) = slice(&chars, offsets.tier) else {
        return LineResult::Unparseable;
    };
    let Some(framework) = slice(&chars, offsets.framework) else {
        return LineResult::Unparseable;
    };
    let Some(nodes_s) = slice(&chars, offsets.nodes) else {
        return LineResult::Unparseable;
    };
    let Some(flags_s) = slice(&chars, offsets.flags) else {
        return LineResult::Unparseable;
    };
    let Some(pct_s) = slice(&chars, offsets.pct) else {
        return LineResult::Unparseable;
    };
    let Some(ms_s) = slice(&chars, offsets.ms) else {
        return LineResult::Unparseable;
    };
    let Some(suspect_s) = slice(&chars, offsets.suspect) else {
        return LineResult::Unparseable;
    };
    let Some(man_s) = slice(&chars, offsets.man) else {
        return LineResult::Unparseable;
    };
    let misattribution_suspect_count = match offsets.misattr {
        Some(range) => match slice(&chars, range) {
            Some(s) => match s.parse::<usize>() {
                Ok(n) => Some(n),
                Err(_) => return LineResult::Unparseable,
            },
            None => return LineResult::Unparseable,
        },
        None => None,
    };
    let existence_fabrication_count = match offsets.existence {
        Some(range) => match slice(&chars, range) {
            Some(s) => match s.parse::<usize>() {
                Ok(n) => Some(n),
                Err(_) => return LineResult::Unparseable,
            },
            None => return LineResult::Unparseable,
        },
        None => None,
    };
    let bundle_collapse_count = match offsets.bundle {
        Some(range) => match slice(&chars, range) {
            Some(s) => match s.parse::<usize>() {
                Ok(n) => Some(n),
                Err(_) => return LineResult::Unparseable,
            },
            None => return LineResult::Unparseable,
        },
        None => None,
    };
    if chars.len() < offsets.status_start {
        return LineResult::Unparseable;
    }
    let status: String = chars[offsets.status_start..].iter().collect::<String>();
    let status = status.trim().to_string();
    if status.is_empty() {
        return LineResult::Unparseable;
    }

    let (Ok(nodes), Ok(flags), Ok(ms), Ok(suspicious_nodes)) = (
        nodes_s.parse::<usize>(),
        flags_s.parse::<usize>(),
        ms_s.parse::<u128>(),
        suspect_s.parse::<usize>(),
    ) else {
        return LineResult::Unparseable;
    };
    let pct_flags_with_text = pct_s
        .trim_end_matches('%')
        .parse::<f64>()
        .ok()
        .filter(|_| pct_s != "—" && pct_s != "-");
    let man_shaped = man_s == "yes";

    LineResult::Row(Box::new(ParsedRow {
        tool,
        tiers,
        framework,
        nodes,
        flags,
        pct_flags_with_text,
        ms,
        suspicious_nodes,
        man_shaped,
        misattribution_suspect_count,
        existence_fabrication_count,
        bundle_collapse_count,
        status,
    }))
}

/// Parse a rendered [`crate::coverage::ScoreFormat::Text`] scoreboard back
/// into rows. Every `#`-prefixed line (the aggregate footer and every
/// informational section after it — `render_text`'s own convention) and
/// every blank line is skipped; the header line is used only to detect
/// [`has_misattr_column`] and is never itself parsed as data.
pub fn parse_scoreboard(text: &str) -> ParsedScoreboard {
    let mut out = ParsedScoreboard::default();
    let mut lines = text.lines();
    let Some(header) = lines.find(|l| !l.trim().is_empty()) else {
        return out;
    };
    let offsets = row_offsets(
        has_misattr_column(header),
        has_existence_column(header),
        has_bundle_column(header),
    );

    // Two passes over the same remaining lines: the first (data rows) stops
    // at the first `#`-prefixed line exactly as before; the second (the
    // `#fp` fingerprint footer, added by this task) scans every remaining
    // line regardless, since fingerprint lines live *after* every other
    // footer section (`coverage::render_text`'s emission order) and would
    // never be reached by a loop that breaks on the first `#`. `Lines` is
    // `Clone` (a cheap cursor over the same borrowed `&str`), so this costs
    // no extra allocation or re-reading of the file.
    let footer = lines.clone();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        // Every footer section (`# aggregate: ...`, `# accuracy: ...`,
        // `# framework-detection: ...`, `# worst-parsed ...`, `# ... N.
        // ...`, `# misattribution-suspects ...`) starts with `#` — the one
        // and only marker `render_text` ever uses for non-data lines
        // (`aggregate_footer_line`, `accuracy_unmeasured_line`,
        // `framework_summary_lines`, `worst_parsed_lines_text`,
        // `misattribution_sample_lines_text`, grepped). Once one appears,
        // every remaining line is footer, so stop scanning for data rows
        // entirely rather than re-testing each one.
        if line.starts_with('#') {
            break;
        }
        match parse_line(line, &offsets) {
            LineResult::Row(row) => {
                out.rows.insert(row.tool.clone(), *row);
            }
            LineResult::Truncated => out.truncated_dropped += 1,
            LineResult::Unparseable => out.unparseable_dropped += 1,
        }
    }
    for line in footer {
        if let Some(rest) = line.strip_prefix(FP_LINE_PREFIX_V2) {
            if let Some((tool, fp)) = parse_fingerprint_line(rest) {
                out.fingerprints.insert(tool, fp);
                out.fingerprint_format = Some(FingerprintFormat::V2);
            }
        } else if let Some(rest) = line.strip_prefix(FP_LINE_PREFIX_V1) {
            if let Some((tool, fp)) = parse_fingerprint_line(rest) {
                out.fingerprints.insert(tool, fp);
                // Never downgrade an already-detected V2 to V1 — a
                // genuinely mixed file can't happen from one xtask binary,
                // but if it somehow did, V2 (the richer, current format) is
                // the more informative reading to keep.
                if out.fingerprint_format != Some(FingerprintFormat::V2) {
                    out.fingerprint_format = Some(FingerprintFormat::V1);
                }
            }
        }
    }
    out
}
