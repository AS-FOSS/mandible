//! `mandible mandible` — the about screen.
//!
//! Running the tool on its own name shows this instead of extracting
//! mandible's own `--help`. Self-introspection isn't lost: `mandible
//! --doctor mandible` still runs the real pipeline against the binary,
//! which is the diagnostic form anyone actually wants for that purpose.
//!
//! Everything here is drawn from Cargo metadata at compile time, so the
//! version and repository can't drift from what was actually built. There
//! is deliberately no hand-maintained fact that a release could
//! invalidate — an about screen that lies about its own version is worse
//! than no about screen.

/// The jaws the project is named for: what chews through a CLI's help
/// text. Drawn narrow enough (34 columns) to sit inside an 80-column
/// terminal with room for the text column beside it.
const JAWS: &str = r#"
     __                    __
     \ \                  / /
      \ \                / /
       \ \______  ______/ /
        \       \/       /
        |   ()      ()   |
         \      __      /
          \    /  \    /
           \__/    \__/
"#;

/// ANSI SGR wrapper that becomes a no-op when color is disabled, so
/// `NO_COLOR` and piped output stay clean (spec §9.2).
fn paint(text: &str, code: &str, color: bool) -> String {
    if color {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

/// Print the about screen to stdout.
pub fn print() {
    let color = mandible_tui::style::color_enabled_from_env();

    let accent = "38;5;173"; // muted amber — the chitin the jaws are made of
    let dim = "2";
    let bold = "1";

    println!("{}", paint(JAWS, accent, color));

    println!(
        "  {}  {}",
        paint(env!("CARGO_PKG_NAME"), bold, color),
        paint(&format!("v{}", env!("CARGO_PKG_VERSION")), dim, color),
    );
    println!(
        "  {}",
        paint(
            "A TUI manual for every command-line tool you have.",
            dim,
            color
        )
    );
    println!();

    // The one idea the whole architecture exists to serve. Worth stating
    // plainly on the one screen someone reads out of curiosity.
    println!("  {}", paint("The rule:", bold, color));
    println!("    No per-tool logic, ever. Help text isn't written by hand, it's");
    println!("    generated — so mandible learns the generators (clap, cobra, argparse,");
    println!("    click, GNU argp, and the rest), not the tools. Fixing the argparse");
    println!("    grammar improves every Python CLI ever written.");
    println!();

    println!("  {}", paint("When it can't parse something:", bold, color));
    println!("    It shows you the author's own text, untouched, and says so.");
    println!("    It never invents structure it didn't find.");
    println!();

    println!(
        "  {}   {}",
        paint("repository", dim, color),
        env!("CARGO_PKG_REPOSITORY")
    );
    println!(
        "  {}      {}",
        paint("license", dim, color),
        env!("CARGO_PKG_LICENSE")
    );
    println!();

    println!(
        "  {}",
        paint(
            "try:  mandible git    mandible docker    mandible --doctor <tool>",
            dim,
            color
        )
    );
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_is_a_no_op_without_color() {
        assert_eq!(paint("hi", "1", false), "hi");
    }

    #[test]
    fn paint_wraps_and_resets_with_color() {
        let painted = paint("hi", "1", true);
        assert!(painted.starts_with("\x1b[1m"));
        assert!(painted.ends_with("\x1b[0m"));
    }

    /// The art must stay inside a narrow terminal. 34 columns leaves room
    /// beside it even at 80 wide.
    #[test]
    fn jaws_fit_a_narrow_terminal() {
        let widest = JAWS.lines().map(|l| l.chars().count()).max().unwrap();
        assert!(widest <= 34, "art is {widest} columns wide");
    }

    /// Pure ASCII: this prints before any terminal setup, on whatever
    /// encoding the user's terminal happens to have.
    #[test]
    fn jaws_are_ascii_only() {
        assert!(JAWS.is_ascii());
    }
}
