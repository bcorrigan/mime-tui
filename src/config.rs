use std::fs;
use std::path::PathBuf;
use std::process;
use ratatui::style::Color;
use ratatui::widgets::BorderType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SearchPosition {
    Top,
    Bottom,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CursorShape {
    Block,
    Underline,
    Pipe,
}

/// Resolved theme — all colour fields are concrete strings ready for
/// `Theme::parse_color`. Built from the user's TOML + a preset palette;
/// see `resolve_config`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub border: String,
    pub focus: String,
    pub unfocused: String,
    pub highlight: String,
    /// Body-text foreground — applied to "normal" text throughout the UI
    /// (mime ids, app names, headers etc). Empty → the terminal's default
    /// foreground (so users on hand-tuned palettes get a faithful look).
    /// Presets that target a specific bg context set this explicitly so
    /// dracula looks distinctly "dracula" rather than "dracula accents on
    /// your terminal's white text".
    pub text: String,
    /// Foreground for de-emphasised secondary text — `.desktop` ids,
    /// description suffixes, dimmed hints.
    pub secondary: String,
    /// Foreground colour drawn on top of the selection bar. Was hardcoded
    /// to black; now themable so dark-on-dark presets stay readable.
    pub selection_fg: String,
    /// Scrollbar thumb colour. Empty → follows `focus`.
    pub scrollbar_thumb: String,
    /// Scrollbar track colour. Empty → follows `unfocused`.
    pub scrollbar_track: String,
    /// Per-marker colours in the picker / by-app right pane. Empty fields
    /// fall back to `highlight` / `focus` / `secondary` respectively.
    pub marker_default: String,
    pub marker_associated: String,
    pub marker_declared_only: String,
    /// Foreground for "invalid" rows — mime associations pointing at a
    /// `.desktop` that isn't installed. Rendered bold so the user notices
    /// stale entries to clean up.
    pub invalid: String,
    pub border_style: String,
    pub highlight_type: String,
    /// Empty → follows `focus`.
    pub cursor_color: String,
    pub cursor_shape: CursorShape,
    pub cursor_blink_interval: u64,
}

