//! The listing-invocation probe (spec §6 rule 2d, docs/shapes.md S-214).
//!
//! A root help whose closed sentence says quoted invocations
//! `'<tool> help <option>'` list the tool's subcommands admits exactly those
//! invocations (at most two) as probes; git's root text is the model. The
//! rows are read like any command table and appended to the root's own list.
//! Strict grammar: the quote opens with the tool's name, then `help`, then an
//! option (never `-h`/`--help`); the sentence ends `list(s) [all|available]
//! subcommands|commands`. Rows under a group heading naming a documentation
//! noun (git's "interfaces", the `-g` guide list) are dropped.

use std::path::Path;

use mandible_core::CommandNode;

use super::{pick_stream, sections, EXTRACT_TIMEOUT};
use crate::exec::{InertArgv, Probe};

/// Most invocations one root text may name.
const MAX_LISTINGS: usize = 2;

/// Words in a group heading that make its rows documents, not commands.
const DOC_NOUNS: [&str; 10] = [
    "interface",
    "interfaces",
    "format",
    "formats",
    "protocol",
    "protocols",
    "guide",
    "guides",
    "concept",
    "concepts",
];

/// The options the root text's listing sentence names, in text order.
pub fn listing_options(raw: &str, tool: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for paragraph in raw.split("\n\n") {
        let text = paragraph.split_whitespace().collect::<Vec<_>>().join(" ");
        for at in sentence_starts(&text) {
            if let Some(found) = parse_sentence(&text[at..], tool) {
                for opt in found {
                    if !out.contains(&opt) && out.len() < MAX_LISTINGS {
                        out.push(opt);
                    }
                }
            }
        }
    }
    out
}

/// Byte offsets where a sentence may begin: the paragraph start and after
/// each `. `.
fn sentence_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(text.match_indices(". ").map(|(i, _)| i + 2));
    starts
}

/// `'<tool> help <opt>' [and|or '<tool> help <opt>'] list[s] [all]
/// [available] [sub]commands`, anchored at the start of `s`.
fn parse_sentence(s: &str, tool: &str) -> Option<Vec<String>> {
    let mut rest = s;
    let mut opts = Vec::new();
    loop {
        let (opt, after) = quoted_invocation(rest, tool)?;
        opts.push(opt);
        rest = after.trim_start();
        let joined = rest
            .strip_prefix("and ")
            .or_else(|| rest.strip_prefix("or "))
            .or_else(|| rest.strip_prefix(", "));
        match joined {
            Some(more) if opts.len() < MAX_LISTINGS => rest = more.trim_start(),
            _ => break,
        }
    }
    let mut words = rest.split_whitespace();
    if !matches!(words.next()?, "list" | "lists") {
        return None;
    }
    let mut word = words.next()?;
    if word == "all" {
        word = words.next()?;
    }
    if word == "available" {
        word = words.next()?;
    }
    let word = word.trim_end_matches(|c: char| !c.is_ascii_alphabetic());
    matches!(word, "subcommands" | "commands").then_some(opts)
}

/// One `'<tool> help <opt>'` at the start of `s`: the option and the text
/// after the closing quote.
fn quoted_invocation<'a>(s: &'a str, tool: &str) -> Option<(String, &'a str)> {
    let inner = s.strip_prefix('\'')?;
    let (body, after) = inner.split_once('\'')?;
    let mut tokens = body.split(' ');
    if tokens.next()? != tool || tokens.next()? != "help" {
        return None;
    }
    let opt = tokens.next()?;
    if tokens.next().is_some() || !is_listing_option(opt) {
        return None;
    }
    Some((opt.to_string(), after))
}

