use super::*;

#[test]
fn empty_config_yields_defaults() {
    let cfg = parse_toml_config("").unwrap();
    assert_eq!(cfg.search_position, SearchPosition::Top);
    assert_eq!(cfg.timeout, 0);
    // Default-dark palette baked in:
    assert_eq!(cfg.colors.border, "#ffffff");
    assert_eq!(cfg.colors.focus, "#00ff00");
    assert_eq!(cfg.colors.highlight, "#ffd700");
    assert_eq!(cfg.colors.selection_fg, "#000000");
}

#[test]
fn theme_section_parses() {
    let cfg = parse_toml_config(
        r##"
        [theme]
        border = "#abcdef"
        cursor_shape = "underline"
        "##,
    )
    .unwrap();
    assert_eq!(cfg.colors.border, "#abcdef");
    assert_eq!(cfg.colors.cursor_shape, CursorShape::Underline);
    // Untouched theme fields still default
    assert_eq!(cfg.colors.unfocused, "#808080");
}

#[test]
fn cursor_color_stays_empty_when_unset() {
    // Empty `cursor_color` means "don't touch the terminal cursor".
    // Previously we fell back to `focus`, which forced a themed cursor
    // on every preset — that didn't match what most users actually
    // want (and stomps on their terminal's own cursor styling).
    let cfg = parse_toml_config("").unwrap();
    assert_eq!(cfg.colors.cursor_color, "");
}

#[test]
fn cursor_color_honoured_when_set() {
    let cfg = parse_toml_config(
        r##"
        [theme]
        cursor_color = "#ff00ff"
        "##,
    )
    .unwrap();
    assert_eq!(cfg.colors.cursor_color, "#ff00ff");
}

// ── presets ──────────────────────────────────────────────────────

