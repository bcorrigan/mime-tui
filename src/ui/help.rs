use crate::app::{App, Focus, View};
use crate::config::{MimeTuiConfig, Theme};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

pub fn draw(f: &mut Frame, app: &mut App, config: &MimeTuiConfig) {
    let context = HelpContext::from_app(app);

    // Pass 1: build at unbounded width to estimate natural height.
    let nominal_lines = build_lines(&context, config, u16::MAX).len() as u16;
    let desired_height = nominal_lines.saturating_add(2); // +2 for borders

    let area = centered_rect(f.area(), 70, desired_height);
    f.render_widget(Clear, area);

    let inner_width = area.width.saturating_sub(2);
    let inner_height = area.height.saturating_sub(2);

    // Pass 2: build for actual width so kv rows wrap with hanging indent.
    let lines = build_lines(&context, config, inner_width);
    let visual_rows = lines.len() as u16;

    let max_scroll = visual_rows.saturating_sub(inner_height);
    let scroll = app.help_scroll.min(max_scroll);
    app.help_scroll = scroll;

    let border = Theme::parse_color(&config.colors.focus);
    let title = format!(
        " Help — {} {} ",
        context.title(),
        scroll_indicator(scroll, max_scroll)
    );

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(Theme::parse_border_type(&config.colors.border_style))
        .border_style(Style::default().fg(border));

    f.render_widget(
        Paragraph::new(lines).block(block).scroll((scroll, 0)),
        area,
    );

    crate::ui::layout::render_list_scrollbar(
        f,
        area,
        visual_rows as usize,
        scroll as usize,
        true,
        config,
    );
}

/// Identifies which set of context-specific keys the help should describe.
/// Derived from the application state at the moment `?` was pressed.
struct HelpContext {
    view: View,
    edit_mode: bool, // true when Focus::Right
}

impl HelpContext {
    fn from_app(app: &App) -> Self {
        Self {
            view: app.view,
            edit_mode: app.focus == Focus::Right,
        }
    }

