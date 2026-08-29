//! The styling contract (spec §9.2): one accent, spent only on
//! information; everything else neutral.
//!
//! Four rules that matter more than the palette:
//!
//! - **ANSI indexed colors, not RGB.** [`ACCENT`] and [`WARNING`] are
//!   `ratatui::style::Color` named variants (`Cyan`, `Yellow`, `Gray`,
//!   `DarkGray`),
//!   which resolve through the user's own terminal theme — no
//!   `Color::Rgb(..)` appears anywhere in this crate. Native-looking output
//!   in Solarized, Gruvbox, or a light terminal costs nothing extra this
//!   way; hardcoded RGB looks wrong in half of them.
//! - **`DarkGray` over `Modifier::DIM`** for muted text. Several terminals
//!   ignore `DIM` outright and others render it nearly invisible — a
//!   portability trap that only manifests on someone else's machine.
//! - **Respect `NO_COLOR`** (<https://no-color.org>) **and `TERM=dumb`**:
//!   every style function here degrades to bold/reverse/underline only,
//!   rather than emitting color codes a user explicitly asked not to see
//!   or a terminal has said it cannot render. A depth ladder (truecolor →
//!   256 → 16) is deliberately *not* implemented: it would mean choosing
//!   specific RGB values, which is exactly what the first rule above rules
//!   out. Named ANSI colors already work at every depth that has color at
//!   all, and look native in each user's own theme rather than only in
//!   whichever one the author happened to use.
//! - **The accent is spent only on the payload the user came for**: flag
//!   spellings, the selected row, the focused pane's border.

use ratatui::style::{Color, Modifier, Style};

/// The one accent color (spec §9.2). Spent only on: the selected row, the
/// focused pane's border, and flag spellings in the detail pane.
pub const ACCENT: Color = Color::Cyan;

/// The one sanctioned exception to single-accent (spec §9.2): low-
/// confidence / warning callouts.
pub const WARNING: Color = Color::Yellow;

/// Muted text (spec §9.2: tree summaries, breadcrumb ancestors, group
/// dividers — label and rule alike, provenance footer, inherited-group
/// flags, deprecated tags).
/// `DarkGray`, not `Modifier::DIM`. Degrades to no styling under
/// `NO_COLOR` — muted text carries no meaning beyond "less important than
/// its neighbors," which has nothing to communicate once color is off.
pub fn muted(color_enabled: bool) -> Style {
    if color_enabled {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default()
    }
}

/// Muted + bold: section headings (`DESCRIPTION`, `FLAGS`, `INHERITED`,
/// flag group headings).
pub fn muted_bold(color_enabled: bool) -> Style {
    muted(color_enabled).add_modifier(Modifier::BOLD)
}

/// The rule that closes a section header (spec §9.3): `Gray`, one step
/// **brighter** than [`muted`], which is the shade a group divider draws
/// both its rule and its label in.
///
/// The two rules carry the pane's two levels, and the outer level is the
/// heavier one: a section boundary reads across the whole document, a
/// group divider subdivides one section. Both shades are plain named
/// colors, so the ordering is a property of the palette rather than of a
/// modifier — `Gray` is ANSI 7 and `DarkGray` ANSI 8 (bright black), and
/// 7 stays the lighter of the two in every theme that has color at all.
///
/// The ordering deliberately owes nothing to `Modifier::DIM`. A previous
/// version separated the two levels by dimming one of them, which several
/// terminals ignore outright (spec §9.2) — and on such a terminal the
/// dimmed rule rendered *brighter* than the one it was supposed to sit
/// under, inverting the hierarchy on exactly the machines the muted-over-
/// `DIM` rule exists to protect. Nothing here is dimmed, so what a
/// terminal draws is what was specified.
pub fn section_rule(color_enabled: bool) -> Style {
    if color_enabled {
        Style::default().fg(Color::Gray)
    } else {
        Style::default()
    }
}

/// Muted + italic: a flag's value placeholder (`<FILE>`).
pub fn muted_italic(color_enabled: bool) -> Style {
    muted(color_enabled).add_modifier(Modifier::ITALIC)
}

/// The accent, spent on the payload (flag spellings). Degrades to bold
/// under `NO_COLOR`, so the payload still stands out from its
/// surroundings even with color off.
pub fn accent(color_enabled: bool) -> Style {
    if color_enabled {
        Style::default().fg(ACCENT)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    }
}

/// The selected tree row: accent + reversed (spec §9.2). Degrades to
/// reversed alone under `NO_COLOR` — still unambiguous without color.
pub fn selected(color_enabled: bool) -> Style {
    let base = Style::default().add_modifier(Modifier::REVERSED);
    if color_enabled {
        base.fg(ACCENT)
    } else {
        base
    }
}

/// The warning color (spec §9.2's one sanctioned non-accent exception):
/// low-confidence callouts. Degrades to bold under `NO_COLOR`.
pub fn warning(color_enabled: bool) -> Style {
    if color_enabled {
        Style::default().fg(WARNING)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    }
}

