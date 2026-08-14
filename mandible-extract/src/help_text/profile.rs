//! `FrameworkProfile`: the bounded, per-framework knowledge that dispatches
//! Tier B's *shared* section/layout engine (spec §7 Tier B, batch 6 part
//! 4). `sections::parse_with_profile` stays one engine — this module is
//! the data table that lets it behave slightly differently depending on
//! which of the ~18 known frameworks (spec §7 Tier B's table) produced the
//! text, without ever branching on a tool name. Adding a framework is
//! adding one arm to [`profile`] plus one fingerprint in
//! [`crate::framework`], nothing more — that bounded-knowledge property is
//! the entire reason spec revision 3 exists.
//!
//! **What a profile deliberately does *not* carry.** An early sketch of
//! this struct (the batch instructions that produced it) suggested
//! `value_syntax`, `alias_separator`, and `wraps_descriptions` knobs. They
//! are not here: the shared low-level grammar the engine already had
//! before this batch —
//! [`super::grammar::parse_flag_spec`] for `--opt=VALUE` / `--opt VALUE` /
//! `--opt <value>` / `--opt[=VALUE]`, and [`super::sections::scan_flags_block`]'s
//! indent-relative continuation folding — already handles every one of
//! those shapes uniformly, verified against real GNU argp (`tar`), clap
//! (`cargo`), click, and cobra (`gh`) output in this module's own tests.
//! Giving every one of the ~18 arms below the same hard-coded value for a
//! knob that never varies would be duplicating already-general logic
//! instead of widening it — exactly what the project's golden rule (no
//! per-tool *or over-fitted* knowledge) warns against. If a framework is
//! ever found whose flag-spec or continuation shape the shared grammar
//! genuinely cannot express, the right fix is to widen that grammar (which
//! improves every framework at once), not to add a profile knob that only
//! one arm sets.
//!
//! **What a profile *does* carry** is therefore narrow, on purpose:
//! which extra heading vocabulary this framework uses to introduce a
//! command block (beyond the engine's own generic "mentions command(s)/
//! subcommand(s) as a whole word" test), and whether the framework has a
//! subcommand concept at all. The latter is what directly encodes the
//! fix for [M-10]: a framework profile with an empty marker list and
//! `no_subcommand_concept: true` makes the zero-subcommands outcome
//! *structural*, not incidental to whatever heading text one specific
//! tool happened to print.

use crate::framework::Framework;

