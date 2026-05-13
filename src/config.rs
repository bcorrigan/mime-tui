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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub border: String,
    pub focus: String,
    pub unfocused: String,
    pub highlight: String,
    /// Foreground for de-emphasized secondary text — e.g. the `.desktop` id
    /// trailing an app name in the picker, or a mime's human description.
    /// Light gray by default; theme it separately from `unfocused` (which is
    /// the selection-bar background of an unfocused list).
    pub secondary: String,
    /// Colour of the scrollbar thumb (the moving indicator). Empty means
    /// "follow `focus`"; resolved at load time.
    pub scrollbar_thumb: String,
    /// Colour of the scrollbar track (the rail the thumb slides along).
    /// Empty means "follow `unfocused`"; resolved at load time.
    pub scrollbar_track: String,
    pub border_style: String,
    pub highlight_type: String,
    /// Empty means "follow `focus`"; resolved at load time.
    pub cursor_color: String,
    pub cursor_shape: CursorShape,
    pub cursor_blink_interval: u64,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            border: "#ffffff".into(),
            focus: "#00ff00".into(),
            unfocused: "#808080".into(),
            // Gold-yellow gives strong contrast with the hardcoded black row
            // foreground in `layout::render_list`. The previous default
            // (`#0000ff`) made selected rows nearly unreadable on dark
            // terminal themes.
            highlight: "#ffd700".into(),
            secondary: "#808080".into(),
            scrollbar_thumb: String::new(),
            scrollbar_track: String::new(),
            border_style: "rounded".into(),
            highlight_type: "background".into(),
            cursor_color: String::new(),
            cursor_shape: CursorShape::Block,
            cursor_blink_interval: 0,
        }
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

impl Theme {
    pub fn parse_color(color: &str) -> Color {
        let color = color.trim();

        if color.starts_with('#') {
            let hex = &color[1..];

            match hex.len() {
                3 => {
                    if let (Ok(r), Ok(g), Ok(b)) = (
                        u8::from_str_radix(&hex[0..1], 16),
                        u8::from_str_radix(&hex[1..2], 16),
                        u8::from_str_radix(&hex[2..3], 16),
                    ) {
                        return Color::Rgb(r * 17, g * 17, b * 17);
                    }
                }
                6 => {
                    if let (Ok(r), Ok(g), Ok(b)) = (
                        u8::from_str_radix(&hex[0..2], 16),
                        u8::from_str_radix(&hex[2..4], 16),
                        u8::from_str_radix(&hex[4..6], 16),
                    ) {
                        return Color::Rgb(r, g, b);
                    }
                }
                8 => {
                    if let (Ok(r), Ok(g), Ok(b)) = (
                        u8::from_str_radix(&hex[0..2], 16),
                        u8::from_str_radix(&hex[2..4], 16),
                        u8::from_str_radix(&hex[4..6], 16),
                    ) {
                        return Color::Rgb(r, g, b);
                    }
                }
                _ => {}
            }
        }

        Color::Reset
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

fn parse_toml_config(content: &str) -> Result<MimeTuiConfig, toml::de::Error> {
    let mut cfg: MimeTuiConfig = toml::from_str(content)?;
    resolve_fallback_colors(&mut cfg);
    Ok(cfg)
}

/// Several theme fields default to "empty means follow another field". This
/// resolves those at load time so the renderer sees concrete hex strings.
fn resolve_fallback_colors(cfg: &mut MimeTuiConfig) {
    if cfg.colors.cursor_color.trim().is_empty() {
        cfg.colors.cursor_color = cfg.colors.focus.clone();
    }
    if cfg.colors.scrollbar_thumb.trim().is_empty() {
        cfg.colors.scrollbar_thumb = cfg.colors.focus.clone();
    }
    if cfg.colors.scrollbar_track.trim().is_empty() {
        cfg.colors.scrollbar_track = cfg.colors.unfocused.clone();
    }
}

/// Top-level config loader. Falls back to defaults if no config file exists,
/// and exits with a friendly diagnostic on parse error.
pub fn load_config() -> MimeTuiConfig {
    let user_path = dirs::config_dir()
        .map(|c| c.join("mime-tui/mime-tui.toml"))
        .unwrap_or_else(|| PathBuf::from("~/.config/mime-tui/mime-tui.toml"));
    let system_path = PathBuf::from("/usr/share/doc/mime-tui/mime-tui.toml");

    let path = if user_path.exists() {
        Some(user_path)
    } else if system_path.exists() {
        Some(system_path)
    } else {
        None
    };

    let Some(path) = path else {
        let mut cfg = MimeTuiConfig::default();
        resolve_fallback_colors(&mut cfg);
        return cfg;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_yields_defaults() {
        let cfg = parse_toml_config("").unwrap();
        assert_eq!(cfg.search_position, SearchPosition::Top);
        assert_eq!(cfg.timeout, 0);
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
        assert_eq!(cfg.colors.unfocused, "#808080");
    }

    #[test]
    fn cursor_color_falls_back_to_focus_when_empty() {
        let cfg = parse_toml_config(
            r##"
            [theme]
            focus = "#112233"
            "##,
        )
        .unwrap();
        assert_eq!(cfg.colors.cursor_color, "#112233");
    }
}