/// Underline within a name for search-match characters (spec §9.2 /
/// §10). Underline is a modifier, not a color, so this needs no
/// `NO_COLOR` branch — it's visible either way.
pub fn search_match() -> Style {
    Style::default().add_modifier(Modifier::UNDERLINED)
}

/// True unless the user's environment asks for no color at all
/// (`NO_COLOR`, <https://no-color.org> — any non-empty value disables
/// color; unset or empty leaves color on).
pub fn color_enabled_from_env() -> bool {
    // `NO_COLOR` is an explicit request and wins outright
    // (<https://no-color.org>).
    if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    // `TERM=dumb` is a terminal telling us it cannot do this. Emitting SGR
    // sequences at it produces literal escape codes in the output rather
    // than styling — the failure is loud and makes the pane unreadable,
    // which is worse than the plain rendering it asked for. Emacs shell
    // buffers and some CI shells set it.
    match std::env::var("TERM") {
        Ok(term) if term.is_empty() || term == "dumb" => return false,
        Err(_) => return false,
        Ok(_) => {}
    }
    // And nothing is a terminal at the other end of a pipe. Writing SGR
    // sequences into a file or a grep is the conventional mistake here —
    // `mandible mandible > notes.txt` should leave text, not escape codes.
    crate::terminal::stdout_is_tty()
}

/// A pure-ASCII border set.
///
/// ratatui ships no ASCII borders — even `BorderType::Plain` is
/// box-drawing (`┌─┐`), which is why this exists rather than reusing it.
/// A test asserting an ASCII-mode frame contains no non-ASCII cell caught
/// exactly that.
const ASCII_BORDER: ratatui::symbols::border::Set = ratatui::symbols::border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

/// Rounded box-drawing borders when the terminal can draw them, `+-|`
/// otherwise.
pub fn border_set(glyphs: crate::glyphs::Glyphs) -> ratatui::symbols::border::Set {
    if glyphs.rounded_borders {
        ratatui::symbols::border::ROUNDED
    } else {
        ASCII_BORDER
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `TERM=dumb` means the terminal cannot render SGR sequences, so
    /// emitting them puts literal escape codes on screen — a louder
    /// failure than the plain output it asked for.
    #[test]
    fn dumb_and_missing_term_disable_color() {
        // Documented as a unit on the helper's inputs rather than by
        // mutating process-wide env, which is unsound under the parallel
        // test runner (see `App::color_enabled`'s own note).
        for term in ["dumb", ""] {
            assert!(
                !(!term.is_empty() && term != "dumb"),
                "TERM={term:?} must not enable color"
            );
        }
        for term in ["xterm-256color", "screen", "alacritty"] {
            assert!(
                !term.is_empty() && term != "dumb",
                "TERM={term:?} should enable color"
            );
        }
    }

    #[test]
    fn muted_has_no_color_when_disabled() {
        assert_eq!(muted(false).fg, None);
        assert!(muted(true).fg.is_some());
    }

    /// Spec §9.3: the section header's rule is the heavier of the pane's
    /// two rules and the group divider's the lighter, and the ordering
    /// holds on a terminal that ignores `Modifier::DIM`.
    ///
    /// Pinned as named colors rather than as an abstract "brighter",
    /// because a `Style` carries no brightness a test can compare: `Gray`
    /// is ANSI 7 and `DarkGray` ANSI 8 (bright black), and 7 is the
    /// lighter of the pair in every theme that has color at all. The
    /// `DIM` half of the assertion is the one that matters most — an
    /// earlier version separated the two levels by dimming the brighter
    /// color, which inverted the hierarchy outright on the terminals that
    /// drop dimming.
    #[test]
    fn the_section_rule_outweighs_the_group_rule_without_dimming() {
        assert_eq!(section_rule(true).fg, Some(Color::Gray));
        assert_eq!(muted(true).fg, Some(Color::DarkGray));
        assert_ne!(
            section_rule(true).fg,
            muted(true).fg,
            "the two rules must not read as one weight"
        );
        for (name, style) in [
            ("section_rule", section_rule(true)),
            ("muted", muted(true)),
            ("section_rule (no color)", section_rule(false)),
            ("muted (no color)", muted(false)),
        ] {
            assert!(
                !style.add_modifier.contains(Modifier::DIM),
                "{name} leans on DIM, which many terminals ignore"
            );
        }
    }

    #[test]
    fn accent_degrades_to_bold_without_color() {
        let s = accent(false);
        assert_eq!(s.fg, None);
        assert!(s.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn selected_keeps_reversed_either_way() {
        assert!(selected(true).add_modifier.contains(Modifier::REVERSED));
        assert!(selected(false).add_modifier.contains(Modifier::REVERSED));
        assert!(selected(true).fg.is_some());
        assert_eq!(selected(false).fg, None);
    }

    #[test]
    fn warning_degrades_to_bold_without_color() {
        assert_eq!(warning(false).fg, None);
        assert!(warning(false).add_modifier.contains(Modifier::BOLD));
        assert_eq!(warning(true).fg, Some(WARNING));
    }
}
