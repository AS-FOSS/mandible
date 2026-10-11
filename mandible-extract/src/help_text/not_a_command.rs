//! A probe answer that says the probed word is not a command of the tool
//! (docs/shapes.md S-217): `git: 'citool' is not a git command. See ...` is
//! the tool refusing the word, not help for it, and the did-you-mean block
//! after it is not a command list. Content-keyed: the first non-blank line
//! must name the probed word and say it is not / unknown as a command.

/// Phrases after which the word is refused as a command.
const UNKNOWN: [&str; 4] = [
    "unknown command",
    "unrecognized command",
    "invalid command",
    "no such command",
];

/// True when the first non-blank line of `text` names `word` and says it is
/// not a command (`'x' is not a git command`, `unknown command "x"`).
pub(super) fn says_word_is_not_a_command(text: &str, word: &str) -> bool {
    let Some(line) = text.lines().find(|l| !l.trim().is_empty()) else {
        return false;
    };
    let line = line.to_ascii_lowercase();
    if !names_word(&line, &word.to_ascii_lowercase()) {
        return false;
    }
    UNKNOWN.iter().any(|p| line.contains(p)) || is_not_a_command(&line, &word.to_ascii_lowercase())
}

/// `'word' is not a [..] command`: the word comes right before the phrase,
/// at most two words between `a` and `command`.
fn is_not_a_command(line: &str, word: &str) -> bool {
    let Some((head, rest)) = line.split_once(" is not a ") else {
        return false;
    };
    if !head.trim_end_matches(['\'', '"', '`']).ends_with(word) {
        return false;
    }
    rest.split_whitespace()
        .take(3)
        .any(|w| w.trim_end_matches(|c: char| !c.is_alphanumeric()) == "command")
}

/// `word` occurs delimited: not inside a longer name.
fn names_word(line: &str, word: &str) -> bool {
    line.match_indices(word).any(|(i, _)| {
        let before = line[..i].chars().next_back();
        let after = line[i + word.len()..].chars().next();
        let edge =
            |c: Option<char>| c.is_none_or(|c| !(c.is_alphanumeric() || c == '-' || c == '_'));
        edge(before) && edge(after)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_naming_the_word_is_not_help() {
        let t = "git: 'citool' is not a git command. See 'git --help'.\n\nThe most similar command is\n\tdifftool\n";
        assert!(says_word_is_not_a_command(t, "citool"));
        assert!(says_word_is_not_a_command(
            "Error: unknown command \"x\" for \"k\"\n",
            "x"
        ));
        assert!(says_word_is_not_a_command(
            "\nk: 'x' is not a valid command\n",
            "x"
        ));
    }

    #[test]
    fn real_help_and_other_words_do_not_match() {
        assert!(!says_word_is_not_a_command(
            "usage: git log [<options>]\n",
            "log"
        ));
        assert!(!says_word_is_not_a_command(
            "git: 'citool' is not a git command.\n",
            "cito"
        ));
        assert!(!says_word_is_not_a_command(
            "Show x. This is not a command for y.\n",
            "x"
        ));
        assert!(!says_word_is_not_a_command("", "x"));
    }
}
