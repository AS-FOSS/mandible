use super::*;
use std::io::Write;

/// `std::env::set_var` is process-global and tests run in parallel, so the
/// tests that set `MANDIBLE_CONFIG_DIR` take this lock.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn with_config_dir<R>(dir: &std::path::Path, f: impl FnOnce() -> R) -> R {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(CONFIG_DIR_ENV, dir);
    let result = f();
    std::env::remove_var(CONFIG_DIR_ENV);
    result
}

fn texts(loaded: &Loaded) -> Vec<String> {
    loaded.problems.iter().map(ToString::to_string).collect()
}

#[test]
fn the_defaults_are_the_documented_ones() {
    let c = Config::default();
    assert!(!c.ui.raw_view);
    assert!(!c.ui.horizontal_scroll);
    assert!(c.ui.description_preview);
    assert!(!c.ui.ascii);
    assert_eq!(c.sort.flags, FlagSort::Tool);
    assert_eq!(c.sort.commands, CommandSort::Tool);
    assert_eq!(c.colors.theme, Theme::Default);
}

#[test]
fn an_empty_file_and_empty_tables_give_the_defaults() {
    for raw in ["", "[ui]\n", "[ui]\n[sort]\n[colors]\n", "# nothing\n"] {
        let loaded = parse(raw);
        assert_eq!(loaded.config, Config::default(), "{raw:?}");
        assert!(loaded.problems.is_empty(), "{raw:?}");
        assert_eq!(loaded.notice(), None);
    }
}

#[test]
fn every_key_is_read() {
    let loaded = parse(
        "[ui]\nraw_view = true\nhorizontal_scroll = true\ndescription_preview = false\nascii = true\n\
         [sort]\nflags = \"az-sections\"\ncommands = \"az\"\n[colors]\ntheme = \"none\"\n",
    );
    assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
    let c = loaded.config;
    assert!(c.ui.raw_view && c.ui.horizontal_scroll && c.ui.ascii);
    assert!(!c.ui.description_preview);
    assert_eq!(c.sort.flags, FlagSort::AzSections);
    assert_eq!(c.sort.commands, CommandSort::Az);
    assert_eq!(c.colors.theme, Theme::None);
}

#[test]
fn every_word_of_every_word_setting_is_read() {
    for flags in FlagSort::ALL {
        let loaded = parse(&format!("[sort]\nflags = \"{}\"\n", flags.word()));
        assert_eq!(loaded.config.sort.flags, *flags);
    }
    for commands in CommandSort::ALL {
        let loaded = parse(&format!("[sort]\ncommands = \"{}\"\n", commands.word()));
        assert_eq!(loaded.config.sort.commands, *commands);
    }
    for theme in Theme::ALL {
        let loaded = parse(&format!("[colors]\ntheme = \"{}\"\n", theme.word()));
        assert_eq!(loaded.config.colors.theme, *theme);
    }
}

#[test]
fn an_existing_horizontal_scroll_true_still_works() {
    let loaded = parse("[ui]\nhorizontal_scroll = true\n");
    assert!(loaded.config.ui.horizontal_scroll);
    assert!(loaded.problems.is_empty());
}

#[test]
fn an_unknown_key_costs_only_that_key() {
    let loaded = parse("[ui]\nraw_view = true\n\n[sort]\nsrot = 1\nflags = \"az\"\n");
    assert!(loaded.config.ui.raw_view);
    assert_eq!(loaded.config.sort.flags, FlagSort::Az);
    assert_eq!(texts(&loaded), ["line 5: unknown key \"srot\", ignored"]);
    assert_eq!(
        loaded.notice().as_deref(),
        Some("config.toml line 5: unknown key \"srot\", ignored")
    );
}

#[test]
fn a_bad_value_costs_only_that_key_and_keeps_its_default() {
    let loaded = parse("[sort]\nflags = \"sideways\"\ncommands = \"az\"\n[ui]\nascii = \"yes\"\n");
    assert_eq!(loaded.config.sort.flags, FlagSort::Tool);
    assert_eq!(loaded.config.sort.commands, CommandSort::Az);
    assert!(!loaded.config.ui.ascii);
    assert_eq!(
        texts(&loaded),
        [
            "line 2: bad value for flags, ignored",
            "line 5: bad value for ascii, ignored"
        ]
    );
}

