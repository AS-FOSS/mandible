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

/// ANSI SGR wrapper that becomes a no-op when color is disabled, so
/// `NO_COLOR` and piped output stay clean (spec §9.2).
fn paint(text: &str, code: &str, color: bool) -> String {
    if color {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

/// The wordmark, one entry per letter, each a block of equal-height rows.
///
/// `█`/`▒` rather than a font: block elements are ordinary Unicode and
/// present in effectively every monospace font, unlike the private-use
/// glyphs a Nerd Font would need (see `mandible_tui::glyphs`). In a
/// non-UTF-8 terminal the banner is skipped entirely rather than drawn as
/// tofu — an about screen is exactly the place that can afford to say less.
const LETTERS: &[&[&str]] = &[
    &[
        "                ",
        "                ",
        " █████████████  ",
        "▒▒███▒▒███▒▒███ ",
        " ▒███ ▒███ ▒███ ",
        " ▒███ ▒███ ▒███ ",
        " █████▒███ █████",
        "▒▒▒▒▒ ▒▒▒ ▒▒▒▒▒ ",
    ],
    &[
        "          ",
        "          ",
        "  ██████  ",
        " ▒▒▒▒▒███ ",
        "  ███████ ",
        " ███▒▒███ ",
        "▒▒████████",
        " ▒▒▒▒▒▒▒▒ ",
    ],
    &[
        "           ",
        "           ",
        " ████████  ",
        "▒▒███▒▒███ ",
        " ▒███ ▒███ ",
        " ▒███ ▒███ ",
        " ████ █████",
        "▒▒▒▒ ▒▒▒▒▒ ",
    ],
    &[
        "     █████",
        "    ▒▒███ ",
        "  ███████ ",
        " ███▒▒███ ",
        "▒███ ▒███ ",
        "▒███ ▒███ ",
        "▒▒████████",
        " ▒▒▒▒▒▒▒▒ ",
    ],
    &[
        "  ███ ",
        " ▒▒▒  ",
        " ████ ",
        "▒▒███ ",
        " ▒███ ",
        " ▒███ ",
        " █████",
        "▒▒▒▒▒ ",
    ],
    &[
        " █████    ",
        "▒▒███     ",
        " ▒███████ ",
        " ▒███▒▒███",
        " ▒███ ▒███",
        " ▒███ ▒███",
        " ████████ ",
        "▒▒▒▒▒▒▒▒  ",
    ],
    &[
        " ████ ",
        "▒▒███ ",
        " ▒███ ",
        " ▒███ ",
        " ▒███ ",
        " ▒███ ",
        " █████",
        "▒▒▒▒▒ ",
    ],
    &[
        "         ",
        "         ",
        "  ██████ ",
        " ███▒▒███",
        "▒███████ ",
        "▒███▒▒▒  ",
        "▒▒██████ ",
        " ▒▒▒▒▒▒  ",
    ],
];

/// Vertical offset per animation step. 4 is resting, 0 is the peak; the
/// values between are what make the letter glide rather than jump.
const TRAJECTORY: &[usize] = &[4, 3, 2, 1, 0, 0, 1, 2, 3, 4];
/// Ticks each letter lags behind the one before it, which is what turns
/// eight independent hops into one wave travelling along the word.
const LETTER_DELAY: usize = 2;
const LETTER_HEIGHT: usize = 8;
const LETTER_SPACING: &str = "  ";
const FPS: u64 = 20;

fn max_shift() -> usize {
    TRAJECTORY.iter().copied().max().unwrap_or(0)
}

/// One frame: each letter drawn at its own vertical offset.
fn frame(offsets: &[usize]) -> Vec<String> {
    let canvas_height = LETTER_HEIGHT + max_shift();
    (0..canvas_height)
        .map(|row| {
            let mut line = String::new();
            for (i, letter) in LETTERS.iter().enumerate() {
                let width = letter[0].chars().count();
                match row.checked_sub(offsets[i]) {
                    Some(idx) if idx < LETTER_HEIGHT => line.push_str(letter[idx]),
                    _ => line.push_str(&" ".repeat(width)),
                }
                line.push_str(LETTER_SPACING);
            }
            line.trim_end().to_string()
        })
        .collect()
}

/// Play the wave once, in place, then leave the wordmark at rest.
///
/// Deliberately finite. An about screen that animates until interrupted is
/// a program you have to escape; this is a command that prints something
/// and exits. It redraws by moving the cursor up rather than clearing the
/// screen, so it stays inline in the scrollback like any other command's
/// output instead of wiping what you were looking at.
///
/// Skipped entirely when stdout is not a terminal — piping this to a file
/// should not fill it with cursor-movement escapes — and when the terminal
/// is not in a UTF-8 locale, where the block elements would be tofu.
fn animate(color: bool) {
    use std::io::Write;

    let canvas_height = LETTER_HEIGHT + max_shift();
    let cycle = LETTERS.len() * LETTER_DELAY + TRAJECTORY.len();
    let mut out = std::io::stdout().lock();

    let _ = write!(out, "\x1b[?25l"); // hide cursor
    for tick in 0..cycle {
        let offsets: Vec<usize> = (0..LETTERS.len())
            .map(|i| {
                let local = (tick + cycle - (i * LETTER_DELAY) % cycle) % cycle;
                TRAJECTORY.get(local).copied().unwrap_or(max_shift())
            })
            .collect();

        if tick > 0 {
            let _ = write!(out, "\x1b[{canvas_height}A");
        }
        for line in frame(&offsets) {
            let _ = writeln!(out, "\x1b[2K{}", paint(&line, "38;5;173", color));
        }
        let _ = out.flush();
        std::thread::sleep(std::time::Duration::from_millis(1000 / FPS));
    }
    let _ = write!(out, "\x1b[?25h"); // restore cursor
    let _ = out.flush();
}

/// True when the banner can be drawn: a real terminal, in a UTF-8 locale.
fn banner_possible() -> bool {
    mandible_tui::terminal::stdout_is_tty()
        && mandible_tui::glyphs::from_env() == mandible_tui::glyphs::UNICODE
}

/// Print the about screen to stdout.
pub fn print() {
    let color = mandible_tui::style::color_enabled_from_env();

    let dim = "2";
    let bold = "1";

    println!();
    if banner_possible() {
        animate(color);
        println!();
    }
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

    /// Every letter block is the same height, so a row index means the
    /// same thing across all of them — the animation shears the wordmark
    /// apart otherwise.
    #[test]
    fn letter_blocks_are_uniform_height() {
        for (i, letter) in LETTERS.iter().enumerate() {
            assert_eq!(
                letter.len(),
                LETTER_HEIGHT,
                "letter {i} is the wrong height"
            );
        }
    }

    /// Each letter's rows are equal width, so a letter lifted out of the
    /// canvas leaves a hole exactly its own size and the ones beside it
    /// don't slide sideways mid-wave.
    #[test]
    fn each_letter_has_uniform_row_width() {
        for (i, letter) in LETTERS.iter().enumerate() {
            let w = letter[0].chars().count();
            for (r, row) in letter.iter().enumerate() {
                assert_eq!(row.chars().count(), w, "letter {i} row {r} is ragged");
            }
        }
    }

    /// At rest the whole word sits on one baseline: nothing is mid-hop
    /// before the wave starts or after it finishes.
    #[test]
    fn resting_frame_is_flat() {
        let rest = vec![max_shift(); LETTERS.len()];
        let lines = frame(&rest);
        assert_eq!(lines.len(), LETTER_HEIGHT + max_shift());
        // The rows above the baseline are empty, the rest are not.
        for line in lines.iter().take(max_shift()) {
            assert!(line.is_empty(), "expected blank lead-in, got {line:?}");
        }
        assert!(lines[max_shift()..].iter().any(|l| l.contains('█')));
    }

    /// A lifted letter really does move up: at the peak of its arc it puts
    /// ink on a row that is blank when the word is at rest.
    ///
    /// Uses `d` (index 3) because its own first row carries ink. `m` and
    /// several others begin with blank rows — lifting those moves the
    /// blanks, and the test would prove nothing.
    #[test]
    fn a_lifted_letter_reaches_higher_than_the_baseline() {
        const D: usize = 3;
        assert!(
            LETTERS[D][0].contains('█'),
            "this test needs a letter with ink on its first row"
        );

        let resting = frame(&vec![max_shift(); LETTERS.len()]);
        assert!(resting[0].is_empty(), "top row should be blank at rest");

        let mut offsets = vec![max_shift(); LETTERS.len()];
        offsets[D] = 0;
        let lifted = frame(&offsets);
        assert!(
            lifted[0].contains('█'),
            "top row should carry the lifted letter: {:?}",
            lifted[0]
        );
    }

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
}
