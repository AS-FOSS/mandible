//! The listing-invocation probe (spec §6 rule 2d, docs/shapes.md S-214).
//!
//! A root help whose closed sentence says the tool's own quoted invocations
//! `'<tool> help <option>'` list its subcommands admits exactly those
//! invocations (at most two) as probes; git's, pnpm's and snap's root texts
//! are the models. The rows are read like any command table and appended to
//! the root's own list. Strict grammar: the quote opens with the tool's name,
//! then `help`, then one option (never `-h`/`--help`); the sentence either
//! continues `[to] list(s) [all|available] subcommands|commands` or opens
//! `summary|list|listing|overview of all subcommands|commands, run|use`.
//! Rows under a group heading naming a documentation noun (git's
//! "interfaces", the `-g` guide list) are dropped.

use std::path::Path;

use mandible_core::{CommandNode, Provenance, Source, Text};

use super::{pick_stream, sections};
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

/// The quoted invocations of one sentence that the sentence says list the
/// tool's commands. Two wordings: the invocation is followed by `[to] list[s]
/// [all] [available] [sub]commands` (git, pnpm), or preceded by `<summary|
/// list|listing|overview> of all [sub]commands, run|use` (snap).
fn parse_sentence(s: &str, tool: &str) -> Option<Vec<String>> {
    for (at, _) in s.match_indices('\'') {
        if continues_group(&s[..at]) {
            continue;
        }
        let Some((opts, rest)) = invocation_group(&s[at..], tool) else {
            continue;
        };
        if lists_after(rest) || (at == first_quote(s, tool) && lists_before(&s[..at])) {
            return Some(opts);
        }
    }
    None
}

/// True when the quote after `pre` is joined to an earlier quoted text, so
/// it is not the start of a group of its own.
fn continues_group(pre: &str) -> bool {
    let pre = pre.trim_end();
    pre.ends_with("' and") || pre.ends_with("' or") || pre.ends_with("',")
}

/// Offset of the first quote that opens a valid invocation.
fn first_quote(s: &str, tool: &str) -> usize {
    s.match_indices('\'')
        .map(|(i, _)| i)
        .find(|&i| quoted_invocation(&s[i..], tool).is_some())
        .unwrap_or(usize::MAX)
}

/// One to `MAX_LISTINGS` invocations joined by `and`, `or` or a comma,
/// and the text after the last.
fn invocation_group<'a>(s: &'a str, tool: &str) -> Option<(Vec<String>, &'a str)> {
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
            Some(more) if more.trim_start().starts_with('\'') => {
                if opts.len() >= MAX_LISTINGS {
                    return None;
                }
                rest = more.trim_start();
            }
            _ => return Some((opts, rest)),
        }
    }
}

/// `[to] list[s] [all] [available] [sub]commands` right after the quotes.
fn lists_after(rest: &str) -> bool {
    let rest = rest.strip_prefix("to ").unwrap_or(rest);
    let mut words = rest.split_whitespace();
    if !matches!(words.next(), Some("list" | "lists")) {
        return false;
    }
    let Some(mut word) = words.next() else {
        return false;
    };
    if word == "all" {
        let Some(w) = words.next() else { return false };
        word = w;
    }
    if word == "available" {
        let Some(w) = words.next() else { return false };
        word = w;
    }
    let word = word.trim_end_matches(|c: char| !c.is_ascii_alphabetic());
    matches!(word, "subcommands" | "commands")
}

