//! `description-continuation-dash-flag` (atlas S-140): a two-column row's
//! own wrapped description continues onto a physical line that opens, at
//! that row's own description column, with a dash-led token
//! (`fail2ban-client`'s `get <JAIL> banip [<SEP>|--with-time] ... the
//! option '` wraps onto `--with-time' (printing the times`). The generic
//! layout engine's heading fallback misreads the row above as a section
//! heading and the continuation itself as a fresh flag row, valued at the
//! trailing quote.
//!
//! Two independently measured halves, kept as separate [`Detector`]s
//! since one can occur without the other: [`RawContinuationShape`] reads
//! only `raw` (the structural signal — a continuation aligned on a real
//! description column, opening with a dash) and [`BareQuoteValue`] reads
//! only `root` (the tree artifact this shape produces — a flag whose
//! value is nothing but a single quote character). `family()` is `None`
//! for both: no seed-2/4/5/6/7 audit tool carries this shape.

use crate::detector::{Detector, Expect, Scope, SelfCheck, ToolEvidence};
use mandible_core::{CommandNode, Entity, Provenance, Source};

/// The byte column of `line`'s own description, if it has one: the first
/// run of 2+ spaces after some non-blank content, one space past the run.
/// A local, independent re-derivation — not an import of the grammar's
/// own column-gap finder — so this oracle cannot agree with the parser it
/// is meant to check merely by construction.
fn description_column(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut seen_content = false;
    while i < bytes.len() {
        if bytes[i] != b' ' {
            seen_content = true;
            i += 1;
            continue;
        }
        if !seen_content {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i] == b' ' {
            i += 1;
        }
        if i - start >= 2 && i < bytes.len() {
            return Some(i);
        }
    }
    None
}

/// One raw-shape finding: a continuation line that opens, at the column
/// the previous line's own description started, with a dash.
pub struct ShapeFinding {
    pub row: String,
    pub continuation: String,
}

/// One tree-artifact finding: a flag whose value is a bare quote.
pub struct ValueFinding {
    pub spelling: String,
}

pub struct Report {
    pub shape: Vec<ShapeFinding>,
    pub value: Vec<ValueFinding>,
}

impl Report {
    pub fn shape_count(&self) -> usize {
        self.shape.len()
    }
    pub fn value_count(&self) -> usize {
        self.value.len()
    }
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let lines: Vec<&str> = raw.lines().collect();
    let mut shape = Vec::new();
    // The description column stays owned by the row that opened it for
    // every physical line that continues it — not just the one directly
    // beneath the row — since a wrapped description can span several
    // lines before the dash-led one appears (fail2ban-client's own
    // `banip` row wraps five lines deep). Reset whenever a line neither
    // opens a fresh description column of its own nor sits exactly at
    // the one already open.
    let mut owner: Option<(&str, usize)> = None;
    for line in &lines {
        let indent = line.len() - line.trim_start().len();
        if let Some((row, col)) = owner {
            if indent == col {
                let trimmed = line.trim_start();
                if trimmed.starts_with('-') {
                    shape.push(ShapeFinding {
                        row: row.trim().to_string(),
                        continuation: trimmed.to_string(),
                    });
                }
                continue;
            }
        }
        owner = description_column(line).map(|col| (*line, col));
    }
    let mut value = Vec::new();
    for f in root.flags() {
        if f.value_name.as_deref() == Some("'") {
            value.push(ValueFinding {
                spelling: f.spelling(),
            });
        }
    }
    Report { shape, value }
}

pub struct RawContinuationShape;

impl Detector for RawContinuationShape {
    fn name(&self) -> &'static str {
        "description-continuation-dash-flag-shape"
    }
    fn family(&self) -> Option<&'static str> {
        None
    }
    fn describes(&self) -> &'static str {
        "a two-column row's wrapped description continues on a physical line opening, at the \
         row's own description column, with a dash-led token"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        detect(evidence.raw, evidence.root)
            .shape
            .iter()
            .map(|f| {
                format!(
                    "{:?} opens at the description column of {:?}",
                    f.continuation, f.row
                )
            })
            .collect()
    }
    fn scope(&self) -> Scope {
        Scope::full()
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        shape_self_checks()
    }
}

