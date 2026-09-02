//! `FrameworkProfile`: bounded, per-framework knowledge dispatching Tier
//! B's shared section/layout engine (spec §7 Tier B). Adding a framework
//! is one arm in [`profile`] plus one fingerprint in [`crate::framework`],
//! never a branch on tool name.
//!
//! A profile deliberately does not carry value-syntax or separator knobs —
//! [`super::grammar::parse_flag_spec`] and
//! [`super::sections::scan_flags_block`] already handle those generically.
//! It carries only: extra heading vocabulary introducing a command block,
//! and whether the framework has a subcommand concept at all (the
//! structural fix for spec §13 defect M-10 — see S-013).

use crate::framework::Framework;

/// Per-framework knowledge for the shared Tier B layout engine.
pub struct FrameworkProfile {
    /// Extra heading substrings (lowercase) beyond the engine's generic
    /// "mentions command(s)/subcommand(s)" test that introduce a
    /// subcommand block. Empty when the framework's own vocabulary is
    /// already caught generically, or has no subcommand concept.
    pub command_heading_markers: &'static [&'static str],
    /// Heading substrings that are positively not a command block, even
    /// though the engine's same-indent sticky chain would otherwise carry
    /// command mode into them — and that also break the chain for
    /// anything after. See docs/shapes.md S-094 (gh's `HELP TOPICS`).
    pub non_command_heading_markers: &'static [&'static str],
    /// Heading substrings under which this framework declares its
    /// positional operands, one per row with a description — a
    /// declaration, unlike the usage synopsis which
    /// [`super::sections::extract_positionals`] can only infer from.
    /// See docs/shapes.md S-078.
    pub positional_heading_markers: &'static [&'static str],
    /// True when this framework structurally has no subcommand concept —
    /// forces the parser to never enter command mode for it, independent
    /// of `command_heading_markers` (always empty when true). Structural
    /// fix for spec M-10. See docs/shapes.md S-013.
    pub no_subcommand_concept: bool,
    /// True only for [`Framework::Argparse`]: gates the dedicated
    /// `{choice,...}` pseudo-entry scan
    /// ([`super::sections::scan_argparse_subparsers`]) for a shape a data
    /// table can't express. See docs/shapes.md S-073.
    pub argparse_subparser_quirk: bool,
    /// True only for [`Framework::Busybox`]: gates the dedicated flat
    /// comma-separated applet list scan
    /// (`super::sections::scan_comma_separated_commands`). See
    /// docs/shapes.md S-093.
    pub comma_separated_command_list: bool,
}