    fn title(&self) -> &'static str {
        match (self.view, self.edit_mode) {
            (View::ByMime, false) => "by-mime · navigation",
            (View::ByMime, true) => "by-mime · editing associations",
            (View::ByApp, false) => "by-app · navigation",
            (View::ByApp, true) => "by-app · editing associations",
        }
    }

    fn summary(&self) -> Vec<&'static str> {
        match (self.view, self.edit_mode) {
            (View::ByMime, false) => vec![
                "Browsing MIME types. Selecting one shows its current default",
                "app and full associations on the right. Press → to switch into",
                "edit mode where you can change which app handles it.",
            ],
            (View::ByMime, true) => vec![
                "You're editing the highlighted MIME type's associations.",
                "The list on the right shows every app handling it: ★ is the",
                "default; the rest are alternates file managers may offer.",
            ],
            (View::ByApp, false) => vec![
                "Browsing installed applications. Selecting one shows the MIME",
                "types it handles on the right. Press → to switch into edit",
                "mode where you can add or remove this app's associations.",
            ],
            (View::ByApp, true) => vec![
                "You're editing the highlighted app's MIME associations.",
                "Markers: ★ default, ✓ associated, · declared by the .desktop",
                "but currently suppressed (e.g. you'd removed the association).",
            ],
        }
    }

    /// Context keys: the ones whose behaviour depends on the current view
    /// and/or focus. Universal keys (Ctrl-S, ?, Esc, …) are in their own
    /// section below.
    fn keys(&self) -> Vec<(&'static str, &'static str)> {
        match (self.view, self.edit_mode) {
            (_, false) => vec![
                ("↑ / ↓", "Move the highlighted row up or down by one."),
                ("PgUp / PgDn", "Move by a full screen at a time."),
                (
                    "type",
                    "Fuzzy-filter the list as you type. Prefix matches rank above fuzzy hits; \
                     the row count and selection reset on every keystroke.",
                ),
                (
                    "→",
                    "Move focus to the right pane and enter edit mode — that's where d/r/c/a \
                     become live actions on the highlighted pair.",
                ),
                (
                    "Tab",
                    "Switch between the by-mime and by-app views. The search filter is cleared \
                     on the switch since it rarely makes sense across axes.",
                ),
                (
                    "Shift+← / →",
                    "Horizontally scroll the list when a row's content extends past the pane \
                     (long mime ids in particular).",
                ),
            ],
            (View::ByMime, true) => vec![
                (
                    "↑ / ↓",
                    "Move between apps in the associations list. ★ marks the default.",
                ),
                (
                    "d",
                    "Make the highlighted app the default for this MIME type. \
                     The change is staged in memory until you press Ctrl-S; the * count in \
                     the status bar tells you how many pending edits there are.",
                ),
                (
                    "r",
                    "Remove the highlighted app from this MIME type's associations. \
                     Recorded as an explicit \"removed\" entry so it sticks even if the app's \
                     .desktop declares it as a handler.",
                ),
                (
                    "c",
                    "Clear the default app for this MIME type. The first remaining association \
                     becomes the fallback (file managers' \"Open with…\" still works).",
                ),
                (
                    "a",
                    "Open the picker to add more apps. The picker is a multi-toggle UI: \
                     Space/Enter flips an app's association, Ctrl-Space sets a mark so you \
                     can flip an entire range at once.",
                ),
                (
                    "← / Esc",
                    "Return to the MIME types list. (Esc from there with unsaved edits \
                     prompts before quitting.)",
                ),
                (
                    "Shift+← / →",
                    "Horizontally scroll the associations list, useful when an app name is \
                     long enough to overflow.",
                ),
            ],
            (View::ByApp, true) => vec![
                (
                    "↑ / ↓",
                    "Move between MIME types in the right pane.",
                ),
                (
                    "d",
                    "Make this app the default for the highlighted MIME type. \
                     Staged until Ctrl-S writes it to mimeapps.list.",
                ),
                (
                    "r",
                    "Remove this app from the highlighted MIME type's associations.",
                ),
                (
                    "c",
                    "Clear the default app for the highlighted MIME type — useful when this \
                     app is the current default and you want \"no default\" rather than swap.",
                ),
                (
                    "a",
                    "Open the picker to associate this app with additional MIME types. \
                     Multi-toggle with Ctrl-Space marks; ideal for adding many at once.",
                ),
                (
                    "← / Esc",
                    "Return to the applications list.",
                ),
                (
                    "Shift+← / →",
                    "Horizontally scroll for long MIME ids.",
                ),
            ],
        }
    }
}