pub struct BareQuoteValue;

impl Detector for BareQuoteValue {
    fn name(&self) -> &'static str {
        "description-continuation-dash-flag-value"
    }
    fn family(&self) -> Option<&'static str> {
        None
    }
    fn describes(&self) -> &'static str {
        "a flag whose value is nothing but a single quote character — the tree artifact this \
         shape produces once the raw continuation is read as a flag row"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        detect(evidence.raw, evidence.root)
            .value
            .iter()
            .map(|f| {
                format!(
                    "{:?} carries a bare quote character as its value",
                    f.spelling
                )
            })
            .collect()
    }
    fn scope(&self) -> Scope {
        Scope::full()
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        value_self_checks()
    }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

/// `fail2ban-client`'s own bytes, trimmed to the offending row: a
/// command-table entry whose description wraps across several lines, one
/// of which opens with a dash-quoted flag mention.
pub(crate) const F2B_BANIP_EXCERPT: &str = r"    get <JAIL> banip [<SEP>|--with-time]     gets the list of of banned IP
                                             addresses for <JAIL>. Optionally
                                             the separator character ('<SEP>',
                                             default is space) or the option '
                                             --with-time' (printing the times
                                             of ban) may be specified. The IPs
                                             are ordered by end of ban.
";

/// A sibling row from the same table whose own wrapped description never
/// opens with a dash — the real, far more common shape, which must never
/// be claimed.
const F2B_LOGPATH_EXCERPT: &str = r"    get <JAIL> logpath                       gets the list of the monitored
                                             files for <JAIL>
";

fn empty_root() -> CommandNode {
    CommandNode::new("fail2ban-client", Provenance::single(Source::HelpText))
}

fn shape_self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "fail2ban-client's own bytes, the dash-led continuation",
            why: "the defect's raw shape: `--with-time'` opens exactly at the row's own \
                  description column",
            expect: Expect::Fires(1),
            raw: F2B_BANIP_EXCERPT.to_string(),
            root: empty_root(),
        },
        SelfCheck {
            name: "a sibling row whose continuation is ordinary prose",
            why: "the common case: a wrapped description continuing with plain English words \
                  must never be claimed just because it sits at a description column",
            expect: Expect::Silent,
            raw: F2B_LOGPATH_EXCERPT.to_string(),
            root: empty_root(),
        },
    ]
}

fn node_with_bare_quote_flag() -> CommandNode {
    let mut root = empty_root();
    let mut flag = Entity::flag_spelled(
        None,
        Some("with-time".to_string()),
        false,
        false,
        Provenance::single(Source::HelpText),
    );
    flag.value_name = Some("'".to_string());
    root.set_entities_of(mandible_core::EntityKind::Flag, vec![flag]);
    root
}

fn node_with_ordinary_flag() -> CommandNode {
    let mut root = empty_root();
    let mut flag = Entity::flag_spelled(
        None,
        Some("conf".to_string()),
        false,
        false,
        Provenance::single(Source::HelpText),
    );
    flag.value_name = Some("<DIR>".to_string());
    root.set_entities_of(mandible_core::EntityKind::Flag, vec![flag]);
    root
}

fn value_self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "the fabricated --with-time flag, valued at a bare quote",
            why: "the defect's tree artifact: the continuation's own trailing quote survives as \
                  the invented flag's value",
            expect: Expect::Fires(1),
            raw: String::new(),
            root: node_with_bare_quote_flag(),
        },
        SelfCheck {
            name: "an ordinary flag with a real value placeholder",
            why: "a genuine `<DIR>`-valued flag must never be mistaken for this shape merely for \
                  carrying a value",
            expect: Expect::Silent,
            raw: String::new(),
            root: node_with_ordinary_flag(),
        },
    ]
}