/// The sentence text before the quote ends `... of all [sub]commands, run|use`.
fn lists_before(pre: &str) -> bool {
    let pre = pre.trim_end();
    let Some(pre) = pre
        .strip_suffix("run")
        .or_else(|| pre.strip_suffix("use"))
        .map(|p| p.trim_end().trim_end_matches(','))
    else {
        return false;
    };
    let words: Vec<&str> = pre.split_whitespace().collect();
    let n = words.len();
    if n < 4 {
        return false;
    }
    let last = words[n - 1].trim_end_matches(|c: char| !c.is_ascii_alphabetic());
    matches!(last, "commands" | "subcommands")
        && words[n - 2] == "all"
        && words[n - 3] == "of"
        && matches!(words[n - 4], "summary" | "list" | "listing" | "overview")
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
/// group is not a documentation section. The root's sentence says this
/// text lists the tool's commands, so it is read with that lead (a listing
/// like `pnpm help -a` never says "commands" itself), and a column-0
/// introducer line that only announces indented group headings
/// (`Commands can be classified as follows:`) is not a heading of its own.
pub fn listed_subcommands(text: &str, tool: &str) -> Vec<CommandNode> {
    let (text, pairs) = take_two_word_rows(&drop_group_introducers(text));
    let text = format!("The {tool} commands.\n\n{text}");
    let parsed = sections::parse_with_profile(&text, None, Some(tool));
    let mut nodes: Vec<CommandNode> = parsed
        .subcommands
        .into_iter()
        .filter(|n| n.heading_attested)
        .filter(|n| !n.group.as_deref().is_some_and(is_documentation_group))
        .collect();
    attach_pairs(&mut nodes, pairs);
    nodes
}

/// A `<word> <word>  description` row of the listing, with its group.
struct PairRow {
    parent: String,
    child: String,
    summary: String,
    group: Option<String>,
}

fn is_word(t: &str) -> bool {
    t.starts_with(|c: char| c.is_ascii_lowercase())
        && t.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Parse `<indent>word word<2+ spaces>description` into its parts.
fn pair_row(line: &str) -> Option<(String, String, String)> {
    if indent_of(line) == 0 {
        return None;
    }
    let t = line.trim();
    let (names, desc) = t.split_once("  ")?;
    let (a, b) = names.split_once(' ')?;
    let desc = desc.trim();
    (is_word(a) && is_word(b) && desc.starts_with(|c: char| c.is_alphabetic()))
        .then(|| (a.to_string(), b.to_string(), desc.to_string()))
}

/// Pull out the two-word rows whose first word leads two or more rows of
/// the listing (with their wrapped description lines); the rest of the text
/// is returned for the ordinary reader. A first word leading one row only
/// stays where it is.
fn take_two_word_rows(text: &str) -> (String, Vec<PairRow>) {
    let lines: Vec<&str> = text.lines().collect();
    let mut rows: Vec<(usize, usize, PairRow)> = Vec::new();
    let mut group: Option<String> = None;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if indent_of(line) == 0 && line.trim_end().ends_with(':') {
            group = Some(line.trim().to_string());
        }
        if let Some((parent, child, mut summary)) = pair_row(line) {
            let base = indent_of(line);
            let mut end = i + 1;
            while end < lines.len()
                && !lines[end].trim().is_empty()
                && indent_of(lines[end]) > base
                && pair_row(lines[end]).is_none()
            {
                summary.push(' ');
                summary.push_str(lines[end].trim());
                end += 1;
            }
            rows.push((
                i,
                end,
                PairRow {
                    parent,
                    child,
                    summary,
                    group: group.clone(),
                },
            ));
            i = end;
            continue;
        }
        i += 1;
    }
    let counts: Vec<usize> = rows
        .iter()
        .map(|r| rows.iter().filter(|o| o.2.parent == r.2.parent).count())
        .collect();
    let keep: Vec<(usize, usize, PairRow)> = rows
        .into_iter()
        .zip(counts)
        .filter(|(_, n)| *n >= 2)
        .map(|(r, _)| r)
        .collect();
    let mut drop = vec![false; lines.len()];
    for (a, b, _) in &keep {
        for d in &mut drop[*a..*b] {
            *d = true;
        }
    }
    let rest = lines
        .iter()
        .zip(&drop)
        .filter(|(_, d)| !**d)
        .map(|(l, _)| *l)
        .collect::<Vec<_>>()
        .join("\n");
    (rest, keep.into_iter().map(|r| r.2).collect())
}

/// Each pair row becomes a child of the node named by its first word,
/// created (after the listed rows, in the group of its first row) when the
/// listing has no row for that word.
fn attach_pairs(nodes: &mut Vec<CommandNode>, pairs: Vec<PairRow>) {
    for row in pairs {
        let at = match nodes.iter().position(|n| n.name == row.parent) {
            Some(at) => at,
            None => {
                let mut parent =
                    CommandNode::new(&row.parent, Provenance::single(Source::HelpText));
                parent.heading_attested = true;
                parent.group = row.group.clone();
                nodes.push(parent);
                nodes.len() - 1
            }
        };
        let mut child = CommandNode::new(&row.child, Provenance::single(Source::HelpText));
        child.summary = Some(Text::sanitize(&row.summary));
        child.heading_attested = true;
        child.group = row.group;
        nodes[at].subcommands.push(child);
    }
}

/// Drop each column-0 line ending in `:` whose next non-blank line is an
/// indented line that also ends in `:` (a group heading one level down).
fn drop_group_introducers(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        let next = lines[i + 1..].iter().find(|l| !l.trim().is_empty());
        let introducer = !line.starts_with(char::is_whitespace)
            && line.trim_end().ends_with(':')
            && next
                .is_some_and(|n| n.starts_with(char::is_whitespace) && n.trim_end().ends_with(':'));
        if !introducer {
            out.push(line);
        }
    }
    out.join("\n")
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
        let Ok(out) = crate::exec::run_probe(probe, tool_path, &InertArgv::HelpListing { option })
        else {
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
            "use 'pnpm help -a' and then list all commands",
            "For a short summary of all commands, see 'snap help --all'.",
            "Run 'snap help --all' for a summary of all commands",
            "For a summary of all commands, run 'snap help --all' or 'snap help -x' or 'snap help -y'.",
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
    fn verb_after_and_summary_before_wordings() {
        assert_eq!(
            listing_options(
                "These are common pnpm commands, use 'pnpm help -a' to list all commands",
                "pnpm"
            ),
            vec!["-a"]
        );
        assert_eq!(
            listing_options(
                "For more, run 'snap help <command>'.\nFor a short summary of all commands, run 'snap help --all'.",
                "snap"
            ),
            vec!["--all"]
        );
        assert!(listing_options("use 'pnpm help -a' to list all commands", "pn").is_empty());
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

    #[test]
    fn plain_and_introduced_group_listings_are_read() {
        let pnpm = "Usage: pnpm [command]\n\nManage your dependencies:\n      add      Installs a package\n   i, install  Install all\n\nOther:\n      init     Create a package.json file\n";
        let names = |t: &str, tool: &str| -> Vec<String> {
            listed_subcommands(t, tool)
                .into_iter()
                .map(|n| n.name)
                .collect()
        };
        assert_eq!(names(pnpm, "pnpm"), ["add", "install", "init"]);
        let snap = "Usage: snap <command>\n\nCommands can be classified as follows:\n\n  Basics (basic management):\n    find     Find packages\n    info     Show details\n\n  History (changes):\n    tasks    List tasks\n";
        assert_eq!(names(snap, "snap"), ["find", "info", "tasks"]);
    }

    #[test]
    fn two_word_rows_lead_children_only_when_the_first_word_repeats() {
        let t = "Usage: pnpm [command]\n\nOther:\n      init     Create a file\n\nStore:\n      store add     Adds new\n                    packages\n      store path    Prints the path\n      solo one      A lone pair\n";
        let nodes = listed_subcommands(t, "pnpm");
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["init", "store"], "{names:?}");
        let kids: Vec<(&str, String)> = nodes[1]
            .subcommands
            .iter()
            .map(|n| (n.name.as_str(), n.summary.as_ref().unwrap().to_string()))
            .collect();
        assert_eq!(kids[0], ("add", "Adds new packages".to_string()));
        assert_eq!(kids[1].0, "path");
    }
}
