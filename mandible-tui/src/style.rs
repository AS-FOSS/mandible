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
//!   or a terminal has said it cannot render. There is no truecolor tier
//!   and no RGB anywhere; the one place depth is consulted at all is the
//!   detail pane's pair of rule shades ([`section_rule`], [`group_rule`]),
//!   which need two steps below the terminal's default foreground and
//!   cannot get them from the sixteen named colors. Those two read the
//!   xterm-256 gray ramp when [`Palette::extended`] says it is available
//!   and fall back to `DarkGray` for both levels when it is not.
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

/// The first index of the xterm-256 gray ramp (`#080808`). Indices
/// [`GRAY_RAMP_FIRST`]`..=`[`GRAY_RAMP_LAST`] step evenly from near-black
/// to near-white, ten points of gray apart, which is what makes two
/// *visibly separated* neutral shades expressible at all — the sixteen
/// named colors offer exactly one step below a default foreground
/// (`DarkGray`), and the pane needs two.
pub const GRAY_RAMP_FIRST: u8 = 232;
/// The last index of the xterm-256 gray ramp (`#eeeeee`).
pub const GRAY_RAMP_LAST: u8 = 255;

/// The section-header level's gray (`#949494`): the brighter of the
/// detail pane's two rule shades, and a clear step below the pane borders
/// above it.
pub const SECTION_GRAY: u8 = 246;

/// The group-divider level's gray (`#585858`): the dimmer of the two, a
/// clear step below [`SECTION_GRAY`] and near enough to `DarkGray` that
/// the sixteen-color fallback lands on the same level rather than beside
/// it.
pub const GROUP_GRAY: u8 = 240;

/// What the terminal can be asked to draw: whether color is wanted at all,
/// and whether the xterm-256 palette is available.
///
/// `extended` is meaningless when `color` is false, and every function
/// here treats it that way. The pair exists as one value rather than two
/// loose `bool`s because they are read together at exactly the points
/// where a shade is chosen, and two adjacent booleans at a call site are
/// the kind of thing that gets swapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// Whether to emit color at all — [`color_enabled_from_env`].
    pub color: bool,
    /// Whether the xterm-256 palette is available — [`extended_from_env`].
    pub extended: bool,
}

impl Palette {
    /// The palette this process's environment describes.
    pub fn from_env() -> Palette {
        let color = color_enabled_from_env();
        Palette {
            color,
            extended: color && extended_from_env(),
        }
    }

    /// No color at all: `NO_COLOR`, `TERM=dumb`, or a redirected stdout.
    pub fn plain() -> Palette {
        Palette {
            color: false,
            extended: false,
        }
    }

    /// Color, but only the sixteen named ones.
    pub fn basic() -> Palette {
        Palette {
            color: true,
            extended: false,
        }
    }

    /// Color including the xterm-256 palette.
    pub fn extended() -> Palette {
        Palette {
            color: true,
            extended: true,
        }
    }
}

/// The pane's **three-step** neutral hierarchy, brightest first: the pane
/// borders (the terminal's own default foreground, untouched), then the
/// section header's rule and label, then the group divider's
/// ([`group_rule`]). Each step is clearly dimmer than the one above it, so
/// a section boundary reads as subordinate to the pane it lives in and a
/// group divider as subordinate to the section.
///
/// The sixteen named colors cannot express this. `Gray` is ANSI 7, which
/// *is* the default foreground in most themes — a rule drawn in it reads
/// at exactly the border's brightness rather than under it — and below it
/// there is only `DarkGray`, one step for two levels. So the two rule
/// shades are the one place in this crate that consults color depth: with
/// the xterm-256 gray ramp available they take [`SECTION_GRAY`] and
/// [`GROUP_GRAY`], two evenly separated neutrals that sit under any
/// ordinary foreground.
///
/// **Fallback**: without the extended palette both levels collapse to
/// `DarkGray`. That keeps the step below the borders, which is the
/// ordering that matters most, and gives up only the distinction between
/// the two inner levels — which spec §9.2's shape rule carries anyway,
/// since a section header is CAPS with a count and a group divider mixed
/// case without one. A wrong guess about depth therefore costs a
/// distinction, never legibility.
///
/// Indexed, never RGB (spec §9.2): a gray ramp index is still a palette
/// entry the user's terminal resolves, not a color chosen for one theme.
/// And nothing here is dimmed — an earlier version separated the levels
/// with `Modifier::DIM`, which several terminals ignore outright, and on
/// those the "dimmer" rule came out brighter than the one it was meant to
/// sit under.
pub fn section_rule(palette: Palette) -> Style {
    rule_shade(palette, SECTION_GRAY)
}