/// `-a` style or `--word` style, and not the help flags themselves.
fn is_listing_option(opt: &str) -> bool {
    if matches!(opt, "-h" | "--help") {
        return false;
    }
    if let Some(long) = opt.strip_prefix("--") {
        return long.chars().next().is_some_and(|c| c.is_ascii_lowercase())
            && long.chars().all(|c| c.is_ascii_lowercase() || c == '-');
    }
    opt.strip_prefix('-')
        .is_some_and(|short| short.len() == 1 && short.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// True when the group heading names a documentation noun.
fn is_documentation_group(group: &str) -> bool {
    group
        .split(|c: char| !c.is_ascii_alphabetic())
        .any(|w| DOC_NOUNS.contains(&w.to_ascii_lowercase().as_str()))
}

/// The subcommands a listing's text names: heading-attested rows whose
/// group is not a documentation section.
pub fn listed_subcommands(text: &str, tool: &str) -> Vec<CommandNode> {
    let parsed = sections::parse_with_profile(text, None, Some(tool));
    parsed
        .subcommands
        .into_iter()
        .filter(|n| n.heading_attested)
        .filter(|n| !n.group.as_deref().is_some_and(is_documentation_group))
        .collect()
}

/// Append the listed names the root does not already carry. Existing
/// entries are untouched.
pub fn merge_listed(subcommands: &mut Vec<CommandNode>, listed: Vec<CommandNode>) {
    for node in listed {
        if !subcommands.iter().any(|n| n.name == node.name) {
            subcommands.push(node);
        }
    }
}

/// Run each listing invocation the root text names and merge what they
/// list into `root_subcommands`. A failed or empty probe adds nothing.
pub fn probe_and_merge(
    probe: &dyn Probe,
    tool_path: &Path,
    raw: &str,
    tool: &str,
    root_subcommands: &mut Vec<CommandNode>,
) {
    for option in listing_options(raw, tool) {
        let Ok(out) = probe.run(
            tool_path,
            &InertArgv::HelpListing { option },
            EXTRACT_TIMEOUT,
        ) else {
            continue;
        };
        let text = pick_stream(&out.stdout, &out.stderr);
        merge_listed(root_subcommands, listed_subcommands(&text, tool));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIT: &str = "usage: git [-v]\n\n'git help -a' and 'git help -g' list available subcommands and some\nconcept guides. See 'git help <command>' or 'git help <concept>'\nto read about a specific subcommand or concept.\n";

    #[test]
    fn git_sentence_names_both_invocations() {
        assert_eq!(listing_options(GIT, "git"), vec!["-a", "-g"]);
    }

    #[test]
    fn prose_mentions_and_other_wordings_never_match() {
        for t in [
            "See 'git help <command>' to read about a specific subcommand",
            "use 'pnpm help -a' to list all commands",
            "For a short summary of all commands, run 'snap help --all'.",
            "'git help -h' lists commands",
            "'other help -a' lists commands",
            "'git help -a' lists files",
            "'git help -a and more' lists commands",
        ] {
            assert!(
                listing_options(t, "git").is_empty()
                    && listing_options(t, "pnpm").is_empty()
                    && listing_options(t, "snap").is_empty(),
                "{t}"
            );
        }
    }

    #[test]
    fn single_and_long_option_forms() {
        assert_eq!(
            listing_options("'tool help --all' lists all available commands.", "tool"),
            vec!["--all"]
        );
        assert_eq!(
            listing_options(
                "Try 'tool help -a' or 'tool help -b' to go.\n\nNote. 'tool help -c' list commands",
                "tool"
            ),
            vec!["-c"]
        );
    }

    #[test]
    fn at_most_two_invocations() {
        let t = "'t help -a', 't help -b' and 't help -c' list commands";
        assert!(listing_options(t, "t").is_empty());
    }

    #[test]
    fn documentation_groups_are_not_commands() {
        assert!(is_documentation_group(
            "User-facing repository, command and file interfaces"
        ));
        assert!(is_documentation_group(
            "Developer-facing file formats, protocols and other interfaces"
        ));
        assert!(!is_documentation_group("Interacting with Others"));
        assert!(!is_documentation_group(
            "Low-level Commands / Internal Helpers"
        ));
    }

    #[test]
    fn listed_rows_under_command_headings_only() {
        let text = "Main Porcelain Commands\n   add     Add file contents to the index\n   am      Apply a series of patches\n\nUser-facing repository, command and file interfaces\n   attributes   Defining attributes per path\n   cli          Git command-line interface and conventions\n";
        let nodes = listed_subcommands(text, "git");
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, vec!["add", "am"], "{names:?}");
    }
}