#[test]
fn a_key_in_the_wrong_section_is_unknown() {
    let loaded = parse("[ui]\nflags = \"az\"\n");
    assert_eq!(loaded.config.sort.flags, FlagSort::Tool);
    assert_eq!(texts(&loaded), ["line 2: unknown key \"flags\", ignored"]);
}

#[test]
fn an_unknown_section_is_named_and_the_rest_applies() {
    let loaded = parse("[future]\nx = 1\n[ui]\nraw_view = true\n");
    assert!(loaded.config.ui.raw_view);
    assert_eq!(
        texts(&loaded),
        ["line 1: unknown section \"future\", ignored"]
    );
}

#[test]
fn several_problems_are_counted_and_the_first_is_named() {
    let loaded = parse("[ui]\nx = 1\ny = 2\nraw_view = 3\n");
    assert_eq!(loaded.problems.len(), 3);
    assert_eq!(
        loaded.notice().as_deref(),
        Some("3 problems in config.toml: line 2: unknown key \"x\", ignored")
    );
}

#[test]
fn a_file_that_does_not_parse_gives_every_default() {
    let loaded = parse("[ui]\nraw_view = true\nthis is not toml\n");
    assert_eq!(loaded.config, Config::default());
    assert_eq!(texts(&loaded), ["line 3: not valid TOML, defaults used"]);
    assert_eq!(
        loaded.notice().as_deref(),
        Some("config.toml line 3: not valid TOML, defaults used")
    );
}

#[test]
fn load_reads_the_file_in_the_config_dir() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), "[ui]\nraw_view = true\n").unwrap();
    let loaded = with_config_dir(dir.path(), load);
    assert!(loaded.config.ui.raw_view);
}

#[test]
fn a_missing_file_is_the_default_without_a_problem() {
    let dir = tempfile::tempdir().unwrap();
    let loaded = with_config_dir(dir.path(), load);
    assert_eq!(loaded, Loaded::default());
}

#[test]
fn an_unreadable_file_is_a_problem_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = std::fs::File::create(dir.path().join("config.toml")).unwrap();
    f.write_all(&[0xff, 0xfe, 0x00]).unwrap();
    drop(f);
    let loaded = with_config_dir(dir.path(), load);
    assert_eq!(loaded.config, Config::default());
    assert_eq!(loaded.problems.len(), 1);
}

#[test]
fn config_dir_env_is_honored_for_the_path_too() {
    let dir = tempfile::tempdir().unwrap();
    let path = with_config_dir(dir.path(), || config_path().unwrap());
    assert_eq!(path, dir.path().join("config.toml"));
}

const FIXTURE: &str = "# my settings\n[ui]\nraw_view = false # start parsed\nfuture_key = 7\n\n# sorting\n[sort]\nflags = \"tool\"\n\n[other]\nkeep = \"me\"\n";

#[test]
fn save_changes_only_that_key_and_keeps_comments_order_and_unknown_keys() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, FIXTURE).unwrap();
    save(&path, Key::RawView, Value::Bool(true)).unwrap();
    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        after,
        FIXTURE.replace("raw_view = false", "raw_view = true")
    );
    save(&path, Key::SortFlags, Value::Word("az")).unwrap();
    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        after,
        FIXTURE
            .replace("raw_view = false", "raw_view = true")
            .replace("flags = \"tool\"", "flags = \"az\"")
    );
}

