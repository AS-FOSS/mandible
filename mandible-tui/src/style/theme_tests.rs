//! What each theme paints: the seven roles, the fallbacks and the
//! modifiers a theme must leave alone (docs/design.md §9.2).

use super::*;
use mandible_core::config::{ColorIndex, Role, Roles, Theme};

fn named(i: u8) -> Color {
    Palette::extended().pick(ColorIndex(i), Color::Reset)
}

/// The colors the seven styles draw, in role order.
fn drawn(p: Palette) -> [Option<Color>; 7] {
    [
        accent(p).fg,
        warning(p).fg,
        muted(p).fg,
        section_rule(p).fg,
        group_rule(p).fg,
        bar_text(p).bg,
        bar_text(p).fg,
    ]
}

#[test]
fn the_default_theme_draws_what_the_styles_always_drew() {
    let p = Palette::extended().themed(Theme::Default, Roles::DEFAULT);
    assert_eq!(p, Palette::extended());
    assert_eq!(
        drawn(p),
        [
            Some(Color::Cyan),
            Some(Color::Yellow),
            Some(Color::DarkGray),
            Some(Color::Indexed(246)),
            Some(Color::Indexed(240)),
            Some(Color::Indexed(237)),
            Some(Color::Indexed(252)),
        ]
    );
    assert_eq!(bar_name(p).fg, Some(Color::Cyan));
    assert_eq!(bar_name(p).bg, Some(Color::Indexed(237)));
}

#[test]
fn every_theme_draws_its_seven_values() {
    let table: [(Theme, [u8; 7]); 7] = [
        (Theme::Default, [6, 3, 8, 246, 240, 237, 252]),
        (Theme::Light, [25, 130, 237, 234, 240, 252, 232]),
        (Theme::HighContrast, [14, 11, 248, 253, 248, 238, 231]),
        (Theme::GruvboxStyle, [108, 214, 245, 223, 243, 237, 223]),
        (Theme::NordStyle, [110, 222, 102, 109, 60, 238, 255]),
        (Theme::DraculaStyle, [141, 228, 103, 117, 60, 238, 231]),
        (Theme::CatppuccinStyle, [183, 223, 103, 146, 60, 236, 189]),
    ];
    for (theme, values) in table {
        let p = Palette::extended().themed(theme, Roles::DEFAULT);
        let want: Vec<_> = values.iter().map(|v| Some(named(*v))).collect();
        assert_eq!(drawn(p).to_vec(), want, "{}", theme.word());
    }
}

#[test]
fn the_names_zero_to_fifteen_are_the_named_colors_and_the_rest_are_indexed() {
    let want = [
        Color::Black,
        Color::Red,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::Cyan,
        Color::Gray,
        Color::DarkGray,
        Color::LightRed,
        Color::LightGreen,
        Color::LightYellow,
        Color::LightBlue,
        Color::LightMagenta,
        Color::LightCyan,
        Color::White,
    ];
    for (i, color) in want.iter().enumerate() {
        assert_eq!(named(u8::try_from(i).unwrap()), *color, "{i}");
    }
    assert_eq!(named(16), Color::Indexed(16));
    assert_eq!(named(255), Color::Indexed(255));
}

#[test]
fn a_custom_theme_draws_the_roles_it_was_given() {
    let mut custom = Roles::DEFAULT;
    custom.set(Role::Accent, ColorIndex(14));
    custom.set(Role::Selection, ColorIndex(17));
    let p = Palette::extended().themed(Theme::Custom, custom);
    assert_eq!(accent(p).fg, Some(Color::LightCyan));
    assert_eq!(bar_text(p).bg, Some(Color::Indexed(17)));
    assert_eq!(bar_name(p).fg, Some(Color::LightCyan));
    assert_eq!(warning(p).fg, Some(Color::Yellow));
}