impl Default for Theme {
    fn default() -> Self {
        // The hardcoded default is the "default-dark" palette below, so
        // `MimeTuiConfig::default()` (used in tests and the empty-config
        // path) gives sensible values without going through the preset
        // machinery.
        palette_to_theme(&DEFAULT_DARK)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct MimeTuiConfig {
    pub search_position: SearchPosition,
    #[serde(rename = "theme")]
    pub colors: Theme,
    pub timeout: u64,
}

impl Default for MimeTuiConfig {
    fn default() -> Self {
        Self {
            search_position: SearchPosition::Top,
            colors: Theme::default(),
            timeout: 0,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Parse intermediates — all-Option per field so we can tell at resolve
// time which knobs the user set vs which to take from the preset palette.
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "snake_case")]
struct RawTheme {
    border: Option<String>,
    focus: Option<String>,
    unfocused: Option<String>,
    highlight: Option<String>,
    text: Option<String>,
    secondary: Option<String>,
    selection_fg: Option<String>,
    scrollbar_thumb: Option<String>,
    scrollbar_track: Option<String>,
    marker_default: Option<String>,
    marker_associated: Option<String>,
    marker_declared_only: Option<String>,
    invalid: Option<String>,
    border_style: Option<String>,
    highlight_type: Option<String>,
    cursor_color: Option<String>,
    cursor_shape: Option<CursorShape>,
    cursor_blink_interval: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "snake_case")]
struct RawConfig {
    preset: Option<String>,
    search_position: Option<SearchPosition>,
    timeout: Option<u64>,
    #[serde(rename = "theme")]
    colors: RawTheme,
}

// ─────────────────────────────────────────────────────────────────────────
// Preset palettes. Add new ones here and route in `palette_for`.
// ─────────────────────────────────────────────────────────────────────────

/// Internal palette spec. `&'static str` keeps things const-constructible
/// so each preset is a true compile-time constant.
///
/// "" (empty string) in a colour field means "fall back to something else"
/// per `resolve_fallback_colors` — used for cursor / scrollbar / etc.
struct Palette {
    border: &'static str,
    focus: &'static str,
    unfocused: &'static str,
    highlight: &'static str,
    text: &'static str,
    secondary: &'static str,
    selection_fg: &'static str,
    scrollbar_thumb: &'static str,
    scrollbar_track: &'static str,
    marker_default: &'static str,
    marker_associated: &'static str,
    marker_declared_only: &'static str,
    invalid: &'static str,
    border_style: &'static str,
    highlight_type: &'static str,
    cursor_color: &'static str,
    cursor_shape: CursorShape,
    cursor_blink_interval: u64,
}

const DEFAULT_DARK: Palette = Palette {
    border: "#ffffff",
    focus: "#00ff00",
    unfocused: "#808080",
    highlight: "#ffd700",
    // Empty = "follow terminal fg" — preserves the original behaviour for
    // users who haven't switched preset. Other presets override.
    text: "",
    secondary: "#808080",
    selection_fg: "#000000",
    scrollbar_thumb: "",
    scrollbar_track: "",
    marker_default: "",
    marker_associated: "",
    marker_declared_only: "",
    invalid: "#ff5252",          // bright red — phantom mime assocs
    border_style: "rounded",
    highlight_type: "background",
    cursor_color: "",
    cursor_shape: CursorShape::Block,
    cursor_blink_interval: 0,
};

/// Inverted for light-background terminals: dark borders, dark text accents,
/// amber highlight still pairs with black text on top.
const DEFAULT_LIGHT: Palette = Palette {
    border: "#404040",
    focus: "#1976d2",            // navy-blue, modern flat feel
    unfocused: "#cccccc",
    highlight: "#fbc02d",        // amber (legible on white)
    // Empty (like DEFAULT_DARK) — both "default" presets defer to the
    // terminal's own foreground colour. The opinionated palettes below
    // (gruvbox, dracula, nord, …) explicitly override it; the "default"
    // pair is meant to adapt to whatever the user has set up.
    text: "",
    secondary: "#666666",
    selection_fg: "#000000",
    scrollbar_thumb: "",
    scrollbar_track: "",
    marker_default: "#f57c00",   // darker amber so ★ pops on white bg
    marker_associated: "#388e3c",
    marker_declared_only: "#888888",
    invalid: "#c62828",          // dark red — contrastful on white
    border_style: "rounded",
    highlight_type: "background",
    cursor_color: "",
    cursor_shape: CursorShape::Block,
    cursor_blink_interval: 0,
};

/// Gruvbox dark (medium). The signature warm-earthy palette — soft yellows,
/// muted oranges, oxidised greens. ★ → bright orange (gruvbox's headline
/// accent), ✓ → bright green.
const GRUVBOX_DARK: Palette = Palette {
    border: "#a89984",
    focus: "#83a598",            // bright blue
    unfocused: "#504945",        // bg2
    highlight: "#fabd2f",        // bright yellow
    text: "#ebdbb2",             // fg1 — the canonical gruvbox cream
    secondary: "#928374",        // fg4 / "gray"
    selection_fg: "#282828",     // gruvbox bg
    scrollbar_thumb: "#fabd2f",
    scrollbar_track: "#504945",
    marker_default: "#fe8019",   // bright orange (gruvbox signature)
    marker_associated: "#b8bb26",
    marker_declared_only: "#928374",
    invalid: "#fb4934",          // gruvbox bright red
    border_style: "rounded",
    highlight_type: "background",
    cursor_color: "",
    cursor_shape: CursorShape::Block,
    cursor_blink_interval: 0,
};

/// Solarized light. Cream background, muted blue-gray accents. Highlight is
/// the canonical Solarized yellow, with text drawn against it in the
/// darkest base.
const SOLARIZED_LIGHT: Palette = Palette {
    border: "#93a1a1",           // base1
    focus: "#268bd2",            // blue
    unfocused: "#eee8d5",        // base2
    highlight: "#b58900",        // yellow
    text: "#586e75",             // base01 — Solarized's "body text" colour
    // base00 instead of the canonical base1 — base1 is Solarized's
    // "comment" colour, deliberately faint, which read as unreadable in
    // our status-bar and detail-pane hints. base00 is one step darker:
    // still subordinate to `text`, but legible on the cream bg.
    secondary: "#657b83",
    selection_fg: "#002b36",     // base03 (darkest) — readable on yellow
    scrollbar_thumb: "#268bd2",
    scrollbar_track: "#eee8d5",
    marker_default: "#cb4b16",   // orange
    marker_associated: "#859900",
    marker_declared_only: "#93a1a1",
    invalid: "#dc322f",          // solarized red
    border_style: "rounded",
    highlight_type: "background",
    cursor_color: "",
    cursor_shape: CursorShape::Block,
    cursor_blink_interval: 0,
};

/// Dracula. Iconic purple accent + bright vivid markers. Background is the
/// canonical #282a36; selection foreground inherits that so we stay on the
/// theme's character even when reading highlighted rows.
const DRACULA: Palette = Palette {
    border: "#6272a4",           // comment
    focus: "#bd93f9",            // purple (dracula's signature)
    unfocused: "#44475a",        // current-line
    highlight: "#f1fa8c",        // yellow
    text: "#f8f8f2",             // dracula's canonical foreground
    secondary: "#6272a4",
    selection_fg: "#282a36",
    scrollbar_thumb: "#bd93f9",
    scrollbar_track: "#44475a",
    marker_default: "#ffb86c",   // orange (★ pops)
    marker_associated: "#50fa7b",
    marker_declared_only: "#6272a4",
    invalid: "#ff5555",          // dracula red
    border_style: "rounded",
    highlight_type: "background",
    cursor_color: "",
    cursor_shape: CursorShape::Block,
    cursor_blink_interval: 0,
};

/// Nord. Cool polar-night/frost palette. Frost cyan-blue carries focus;
/// aurora yellow drives the highlight bar.
const NORD: Palette = Palette {
    border: "#4c566a",           // polar night 3
    focus: "#88c0d0",            // frost (cyan-blue)
    unfocused: "#3b4252",        // polar night 1
    highlight: "#ebcb8b",        // aurora yellow
    text: "#d8dee9",             // snow storm 4 — nord's body fg
    secondary: "#81a1c1",        // frost (slightly lighter)
    selection_fg: "#2e3440",     // polar night 0 (darkest)
    scrollbar_thumb: "#88c0d0",
    scrollbar_track: "#3b4252",
    marker_default: "#d08770",   // aurora orange
    marker_associated: "#a3be8c",
    marker_declared_only: "#4c566a",
    invalid: "#bf616a",          // nord aurora red
    border_style: "rounded",
    highlight_type: "background",
    cursor_color: "",
    cursor_shape: CursorShape::Block,
    cursor_blink_interval: 0,
};

/// Monokai. Vivid contrast palette — neon pink, electric green, sunny
/// yellow on a warm dark background.
const MONOKAI: Palette = Palette {
    border: "#75715e",           // comment gray
    focus: "#66d9ef",            // blue
    unfocused: "#3e3d32",        // darker bg
    highlight: "#e6db74",        // yellow
    text: "#f8f8f2",             // monokai's canonical foreground
    secondary: "#75715e",
    selection_fg: "#272822",     // bg
    scrollbar_thumb: "#66d9ef",
    scrollbar_track: "#3e3d32",
    marker_default: "#f92672",   // pink (monokai signature)
    marker_associated: "#a6e22e",
    marker_declared_only: "#75715e",
    invalid: "#ff6188",          // bright pink-red (distinct from default pink)
    border_style: "rounded",
    highlight_type: "background",
    cursor_color: "",
    cursor_shape: CursorShape::Block,
    cursor_blink_interval: 0,
};

/// All shipped preset names, in display order. Used by the in-app theme
/// picker (Ctrl-T) to show a browseable list.
pub const PRESET_NAMES: &[&str] = &[
    "default-dark",
    "default-light",
    "gruvbox-dark",
    "solarized-light",
    "dracula",
    "nord",
    "monokai",
];

/// Replace `cfg.colors` with the named preset's palette (or `default-dark`
/// if the name is unknown). User overrides are dropped — this is meant for
/// the in-app "let me see how it looks" preview path, not for the loaded
/// config (which goes through `resolve_config` and preserves overrides).
pub fn apply_preset(cfg: &mut MimeTuiConfig, name: &str) {
    let palette = palette_for(Some(name));
    cfg.colors = palette_to_theme(palette);
    resolve_fallback_colors(cfg);
}

fn palette_for(name: Option<&str>) -> &'static Palette {
    match name.map(str::trim).filter(|s| !s.is_empty()) {
        None | Some("default-dark") => &DEFAULT_DARK,
        Some("default-light") => &DEFAULT_LIGHT,
        Some("gruvbox-dark") => &GRUVBOX_DARK,
        Some("solarized-light") => &SOLARIZED_LIGHT,
        Some("dracula") => &DRACULA,
        Some("nord") => &NORD,
        Some("monokai") => &MONOKAI,
        Some(other) => {
            eprintln!(
                "mime-tui: unknown preset {:?} — falling back to default-dark.\n         \
                 Known presets: default-dark, default-light, gruvbox-dark, solarized-light, \
                 dracula, nord, monokai.",
                other
            );
            &DEFAULT_DARK
        }
    }
}

fn palette_to_theme(p: &Palette) -> Theme {
    Theme {
        border: p.border.into(),
        focus: p.focus.into(),
        unfocused: p.unfocused.into(),
        highlight: p.highlight.into(),
        text: p.text.into(),
        secondary: p.secondary.into(),
        selection_fg: p.selection_fg.into(),
        scrollbar_thumb: p.scrollbar_thumb.into(),
        scrollbar_track: p.scrollbar_track.into(),
        marker_default: p.marker_default.into(),
        marker_associated: p.marker_associated.into(),
        marker_declared_only: p.marker_declared_only.into(),
        invalid: p.invalid.into(),
        border_style: p.border_style.into(),
        highlight_type: p.highlight_type.into(),
        cursor_color: p.cursor_color.into(),
        cursor_shape: p.cursor_shape.clone(),
        cursor_blink_interval: p.cursor_blink_interval,
    }
}

fn merge_str(user: Option<String>, palette: &str) -> String {
    user.unwrap_or_else(|| palette.into())
}

fn resolve_config(raw: RawConfig) -> MimeTuiConfig {
    let palette = palette_for(raw.preset.as_deref());

    let theme = Theme {
        border: merge_str(raw.colors.border, palette.border),
        focus: merge_str(raw.colors.focus, palette.focus),
        unfocused: merge_str(raw.colors.unfocused, palette.unfocused),
        highlight: merge_str(raw.colors.highlight, palette.highlight),
        text: merge_str(raw.colors.text, palette.text),
        secondary: merge_str(raw.colors.secondary, palette.secondary),
        selection_fg: merge_str(raw.colors.selection_fg, palette.selection_fg),
        scrollbar_thumb: merge_str(raw.colors.scrollbar_thumb, palette.scrollbar_thumb),
        scrollbar_track: merge_str(raw.colors.scrollbar_track, palette.scrollbar_track),
        marker_default: merge_str(raw.colors.marker_default, palette.marker_default),
        marker_associated: merge_str(raw.colors.marker_associated, palette.marker_associated),
        marker_declared_only: merge_str(
            raw.colors.marker_declared_only,
            palette.marker_declared_only,
        ),
        invalid: merge_str(raw.colors.invalid, palette.invalid),
        border_style: merge_str(raw.colors.border_style, palette.border_style),
        highlight_type: merge_str(raw.colors.highlight_type, palette.highlight_type),
        cursor_color: merge_str(raw.colors.cursor_color, palette.cursor_color),
        cursor_shape: raw.colors.cursor_shape.unwrap_or_else(|| palette.cursor_shape.clone()),
        cursor_blink_interval: raw
            .colors
            .cursor_blink_interval
            .unwrap_or(palette.cursor_blink_interval),
    };

    let mut cfg = MimeTuiConfig {
        search_position: raw.search_position.unwrap_or(SearchPosition::Top),
        colors: theme,
        timeout: raw.timeout.unwrap_or(0),
    };
    resolve_fallback_colors(&mut cfg);
    cfg
}

// ─────────────────────────────────────────────────────────────────────────
// Theme — colour parsing + fallback resolution
// ─────────────────────────────────────────────────────────────────────────

impl Theme {
    /// Accept hex (`#RGB` / `#RRGGBB` / `#RRGGBBAA`), ANSI named colours
    /// (`red`, `bright_yellow`, `cyan`, …), or `reset` / `default` /
    /// empty-string for "terminal default fg/bg". Case-insensitive.
    pub fn parse_color(color: &str) -> Color {
        let lower = color.trim().to_lowercase();
        // Named ANSI / palette references first.
        match lower.as_str() {
            "" | "reset" | "default" | "terminal" => return Color::Reset,
            "black" => return Color::Black,
            "red" => return Color::Red,
            "green" => return Color::Green,
            "yellow" => return Color::Yellow,
            "blue" => return Color::Blue,
            "magenta" | "pink" => return Color::Magenta,
            "cyan" => return Color::Cyan,
            "gray" | "grey" => return Color::Gray,
            "dark_gray" | "dark_grey" | "bright_black" => return Color::DarkGray,
            "light_red" | "bright_red" => return Color::LightRed,
            "light_green" | "bright_green" => return Color::LightGreen,
            "light_yellow" | "bright_yellow" => return Color::LightYellow,
            "light_blue" | "bright_blue" => return Color::LightBlue,
            "light_magenta" | "bright_magenta" | "light_pink" => return Color::LightMagenta,
            "light_cyan" | "bright_cyan" => return Color::LightCyan,
            "white" | "bright_white" => return Color::White,
            _ => {}
        }
        parse_hex_color(&lower).unwrap_or(Color::Reset)
    }

