//! The `?` pop-up's tabs and the settings tab's keys (docs/design.md §9.5).

use super::*;
use crate::settings::{self, HelpTab, Row};
use mandible_core::config::{self, ColorIndex, CommandSort, FlagSort, Role, Theme};

/// Longest text a colour row takes; `bright-magenta` is 14.
const TYPED_MAX: usize = 14;

/// The value `dir` steps from `current` in `all`, wrapping at both ends.
fn step<T: Copy + PartialEq>(all: &[T], current: T, dir: isize) -> T {
    let len = isize::try_from(all.len()).unwrap_or(1).max(1);
    let at = all
        .iter()
        .position(|v| *v == current)
        .and_then(|i| isize::try_from(i).ok())
        .unwrap_or(0);
    let next = usize::try_from((at + dir).rem_euclid(len)).unwrap_or(0);
    all[next]
}

impl App {
    /// Apply the settings `config.toml` gave, at startup.
    pub fn apply_config(&mut self, cfg: &config::Config) {
        self.set_wrap_everywhere(cfg.ui.horizontal_scroll);
        self.settings.description_preview = cfg.ui.description_preview;
        self.set_ascii(cfg.ui.ascii);
        self.settings.custom = cfg.colors.custom;
        self.set_theme(cfg.colors.theme);
        self.set_detail_sort(settings::detail_sort(cfg.sort.flags));
        self.set_tree_sort(cfg.sort.commands == CommandSort::Az);
        if cfg.ui.raw_view && !self.raw_mode {
            self.toggle_raw_mode();
        }
    }

    /// Both views scroll sideways (`true`) or fold to the pane (`false`).
    fn set_wrap_everywhere(&mut self, sideways: bool) {
        self.adopt_reflow_scroll();
        self.horizontal_scroll_enabled = sideways;
        self.parsed_wrap = !sideways;
        self.detail_hscroll = 0;
    }

    /// Force the ASCII glyph set, or go back to what the environment gave.
    fn set_ascii(&mut self, on: bool) {
        self.settings.ascii = on;
        self.glyphs = if on {
            crate::glyphs::ASCII
        } else {
            self.settings.glyphs_base
        };
    }

    /// Switch to `theme`: the palette it yields replaces the current one.
    pub(crate) fn set_theme(&mut self, theme: Theme) {
        self.settings.theme = theme;
        self.apply_theme();
    }

    /// Rebuild the palette from the depth the terminal has and the values
    /// the theme in force gives.
    pub(crate) fn apply_theme(&mut self) {
        let palette = self
            .settings
            .palette_base
            .themed(self.settings.theme, self.settings.custom);
        self.color_enabled = palette.color;
        self.palette = palette;
    }

    /// `Tab` in the pop-up: the other tab.
    pub fn help_switch_tab(&mut self) {
        self.settings.tab = match self.settings.tab {
            HelpTab::Keys => HelpTab::Settings,
            HelpTab::Settings => HelpTab::Keys,
        };
    }

    /// `↑`/`↓` on the settings tab: highlight the enabled row `delta` away,
    /// stopping at the first and last.
    pub fn settings_move(&mut self, delta: isize) {
        let step = delta.signum();
        for _ in 0..delta.unsigned_abs() {
            let mut at = self.settings.row;
            let found = loop {
                match at.checked_add_signed(step).filter(|n| *n < Row::ALL.len()) {
                    Some(n) if self.settings.enabled(Row::ALL[n]) => break Some(n),
                    Some(n) => at = n,
                    None => break None,
                }
            };
            match found {
                Some(n) => self.settings.row = n,
                None => break,
            }
        }
    }

    /// `←`/`→` on the settings tab: step the highlighted row's value, apply
    /// it at once and save it. A row an environment variable decides does
    /// nothing.
    pub fn settings_change(&mut self, dir: isize) -> Option<Effect> {
        let row = self.settings.selected();
        if !self.settings.enabled(row) {
            return None;
        }
        let mut effect = None;
        match row {
            Row::RawView => effect = self.toggle_raw_mode(),
            Row::Scroll => self.set_wrap_everywhere(!self.sideways()),
            Row::Preview => self.settings.description_preview = !self.settings.description_preview,
            Row::Ascii => self.set_ascii(!self.settings.ascii),
            Row::Flags => {
                let now = settings::flag_sort(self.sorts.detail);
                self.set_detail_sort(settings::detail_sort(step(FlagSort::ALL, now, dir)));
            }
            Row::Commands => {
                let now = if self.sorts.tree {
                    CommandSort::Az
                } else {
                    CommandSort::Tool
                };
                let next = step(CommandSort::ALL, now, dir);
                self.set_tree_sort(next == CommandSort::Az);
            }
            Row::Theme => self.set_theme(step(Theme::ALL, self.settings.theme, dir)),
            Row::Color(role) => self.set_color(role, self.settings.custom.get(role).step(dir)),
        }
        self.save_row(row);
        effect
    }

    /// Give the custom theme's `role` the value `color`; it shows at once.
    fn set_color(&mut self, role: Role, color: ColorIndex) {
        self.settings.custom.set(role, color);
        self.apply_theme();
    }

    /// `Enter` on an enabled colour row: start typing its value.
    pub fn settings_begin_typing(&mut self) {
        let row = self.settings.selected();
        if row.role().is_some() && self.settings.enabled(row) {
            self.settings.typing = Some(String::new());
        }
    }

    /// A key typed into a colour row: `Enter` sets the value when the text
    /// names a colour and otherwise changes nothing, `Esc` gives it up.
    pub fn settings_typing_key(&mut self, code: crossterm::event::KeyCode) {
        use crossterm::event::KeyCode;
        let Some(text) = self.settings.typing.as_mut() else {
            return;
        };
        match code {
            KeyCode::Esc => self.settings.typing = None,
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Char(c) if text.chars().count() < TYPED_MAX => text.push(c),
            KeyCode::Enter => {
                let row = self.settings.selected();
                if let (Some(role), Some(color)) = (row.role(), self.settings.typed_color()) {
                    self.settings.typing = None;
                    self.set_color(role, color);
                    self.save_row(row);
                }
            }
            _ => {}
        }
    }

    /// Write `row`'s state to `config.toml`; a failure becomes the footer
    /// line and the setting stays applied for the session.
    fn save_row(&mut self, row: Row) {
        let Some(path) = self.settings.config_path.clone() else {
            return;
        };
        if let Err(error) = config::save(&path, row.key(), settings::current_value(self, row)) {
            self.notice = Some(format!("could not save config.toml: {error}"));
        }
    }
}
