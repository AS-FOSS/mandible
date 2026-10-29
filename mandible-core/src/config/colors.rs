//! Colour values, the seven colour roles and the built-in themes
//! (docs/design.md §9.2).
//!
//! A colour is a number 0 to 255. 0 to 15 are written by name in the
//! file and shown by name; 16 to 255 are written and shown as numbers.
//! There is no RGB and no hex anywhere.

/// The names of colours 0 to 15, in order.
pub const NAMES: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "bright-black",
    "bright-red",
    "bright-green",
    "bright-yellow",
    "bright-blue",
    "bright-magenta",
    "bright-cyan",
    "bright-white",
];

/// One colour of the 256-colour palette. 0 to 15 have names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorIndex(pub u8);

impl ColorIndex {
    /// The colour a name stands for, `bright-cyan` and so on.
    pub fn from_name(name: &str) -> Option<ColorIndex> {
        let at = NAMES.iter().position(|n| *n == name)?;
        u8::try_from(at).ok().map(ColorIndex)
    }

    /// The colour typed or written as `text`: a name, or a number from 0
    /// to 255 in digits. Hex, RGB and signs are refused.
    pub fn parse(text: &str) -> Option<ColorIndex> {
        let text = text.trim();
        if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
            return text.parse::<u8>().ok().map(ColorIndex);
        }
        ColorIndex::from_name(text)
    }

    /// The name of colours 0 to 15.
    pub fn name(self) -> Option<&'static str> {
        NAMES.get(usize::from(self.0)).copied()
    }

    /// How the colour is shown: its name, or its number.
    pub fn shown(self) -> String {
        self.name()
            .map_or_else(|| self.0.to_string(), str::to_string)
    }

    /// The next colour `dir` steps away in 0 to 255, wrapping at both ends.
    pub fn step(self, dir: isize) -> ColorIndex {
        let next = (isize::from(self.0) + dir).rem_euclid(256);
        ColorIndex(u8::try_from(next).unwrap_or(0))
    }
}

/// What mandible colours. Each theme gives a value to all seven.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Flag spellings, the focused border, the selected name.
    Accent,
    /// Low-confidence callouts and notices.
    Warning,
    /// Muted text.
    Muted,
    /// Section headings and the first preview fade row.
    Section,
    /// Group headings and the second preview fade row.
    Group,
    /// The selection bar's background.
    Selection,
    /// Text on the selection bar.
    SelectionText,
}

impl Role {
    /// Every role, in the order the settings tab lists them.
    pub const ALL: [Role; 7] = [
        Role::Accent,
        Role::Warning,
        Role::Muted,
        Role::Section,
        Role::Group,
        Role::Selection,
        Role::SelectionText,
    ];

    /// The role's key in `[colors]`.
    pub fn key(self) -> &'static str {
        match self {
            Role::Accent => "accent",
            Role::Warning => "warning",
            Role::Muted => "muted",
            Role::Section => "section",
            Role::Group => "group",
            Role::Selection => "selection",
            Role::SelectionText => "selection_text",
        }
    }
}

/// A value for each of the seven roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Roles {
    /// [`Role::Accent`].
    pub accent: ColorIndex,
    /// [`Role::Warning`].
    pub warning: ColorIndex,
    /// [`Role::Muted`].
    pub muted: ColorIndex,
    /// [`Role::Section`].
    pub section: ColorIndex,
    /// [`Role::Group`].
    pub group: ColorIndex,
    /// [`Role::Selection`].
    pub selection: ColorIndex,
    /// [`Role::SelectionText`].
    pub selection_text: ColorIndex,
}

impl Roles {
    /// The `default` theme.
    pub const DEFAULT: Roles = Roles::of([6, 3, 8, 246, 240, 237, 252]);

    const fn of(v: [u8; 7]) -> Roles {
        Roles {
            accent: ColorIndex(v[0]),
            warning: ColorIndex(v[1]),
            muted: ColorIndex(v[2]),
            section: ColorIndex(v[3]),
            group: ColorIndex(v[4]),
            selection: ColorIndex(v[5]),
            selection_text: ColorIndex(v[6]),
        }
    }

    /// The `light` theme.
    pub const LIGHT: Roles = Roles::of([25, 130, 237, 234, 240, 252, 232]);
    /// The `high-contrast` theme.
    pub const HIGH_CONTRAST: Roles = Roles::of([14, 11, 248, 253, 248, 238, 231]);
    /// The `gruvbox-style` theme.
    pub const GRUVBOX: Roles = Roles::of([108, 214, 245, 223, 243, 237, 223]);
    /// The `nord-style` theme.
    pub const NORD: Roles = Roles::of([110, 222, 102, 109, 60, 238, 255]);
    /// The `dracula-style` theme.
    pub const DRACULA: Roles = Roles::of([141, 228, 103, 117, 60, 238, 231]);
    /// The `catppuccin-style` theme.
    pub const CATPPUCCIN: Roles = Roles::of([183, 223, 103, 146, 60, 236, 189]);

    /// The value of `role`.
    pub fn get(&self, role: Role) -> ColorIndex {
        match role {
            Role::Accent => self.accent,
            Role::Warning => self.warning,
            Role::Muted => self.muted,
            Role::Section => self.section,
            Role::Group => self.group,
            Role::Selection => self.selection,
            Role::SelectionText => self.selection_text,
        }
    }

    /// Give `role` the value `color`.
    pub fn set(&mut self, role: Role, color: ColorIndex) {
        match role {
            Role::Accent => self.accent = color,
            Role::Warning => self.warning = color,
            Role::Muted => self.muted = color,
            Role::Section => self.section = color,
            Role::Group => self.group = color,
            Role::Selection => self.selection = color,
            Role::SelectionText => self.selection_text = color,
        }
    }
}

impl Default for Roles {
    fn default() -> Self {
        Roles::DEFAULT
    }
}