    pub fn parse_border_type(style: &str) -> BorderType {
        match style.to_lowercase().as_str() {
            "plain" => BorderType::Plain,
            "rounded" => BorderType::Rounded,
            "thick" => BorderType::Thick,
            "double" => BorderType::Double,
            _ => BorderType::Plain,
        }
    }
}

fn parse_hex_color(color: &str) -> Option<Color> {
    let color = color.trim();
    if !color.starts_with('#') {
        return None;
    }
    let hex = &color[1..];
    match hex.len() {
        3 => {
            let r = u8::from_str_radix(&hex[0..1], 16).ok()?;
            let g = u8::from_str_radix(&hex[1..2], 16).ok()?;
            let b = u8::from_str_radix(&hex[2..3], 16).ok()?;
            Some(Color::Rgb(r * 17, g * 17, b * 17))
        }
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some(Color::Rgb(r, g, b))
        }
        // #RRGGBBAA — alpha ignored; terminal cells don't blend.
        8 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some(Color::Rgb(r, g, b))
        }
        _ => None,
    }
}

fn parse_toml_config(content: &str) -> Result<MimeTuiConfig, toml::de::Error> {
    let raw: RawConfig = toml::from_str(content)?;
    Ok(resolve_config(raw))
}

/// Several theme fields default to "empty means follow another field". This
/// resolves those at load time so the renderer sees concrete hex strings.
///
/// `cursor_color` is intentionally NOT in this list — empty means "don't
/// touch the terminal cursor at all" (skip the OSC 12 sequence on startup
/// and the OSC 112 reset on exit). Users who want a themed cursor set it
/// explicitly.
fn resolve_fallback_colors(cfg: &mut MimeTuiConfig) {
    if cfg.colors.scrollbar_thumb.trim().is_empty() {
        cfg.colors.scrollbar_thumb = cfg.colors.focus.clone();
    }
    if cfg.colors.scrollbar_track.trim().is_empty() {
        cfg.colors.scrollbar_track = cfg.colors.unfocused.clone();
    }
    if cfg.colors.marker_default.trim().is_empty() {
        cfg.colors.marker_default = cfg.colors.highlight.clone();
    }
    if cfg.colors.marker_associated.trim().is_empty() {
        cfg.colors.marker_associated = cfg.colors.focus.clone();
    }
    if cfg.colors.marker_declared_only.trim().is_empty() {
        cfg.colors.marker_declared_only = cfg.colors.secondary.clone();
    }
    if cfg.colors.invalid.trim().is_empty() {
        // Sensible terminal-aware fallback when the preset doesn't set
        // one — `red` resolves through `parse_color` to the terminal's
        // ANSI red, which is the right "broken state" cue everywhere.
        cfg.colors.invalid = "red".into();
    }
}