/// Per-framework knowledge for the shared Tier B layout engine. See this
/// module's doc comment for what is (and isn't) here, and why.
pub struct FrameworkProfile {
    /// Extra heading markers (lowercase substrings, checked in addition to
    /// the engine's own generic "mentions command(s)/subcommand(s) as a
    /// whole word" test) that this framework's own vocabulary uses to
    /// introduce a subcommand block. Empty is meaningful only combined
    /// with `no_subcommand_concept`; for a framework that *does* have
    /// subcommands but whose own vocabulary is already caught by the
    /// generic word test (`"Commands:"`, `"Available Commands:"`, ...),
    /// this is simply empty too — there is nothing extra to add.
    pub command_heading_markers: &'static [&'static str],
    /// Heading markers that are positively *not* a command block for this
    /// framework, even while the engine's "sticky chain" (`command_mode`
    /// in `sections::parse_with_profile` — a same-indent run of group
    /// headings following one recognized/mentioned earlier, which is what
    /// lets git's own un-labelled group headings like `"start a working
    /// area (see also: ...)"` still count) would otherwise have carried
    /// forward into it. Real-world discovery: cobra apps commonly add a
    /// `"Help topics"`-shaped section (cobra's own
    /// `IsAdditionalHelpTopicCommand` concept) listing names that are
    /// *not* invokable subcommands — `gh --help`'s own `HELP TOPICS`
    /// group lists `environment`, `reference`, etc., none of which are
    /// real `gh <name>` commands. A heading matching this list both stops
    /// itself from being read as commands *and* breaks the sticky chain
    /// for anything after it (matching a real non-command heading is
    /// strong evidence the chain has ended), unlike a plain "not
    /// recognized" heading a chain-following framework will still
    /// includes today. Empty for every framework except the ones known to
    /// have this exact shape.
    pub non_command_heading_markers: &'static [&'static str],
    /// Heading markers (lowercase substrings, same matching rule as the two
    /// lists above) under which this framework prints its **positional
    /// operands** — the block that names the arguments a user types with no
    /// `-` in front of them, one per row, with a description.
    ///
    /// This exists because a usage synopsis is *inference* and a block like
    /// this is a *declaration*. [`super::sections::extract_positionals`] can
    /// only guess from synopsis notation, and it deliberately guesses
    /// narrowly: `<angled>` tokens and bare `UPPERCASE` words, nothing else.
    /// That misses every operand a framework writes as a plain lowercase
    /// word — argparse's own default rendering of one:
    ///
    /// ```text
    /// usage: uobjnew [-h] [-l {c,java,ruby,tcl}] [-v] pid [interval]
    ///
    /// positional arguments:
    ///   pid                   process id to attach to
    ///   interval              print every specified number of seconds
    /// ```
    ///
    /// `pid` and `interval` are unrecoverable from that synopsis without
    /// promoting every bare lowercase word in it to an operand, which would
    /// invent one out of `vim [arguments] [file ..]`'s `arguments` — the
    /// option-list placeholder, not an operand (see
    /// [`super::sections::OPTION_LIST_PLACEHOLDERS`]). The block says
    /// outright which tokens are operands, so reading it needs no guess at
    /// all: the block supplies the names and descriptions, and the synopsis
    /// is consulted only for the two shape bits it does state
    /// unambiguously — `[x]` is optional, a trailing `...` is variadic.
    ///
    /// Empty for every framework except the ones whose own template emits a
    /// fixed heading for this. A framework that renames the group (argparse's
    /// `add_argument_group("inputs")`) is a declared miss, not a silent one:
    /// nothing is inferred from an unrecognized heading.
    pub positional_heading_markers: &'static [&'static str],
    /// True when this framework structurally has no subcommand concept at
    /// all — forces the parser to never enter command mode for a node
    /// identified as this framework, regardless of `command_heading_markers`
    /// (always empty when this is true) or the engine's own generic
    /// heading test (spec §7 Tier B rule 1: "must produce zero
    /// subcommands"). This is the direct, structural fix for [M-10]: GNU
    /// argp classics (`tar`, `dd`, `sed`, `find`, `less`, ...) and terse
    /// BSD tools are exactly the tools that bug hit, and both frameworks
    /// carry this flag.
    pub no_subcommand_concept: bool,
    /// True only for [`Framework::Argparse`]: argparse's `add_subparsers()`
    /// renders as a structurally distinct shape a data table can't express
    /// (see [`super::sections::scan_argparse_subparsers`]'s doc comment for
    /// why it earns dedicated code instead of a profile field) — a
    /// `{choice,choice,...}` pseudo-entry under (usually undecorated)
    /// `positional arguments:`, with real subcommands one indent level
    /// *deeper* than that pseudo-entry, the opposite of the engine's
    /// general "deeper means continuation" rule. Gates whether the engine
    /// even attempts that dedicated scan.
    pub argparse_subparser_quirk: bool,
    /// True only for [`Framework::Busybox`] (spec issue #1): busybox's
    /// `Currently defined functions:` block lists every applet as one flat,
    /// tab-indented, comma-separated run —
    ///
    /// ```text
    /// Currently defined functions:
    ///     [, [[, acpid, add-shell, addgroup, adduser, adjtimex, ar, arch, arp,
    ///     arping, ash, awk, base32, base64, basename, bc, bunzip2, bzcat, ...
    /// ```
    ///
    /// — a shape the engine's ordinary bare-block scan (one entry per
    /// *line*, split at a 2+-space column gap) cannot express at all: every
    /// line is many entries, separated by `, `, with no per-entry
    /// description and no column gap anywhere. Same reasoning as
    /// `argparse_subparser_quirk` above (see
    /// [`super::sections::scan_argparse_subparsers`]'s doc comment) — this
    /// is a genuinely distinct structural shape, not a knob every profile
    /// could plausibly set, so it earns its own dedicated scan
    /// (`super::sections::scan_comma_separated_commands`) gated on this
    /// flag rather than a general "sometimes commas separate entries" rule
    /// loosening the shared engine for everyone. The general grid rule
    /// (`looks_like_word_grid_start`/`_line`, spec [M-10]) stays strict
    /// because this never touches it.
    pub comma_separated_command_list: bool,
}

