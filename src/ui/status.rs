use crate::app::{App, Focus, Mode, View};
use crate::config::{MimeTuiConfig, Theme};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

pub fn draw(f: &mut Frame, app: &App, area: Rect, config: &MimeTuiConfig) {
    let key = Style::default()
        .fg(Theme::parse_color(&config.colors.focus))
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Theme::parse_color(&config.colors.unfocused));

    // Flash messages take precedence — show them in the status line for a few
    // seconds after the action, then fall back to the keybind hints.
    if let Some(msg) = app.flash_message() {
        let line = Line::from(vec![Span::styled(format!(" {} ", msg), key)]);
        f.render_widget(Paragraph::new(line), area);
        return;
    }

    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::raw(format!(
        " [{}] {} ",
        view_label(app.view),
        if app.is_dirty() {
            format!("* {} pending  ", app.pending.count())
        } else {
            String::new()
        }
    )));

    match &app.mode {
        Mode::Browse => browse_hints(&mut spans, app, key, dim),
        Mode::PickApp { .. } | Mode::PickMime { .. } => {
            spans.extend([
                Span::styled("Enter", key),
                Span::styled(": add  ", dim),
                Span::styled("Esc", key),
                Span::styled(": cancel", dim),
            ]);
        }
        Mode::ConfirmQuit => {
            spans.extend([
                Span::styled("y", key),
                Span::styled(": discard  ", dim),
                Span::styled("s", key),
                Span::styled(": save  ", dim),
                Span::styled("n", key),
                Span::styled(": cancel", dim),
            ]);
        }
        Mode::Help => {
            spans.push(Span::styled("any key", key));
            spans.push(Span::styled(": dismiss", dim));
        }
    }

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn browse_hints<'a>(spans: &mut Vec<Span<'a>>, app: &App, key: Style, dim: Style) {
    spans.extend([
        Span::styled("Tab", key),
        Span::styled(": view  ", dim),
    ]);

    if app.focus == Focus::Right {
        spans.extend([
            Span::styled("d", key),
            Span::styled(":default  ", dim),
            Span::styled("r", key),
            Span::styled(":remove  ", dim),
            Span::styled("c", key),
            Span::styled(":clear  ", dim),
            Span::styled("a", key),
            Span::styled(":add  ", dim),
            Span::styled("←", key),
            Span::styled(":back  ", dim),
        ]);
    } else {
        // Focus::Left/Search — letters go to search, so no letter shortcuts
        // here. → enters edit mode where the actions become live.
        spans.extend([
            Span::styled("→", key),
            Span::styled(":edit  ", dim),
        ]);
    }

    spans.extend([
        Span::styled("Ctrl-S", key),
        Span::styled(":save  ", dim),
    ]);
    if app.is_dirty() {
        spans.extend([
            Span::styled("Ctrl-Z", key),
            Span::styled(":discard  ", dim),
        ]);
    }
    spans.extend([
        Span::styled("?", key),
        Span::styled(":help  ", dim),
        Span::styled("Esc", key),
        Span::styled(":quit", dim),
    ]);
}

fn view_label(v: View) -> &'static str {
    match v {
        View::ByMime => "by-mime",
        View::ByApp => "by-app",
    }
}