/// Top-level config loader. On first startup (no user config file present
/// and no system fallback) writes a small commented default to
/// `~/.config/mime-tui/mime-tui.toml` so the user has something to edit.
/// Failures to create that file are silently ignored — we'll still load
/// defaults in-memory.
pub fn load_config() -> MimeTuiConfig {
    let user_path = dirs::config_dir()
        .map(|c| c.join("mime-tui/mime-tui.toml"))
        .unwrap_or_else(|| PathBuf::from("~/.config/mime-tui/mime-tui.toml"));
    let system_path = PathBuf::from("/usr/share/doc/mime-tui/mime-tui.toml");

    let path = if user_path.exists() {
        user_path
    } else if system_path.exists() {
        system_path
    } else {
        // First startup. Try to seed the user config so future runs
        // (and subsequent edits) have a real file to load — best-effort.
        let _ = write_default_config_at(&user_path);
        if user_path.exists() {
            user_path
        } else {
            // Couldn't create it (read-only home, etc.) — still load
            // defaults in memory so the app runs.
            return resolve_config(RawConfig::default());
        }
    };

    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("mime-tui: failed to read {}: {}", path.display(), e);
            process::exit(1);
        }
    };

    parse_toml_config(&content).unwrap_or_else(|e| {
        eprintln!(
            "mime-tui: failed to parse {}:\n{}",
            path.display(),
            e
        );
        process::exit(1);
    })
}

