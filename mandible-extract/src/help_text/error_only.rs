//! A probe answer that is only an error from the tool (docs/shapes.md S-238):
//! `pnpm start --help` run outside a project prints
//! `[ERR_PNPM_NO_IMPORTER_MANIFEST_FOUND] No package.json ... was found in "$PWD".`
//! and nothing else. That is the tool failing, not help for the command, so it
//! must not become the node's page. Content-keyed: one or two non-blank lines,
//! the first led by an error marker, and nothing that reads as usage or a list.

/// Leads that mark a tool's own error line, compared case-sensitively on the
/// first word so `Errors:` (a heading) and prose never match.
fn leads_with_error_marker(line: &str) -> bool {
    let first = line.split_whitespace().next().unwrap_or("");
    let bracketed = first.starts_with("[ERR") && first.contains(']');
    bracketed
        || matches!(
            first,
            "error:" | "Error:" | "ERROR:" | "fatal:" | "Fatal:" | "FATAL:"
        )
        || first.starts_with("ERR_")
}

/// True when `text` is one or two non-blank lines, the first an error line.
pub(super) fn is_error_only(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() || lines.len() > 2 {
        return false;
    }
    if !leads_with_error_marker(lines[0].trim_start()) {
        return false;
    }
    // A second line that opens a usage form or an option row is help.
    lines.get(1).is_none_or(|l| {
        let t = l.trim_start().to_ascii_lowercase();
        !(t.starts_with("usage") || t.starts_with('-'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_one_line_error_is_error_only() {
        assert!(is_error_only(
            "[ERR_PNPM_NO_IMPORTER_MANIFEST_FOUND] No package.json (or package.yaml, or package.json5) was found in \"/x\".\n"
        ));
        assert!(is_error_only("error: no such subcommand\n"));
        assert!(is_error_only("Error: not a project\nTry again.\n"));
    }

    #[test]
    fn help_is_never_error_only() {
        assert!(!is_error_only(""));
        assert!(!is_error_only("Usage: tool [options]\n"));
        assert!(!is_error_only("error: bad\nUsage: tool [options]\n"));
        assert!(!is_error_only("error: bad\n  -h, --help  show help\n"));
        assert!(!is_error_only("error: a\nb\nc\n"));
        assert!(!is_error_only("Errors: listed below\n"));
    }

    /// A subcommand whose whole answer is the tool's error line has no page
    /// of its own: the probe is an error, not a verbatim node of that line.
    #[test]
    fn a_subcommand_answering_only_an_error_yields_no_node() {
        use crate::tier::{ExtractionTier, NodeHints};
        let out = |s: &str| crate::exec::ExecOutput {
            stdout: s.as_bytes().to_vec(),
            stderr: Vec::new(),
            exit_code: Some(1),
            timed_out: false,
        };
        let transcript = crate::exec::Transcript::new([(
            vec!["start".to_string(), "--help".to_string()],
            out("[ERR_X_NO_MANIFEST] No manifest was found in \"$PWD\".\n"),
        )]);
        let tier = super::super::HelpTextTier::new(std::sync::Arc::new(transcript));
        let tool = crate::resolve::ResolvedTool {
            name: "widget".to_string(),
            path: Some(std::path::PathBuf::from("/replayed/widget")),
            version: None,
        };
        let hints = NodeHints {
            heading_attested: true,
            abbrev_probe_attested: false,
            help_word_attested: false,
        };
        let result = tier.extract_node(&tool, &["widget".to_string(), "start".to_string()], hints);
        assert!(
            result.is_err(),
            "an error-only answer must not become a page"
        );
    }
}
