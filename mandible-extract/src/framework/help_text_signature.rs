//! Step 2 of framework identification (spec §7 Tier A′): distinctive
//! marker strings in `--help` output itself, tried only when artifact
//! scanning (step 1) didn't resolve anything — e.g. a dynamically-linked
//! tool whose GNU argp usage/help strings live in `libc.so`, not the
//! tool's own binary, so scanning the tool's own bytes finds nothing even
//! though running it still prints the marker text.
//!
//! **Deliberately does not include cobra's `Common Commands:` heading**
//! alongside `Available Commands:`: docker uses that variant instead of
//! cobra's own default, and a prose signature added here just to catch it
//! would be spec §1's forbidden per-tool special case, wearing a
//! framework's name instead of a tool's. Docker missing this table
//! entirely is exactly why step 1 (artifact scanning, which finds
//! `spf13/cobra` in docker's own bytes 583 times [M-13]) leads and is
//! authoritative when it matches — this module is the fallback for when
//! it doesn't.

use super::Framework;

/// Literal substring → framework. Checked in order; the first match wins.
/// Order mostly doesn't matter (these markers essentially never co-occur
/// in one real tool's `--help` output) except that more specific markers
/// are listed before the coarser fallbacks below.
const SIGNATURES: &[(&str, Framework)] = &[
    ("show this help message and exit", Framework::Argparse),
    ("Show this message and exit.", Framework::Click),
    ("Available Commands:", Framework::Cobra),
    // Two distinct wordings measured on real systems — see
    // `artifact::BINARY_MARKERS`'s doc comment for which tools use which.
    (
        "Mandatory arguments to long options are mandatory for short options too.",
        Framework::GnuArgp,
    ),
    (
        "Mandatory or optional arguments to long options are also mandatory or optional",
        Framework::GnuArgp,
    ),
    ("BusyBox is copyrighted", Framework::Busybox),
];

/// Scan already-fetched `--help` output for a framework signature.
pub fn scan(help_text: &str) -> Option<Framework> {
    for (marker, framework) in SIGNATURES {
        if help_text.contains(marker) {
            return Some(*framework);
        }
    }
    if let Some(framework) = scan_go_flag_usage(help_text) {
        return Some(framework);
    }
    if looks_like_bsd_terse(help_text) {
        return Some(Framework::BsdTerse);
    }
    None
}

/// Go's standard library `flag` package prints a literal `"Usage of
/// %s:\n"` header — distinct wording from every other framework here, and
/// present whether or not the package's error-message string (the
/// artifact-scan marker) happens to still be in the tool's own binary.
fn scan_go_flag_usage(help_text: &str) -> Option<Framework> {
    help_text
        .lines()
        .any(|l| l.starts_with("Usage of ") && l.trim_end().ends_with(':'))
        .then_some(Framework::GoFlag)
}

/// Coarse fallback for terse BSD-style `usage:` output: a lowercase
/// `usage:` line, no long-form (`--`) flags anywhere, and a short overall
/// output. This is a genuinely weak signal — spec §7 Tier A′ step 2 is
/// explicitly allowed to be "deliberately crude" and still add coverage
/// [M-12] — it exists to give classic single-letter-flag Unix tools
/// *something* better than falling through to unidentified, not to be a
/// precise fingerprint. Checked last, after every more specific signature
/// above has had a chance (argparse's own usage line is also lowercase
/// `usage:`, but its `--help` text always also contains "show this help
/// message and exit", which is checked first).
fn looks_like_bsd_terse(help_text: &str) -> bool {
    let has_lowercase_usage = help_text
        .lines()
        .any(|l| l.trim_start().starts_with("usage:"));
    let has_long_flags = help_text.contains("--");
    let non_blank_lines = help_text.lines().filter(|l| !l.trim().is_empty()).count();
    has_lowercase_usage && !has_long_flags && non_blank_lines > 0 && non_blank_lines <= 20
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_argparse() {
        let text = "usage: tool [-h]\n\noptions:\n  -h, --help  show this help message and exit\n";
        assert_eq!(scan(text), Some(Framework::Argparse));
    }

    #[test]
    fn detects_click() {
        let text = "Usage: tool [OPTIONS]\n\nOptions:\n  --help  Show this message and exit.\n";
        assert_eq!(scan(text), Some(Framework::Click));
    }

    #[test]
    fn detects_cobra_via_available_commands() {
        let text =
            "Usage:\n  tool [command]\n\nAvailable Commands:\n  help  Help about any command\n";
        assert_eq!(scan(text), Some(Framework::Cobra));
    }

    /// The core regression this module's doc comment is about: docker's
    /// actual heading must NOT be recognized here — that would silently
    /// reintroduce a per-tool special case (spec §1). It's expected to
    /// come back unidentified from this step alone; artifact scanning is
    /// what actually catches docker (spec §7 Tier A′ step 1).
    #[test]
    fn common_commands_heading_is_deliberately_not_a_cobra_signature() {
        let text = "Usage:\n  docker [OPTIONS] COMMAND\n\nCommon Commands:\n  run    Create and run a new container\n";
        assert_eq!(scan(text), None);
    }

    #[test]
    fn detects_gnu_argp() {
        let text = "Usage: tool [OPTION...]\n\n  -v, --verbose   be verbose\n\nMandatory arguments to long options are mandatory for short options too.\n";
        assert_eq!(scan(text), Some(Framework::GnuArgp));
    }

    #[test]
    fn detects_go_flag_usage_header() {
        let text = "Usage of tool:\n  -v\tbe verbose\n";
        assert_eq!(scan(text), Some(Framework::GoFlag));
    }

    #[test]
    fn detects_busybox() {
        let text = "BusyBox is copyrighted software licensed under the GNU General Public License.\n\nUsage: busybox [function] [arguments]...\n";
        assert_eq!(scan(text), Some(Framework::Busybox));
    }

    #[test]
    fn detects_bsd_terse_fallback() {
        let text = "usage: ls [-ABCFGHLOPRSTUWabcdefghiklmnopqrstuwx1] [file ...]\n";
        assert_eq!(scan(text), Some(Framework::BsdTerse));
    }

    /// A tool with long-form flags is not the terse-BSD shape, even if it
    /// happens to have a lowercase `usage:` line.
    #[test]
    fn bsd_terse_fallback_does_not_fire_when_long_flags_are_present() {
        let text = "usage: tool [-v] [--verbose]\n";
        assert_eq!(scan(text), None);
    }

    #[test]
    fn empty_input_is_unidentified() {
        assert_eq!(scan(""), None);
    }
}