/// Persist a preset choice into the user's `mime-tui.toml`. The strategy is
/// deliberately line-level: find the existing `preset = "..."` line and
/// replace its value, leaving every comment, whitespace, and unrelated
/// setting verbatim. If no `preset` line exists, insert one above the
/// first non-comment, non-blank line (or append at end if the file is
/// all comments / empty).
///
/// Atomic: writes to `<file>.tmp` then renames. Errors propagate so the
/// caller can flash a friendly message if disk write fails.
pub fn save_preset_to_config(name: &str) -> std::io::Result<()> {
    let path = dirs::config_dir()
        .map(|c| c.join("mime-tui/mime-tui.toml"))
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "could not resolve XDG_CONFIG_HOME",
            )
        })?;
    save_preset_to_config_at(&path, name)
}

/// Same as [`save_preset_to_config`] but with an explicit target path —
/// used by tests.
pub(crate) fn save_preset_to_config_at(
    path: &std::path::Path,
    name: &str,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let existing = fs::read_to_string(path).unwrap_or_default();
    let updated = update_preset_line(&existing, name);

    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, updated)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Pure transformer: given a TOML source string, return a new string with
/// the preset line updated (or inserted). Comments, blank lines, and every
/// other key are left untouched. Quoting uses double-quotes for the new
/// value regardless of the original quoting style.
fn update_preset_line(content: &str, new_preset: &str) -> String {
    let mut lines: Vec<String> = content.lines().map(String::from).collect();
    let new_line = format!("preset = \"{}\"", new_preset);

    // Look for an existing active `preset = ...` (skip comments / blanks).
    // Stop at the first table header (`[section]`) — preset belongs to the
    // top-level table only.
    let mut found = false;
    for line in &mut lines {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed.starts_with('[') {
            // Entered a table — preset must precede it. Stop searching.
            break;
        }
        // Match `preset` as the key (followed by '=' eventually). Allow
        // spaces around the '='.
        let after_key = trimmed.strip_prefix("preset").map(str::trim_start);
        if let Some(rest) = after_key {
            if rest.starts_with('=') {
                let indent_len = line.len() - trimmed.len();
                let indent: String = line.chars().take(indent_len).collect();
                *line = format!("{}{}", indent, new_line);
                found = true;
                break;
            }
        }
    }

    if !found {
        // Insert above the first non-comment / non-blank / non-section line
        // so the new preset sits at the top of the active settings. If
        // there are no such lines, append.
        let insert_at = lines
            .iter()
            .position(|l| {
                let t = l.trim_start();
                !t.is_empty() && !t.starts_with('#')
            })
            .unwrap_or(lines.len());
        lines.insert(insert_at, new_line);
    }

    let mut result = lines.join("\n");
    // Preserve the original trailing-newline convention.
    if content.ends_with('\n') && !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

/// Seed a fresh `mime-tui.toml` at `path` if it doesn't already exist.
/// Creates parent directories as needed.
fn write_default_config_at(path: &std::path::Path) -> std::io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, DEFAULT_CONFIG_TOML)
}

/// Initial config emitted on first run. Set just enough to be useful as a
/// starting point — one active line (preset) and a commented-out theme
/// override example. Listing the preset names in a comment saves users a
/// trip to the README.
const DEFAULT_CONFIG_TOML: &str = r##"# mime-tui configuration
# Auto-created on first run; safe to edit or delete.
#
# Every field is optional. Pick a preset for a colour scheme in one line;
# the [theme] section then overrides individual fields on top.
#
# Shipped presets:
#   default-dark     — built-in defaults (green focus, gold highlight)
#   default-light    — for light-background terminals
#   gruvbox-dark     — warm earthy palette
#   solarized-light  — cream background, blue accents
#   dracula          — signature purple focus
#   nord             — cool polar / frost palette
#   monokai          — vivid neon (★ in pink)
#
# Full theme reference: see the project README.

preset = "default-dark"

# Example override — uncomment and edit:
# [theme]
# highlight = "#ffaa00"
"##;

#[cfg(test)]
mod tests;