#[test]
fn without_the_256_palette_a_number_falls_back_the_way_the_greys_always_did() {
    for theme in [Theme::Default, Theme::NordStyle, Theme::Light] {
        let p = Palette::basic().themed(theme, Roles::DEFAULT);
        assert_eq!(accent(p).fg, Some(Color::Cyan), "{}", theme.word());
        assert_eq!(muted(p).fg, Some(Color::DarkGray));
        assert_eq!(section_rule(p).fg, Some(Color::DarkGray));
        assert_eq!(group_rule(p).fg, Some(Color::DarkGray));
        for s in [bar_name(p), bar_text(p)] {
            assert!(s.add_modifier.contains(Modifier::REVERSED));
            assert_eq!(s.bg, None);
        }
    }
    let high = Palette::basic().themed(Theme::HighContrast, Roles::DEFAULT);
    assert_eq!(
        accent(high).fg,
        Some(Color::LightCyan),
        "a name needs no 256"
    );
    assert_eq!(warning(high).fg, Some(Color::LightYellow));
    assert_eq!(
        warning(Palette::basic().themed(Theme::Light, Roles::DEFAULT)).fg,
        Some(Color::Yellow)
    );
}

#[test]
fn named_selection_colors_draw_the_bar_at_sixteen_colors_and_one_number_does_not() {
    let mut custom = Roles::DEFAULT;
    custom.set(Role::Selection, ColorIndex(4));
    custom.set(Role::SelectionText, ColorIndex(15));
    let p = Palette::basic().themed(Theme::Custom, custom);
    assert_eq!(bar_text(p).bg, Some(Color::Blue));
    assert_eq!(bar_text(p).fg, Some(Color::White));
    custom.set(Role::SelectionText, ColorIndex(200));
    let p = Palette::basic().themed(Theme::Custom, custom);
    assert!(bar_text(p).add_modifier.contains(Modifier::REVERSED));
}

#[test]
fn none_and_no_color_draw_no_color_at_all() {
    for p in [
        Palette::extended().themed(Theme::None, Roles::DEFAULT),
        Palette::plain().themed(Theme::Light, Roles::LIGHT),
    ] {
        assert!(!p.color);
        assert_eq!(drawn(p), [None, None, None, None, None, None, None]);
        assert!(bar_name(p).add_modifier.contains(Modifier::REVERSED));
        assert_eq!(p.sgr(Role::Accent), None);
    }
}

#[test]
fn a_theme_changes_colors_and_never_a_modifier() {
    for theme in Theme::ALL {
        for base in [Palette::extended(), Palette::basic()] {
            let p = base.themed(*theme, Roles::DEFAULT);
            let d = base;
            assert_eq!(
                p.color,
                base.color && *theme != Theme::None,
                "{}",
                theme.word()
            );
            if !p.color {
                continue;
            }
            let same = |a: Style, b: Style| {
                a.add_modifier == b.add_modifier && a.sub_modifier == b.sub_modifier
            };
            assert!(same(accent(p), accent(d)));
            assert!(same(warning(p), warning(d)));
            assert!(same(muted_bold(p), muted_bold(d)));
            assert!(same(muted_italic(p), muted_italic(d)));
            assert!(same(ghost(p), ghost(d)));
            assert!(same(bar_name(p), bar_name(d)));
            assert!(same(bar_text(p), bar_text(d)));
        }
    }
}

#[test]
fn the_about_screen_codes_follow_the_theme_and_the_depth() {
    let p = Palette::extended().themed(Theme::Default, Roles::DEFAULT);
    assert_eq!(p.sgr(Role::Accent).as_deref(), Some("36"));
    assert_eq!(p.sgr(Role::Muted).as_deref(), Some("90"));
    let p = Palette::extended().themed(Theme::NordStyle, Roles::DEFAULT);
    assert_eq!(p.sgr(Role::Accent).as_deref(), Some("38;5;110"));
    assert_eq!(p.sgr(Role::Muted).as_deref(), Some("38;5;102"));
    let p = Palette::basic().themed(Theme::NordStyle, Roles::DEFAULT);
    assert_eq!(p.sgr(Role::Accent).as_deref(), Some("36"));
    let p = Palette::extended().themed(Theme::HighContrast, Roles::DEFAULT);
    assert_eq!(p.sgr(Role::Accent).as_deref(), Some("96"));
}