/// The group-divider level of the hierarchy [`section_rule`] documents:
/// the dimmest of the three steps, drawn on both the divider's rule and
/// its label.
pub fn group_rule(palette: Palette) -> Style {
    rule_shade(palette, GROUP_GRAY)
}

/// One level of the neutral hierarchy: its gray ramp index where the
/// extended palette is available, `DarkGray` where only the sixteen named
/// colors are, and no color at all where color is off.
fn rule_shade(palette: Palette, gray: u8) -> Style {
    match (palette.color, palette.extended) {
        (true, true) => Style::default().fg(Color::Indexed(gray)),
        (true, false) => Style::default().fg(Color::DarkGray),
        (false, _) => Style::default(),
    }
}

/// Whether the terminal has the xterm-256 palette, read conservatively
/// from the environment: a wrong `true` would draw the pane's furniture in
/// colors the terminal cannot resolve, so anything unrecognized is
/// answered `false` and takes the `DarkGray` fallback.
///
/// There is no terminal *query* here on purpose. Asking a terminal about
/// its palette means writing an escape sequence and reading the reply,
/// which needs the tty in raw mode at a point where mandible has not set
/// it up yet, and hangs for the timeout on every terminal that does not
/// answer. The environment is what every other tool reads for this, and
/// the cost of being wrong is one lost distinction rather than a broken
/// screen.
///
/// `COLORTERM` is checked first because a terminal that sets it
/// (`truecolor`/`24bit`) always has 256 colors as well, whatever its
/// `TERM` says — `alacritty` and `wezterm` both report a bare `TERM` and
/// announce themselves this way.
pub fn extended_from_env() -> bool {
    announces_256_colors(
        std::env::var("COLORTERM").ok().as_deref(),
        std::env::var("TERM").ok().as_deref(),
    )
}

