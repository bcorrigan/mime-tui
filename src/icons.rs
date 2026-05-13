use nerd_font_symbols::{fa, md, oct};

/// Icon for a MIME type, keyed on the media (top-level) component.
pub fn mime_icon(mime: &str) -> &'static str {
    let media = mime.split('/').next().unwrap_or("");
    match media {
        "text" => fa::FA_FILE_LINES,
        "image" => fa::FA_IMAGE,
        "audio" => fa::FA_MUSIC,
        "video" => fa::FA_FILM,
        "application" => fa::FA_FILE_ZIPPER,
        "font" => fa::FA_FONT,
        "model" => fa::FA_CUBE,
        "multipart" => fa::FA_LAYER_GROUP,
        "message" => fa::FA_ENVELOPE,
        _ => oct::OCT_DASH,
    }
}

/// Icon for an app's deduced `Categories=` bucket. Ported from bstl so the
/// glyph set matches users' muscle memory if they've ever used the launcher.
pub fn category_icon(category: &str) -> &'static str {
    match category {
        "Utilities" => fa::FA_GEAR,
        "Development" => fa::FA_HAMMER,
        "Network" => md::MD_EARTH,
        "Audio/Video" => fa::FA_MUSIC,
        "Graphics" => fa::FA_PAINTBRUSH,
        "System" => fa::FA_DESKTOP,
        "Office" => fa::FA_BOOK,
        "Games" => fa::FA_GAMEPAD,
        "Education" => fa::FA_GRADUATION_CAP,
        "Settings" => fa::FA_SLIDERS,
        "TUI" => fa::FA_TERMINAL,
        other => {
            let lower = other.to_ascii_lowercase();
            if lower.contains("script") {
                md::MD_SCRIPT_TEXT
            } else if lower.contains("terminal")
                || lower.contains("tui")
                || lower.contains("console")
            {
                fa::FA_TERMINAL
            } else {
                oct::OCT_DASH
            }
        }
    }
}