#[test]
fn preset_applies_when_specified() {
    let cfg = parse_toml_config(r##"preset = "dracula""##).unwrap();
    assert_eq!(cfg.colors.focus, "#bd93f9");
    assert_eq!(cfg.colors.highlight, "#f1fa8c");
    assert_eq!(cfg.colors.marker_default, "#ffb86c");
}

#[test]
fn user_theme_overrides_preset() {
    let cfg = parse_toml_config(
        r##"
        preset = "dracula"
        [theme]
        focus = "#ff0000"
        "##,
    )
    .unwrap();
    // Override wins:
    assert_eq!(cfg.colors.focus, "#ff0000");
    // Other dracula fields still apply:
    assert_eq!(cfg.colors.highlight, "#f1fa8c");
}

#[test]
fn unknown_preset_falls_back_to_default_dark() {
    let cfg = parse_toml_config(r##"preset = "nonexistent""##).unwrap();
    assert_eq!(cfg.colors.focus, "#00ff00"); // default-dark green
}

#[test]
fn light_preset_has_dark_borders_for_white_bg() {
    let cfg = parse_toml_config(r##"preset = "default-light""##).unwrap();
    // Default-light uses dark border instead of #ffffff so it's visible.
    assert_eq!(cfg.colors.border, "#404040");
}

// ── colour parser ────────────────────────────────────────────────

#[test]
fn parse_color_handles_named_ansi() {
    assert_eq!(Theme::parse_color("red"), Color::Red);
    assert_eq!(Theme::parse_color("green"), Color::Green);
    assert_eq!(Theme::parse_color("bright_yellow"), Color::LightYellow);
    assert_eq!(Theme::parse_color("light_blue"), Color::LightBlue);
    assert_eq!(Theme::parse_color("dark_gray"), Color::DarkGray);
    assert_eq!(Theme::parse_color("white"), Color::White);
}

#[test]
fn parse_color_handles_reset_and_synonyms() {
    assert_eq!(Theme::parse_color(""), Color::Reset);
    assert_eq!(Theme::parse_color("reset"), Color::Reset);
    assert_eq!(Theme::parse_color("default"), Color::Reset);
    assert_eq!(Theme::parse_color("terminal"), Color::Reset);
}

#[test]
fn parse_color_still_handles_hex() {
    assert_eq!(Theme::parse_color("#ff0000"), Color::Rgb(255, 0, 0));
    assert_eq!(Theme::parse_color("#f00"), Color::Rgb(0xff, 0x00, 0x00));
    // RRGGBBAA — alpha ignored
    assert_eq!(
        Theme::parse_color("#ff0000aa"),
        Color::Rgb(255, 0, 0),
    );
}

#[test]
fn parse_color_is_case_insensitive() {
    assert_eq!(Theme::parse_color("RED"), Color::Red);
    assert_eq!(Theme::parse_color("Bright_Blue"), Color::LightBlue);
    assert_eq!(Theme::parse_color("#FF00FF"), Color::Rgb(255, 0, 255));
}

// ── first-run config writer ──────────────────────────────────────

#[test]
fn write_default_config_seeds_a_missing_file() {
    let dir = std::env::temp_dir().join(format!(
        "mime_tui_seed_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let path = dir.join("mime-tui").join("mime-tui.toml");

    // Parent doesn't exist either — function should mkdir.
    assert!(!path.exists());
    write_default_config_at(&path).unwrap();
    assert!(path.exists());

    let content = std::fs::read_to_string(&path).unwrap();
    // Should contain the active preset line.
    assert!(content.contains("preset = "));
    // Comment lists at least one preset name — sanity check the template.
    assert!(content.contains("gruvbox-dark"));

    // And the file should parse cleanly through the normal pipeline.
    let cfg = parse_toml_config(&content).unwrap();
    assert_eq!(cfg.colors.focus, "#00ff00"); // default-dark green

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn apply_preset_swaps_in_palette() {
    let mut cfg = MimeTuiConfig::default();
    assert_eq!(cfg.colors.focus, "#00ff00"); // default-dark
    apply_preset(&mut cfg, "dracula");
    assert_eq!(cfg.colors.focus, "#bd93f9");
    assert_eq!(cfg.colors.highlight, "#f1fa8c");
}

#[test]
fn apply_preset_unknown_falls_back_to_default_dark() {
    let mut cfg = MimeTuiConfig::default();
    apply_preset(&mut cfg, "dracula");
    apply_preset(&mut cfg, "nonsense");
    // After applying nonsense, palette_for falls back to default-dark.
    assert_eq!(cfg.colors.focus, "#00ff00");
}

#[test]
fn preset_names_includes_every_palette() {
    // Sanity guard — if someone adds a preset they shouldn't forget to
    // wire it into PRESET_NAMES (the picker iterates this list).
    for name in PRESET_NAMES {
        let mut cfg = MimeTuiConfig::default();
        apply_preset(&mut cfg, name);
        // None of the named presets should leave the focus colour
        // empty; that would mean palette_for returned an unwired stub.
        assert!(!cfg.colors.focus.is_empty(), "preset {} has no focus colour", name);
    }
}

// ── line-level preset update ─────────────────────────────────────

#[test]
fn update_preset_replaces_existing_line() {
    let input = "preset = \"default-dark\"\n[theme]\nfocus = \"#ff0000\"\n";
    let out = update_preset_line(input, "dracula");
    assert!(out.contains("preset = \"dracula\""));
    assert!(out.contains("focus = \"#ff0000\""));
    assert!(!out.contains("default-dark"));
}

#[test]
fn update_preset_preserves_comments_and_unrelated_lines() {
    let input = "# my config\n# generated\npreset = \"default-dark\"\n# trailing comment\n[theme]\nfocus = \"#ff0000\"\n";
    let out = update_preset_line(input, "monokai");
    assert!(out.starts_with("# my config\n"));
    assert!(out.contains("# generated"));
    assert!(out.contains("# trailing comment"));
    assert!(out.contains("preset = \"monokai\""));
    assert!(out.contains("focus = \"#ff0000\""));
}

#[test]
fn update_preset_inserts_when_missing_above_other_settings() {
    let input = "# my config\nsearch_position = \"bottom\"\n[theme]\nborder = \"#ff0000\"\n";
    let out = update_preset_line(input, "nord");
    // New preset line should appear above search_position (the first
    // non-comment line) — and before the [theme] header.
    let nord_pos = out.find("preset = \"nord\"").expect("preset inserted");
    let search_pos = out.find("search_position").unwrap();
    let theme_pos = out.find("[theme]").unwrap();
    assert!(nord_pos < search_pos, "preset should be above search_position");
    assert!(nord_pos < theme_pos);
    // Comment + everything else preserved.
    assert!(out.contains("# my config"));
    assert!(out.contains("border = \"#ff0000\""));
}

#[test]
fn update_preset_inserts_at_end_when_only_comments() {
    let input = "# nothing else\n";
    let out = update_preset_line(input, "nord");
    assert!(out.contains("# nothing else"));
    assert!(out.contains("preset = \"nord\""));
    // The comment should come first, then the preset.
    let comment_pos = out.find("# nothing else").unwrap();
    let preset_pos = out.find("preset =").unwrap();
    assert!(comment_pos < preset_pos);
}

#[test]
fn update_preset_ignores_commented_out_preset_lines() {
    // A `# preset = "x"` should be treated as a comment, not replaced.
    let input = "# preset = \"foo\"\nsearch_position = \"top\"\n";
    let out = update_preset_line(input, "dracula");
    // The original commented line stays intact …
    assert!(out.contains("# preset = \"foo\""));
    // … and a new active preset is inserted.
    assert!(out.contains("preset = \"dracula\""));
}

#[test]
fn update_preset_replaces_with_unusual_whitespace() {
    // Tab indent + extra spaces around the `=` should still match.
    let input = "\tpreset   =   \"old\"\n";
    let out = update_preset_line(input, "nord");
    assert!(out.contains("preset = \"nord\""));
    assert!(!out.contains("old"));
    // Original indent preserved (we keep the leading whitespace).
    assert!(out.contains("\tpreset"));
}

#[test]
fn save_preset_to_config_writes_atomically() {
    let dir = std::env::temp_dir().join(format!(
        "mime_tui_savepreset_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mime-tui.toml");
    std::fs::write(&path, "preset = \"default-dark\"\n[theme]\nborder = \"#ff0000\"\n")
        .unwrap();

    save_preset_to_config_at(&path, "dracula").unwrap();

    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("preset = \"dracula\""));
    assert!(content.contains("border = \"#ff0000\""));
    // No tempfile leftover.
    assert!(!path.with_extension("toml.tmp").exists());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn write_default_config_is_no_op_if_file_already_exists() {
    let dir = std::env::temp_dir().join(format!(
        "mime_tui_noop_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mime-tui.toml");
    std::fs::write(&path, "preset = \"dracula\"\n").unwrap();

    write_default_config_at(&path).unwrap();

    // Pre-existing content untouched.
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "preset = \"dracula\"\n");

    let _ = std::fs::remove_dir_all(&dir);
}
