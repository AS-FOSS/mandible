//! `mandible --report <TOOL>`: a paste-ready bug report (spec §5.3-adjacent;
//! see `.github/ISSUE_TEMPLATE/parsing-issue.md` for the shape this fills).
//!
//! Collapses "file a good parsing bug report" to one command. Someone who
//! hits a bad parse has mandible installed and open — they do not have
//! `CONTRIBUTING.md` or GitHub's issue-template picker open, and typing out
//! `--doctor` output plus a raw `--help` capture by hand is exactly the kind
//! of friction that means a real bug never gets filed. This assembles: the
//! mandible version, the target tool's version (best-effort), the
//! `--doctor` diagnostic, and the raw `--help` capture, as one fenced,
//! paste-ready block, followed by the repository's issues URL.
//!
//! **The raw `--help` capture goes through the same sanctioned chokepoint
//! as everything else** ([`mandible_extract::help_text::raw_help`]) — this
//! module never spawns a process of its own and never asks for an argv
//! shape outside spec §6 rule 2's closed [`mandible_extract::exec::InertArgv`]
//! list. For a never-probe tool (spec §6 rule 0: `pkill`, `shutdown`, ...)
//! that means exactly the same thing it means for `--doctor` — the root
//! `--help` probe is attempted (it is the one shape rule 0 permits, and the
//! runner's own root extraction already needed it to build `loaded` in the
//! first place), nothing else is ever tried on top, and if the sanctioned
//! path can't produce text, the report says so and prints everything else
//! it does have rather than failing outright.
//!
//! **Byte-exactness caveat.** Pasting this block into a GitHub issue textarea
//! is not byte-exact — trailing whitespace is stripped by markdown rendering,
//! among other things a plain-text paste can lose. That's fine for a first
//! report: it's meant to get a maintainer from "no report" to "enough to
//! start," and if the fix needs the exact bytes, re-capturing against the
//! real tool is the maintainer's job, not something a pasted issue body can
//! guarantee on its own.

use crate::doctor;
use crate::pipeline::LoadedTool;
use mandible_extract::{help_text, resolve_tool, NodeHints};

/// Print the paste-ready report for `loaded` to stdout.
pub fn print_report(loaded: &LoadedTool) {
    print!("{}", build_report(loaded));
}

fn build_report(loaded: &LoadedTool) -> String {
    let tool = &loaded.tool;
    let resolved = resolve_tool(tool);
    // Fetched once and shared by both the version scrape and the raw-text
    // section below, rather than probing twice: the sanctioned chokepoint
    // (spec §6) is cheap but not free, and a never-probe tool's refusal
    // should be reported once, consistently, not risk saying two different
    // things depending on which call happened to be asked first.
    let raw = raw_help(&resolved, tool);

    let mut body = String::new();
    body.push_str(&format!("mandible:   v{}\n", env!("CARGO_PKG_VERSION")));
    body.push_str(&format!(
        "{tool} version: {}\n",
        scrape_version(raw.as_ref().ok(), tool).unwrap_or_else(|| "unknown".to_string())
    ));
    body.push('\n');

    body.push_str("--- mandible --doctor ");
    body.push_str(tool);
    body.push_str(" ---\n");
    body.push_str(&doctor::build_report(loaded));
    body.push('\n');

    body.push_str("--- ");
    body.push_str(tool);
    body.push_str(" --help (raw) ---\n");
    body.push_str(&raw_help_block(tool, raw.as_ref()));

    let mut out = String::new();
    out.push_str(
        "Paste-ready bug report. Note: a GitHub issue textarea is not byte-exact \
         (trailing whitespace is dropped by markdown rendering) — that's fine for \
         a first report; exact-byte re-capture is the maintainer's job if the fix \
         needs it.\n\n",
    );
    out.push_str("```console\n");
    out.push_str(&body);
    // The captured text may or may not itself end in a newline (a `--help`
    // whose last line had no trailing `\n` vs. one that did); either way the
    // fence needs its own line, so trim first rather than risk a blank line
    // inside the block or, worse, the fence and the last line of output
    // colliding on one line and no longer parsing as a fence at all.
    while out.ends_with('\n') {
        out.pop();
    }
    out.push('\n');
    out.push_str("```\n\n");
    out.push_str(&format!(
        "File it at: {}/issues\n",
        env!("CARGO_PKG_REPOSITORY")
    ));
    out
}

/// Fetch the root's raw `--help` text through the same sanctioned probe
/// chokepoint [`doctor`] and the extraction pipeline itself already use —
/// this module never spawns a process of its own. Shared by
/// [`raw_help_block`] (the report's raw-text section) and [`scrape_version`]
/// (the best-effort version line) so a never-probe tool's refusal is
/// reported once, consistently, rather than risking the two callers seeing
/// different things depending on call order.
fn raw_help(
    resolved: &mandible_extract::ResolvedTool,
    tool: &str,
) -> Result<(Vec<mandible_core::Text>, String), mandible_extract::ExtractError> {
    let root_path = vec![tool.to_string()];
    // Same root hint the runner uses (`Runner::extract_full_for`): the root
    // is the name the user typed, never a word a parser guessed at, so it
    // is attested by construction — there is no heading to point to
    // because none was needed.
    let hints = NodeHints {
        heading_attested: true,
    };
    help_text::raw_help(resolved, &root_path, hints)
}

