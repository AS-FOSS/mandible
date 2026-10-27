//! The detail pane's drawn page for every node of a fixture, pinned beside `expected.snap`.
use super::*;

/// The pane's text width at a 160-column terminal.
const PANE_WIDTH: usize = 76;

/// Marks the line that starts one node's page.
const HEADER_OPEN: &str = "=== ";

fn walk<'a>(
    node: &'a CommandNode,
    path: &mut Vec<&'a str>,
    out: &mut Vec<(String, &'a CommandNode)>,
) {
    path.push(&node.name);
    out.push((path.join(" "), node));
    for child in &node.subcommands {
        walk(child, path, out);
    }
    path.pop();
}

/// Every parsed node's page, each under a `=== <path> ===` header, in tree
/// order. A node the pane does not draw as a parsed page is left out.
pub(crate) fn render_screens(root: &CommandNode) -> String {
    let mut nodes = Vec::new();
    walk(root, &mut Vec::new(), &mut nodes);
    let mut out = String::new();
    for (i, (path, node)) in nodes.into_iter().enumerate() {
        let Some(page) = mandible_tui::render::page_text(node, PANE_WIDTH, i == 0) else {
            continue;
        };
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("{HEADER_OPEN}{path} ===\n"));
        for line in page {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

/// The result of comparing a fixture's drawn pages against `expected.screen`.
pub(crate) enum ScreenCheck {
    /// No tier produced a root at all.
    NoRoot,
    /// `expected.screen` doesn't exist.
    Missing,
    /// Byte-identical.
    Match,
    /// Differs, starting at the given 1-indexed line, inside `node`.
    Mismatch {
        line: usize,
        node: String,
        expected: String,
        actual: String,
    },
}

pub(crate) fn check_screens(
    fixture: &Fixture,
    root: Option<&CommandNode>,
) -> anyhow::Result<ScreenCheck> {
    let Some(root) = root else {
        return Ok(ScreenCheck::NoRoot);
    };
    let path = fixture.expected_screen_path();
    if !path.is_file() {
        return Ok(ScreenCheck::Missing);
    }
    let expected = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
    let actual = render_screens(root);
    if expected == actual {
        return Ok(ScreenCheck::Match);
    }
    let expected_lines: Vec<&str> = expected.lines().collect();
    let actual_lines: Vec<&str> = actual.lines().collect();
    let first = expected_lines
        .iter()
        .zip(actual_lines.iter())
        .position(|(e, a)| e != a)
        .unwrap_or_else(|| expected_lines.len().min(actual_lines.len()));
    let node = actual_lines[..first.min(actual_lines.len())]
        .iter()
        .chain(actual_lines.get(first))
        .rev()
        .find_map(|l| l.strip_prefix(HEADER_OPEN)?.strip_suffix(" ==="))
        .unwrap_or("?")
        .to_string();
    let at = |lines: &[&str]| lines.get(first).unwrap_or(&"<end of file>").to_string();
    Ok(ScreenCheck::Mismatch {
        line: first + 1,
        node,
        expected: at(&expected_lines),
        actual: at(&actual_lines),
    })
}

impl ScreenCheck {
    /// Whether the check holds; a missing file is legal only while `[xfail]`.
    pub(crate) fn passes(&self, is_xfail: bool) -> bool {
        match self {
            ScreenCheck::Match => true,
            ScreenCheck::Missing => is_xfail,
            ScreenCheck::NoRoot | ScreenCheck::Mismatch { .. } => false,
        }
    }

    /// The line this check adds to the fixture's detail, if any.
    pub(crate) fn detail(&self, is_xfail: bool) -> Option<String> {
        match self {
            ScreenCheck::Match => Some("screen: match".to_string()),
            ScreenCheck::Missing if is_xfail => {
                Some("screen: none yet (legal while [xfail])".to_string())
            }
            ScreenCheck::Missing => Some(
                "screen: missing expected.screen (required unless marked [xfail])".to_string(),
            ),
            ScreenCheck::NoRoot => None,
            ScreenCheck::Mismatch {
                line,
                node,
                expected,
                actual,
            } => Some(format!(
                "screen mismatch in `{node}` at expected.screen:{line}: expected `{expected}` got `{actual}`"
            )),
        }
    }
}

/// Write `expected.screen` for a fixture whose tree is `root`.
pub(crate) fn bless_screens(fixture: &Fixture, root: &CommandNode) -> anyhow::Result<()> {
    let path = fixture.expected_screen_path();
    std::fs::write(&path, render_screens(root))
        .map_err(|e| anyhow::anyhow!("writing {}: {e}", path.display()))
}