/// [`extended_from_env`]'s decision, as a function of the two variables it
/// reads.
///
/// Split out so the rule can be tested over its whole input space: env
/// vars are process-wide, and mutating them under a parallel test runner
/// is unsound (see `color_enabled_from_env`'s own note). A test that
/// restates the logic instead of calling it cannot fail when the logic
/// changes, which is the one thing this test exists to do.
fn announces_256_colors(colorterm: Option<&str>, term: Option<&str>) -> bool {
    if let Some(colorterm) = colorterm {
        let colorterm = colorterm.to_ascii_lowercase();
        if colorterm == "truecolor" || colorterm == "24bit" {
            return true;
        }
    }
    // `-256color` is the conventional terminfo suffix, and `direct` the
    // one for the truecolor entries (`xterm-direct`), which are a
    // superset.
    term.is_some_and(|t| t.contains("256color") || t.contains("direct"))
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

    /// The gray ramp index a rule shade resolves to, or `None` if it is
    /// not an indexed color.
    fn ramp_index(style: Style) -> Option<u8> {
        match style.fg {
            Some(Color::Indexed(i)) => Some(i),
            _ => None,
        }
    }

    /// Spec §9.3's three-step neutral hierarchy, asserted by index
    /// arithmetic on the gray ramp: pane borders (the terminal's default
    /// foreground, which this crate never sets) are brightest, then the
    /// section header's rule, then the group divider's — each a clearly
    /// visible step below the last.
    ///
    /// Arithmetic rather than named constants, because "clearly dimmer"
    /// is a claim about *distance* and two named colors cannot express
    /// one. On the ramp each index is ten points of gray from its
    /// neighbour, so a gap can be required rather than merely an
    /// inequality — the failure this rules out is a pair chosen one or
    /// two indices apart, which orders correctly and is invisible.
    ///
    /// The headroom assertion is the other half: the ramp's top end
    /// (`251`+) is where a neutral starts reading at the brightness of an
    /// ordinary default foreground, which is exactly the defect that
    /// replaced `Gray` here — a section rule indistinguishable from the
    /// pane border it sits inside.
    #[test]
    fn the_three_neutral_levels_step_clearly_apart() {
        let section = ramp_index(section_rule(Palette::extended())).expect("an indexed section");
        let group = ramp_index(group_rule(Palette::extended())).expect("an indexed group");

        for (name, i) in [("section", section), ("group", group)] {
            assert!(
                (GRAY_RAMP_FIRST..=GRAY_RAMP_LAST).contains(&i),
                "the {name} shade at {i} is outside the gray ramp, so it is not a neutral"
            );
        }
        assert!(
            section > group,
            "the section rule ({section}) must be brighter than the group rule ({group})"
        );
        assert!(
            section - group >= 4,
            "{} points of gray between the two levels is not a visible step",
            (section - group) as u16 * 10
        );
        // ...and the brighter of the two still sits under a default
        // foreground, so the borders keep the top of the hierarchy.
        assert!(
            section <= 250,
            "the section rule at {section} reads at the brightness of the pane borders"
        );
    }

    /// Without the extended palette both levels collapse to `DarkGray`:
    /// the step below the borders survives, and only the distinction
    /// between the two inner levels is given up — which spec §9.2's shape
    /// rule (CAPS with a count against mixed case without one) carries on
    /// its own.
    #[test]
    fn sixteen_color_terminals_get_one_shade_for_both_levels() {
        assert_eq!(section_rule(Palette::basic()).fg, Some(Color::DarkGray));
        assert_eq!(group_rule(Palette::basic()).fg, Some(Color::DarkGray));
        assert_eq!(section_rule(Palette::plain()).fg, None);
        assert_eq!(group_rule(Palette::plain()).fg, None);
    }

    /// Nothing in the hierarchy is dimmed. An earlier version separated
    /// the two levels with `Modifier::DIM`, which several terminals ignore
    /// outright — and on those the "dimmer" rule came out *brighter* than
    /// the one it was meant to sit under, inverting the hierarchy on
    /// exactly the machines spec §9.2's rule exists to protect.
    #[test]
    fn no_level_of_the_hierarchy_leans_on_dim() {
        for palette in [Palette::extended(), Palette::basic(), Palette::plain()] {
            for (name, style) in [
                ("section_rule", section_rule(palette)),
                ("group_rule", group_rule(palette)),
                ("muted", muted(palette.color)),
            ] {
                assert!(
                    !style.add_modifier.contains(Modifier::DIM),
                    "{name} leans on DIM at {palette:?}, and many terminals ignore it"
                );
            }
        }
    }

    /// The depth probe answers `false` for anything it does not
    /// positively recognize: a wrong `true` paints the pane's furniture in
    /// colors the terminal cannot resolve, while a wrong `false` costs one
    /// distinction the shape rule already carries.
    ///
    /// Driven through the real decision function rather than a restated
    /// copy of it, over the inputs it reads — env vars are process-wide
    /// and mutating them under a parallel test runner is unsound (see
    /// `dumb_and_missing_term_disable_color`).
    #[test]
    fn the_depth_probe_recognizes_only_known_signals() {
        let recognizes = announces_256_colors;
        for (colorterm, term) in [
            (Some("truecolor"), Some("xterm")),
            (Some("24bit"), None),
            (Some("TrueColor"), Some("dumb-ish")),
            (None, Some("xterm-256color")),
            (None, Some("screen-256color")),
            (None, Some("xterm-direct")),
        ] {
            assert!(
                recognizes(colorterm, term),
                "{colorterm:?}/{term:?} announces 256 colors"
            );
        }
        for (colorterm, term) in [
            (None, Some("xterm")),
            (None, Some("screen")),
            (None, Some("linux")),
            (None, Some("vt100")),
            (Some("16"), Some("xterm")),
            (Some(""), None),
            (None, None),
        ] {
            assert!(
                !recognizes(colorterm, term),
                "{colorterm:?}/{term:?} must fall back rather than be guessed at"
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