/// The profile for `framework`. Exhaustive `match`, no wildcard arm, so a
/// new `Framework` variant is a compile error here until it gets one.
pub fn profile(framework: Framework) -> FrameworkProfile {
    match framework {
        // --- spec §7 Tier B priority order, most measured payoff first ---
        Framework::GnuArgp => FrameworkProfile {
            // Classic Unix utilities (tar, dd, sed, find, less): M-10's
            // casualty list. See docs/shapes.md S-013.
            command_heading_markers: &[],
            non_command_heading_markers: &[],
            positional_heading_markers: &[],
            no_subcommand_concept: true,
            argparse_subparser_quirk: false,
            comma_separated_command_list: false,
        },
        Framework::ClapV3V4 => FrameworkProfile {
            // Renders "Commands:", already caught generically. Verified
            // against cargo --help, not ripgrep (hand-rolled formatter,
            // M-13).
            command_heading_markers: &[],
            non_command_heading_markers: &[],
            positional_heading_markers: &[],
            no_subcommand_concept: false,
            argparse_subparser_quirk: false,
            comma_separated_command_list: false,
        },
        Framework::Argparse => FrameworkProfile {
            // Titled add_subparsers(title=...) renders as an ordinary
            // heading, caught generically. Untitled case renders under
            // "positional arguments:", handled by argparse_subparser_quirk
            // below, not a heading marker (would fabricate subcommands
            // from ordinary positionals with no subparser). S-073.
            command_heading_markers: &[],
            non_command_heading_markers: &[],
            // When the block holds no `{...}` pseudo-entry, it is
            // argparse's literal rendering of positional operands instead.
            // See docs/shapes.md S-078.
            positional_heading_markers: &["positional arguments"],
            no_subcommand_concept: false,
            argparse_subparser_quirk: true,
            comma_separated_command_list: false,
        },
        Framework::Cobra => FrameworkProfile {
            // Every "<Group> Commands" variant (gh, docker) contains the
            // word "commands", caught generically. Verified against real
            // gh --help / docker --help output.
            command_heading_markers: &[],
            // gh's own HELP TOPICS group; see docs/shapes.md S-094.
            non_command_heading_markers: &["help topics"],
            positional_heading_markers: &[],
            no_subcommand_concept: false,
            argparse_subparser_quirk: false,
            comma_separated_command_list: false,
        },
        Framework::Click => FrameworkProfile {
            // Renders plain "Commands:", caught generically.
            command_heading_markers: &[],
            non_command_heading_markers: &[],
            positional_heading_markers: &[],
            no_subcommand_concept: false,
            argparse_subparser_quirk: false,
            comma_separated_command_list: false,
        },

        // --- best-effort: plausible from documented conventions, not
        // fixture-tested here (spec §7 Tier B's priority list stops at
        // the five above) ---
        Framework::ClapV2 => FrameworkProfile {
            command_heading_markers: &[],
            non_command_heading_markers: &[],
            positional_heading_markers: &[],
            no_subcommand_concept: false,
            argparse_subparser_quirk: false,
            comma_separated_command_list: false,
        },
        Framework::UrfaveCli => FrameworkProfile {
            // urfave/cli's default template renders `"COMMANDS:"`.
            command_heading_markers: &[],
            non_command_heading_markers: &[],
            positional_heading_markers: &[],
            no_subcommand_concept: false,
            argparse_subparser_quirk: false,
            comma_separated_command_list: false,
        },
        Framework::GoFlag => FrameworkProfile {
            // The stdlib flag package has no subcommand mechanism of its
            // own. See docs/shapes.md S-013.
            command_heading_markers: &[],
            non_command_heading_markers: &[],
            positional_heading_markers: &[],
            no_subcommand_concept: true,
            argparse_subparser_quirk: false,
            comma_separated_command_list: false,
        },
        Framework::Docopt => FrameworkProfile {
            // Usage-line-only (spec §7 Tier B): the pattern itself is the
            // grammar, no separate command-list section. S-013.
            command_heading_markers: &[],
            non_command_heading_markers: &[],
            positional_heading_markers: &[],
            no_subcommand_concept: true,
            argparse_subparser_quirk: false,
            comma_separated_command_list: false,
        },
        Framework::BsdTerse => FrameworkProfile {
            // Fingerprint (looks_like_bsd_terse): a short usage: line, no
            // long-form flags — terse single-purpose shape. S-013.
            command_heading_markers: &[],
            non_command_heading_markers: &[],
            positional_heading_markers: &[],
            no_subcommand_concept: true,
            argparse_subparser_quirk: false,
            comma_separated_command_list: false,
        },
        Framework::Busybox => FrameworkProfile {
            // Heading "Currently defined functions:" doesn't say
            // "command", needs an explicit marker. See docs/shapes.md
            // S-093.
            command_heading_markers: &["currently defined functions"],
            non_command_heading_markers: &[],
            positional_heading_markers: &[],
            no_subcommand_concept: false,
            argparse_subparser_quirk: false,
            comma_separated_command_list: true,
        },
        Framework::Commander => FrameworkProfile {
            // commander's default help renders `"Commands:"`.
            command_heading_markers: &[],
            non_command_heading_markers: &[],
            positional_heading_markers: &[],
            no_subcommand_concept: false,
            argparse_subparser_quirk: false,
            comma_separated_command_list: false,
        },
        Framework::Yargs => FrameworkProfile {
            // yargs renders `"Commands:"` alongside `"Positionals:"` /
            // `"Options:"`.
            command_heading_markers: &[],
            non_command_heading_markers: &[],
            positional_heading_markers: &[],
            no_subcommand_concept: false,
            argparse_subparser_quirk: false,
            comma_separated_command_list: false,
        },
        Framework::Oclif => FrameworkProfile {
            // Groups topics under "TOPICS" alongside "COMMANDS"; "topics"
            // doesn't contain the word "command".
            command_heading_markers: &["topics"],
            non_command_heading_markers: &[],
            positional_heading_markers: &[],
            no_subcommand_concept: false,
            argparse_subparser_quirk: false,
            comma_separated_command_list: false,
        },
        Framework::Picocli => FrameworkProfile {
            // picocli's usage help renders `"Commands:"`.
            command_heading_markers: &[],
            non_command_heading_markers: &[],
            positional_heading_markers: &[],
            no_subcommand_concept: false,
            argparse_subparser_quirk: false,
            comma_separated_command_list: false,
        },
        Framework::DotNetSystemCommandLine => FrameworkProfile {
            // System.CommandLine's default help renders `"Commands:"`.
            command_heading_markers: &[],
            non_command_heading_markers: &[],
            positional_heading_markers: &[],
            no_subcommand_concept: false,
            argparse_subparser_quirk: false,
            comma_separated_command_list: false,
        },
        Framework::SymfonyConsole => FrameworkProfile {
            // Symfony Console's `list` output renders `"Available
            // commands:"`, already caught generically.
            command_heading_markers: &[],
            non_command_heading_markers: &[],
            positional_heading_markers: &[],
            no_subcommand_concept: false,
            argparse_subparser_quirk: false,
            comma_separated_command_list: false,
        },
        Framework::OptionParserOrThor => FrameworkProfile {
            // Fingerprint can't distinguish Thor (has "Commands:") from
            // plain OptionParser (no subcommands); assumes the more
            // conservative case rather than forcing zero.
            command_heading_markers: &[],
            non_command_heading_markers: &[],
            positional_heading_markers: &[],
            no_subcommand_concept: false,
            argparse_subparser_quirk: false,
            comma_separated_command_list: false,
        },
    }
}

