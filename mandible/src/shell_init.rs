//! `mandible --shell-init <shell>`: the snippet that turns
//! `--print-selection` into a key on the user's prompt.
//!
//! A TUI cannot type into the shell that launched it, so the handoff is the
//! one every picker uses: the shell runs mandible in a command
//! substitution, mandible draws on stderr and prints the composed command
//! on stdout, and the shell's own line editor puts that line where the
//! cursor is. bash does it through `READLINE_LINE`/`READLINE_POINT` in a
//! `bind -x` function; zsh through `BUFFER`/`CURSOR` in a zle widget.
//!
//! The snippets are files in `packaging/shell/`, compiled in with
//! `include_str!` and printed by the binary rather than installed to a
//! path. That is spec §15's completions rule applied to the same problem:
//! one generator, so no packaging channel can ship a snippet that
//! disagrees with the flags the installed binary actually has, and
//! `cargo install` users get it without any packaging at all.

use clap::ValueEnum;

/// A shell `--shell-init` can emit an integration for.
///
/// Deliberately not `clap_complete::Shell`: that enum names every shell
/// clap can complete for, and this flag can only honestly offer the ones
/// with a snippet written and tested for them. Offering `fish` in the help
/// text and printing nothing for it would be worse than not offering it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ShellInit {
    /// bash 4.0 or newer (`READLINE_LINE` in a `bind -x` function).
    Bash,
    /// zsh (`BUFFER` in a zle widget).
    Zsh,
}

impl ShellInit {
    /// The snippet to print, verbatim.
    pub fn snippet(self) -> &'static str {
        match self {
            ShellInit::Bash => include_str!("../../packaging/shell/mandible.bash"),
            ShellInit::Zsh => include_str!("../../packaging/shell/mandible.zsh"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each snippet has to actually edit its shell's line buffer — the
    /// whole point of the handoff — and each has to invoke the mode that
    /// makes the composed line the only thing on stdout. A snippet that
    /// called plain `mandible <tool>` would hang the shell on a UI whose
    /// output it was capturing.
    #[test]
    fn each_snippet_edits_its_shells_line_buffer_through_print_selection() {
        for (shell, buffer, cursor) in [
            (ShellInit::Bash, "READLINE_LINE", "READLINE_POINT"),
            (ShellInit::Zsh, "BUFFER", "CURSOR"),
        ] {
            let snippet = shell.snippet();
            assert!(snippet.contains("mandible --print-selection"), "{shell:?}");
            assert!(snippet.contains(buffer), "{shell:?} must set {buffer}");
            assert!(snippet.contains(cursor), "{shell:?} must set {cursor}");
        }
    }

    /// Both snippets bind the same key, so the two rc files a user might
    /// have do not disagree about how to reach the same feature.
    #[test]
    fn both_snippets_bind_the_same_key() {
        assert!(ShellInit::Bash.snippet().contains(r#""\C-xm""#));
        assert!(ShellInit::Zsh.snippet().contains("'^Xm'"));
    }
}
