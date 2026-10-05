//! A bracket row `-n|--notable|--table {<table>}`: a short and a long
//! spelling, then a second long spelling that alone carries the value. The
//! first two are one option with two spellings, the last is a separate
//! option with the value. `docs/shapes.md` S-209,
//! `corpus/dmsetup/1.02.185-create-stats`.

use super::*;

fn is_bare_short(m: &str) -> bool {
    m.len() == 2 && m.starts_with('-') && is_flag_shaped(m) && !m.starts_with("--")
}

fn is_bare_long(m: &str) -> bool {
    m.starts_with("--") && is_flag_shaped(m) && !m.contains(char::is_whitespace)
}

/// The row split in two at the second `|`, when it has exactly the shape
/// above; otherwise the row whole.
pub(super) fn split_alias_pair_then_valued_long(content: &str) -> Vec<&str> {
    let members = split_top_level_pipe(content);
    if let [short, long, valued] = members.as_slice() {
        let (name, value) = valued.split_once(char::is_whitespace).unwrap_or((valued, ""));
        if is_bare_short(short)
            && is_bare_long(long)
            && is_bare_long(name)
            && !value.trim().is_empty()
        {
            let cut = content.len() - valued.len() - 1;
            if let (Some(pair), Some(rest)) = (content.get(..cut), content.get(cut + 1..)) {
                return vec![pair, rest];
            }
        }
    }
    vec![content]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_long_pair_then_a_valued_long_splits_in_two() {
        assert_eq!(
            split_alias_pair_then_valued_long("-n|--notable|--table {<table>|<table_file>}"),
            vec!["-n|--notable", "--table {<table>|<table_file>}"]
        );
    }

    #[test]
    fn other_shapes_stay_whole() {
        for c in [
            "-U|--uid <uid>",
            "-n|--notable|--table",
            "--a|--b|--c <v>",
            "-n|-m|--table <v>",
            "-n|--notable|--table {x}|y",
        ] {
            assert_eq!(split_alias_pair_then_valued_long(c), vec![c], "{c}");
        }
    }
}