/// True if `heading` (already lowercased by the caller) contains any of
/// `markers` as a substring.
pub(super) fn heading_matches_markers(heading_lower: &str, markers: &[&str]) -> bool {
    markers.iter().any(|m| heading_lower.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `Framework` variant must have a profile.
    #[test]
    fn every_framework_has_a_profile() {
        let all = [
            Framework::ClapV3V4,
            Framework::ClapV2,
            Framework::Cobra,
            Framework::UrfaveCli,
            Framework::GoFlag,
            Framework::Argparse,
            Framework::Click,
            Framework::Docopt,
            Framework::GnuArgp,
            Framework::BsdTerse,
            Framework::Busybox,
            Framework::Commander,
            Framework::Yargs,
            Framework::Oclif,
            Framework::Picocli,
            Framework::DotNetSystemCommandLine,
            Framework::SymfonyConsole,
            Framework::OptionParserOrThor,
        ];
        for f in all {
            let p = profile(f);
            // no_subcommand_concept and non-empty command_heading_markers
            // together would be self-contradictory data.
            if p.no_subcommand_concept {
                assert!(
                    p.command_heading_markers.is_empty(),
                    "{f:?}: no_subcommand_concept but non-empty command_heading_markers"
                );
            }
        }
    }

    #[test]
    fn busybox_marker_is_lowercase_for_case_insensitive_matching() {
        let p = profile(Framework::Busybox);
        for m in p.command_heading_markers {
            assert_eq!(*m, m.to_lowercase());
        }
    }
}
