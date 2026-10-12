//! Usage forms that each open with a different literal verb after the
//! command path (`git bisect start ...`, `git bisect (good|bad) ...`;
//! docs/shapes.md S-215). The verbs are the command's subcommands; each node
//! carries the forms naming it and the flags and operands they spell. As in
//! S-179 and S-211 the nodes are `invocation_attested`: listed, never probed.
//! The parent keeps those rows unless every form is a verb's (S-215).
//!
//! Guards: two or more distinct verbs open a form directly, in a labelled
//! usage block. A verb is a bare lowercase command-name word or a
//! parenthesized alternation of them; a word after a flag, a `<placeholder>`,
//! an UPPERCASE word or a bracketed word never is. A closed group of flags
//! before the word (`[-v | --verbose] show`) is skipped.

use super::*;

/// One top-level token of a usage form.
fn tokens(form: &str) -> Vec<String> {
    let (mut out, mut cur, mut depth) = (Vec::new(), String::new(), 0i32);
    for c in form.chars() {
        depth += i32::from(matches!(c, '[' | '(')) - i32::from(matches!(c, ']' | ')'));
        if c.is_whitespace() && depth <= 0 {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            depth = 0;
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn is_word(t: &str) -> bool {
    is_command_name_shaped(t)
}

/// `(good|bad)`: two or more literal lowercase words.
fn alternation(t: &str) -> Option<Vec<String>> {
    let inner = t.strip_prefix('(')?.strip_suffix(')')?;
    let words: Vec<&str> = inner.split('|').collect();
    (words.len() >= 2 && words.iter().all(|w| is_word(w)))
        .then(|| words.iter().map(|w| w.to_string()).collect())
}

/// `[-v | --verbose]`: a closed bracket group holding only dash-led words.
fn is_flag_group(t: &str) -> bool {
    let Some(inner) = t.strip_prefix('[').and_then(|r| r.strip_suffix(']')) else {
        return false;
    };
    let mut words = inner.split_whitespace().filter(|w| *w != "|").peekable();
    words.peek().is_some() && words.all(|w| w.starts_with('-'))
}

struct Form {
    toks: Vec<String>,
    text: String,
}

/// The form's program word: the tool's name, or a path whose last component
/// is that name (`/usr/bin/linux-version`, S-240).
fn is_program_word(first: &str, program: &str) -> bool {
    first == program
        || first
            .rsplit_once('/')
            .is_some_and(|(_, last)| !last.is_empty() && last == program)
}

/// The usage entries as forms: label removed, program name stripped.
fn forms(usage: &[String], tool: Option<&str>) -> Vec<Form> {
    let mut prog = tool.map(str::to_string);
    let mut out = Vec::new();
    for entry in usage {
        let mut t = entry.trim();
        let lower = t.to_ascii_lowercase();
        if lower.starts_with("usage:") || lower.starts_with("or:") {
            t = t.split_once(':').map_or("", |(_, r)| r).trim();
        }
        let Some(first) = t.split_whitespace().next() else {
            continue;
        };
        let p = prog.get_or_insert_with(|| first.to_string());
        if !is_program_word(first, p) {
            continue;
        }
        let toks = tokens(t);
        out.push(Form {
            toks: toks[1..].to_vec(),
            text: t.to_string(),
        });
    }
    out
}

/// Words opening every form: the command path (`bisect`).
fn path_len(forms: &[Form]) -> usize {
    let lead = |f: &Form| f.toks.iter().take_while(|t| is_word(t)).count();
    let mut n = forms.iter().map(lead).min().unwrap_or(0);
    for k in 0..n {
        if forms.iter().any(|f| f.toks[k] != forms[0].toks[k]) {
            n = k;
            break;
        }
    }
    n
}

/// The verbs a form opens with, the index of the first token after the
/// skipped flag groups, and whether any group was skipped.
fn verbs_of(f: &Form, at: usize) -> Option<(Vec<String>, usize, bool)> {
    let mut j = at;
    while f.toks.get(j).is_some_and(|t| is_flag_group(t)) {
        j += 1;
    }
    let t = f.toks.get(j)?;
    let verbs = if is_word(t) {
        vec![t.clone()]
    } else {
        alternation(t)?
    };
    Some((verbs, j, j > at))
}

/// Add a node per verb to `subcommands` (names already present are kept
/// untouched). `usage` is the parsed usage entries. True when every usage
/// entry is a form opening with a verb that is now a subcommand.
///
/// `own` is the node's own command word when it is not the root. Verbs must
/// follow the node's own path: a form that does not name it (`git show ...`
/// printed by `git log -h`) is another command's form, not a verb of this one.
pub(crate) fn recover(
    usage: &[String],
    tool_name: Option<&str>,
    own: Option<&str>,
    raw: &str,
    subcommands: &mut Vec<CommandNode>,
) -> bool {
    // Only a labelled usage block: bare indented invocation lines are
    // examples or a command table (S-016), not the tool's forms.
    if !usage
        .first()
        .is_some_and(|u| starts_with_usage_prefix(u.trim()))
    {
        return false;
    }
    let mut forms = forms(usage, tool_name);
    if let Some(own) = own {
        forms.retain(|f| f.toks.iter().take_while(|t| is_word(t)).any(|t| t == own));
    }
    if forms.len() < 2 {
        return false;
    }
    let at = path_len(&forms);
    let found: Vec<Option<(Vec<String>, usize, bool)>> =
        forms.iter().map(|f| verbs_of(f, at)).collect();
    let mut direct: Vec<&String> = found
        .iter()
        .flatten()
        .filter(|(_, _, skipped)| !skipped)
        .flat_map(|(v, _, _)| v.iter())
        .collect();
    direct.sort();
    direct.dedup();
    if direct.len() < 2 {
        return false;
    }
    let mut order: Vec<String> = Vec::new();
    for (verbs, _, _) in found.iter().flatten() {
        for v in verbs {
            if !order.contains(v) {
                order.push(v.clone());
            }
        }
    }
    for verb in order {
        if subcommands.iter().any(|n| n.name == verb)
            || subcommands.len() >= MAX_RECOVERED_ENTRIES
            || !token_occurs_literally(raw, &verb)
        {
            continue;
        }
        let mut node = CommandNode::new(&verb, Provenance::single(Source::HelpText));
        node.invocation_attested = true;
        node.heading_attested = false;
        node.children_filled = true;
        let mut own: Vec<String> = Vec::new();
        for (f, hit) in forms.iter().zip(&found) {
            let Some((verbs, j, _)) = hit else { continue };
            if !verbs.contains(&verb) {
                continue;
            }
            let text = Text::sanitize(&f.text);
            if !node.usage.contains(&text) {
                node.usage.push(text);
            }
            let mut line = vec![verb.clone()];
            line.extend(f.toks[*j + 1..].iter().cloned());
            line.extend(f.toks[at..*j].iter().cloned());
            own.push(line.join(" "));
        }
        let mut flags: Vec<Entity> = Vec::new();
        for flag in extract_usage_flags(&own) {
            if !flag_spelling_already_present(&flag, &flags) {
                flags.push(flag);
            }
        }
        let primary: std::collections::HashSet<usize> = (0..own.len()).collect();
        let mut positionals = if flags.is_empty() {
            extract_positionals_inner(&own, primary, false, true)
        } else {
            extract_positionals(&own, primary)
        };
        super::usage_numbered_caps::add_numbered_operands(&own, &mut positionals);
        node.entities.extend(flags);
        node.entities.extend(positionals);
        subcommands.push(node);
    }
    forms.len() == usage.iter().filter(|u| !u.trim().is_empty()).count()
        && found.iter().all(|hit| {
            hit.as_ref().is_some_and(|(verbs, _, _)| {
                verbs
                    .iter()
                    .all(|v| subcommands.iter().any(|n| &n.name == v))
            })
        })
}

/// Drop the parent's undescribed rows that a verb node carries: every form
/// spelling them is that verb's (S-240), so they are relocated, not lost.
pub(crate) fn drop_relocated(parent: &mut CommandNode) {
    let key = |e: &Entity| {
        let names: Vec<String> = e.spellings.iter().map(|s| s.render()).collect();
        (e.kind, names)
    };
    let carried: std::collections::HashSet<_> = parent
        .subcommands
        .iter()
        .filter(|n| n.invocation_attested)
        .flat_map(|n| n.entities.iter().map(key))
        .collect();
    parent.entities.retain(|e| {
        !matches!(e.kind, EntityKind::Flag | EntityKind::Positional)
            || e.description.is_some()
            || !e.choices.is_empty()
            || !carried.contains(&key(e))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verbs(usage: &[&str], tool: &str) -> Vec<String> {
        let usage: Vec<String> = usage.iter().map(|s| s.to_string()).collect();
        let mut subs = Vec::new();
        recover(&usage, Some(tool), None, &usage.join(" "), &mut subs);
        subs.into_iter().map(|n| n.name).collect()
    }

    #[test]
    fn a_path_then_different_verbs_names_them_and_splits_an_alternation() {
        let u = [
            "usage: git bisect start [--no-checkout] [<bad> [<good>...]]",
            "   or: git bisect (good|bad) [<rev>...]",
            "   or: git bisect next",
            "   or: git bisect run <cmd> [<arg>...]",
        ];
        assert_eq!(verbs(&u, "git"), ["start", "good", "bad", "next", "run"]);
    }

    #[test]
    fn flag_group_before_the_word_is_skipped_and_bracketed_words_never_count() {
        let u = [
            "usage: git remote [-v | --verbose]",
            "   or: git remote add <name> <url>",
            "   or: git remote rename <a> <b>",
            "   or: git remote [-v | --verbose] show [-n] <name>",
            "   or: git remote [show] <x>",
        ];
        assert_eq!(verbs(&u, "git"), ["add", "rename", "show"]);
    }

    #[test]
    fn a_program_word_that_is_a_path_to_the_tool_opens_its_forms() {
        let u = [
            "Usage: /usr/bin/lv compare VERSION1 OP VERSION2",
            "       /usr/bin/lv sort [--reverse] [VERSION1 VERSION2 ...]",
            "       /usr/bin/lv list [--paths]",
        ];
        assert_eq!(verbs(&u, "lv"), ["compare", "sort", "list"]);
        let other = [
            "Usage: /usr/bin/xx compare A",
            "       /usr/bin/xx sort [-r]",
        ];
        assert!(verbs(&other, "lv").is_empty());
    }

    #[test]
    fn a_form_naming_another_command_is_not_a_verb_of_this_node() {
        let usage: Vec<String> = [
            "usage: git log [<options>] [<revision-range>]",
            "   or: git show [<options>] <object>...",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut subs = Vec::new();
        recover(
            &usage,
            Some("git"),
            Some("log"),
            &usage.join(" "),
            &mut subs,
        );
        assert!(subs.is_empty());
    }

    #[test]
    fn a_word_after_a_flag_a_placeholder_and_one_verb_name_nothing() {
        assert!(verbs(&["usage: k -Y sign <f>", "   or: k -Y verify <f>"], "k").is_empty());
        assert!(verbs(&["usage: k <cmd> a", "   or: k <cmd> b"], "k").is_empty());
        assert!(verbs(&["usage: k [-q] ARG", "   or: k start"], "k").is_empty());
        assert!(verbs(&["usage: k copy a", "   or: k copy a b"], "k").is_empty());
    }

    #[test]
    fn each_verb_node_carries_its_own_flags_and_is_never_probed() {
        let u = [
            "usage: t add [-f] <name>",
            "   or: t rm [--force] <name>...",
        ];
        let usage: Vec<String> = u.iter().map(|s| s.to_string()).collect();
        let mut subs = Vec::new();
        recover(&usage, Some("t"), None, &usage.join(" "), &mut subs);
        let rm = subs.iter().find(|n| n.name == "rm").unwrap();
        assert!(rm.invocation_attested && !rm.heading_attested);
        assert!(rm.flags().any(|f| f.long() == Some("force")));
        assert!(!rm.flags().any(|f| f.short() == Some('f')));
    }

    /// The SYNOPSIS of `git history` (maintainer capture, git 2.55) written
    /// as the usage forms `-h` prints.
    #[test]
    fn git_history_forms_give_three_verbs_with_choice_valued_flags() {
        let usage: Vec<String> = [
            "usage: git history fixup <commit> [--dry-run] [--update-refs=(branches|head)] [--reedit-message] [--empty=(drop|keep|abort)]",
            "   or: git history reword <commit> [--dry-run] [--update-refs=(branches|head)]",
            "   or: git history split <commit> [--dry-run] [--update-refs=(branches|head)] [--] [<pathspec>...]",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut subs = Vec::new();
        recover(&usage, Some("git"), None, &usage.join(" "), &mut subs);
        let names: Vec<&str> = subs.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["fixup", "reword", "split"]);
        let empty = subs[0].flags().find(|f| f.long() == Some("empty")).unwrap();
        assert_eq!(empty.value_name.as_deref(), Some("(drop|keep|abort)"));
    }

    /// git subtree: verbs padded with extra spaces, one verb in two forms.
    #[test]
    fn padded_verbs_and_a_verb_in_two_forms_fold_to_one_node() {
        let u = [
            "usage: git subtree add   --prefix=<prefix> <commit>",
            "   or: git subtree add   --prefix=<prefix> <repository> <ref>",
            "   or: git subtree merge --prefix=<prefix> <commit>",
            "   or: git subtree split --prefix=<prefix> [<commit>]",
        ];
        assert_eq!(verbs(&u, "git"), ["add", "merge", "split"]);
    }

    #[test]
    fn fsmonitor_daemon_verbs_with_a_double_dash_command_path() {
        let u = [
            "usage: git fsmonitor--daemon start [<options>]",
            "   or: git fsmonitor--daemon run [<options>]",
            "   or: git fsmonitor--daemon stop",
            "   or: git fsmonitor--daemon status",
        ];
        assert_eq!(verbs(&u, "git"), ["start", "run", "stop", "status"]);
    }
}