#[test]
fn save_adds_a_missing_key_and_a_missing_section() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, FIXTURE).unwrap();
    save(&path, Key::Ascii, Value::Bool(true)).unwrap();
    save(&path, Key::Theme, Value::Word("none")).unwrap();
    let after = std::fs::read_to_string(&path).unwrap();
    assert!(after.contains("ascii = true"), "{after}");
    assert!(after.contains("[colors]\ntheme = \"none\""), "{after}");
    assert!(after.starts_with("# my settings\n"), "{after}");
    assert!(after.contains("keep = \"me\""), "{after}");
    let loaded = parse(&after);
    assert!(loaded.config.ui.ascii);
    assert_eq!(loaded.config.colors.theme, Theme::None);
    assert_eq!(
        texts(&loaded),
        [
            "line 4: unknown key \"future_key\", ignored",
            "line 11: unknown section \"other\", ignored"
        ]
    );
}

#[test]
fn save_creates_the_file_and_its_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a").join("b").join("config.toml");
    save(&path, Key::DescriptionPreview, Value::Bool(false)).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "[ui]\ndescription_preview = false\n"
    );
}

#[test]
fn save_leaves_a_file_that_does_not_parse_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "not toml [[[").unwrap();
    assert!(save(&path, Key::RawView, Value::Bool(true)).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "not toml [[[");
}

#[test]
fn save_reports_a_write_failure() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "x").unwrap();
    let path = blocker.join("config.toml");
    assert!(save(&path, Key::RawView, Value::Bool(true)).is_err());
}

#[test]
fn every_key_round_trips_through_save_and_parse() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let cases = [
        (Key::RawView, Value::Bool(true)),
        (Key::HorizontalScroll, Value::Bool(true)),
        (Key::DescriptionPreview, Value::Bool(false)),
        (Key::Ascii, Value::Bool(true)),
        (Key::SortFlags, Value::Word("az-sections")),
        (Key::SortCommands, Value::Word("az")),
        (Key::Theme, Value::Word("none")),
    ];
    for (key, value) in cases {
        save(&path, key, value).unwrap();
    }
    let loaded = parse(&std::fs::read_to_string(&path).unwrap());
    assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
    let c = loaded.config;
    assert!(c.ui.raw_view && c.ui.horizontal_scroll && c.ui.ascii && !c.ui.description_preview);
    assert_eq!(c.sort.flags, FlagSort::AzSections);
    assert_eq!(c.sort.commands, CommandSort::Az);
    assert_eq!(c.colors.theme, Theme::None);
}

fn idx(values: [u8; 7]) -> Roles {
    let [accent, warning, muted, section, group, selection, selection_text] =
        values.map(ColorIndex);
    Roles {
        accent,
        warning,
        muted,
        section,
        group,
        selection,
        selection_text,
    }
}

#[test]
fn the_themes_come_in_the_documented_order_ending_in_custom_and_none() {
    let words: Vec<_> = Theme::ALL.iter().map(|t| t.word()).collect();
    assert_eq!(
        words,
        [
            "default",
            "light",
            "high-contrast",
            "gruvbox-style",
            "nord-style",
            "dracula-style",
            "catppuccin-style",
            "custom",
            "none"
        ]
    );
}

#[test]
fn each_built_in_theme_has_the_documented_seven_values() {
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
        assert_eq!(theme.roles(), Some(idx(values)), "{}", theme.word());
    }
    assert_eq!(Theme::Custom.roles(), None);
    assert_eq!(Theme::None.roles(), None);
    assert_eq!(ColorIndex(6).shown(), "cyan");
    assert_eq!(ColorIndex(8).shown(), "bright-black");
    assert_eq!(ColorIndex(14).shown(), "bright-cyan");
}

#[test]
fn colours_parse_as_names_and_numbers_and_refuse_everything_else() {
    for (text, want) in [
        ("black", 0),
        ("cyan", 6),
        ("white", 7),
        ("bright-black", 8),
        ("bright-white", 15),
        ("0", 0),
        ("15", 15),
        ("16", 16),
        ("220", 220),
        ("255", 255),
        (" 17 ", 17),
    ] {
        assert_eq!(ColorIndex::parse(text), Some(ColorIndex(want)), "{text:?}");
    }
    for bad in [
        "",
        "256",
        "999",
        "-1",
        "+5",
        "1.5",
        "#ff0000",
        "#f00",
        "ff0000",
        "rgb(1,2,3)",
        "Cyan",
        "bright",
        "grey",
        "0x10",
    ] {
        assert_eq!(ColorIndex::parse(bad), None, "{bad:?}");
    }
}

