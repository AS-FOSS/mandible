//! Two small readings of a usage-synopsis bracket group: several flags side
//! by side in one group, and a mode operand spelled with a run of signs.
//! Split out of `usage.rs` to keep that file under its size ceiling
//! (AGENTS.md section 2).

use super::*;

/// One bracket group that holds several flags side by side:
/// `[-j|--major <major> -m|--minor <minor>]` is two flags, each with its
/// own value, not one flag whose value runs on. A piece opens at a
/// top-level word that is flag-shaped when the piece so far already
/// carries a value word and the word before it does not end an alias run
/// (`|`, `,`). Returns the whole content as the one piece otherwise.
/// See docs/shapes.md S-184.
pub(super) fn split_adjacent_flag_specs(content: &str) -> Vec<&str> {
    let mut pieces: Vec<&str> = Vec::new();
    let mut piece_start = 0usize;
    let mut depth = 0i32;
    let mut word_start: Option<usize> = None;
    let mut prev_word: &str = "";
    let mut piece_has_value = false;
    let mut first_word_of_piece = true;
    let bytes_end = content.len();
    let mut boundaries: Vec<(usize, usize)> = Vec::new();
    for (i, c) in content.char_indices() {
        match c {
            '[' | '{' | '<' => depth += 1,
            ']' | '}' | '>' => depth -= 1,
            _ => {}
        }
        if c.is_whitespace() && depth <= 0 {
            if let Some(ws) = word_start.take() {
                boundaries.push((ws, i));
            }
        } else if word_start.is_none() && !c.is_whitespace() {
            word_start = Some(i);
        }
    }
    if let Some(ws) = word_start {
        boundaries.push((ws, bytes_end));
    }
    for &(ws, we) in &boundaries {
        let word = &content[ws..we];
        let flag_word = word.starts_with('-')
            && is_flag_shaped(word)
            && !prev_word.ends_with('|')
            && !prev_word.ends_with(',');
        if flag_word && piece_has_value && !first_word_of_piece {
            pieces.push(content[piece_start..ws].trim());
            piece_start = ws;
            piece_has_value = false;
        } else if !first_word_of_piece && !word.starts_with('-') && !word.starts_with('|') {
            piece_has_value = true;
        }
        first_word_of_piece = false;
        prev_word = word;
    }
    if pieces.is_empty() {
        return vec![content];
    }
    pieces.push(content[piece_start..].trim());
    pieces
}
/// A bracket row that is nothing but alternative long spellings, no
/// value and no short twin (`[--addnodeonresume|--addnodeoncreate]`):
/// the same pairing rule a group inside a longer line follows, which
/// pairs only one short with one long and keeps every other alternative
/// a flag of its own. See docs/shapes.md S-184.
pub(super) fn split_long_only_alternation(content: &str) -> Vec<&str> {
    if content.contains(char::is_whitespace) {
        return vec![content];
    }
    let members = split_top_level_pipe(content);
    let all_long = members.len() >= 2
        && members.iter().all(|m| {
            m.strip_prefix("--").is_some_and(|rest| {
                rest.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                    && rest
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            })
        });
    if all_long {
        members
    } else {
        vec![content]
    }
}
/// The flags one whole-line bracket group names: several side by side, a
/// run of long alternatives, or (the ordinary case) the one flag with its
/// trailing `|`-list of choices. See docs/shapes.md S-184, S-120.
pub(super) fn bracket_row_flag_specs(content: &str) -> Vec<FlagSpec> {
    let mut pieces = split_adjacent_flag_specs(content);
    if pieces.len() == 1 {
        pieces = split_long_only_alternation(content);
    }
    if pieces.len() > 1 {
        return pieces.into_iter().map(parse_flag_spec).collect();
    }
    let mut spec = parse_flag_spec(content);
    spec.choices = trailing_choice_list(content);
    vec![spec]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_valued_flags_in_one_group_split_into_two_pieces() {
        let pieces = split_adjacent_flag_specs("-j|--major <major> -m|--minor <minor>");
        assert_eq!(pieces, vec!["-j|--major <major>", "-m|--minor <minor>"]);
    }

    #[test]
    fn one_flag_with_a_value_stays_one_piece() {
        for c in ["-A|--autobackup y|n", "-j|--major <major>", "--a <x> y"] {
            assert_eq!(split_adjacent_flag_specs(c), vec![c]);
        }
    }

    #[test]
    fn a_bracket_row_of_long_alternatives_splits_but_short_and_long_does_not() {
        assert_eq!(
            split_long_only_alternation("--addnodeonresume|--addnodeoncreate"),
            vec!["--addnodeonresume", "--addnodeoncreate"]
        );
        assert_eq!(
            split_long_only_alternation("-d|--debug"),
            vec!["-d|--debug"]
        );
    }
}