fn build_lines(ctx: &HelpContext, config: &MimeTuiConfig, width: u16) -> Vec<Line<'static>> {
    let key = Style::default()
        .fg(Theme::parse_color(&config.colors.focus))
        .add_modifier(Modifier::BOLD);
    let heading = Style::default()
        .fg(Theme::parse_color(&config.colors.highlight))
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Theme::parse_color(&config.colors.secondary));

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Context summary — intro paragraph explaining what this screen does.
    // The `summary()` method returns the paragraph as multiple `&str`
    // literals (one per source-code line); join them with spaces so the
    // text wraps at *terminal* width rather than at our source-code line
    // breaks, otherwise narrow terminals get awkward early-break artefacts
    // like a row containing only "current default".
    lines.push(Line::from(""));
    let paragraph: String = ctx.summary().join(" ");
    lines.extend(wrap_kv("  ", &paragraph, dim, dim, width));
    lines.push(Line::from(""));

    let section_title = if ctx.edit_mode {
        "  Editing"
    } else {
        "  Navigation"
    };
    lines.push(Line::from(Span::styled(section_title, heading)));
    for (k, v) in ctx.keys() {
        emit_kv(&mut lines, k, v, key, dim, width);
    }
    lines.push(Line::from(""));

    // Always-available section — same regardless of context.
    lines.push(Line::from(Span::styled("  Always available", heading)));
    let always: &[(&str, &str)] = &[
        (
            "Ctrl-S",
            "Save all pending edits to $XDG_CONFIG_HOME/mimeapps.list. Writes are atomic \
             (tempfile + rename) with a single rolling .bak alongside. Updates \
             mimeapps.cache as a courtesy so other tools see the change immediately.",
        ),
        (
            "Ctrl-Z",
            "Discard every pending edit. Nothing on disk changes; the in-memory dirty count \
             resets to zero.",
        ),
        (
            "?",
            "Show this help. The content changes depending on which pane you're in.",
        ),
        (
            "Esc / Ctrl-C / Ctrl-G",
            "Quit the application. If you have unsaved edits, you're prompted: \
             y = discard & quit, s = save & quit, n = cancel.",
        ),
    ];
    for (k, v) in always {
        emit_kv(&mut lines, k, v, key, dim, width);
    }
    lines.push(Line::from(""));

    // Legend.
    lines.push(Line::from(Span::styled("  Relation markers", heading)));
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled("★", key),
        Span::styled(
            "  default — this is the app file managers will pick by default.",
            dim,
        ),
    ]));
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled("✓", key),
        Span::styled(
            "  associated — an alternate handler (in mimeapps.list or .desktop MimeType=).",
            dim,
        ),
    ]));
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled("·", key),
        Span::styled(
            "  declared-only — the .desktop says it handles this, but a removal entry \
             suppresses it.",
            dim,
        ),
    ]));
    lines.push(Line::from(""));

    // Help-overlay-specific.
    lines.push(Line::from(Span::styled("  This overlay", heading)));
    let overlay: &[(&str, &str)] = &[
        ("Space / PgDn", "Page down."),
        ("b / PgUp", "Page up."),
        ("↑ / ↓", "Line up / down."),
        ("g / G", "Jump to top / bottom."),
        ("q / Esc / Enter", "Close this help."),
    ];
    for (k, v) in overlay {
        emit_kv(&mut lines, k, v, key, dim, width);
    }

    lines
}

/// Emit one `(key, description)` row with the description hanging-indented
/// under itself on wrap. `key` is right-padded so all descriptions line up.
fn emit_kv(
    lines: &mut Vec<Line<'static>>,
    k: &str,
    v: &str,
    key_style: Style,
    dim_style: Style,
    width: u16,
) {
    // Reserve a consistent column for the key so descriptions align even
    // when keys are different widths. 18 columns is roughly the longest key
    // string we emit ("Esc / Ctrl-C / Ctrl-G" is the outlier — let it
    // overflow rather than blow up the indent for everyone else).
    const KEY_COL_W: usize = 18;
    let visible_w = k.chars().count();
    let padded_key = if visible_w < KEY_COL_W {
        format!("  {}{}", k, " ".repeat(KEY_COL_W - visible_w))
    } else {
        // Long key overflows; put description on the next line so they don't
        // run together.
        lines.push(Line::from(Span::styled(format!("  {}", k), key_style)));
        format!("  {}", " ".repeat(KEY_COL_W))
    };
    lines.extend(wrap_kv(&padded_key, v, key_style, dim_style, width));
}

/// Word-wrap a `(key, value)` row with a hanging indent: first visual line
/// keeps the styled key prefix; continuations are indented with blank spaces
/// of the same width so the description column lines up.
fn wrap_kv(
    k: &str,
    v: &str,
    key_style: Style,
    dim_style: Style,
    width: u16,
) -> Vec<Line<'static>> {
    let width = width as usize;
    if width == 0 {
        return vec![Line::from("")];
    }
    let k_width = k.chars().count();

    if k_width >= width {
        return vec![Line::from(vec![
            Span::styled(k.to_string(), key_style),
            Span::styled(v.to_string(), dim_style),
        ])];
    }

    let body_width = width - k_width;
    let words: Vec<&str> = v.split_whitespace().collect();
    if words.is_empty() {
        return vec![Line::from(Span::styled(k.to_string(), key_style))];
    }

    let indent: String = " ".repeat(k_width);
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut current_w: usize = 0;

    for word in words {
        let ww = word.chars().count();
        let needed = if current.is_empty() { ww } else { 1 + ww };
        if !current.is_empty() && current_w + needed > body_width {
            out.push(emit_kv_line(&current, k, &indent, out.is_empty(), key_style, dim_style));
            current.clear();
            current_w = 0;
        }
        if !current.is_empty() {
            current_w += 1;
        }
        current.push(word);
        current_w += ww;
    }
    if !current.is_empty() {
        out.push(emit_kv_line(&current, k, &indent, out.is_empty(), key_style, dim_style));
    }
    out
}