#[test]
fn the_values_step_through_zero_to_255_and_wrap() {
    assert_eq!(ColorIndex(14).step(1), ColorIndex(15));
    assert_eq!(ColorIndex(15).step(1), ColorIndex(16));
    assert_eq!(ColorIndex(16).step(-1), ColorIndex(15));
    assert_eq!(ColorIndex(255).step(1), ColorIndex(0));
    assert_eq!(ColorIndex(0).step(-1), ColorIndex(255));
    assert_eq!(ColorIndex(15).shown(), "bright-white");
    assert_eq!(ColorIndex(16).shown(), "16");
}

#[test]
fn custom_colours_read_names_and_numbers_and_a_number_below_16_is_that_name() {
    let loaded = parse(
        "[colors]\ntheme = \"custom\"\naccent = \"bright-cyan\"\nwarning = 3\nmuted = 244\n\
         section = \"white\"\ngroup = 0\nselection = 17\nselection_text = 255\n",
    );
    assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
    assert_eq!(loaded.config.colors.theme, Theme::Custom);
    assert_eq!(
        loaded.config.colors.custom,
        idx([14, 3, 244, 7, 0, 17, 255])
    );
}

#[test]
fn a_missing_custom_key_takes_the_default_themes_value() {
    let loaded = parse("[colors]\ntheme = \"custom\"\nwarning = 200\n");
    let want = Roles {
        warning: ColorIndex(200),
        ..Roles::DEFAULT
    };
    assert_eq!(loaded.config.colors.custom, want);
    assert_eq!(parse("").config.colors.custom, Roles::DEFAULT);
}

#[test]
fn custom_values_are_kept_whatever_theme_the_file_names() {
    let loaded = parse("[colors]\ntheme = \"nord-style\"\naccent = 99\n");
    assert_eq!(loaded.config.colors.theme, Theme::NordStyle);
    assert_eq!(loaded.config.colors.custom.accent, ColorIndex(99));
}

#[test]
fn a_bad_colour_costs_only_its_key_and_is_reported() {
    let raw =
        "[colors]\ntheme = \"custom\"\naccent = \"#00ffff\"\nwarning = 300\nmuted = \"grey\"\n\
               section = 1.5\ngroup = true\nselection = 17\n";
    let loaded = parse(raw);
    let want = Roles {
        selection: ColorIndex(17),
        ..Roles::DEFAULT
    };
    assert_eq!(loaded.config.colors.custom, want);
    assert_eq!(
        texts(&loaded),
        [
            "line 3: bad value for accent, ignored",
            "line 4: bad value for warning, ignored",
            "line 5: bad value for muted, ignored",
            "line 6: bad value for section, ignored",
            "line 7: bad value for group, ignored",
        ]
    );
    assert_eq!(
        parse("[colors]\nthem = 1\n").problems[0].text,
        "unknown key \"them\", ignored"
    );
    let bad_theme = parse("[colors]\ntheme = \"solarized\"\n");
    assert_eq!(bad_theme.config.colors.theme, Theme::Default);
    assert_eq!(bad_theme.problems[0].text, "bad value for theme, ignored");
}

#[test]
fn a_colour_is_saved_as_a_name_below_16_and_a_number_from_16() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[colors]\ntheme = \"custom\" # mine\naccent = 3 # keep\n",
    )
    .unwrap();
    save(
        &path,
        Key::Color(Role::Accent),
        Value::color(ColorIndex(14)),
    )
    .unwrap();
    save(
        &path,
        Key::Color(Role::Group),
        Value::color(ColorIndex(240)),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "[colors]\ntheme = \"custom\" # mine\naccent = \"bright-cyan\" # keep\ngroup = 240\n"
    );
    let back = parse(&std::fs::read_to_string(&path).unwrap())
        .config
        .colors
        .custom;
    assert_eq!((back.accent, back.group), (ColorIndex(14), ColorIndex(240)));
}
