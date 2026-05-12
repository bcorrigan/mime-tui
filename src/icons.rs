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

/// Icon for an installed application. Currently a generic glyph; specialized
/// per-app icons would require pulling Icon= out of the .desktop file.
pub fn app_icon(_id: &str) -> &'static str {
    md::MD_APPLICATION
}