fn emit_kv_line(
    words: &[&str],
    k: &str,
    indent: &str,
    first: bool,
    key_style: Style,
    dim_style: Style,
) -> Line<'static> {
    let body = words.join(" ");
    if first {
        Line::from(vec![
            Span::styled(k.to_string(), key_style),
            Span::styled(body, dim_style),
        ])
    } else {
        Line::from(vec![
            Span::raw(indent.to_string()),
            Span::styled(body, dim_style),
        ])
    }
}

fn scroll_indicator(scroll: u16, max_scroll: u16) -> &'static str {
    if max_scroll == 0 {
        return "";
    }
    if scroll == 0 {
        "↓"
    } else if scroll >= max_scroll {
        "↑"
    } else {
        "↑↓"
    }
}

fn centered_rect(area: Rect, percent_x: u16, fixed_height: u16) -> Rect {
    let total_h = area.height;
    let top = total_h.saturating_sub(fixed_height) / 2;
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(top),
            Constraint::Length(fixed_height.min(total_h)),
            Constraint::Min(0),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_text(line: &Line<'_>) -> String {
        line.iter().map(|s| s.content.as_ref()).collect()
    }

    fn styles() -> (Style, Style) {
        (Style::default(), Style::default())
    }

    #[test]
    fn short_value_fits_on_one_line() {
        let (k, d) = styles();
        let lines = wrap_kv("  d ", "set as default", k, d, 40);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), "  d set as default");
    }

    #[test]
    fn long_value_wraps_with_hanging_indent() {
        let (k, d) = styles();
        let lines = wrap_kv("  d   ", "alpha beta gamma delta epsilon", k, d, 20);
        assert!(lines.len() >= 2);
        assert!(line_text(&lines[0]).starts_with("  d   "));
        for line in &lines[1..] {
            let t = line_text(line);
            assert!(t.starts_with("      "), "expected hanging indent: {:?}", t);
            assert_ne!(t.chars().nth(6), Some(' '), "indent should be exactly 6: {:?}", t);
        }
    }

    #[test]
    fn empty_value_emits_prefix_only() {
        let (k, d) = styles();
        let lines = wrap_kv("  Tab ", "", k, d, 80);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), "  Tab ");
    }

    #[test]
    fn key_wider_than_width_emits_one_long_line() {
        let (k, d) = styles();
        let lines = wrap_kv("  KEY     ", "value", k, d, 5);
        assert_eq!(lines.len(), 1);
        assert!(line_text(&lines[0]).contains("KEY"));
        assert!(line_text(&lines[0]).contains("value"));
    }

    #[test]
    fn zero_width_does_not_panic() {
        let (k, d) = styles();
        let _ = wrap_kv("  d ", "value", k, d, 0);
    }

    #[test]
    fn very_long_single_word_emits_overflowing() {
        let (k, d) = styles();
        let lines = wrap_kv("  k ", "supercalifragilisticexpialidocious", k, d, 20);
        let joined: String = lines.iter().map(line_text).collect();
        assert!(joined.contains("supercalifragilisticexpialidocious"));
    }

    #[test]
    fn whitespace_only_value_is_treated_as_empty() {
        let (k, d) = styles();
        let lines = wrap_kv("  k ", "    ", k, d, 40);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), "  k ");
    }
}