/// Render [`raw_help`]'s result as the report's raw-text section.
///
/// Degrades to an honest one-line explanation, never a panic or a missing
/// section, when the probe can't produce text: the tool wasn't found, the
/// probe errored, or (spec §6 rule 0) it's a never-probe tool whose
/// `--help` came back empty on both streams, which would otherwise need a
/// `-h` fallback rule 0 refuses. No fallback is attempted here beyond what
/// `raw_help` itself already does — this function does not retry with a
/// different argv shape.
fn raw_help_block(
    tool: &str,
    raw: Result<&(Vec<mandible_core::Text>, String), &mandible_extract::ExtractError>,
) -> String {
    match raw {
        Ok((lines, flag)) => {
            let mut s = format!("$ {tool} {flag}\n");
            for line in lines {
                s.push_str(line.as_str());
                s.push('\n');
            }
            s
        }
        Err(e) => format!(
            "(unavailable: {e})\n\
             Reasons this can happen: the tool isn't on PATH, the probe \
             errored or timed out, or (spec §6 rule 0) it's a tool restricted \
             to exactly `--help` whose own output came back empty on both \
             stdout and stderr — that would ordinarily fall back to `-h`, \
             which rule 0 refuses for those tools. Either way, mandible will \
             not try a different argv shape to fill this in.\n"
        ),
    }
}

/// Best-effort tool version, scraped from the same raw `--help` text this
/// report already captures — never a `--version` probe of its own, which
/// spec §6 rule 2's closed argv list does not include (adding it needs a
/// spec amendment, not a `mandible-extract` patch that quietly bypasses the
/// allowlist). Plenty of `--help` banners already carry a version: clap's
/// own template opens with exactly `"<name> <version>"` (`"zoxide 0.9.9"`),
/// and this looks for that one specific, low-risk shape in the *raw* text
/// rather than the parsed tree — the parser deliberately drops this exact
/// paragraph from `description` once it recognizes the banner shape
/// (`sections::parse_named`'s doc comment on `is_banner_paragraph`), so the
/// version is only ever findable in what the tool actually printed, not in
/// anything `build_node` kept.
///
/// Deliberately conservative: `None` (rendered as `"unknown"`) is a fine,
/// honest outcome for the far more common case where `--help` doesn't open
/// that way, or where the probe produced no text at all (`raw` is `None`
/// for a never-probe tool's refusal, exactly as [`raw_help_block`] reports
/// it) — a looser scan risks misreading an ordinary two-word sentence as a
/// version and misleading the very report meant to be trustworthy.
fn scrape_version(raw: Option<&(Vec<mandible_core::Text>, String)>, tool: &str) -> Option<String> {
    let (lines, _flag) = raw?;
    // The banner, when present, is always the document's opening line
    // (clap's template, and every framework that copies its shape) — bound
    // the scan to a handful of leading lines rather than the whole
    // document, so a coincidental two-token match deep in a flag
    // description (unlikely given the exact-name-match requirement, but
    // not impossible) can never surface as a fabricated "version".
    const BANNER_SCAN_LINES: usize = 5;
    lines
        .iter()
        .take(BANNER_SCAN_LINES)
        .find_map(|line| version_from_banner_line(line.as_str(), tool))
}

/// `true`/`Some(version)` when `line` is exactly `"<name> <version>"`: two
/// whitespace-separated tokens, the first case-insensitively equal to
/// `tool` (or its final path component), the second starting with a digit.
fn version_from_banner_line(line: &str, tool: &str) -> Option<String> {
    let mut words = line.split_whitespace();
    let name = words.next()?;
    let version = words.next()?;
    if words.next().is_some() {
        return None; // more than two tokens: not the clap-style banner shape
    }
    if !name.eq_ignore_ascii_case(tool) {
        return None;
    }
    version
        .starts_with(|c: char| c.is_ascii_digit())
        .then(|| version.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::Text;

    fn raw(lines: &[&str]) -> (Vec<Text>, String) {
        (
            lines.iter().map(|l| Text::sanitize(l)).collect(),
            "--help".to_string(),
        )
    }

    #[test]
    fn scrapes_a_clap_style_name_version_banner() {
        let r = raw(&["zoxide 0.9.9", "Ajeet D'Souza <email>", "", "A smarter cd"]);
        assert_eq!(
            scrape_version(Some(&r), "zoxide"),
            Some("0.9.9".to_string())
        );
    }

    #[test]
    fn does_not_misread_an_ordinary_two_word_usage_line() {
        // "Usage: git" style lines must not be mistaken for a version
        // banner just because they happen to have two tokens.
        let r = raw(&["Usage: git"]);
        assert_eq!(scrape_version(Some(&r), "git"), None);
    }

    #[test]
    fn requires_the_name_to_match_the_tool() {
        let r = raw(&["gitk 1.2.3"]);
        assert_eq!(scrape_version(Some(&r), "git"), None);
    }

    #[test]
    fn does_not_scan_past_the_leading_lines() {
        // A coincidental "tool digits" two-token match deep in the
        // document (well past where a real banner would ever be) must not
        // surface as a fabricated version.
        let mut lines = vec!["real help text"; 10];
        lines.push("git 9.9.9");
        let r = raw(&lines);
        assert_eq!(scrape_version(Some(&r), "git"), None);
    }

    #[test]
    fn no_raw_text_scrapes_to_none() {
        assert_eq!(scrape_version(None, "ghost"), None);
    }

    #[test]
    fn report_never_panics_for_an_unresolvable_tool() {
        let loaded = crate::pipeline::load("definitely-not-a-real-tool-xyz-123");
        let report = build_report(&loaded);
        assert!(report.contains("unknown"));
        assert!(report.contains("/issues"));
    }
}