/// The profile for `framework`. A `match` over all of [`Framework`]'s
/// variants — exhaustive on purpose (no wildcard arm), so adding a new
/// `Framework` variant is a compile error here until it gets its own arm,
/// which is exactly the "one arm plus one fingerprint" bound this module
/// exists to enforce.
pub fn profile(framework: Framework) -> FrameworkProfile {
    match framework {
        // --- spec §7 Tier B priority order, most measured payoff first ---
        Framework::GnuArgp => FrameworkProfile {
            // Classic single-purpose Unix utilities (`tar`, `dd`, `sed`,
            // `find`, `less`, ...) — exactly [M-10]'s casualty list.
            command_heading_markers: &[],
            non_command_heading_markers: &[],
            positional_heading_markers: &[],
            no_subcommand_concept: true,
            argparse_subparser_quirk: false,
            comma_separated_command_list: false,
        },
        Framework::ClapV3V4 => FrameworkProfile {
            // clap's derive/builder help template renders `"Commands:"`,
            // already caught by the engine's generic word test — nothing
            // extra needed. Verified against `cargo --help` (a real
            // `clap_builder`-linked binary; `ripgrep` was deliberately
            // *not* used for this despite depending on clap, because
            // ripgrep's own `--help` formatter is hand-rolled [M-13] and
            // would make this profile look like it's fitting one tool's
            // output rather than the framework's).
            command_heading_markers: &[],
            non_command_heading_markers: &[],
            positional_heading_markers: &[],
            no_subcommand_concept: false,
            argparse_subparser_quirk: false,
            comma_separated_command_list: false,
        },
        Framework::Argparse => FrameworkProfile {
            // A titled `add_subparsers(title=...)` block renders as an
            // ordinary `"<title>:"` heading, already caught generically.
            // The common *untitled* case renders under `"positional
            // arguments:"`, which is handled by the dedicated
            // `argparse_subparser_quirk` scan below, not by a heading
            // marker — see that scan's doc comment for why "positional
            // arguments" can't just be added to this list (it would
            // fabricate subcommands from a tool's ordinary positional
            // arguments whenever there is no subparser at all).
            command_heading_markers: &[],
            non_command_heading_markers: &[],
            // The other half of that same sentence, and the reason this
            // heading gets *two* mentions in one profile: when the block
            // holds no `{...}` pseudo-entry it is not a command list, and
            // what it *is* is argparse's literal, hardcoded rendering of
            // the tool's positional operands. `argparse.ArgumentParser`
            // writes this exact string (`_("positional arguments")`) for
            // every parser that adds an argument with no leading dash, so
            // it is framework vocabulary in the strictest sense — not a
            // heading one tool happened to print.
            positional_heading_markers: &["positional arguments"],
            no_subcommand_concept: false,
            argparse_subparser_quirk: true,
            comma_separated_command_list: false,
        },
        Framework::Cobra => FrameworkProfile {
            // cobra's command-grouping mechanism always names a group
            // `"<Group> Commands"` (`Available Commands`, `Common
            // Commands`, docker's `Management Commands`, gh's own
            // `CORE COMMANDS` / `GITHUB ACTIONS COMMANDS` / ... — note
            // gh's real headings carry no trailing colon at all, which
            // the engine's heading test already tolerates). Every
            // variant contains the word "commands", already caught
            // generically — nothing extra needed. Verified against real
            // captured `gh --help` and `docker --help` output, and (see
            // this module's test suite) a real-argv extraction against
            // the actual `gh` binary when present.
            command_heading_markers: &[],
            // gh's own `HELP TOPICS` group (cobra's
            // `IsAdditionalHelpTopicCommand` concept, not gh-specific —
            // discovered while fixture-testing against real `gh --help`
            // output) sits at the same indent as, and immediately after,
            // several real command groups. Without this, the engine's
            // sticky same-indent chain (meant for git's own unlabelled
            // group headings) carries `command_mode` straight through it
            // and fabricates `environment`/`reference`/... as if they were
            // real `gh <name>` commands — they're documentation topics,
            // never invokable that way.
            non_command_heading_markers: &["help topics"],
            positional_heading_markers: &[],
            no_subcommand_concept: false,
            argparse_subparser_quirk: false,
            comma_separated_command_list: false,
        },
        Framework::Click => FrameworkProfile {
            // click's `Group` help template renders a plain `"Commands:"`
            // heading, already caught generically.
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
            // The stdlib `flag` package has no subcommand mechanism of
            // its own — that absence is precisely why cobra/urfave-cli
            // exist as separate frameworks. A program that hand-rolls
            // subcommands on top of multiple `FlagSet`s is not something
            // this fingerprint can see anyway.
            command_heading_markers: &[],
            non_command_heading_markers: &[],
            positional_heading_markers: &[],
            no_subcommand_concept: true,
            argparse_subparser_quirk: false,
            comma_separated_command_list: false,
        },
        Framework::Docopt => FrameworkProfile {
            // spec §7 Tier B: "docopt is usage-line-only" — the usage
            // pattern itself is the grammar; docopt's own formatter never
            // renders a separate command-list section the way cobra/clap
            // do. A docopt-based CLI *can* implement git-style
            // subcommands by convention, but that's the tool author's
            // choice, not something docopt itself signals structurally —
            // exactly the kind of per-tool variation this profile must
            // not chase.
            command_heading_markers: &[],
            non_command_heading_markers: &[],
            positional_heading_markers: &[],
            no_subcommand_concept: true,
            argparse_subparser_quirk: false,
            comma_separated_command_list: false,
        },
        Framework::BsdTerse => FrameworkProfile {
            // By the fingerprint's own definition (`help_text_signature`'s
            // `looks_like_bsd_terse`): a short, single `usage:` line and
            // no long-form flags at all — the terse single-purpose-tool
            // shape, not a multi-command one.
            command_heading_markers: &[],
            non_command_heading_markers: &[],
            positional_heading_markers: &[],
            no_subcommand_concept: true,
            argparse_subparser_quirk: false,
            comma_separated_command_list: false,
        },
        Framework::Busybox => FrameworkProfile {
            // The opposite of most entries here: busybox's whole point is
            // multi-call dispatch, so its top-level `--help` genuinely
            // does list every applet as a "command". Its own heading
            // (`"Currently defined functions:"`) doesn't say "command" at
            // all, so it needs an explicit marker rather than relying on
            // the generic test.
            command_heading_markers: &["currently defined functions"],
            non_command_heading_markers: &[],
            positional_heading_markers: &[],
            no_subcommand_concept: false,
            argparse_subparser_quirk: false,
            // Issue #1: this is the one framework whose command list is a
            // flat comma-separated run rather than one entry per line.
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
            // oclif groups topics under `"TOPICS"` in addition to
            // `"COMMANDS"` — "topics" doesn't contain the word "command".
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
            // Grouped per spec §7 Tier B's own table. Thor renders
            // `"Commands:"`; plain `OptionParser` scripts typically have
            // no subcommand concept at all. Since this fingerprint can't
            // tell the two apart, it makes the more conservative
            // assumption (a real command list is at least caught
            // generically when present) rather than forcing zero and
            // silently dropping a real Thor tool's commands.
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

    /// Every `Framework` variant must have a profile — the match in
    /// [`profile`] has no wildcard arm, so this is really a compile-time
    /// guarantee; this test just exercises it so `cargo test` fails loudly
    /// (rather than a compile error a future refactor might paper over
    /// with `_ => ...`) if that ever changes.
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
            // The invariant the whole struct exists to encode: a
            // framework with no subcommand concept never also carries
            // extra command headings to recognize (that would be
            // self-contradictory data).
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
