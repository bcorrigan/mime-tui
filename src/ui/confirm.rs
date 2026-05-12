use crate::app::App;
use crate::config::{MimeTuiConfig, Theme};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

pub fn draw(f: &mut Frame, app: &App, config: &MimeTuiConfig) {
    let n = app.pending.count();

    let area = centered_rect(f.area(), 50, 9);
    f.render_widget(Clear, area);

    let border = Theme::parse_color(&config.colors.focus);
    let key = Style::default()
        .fg(Theme::parse_color(&config.colors.focus))
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Theme::parse_color(&config.colors.unfocused));

    let block = Block::default()
        .title(" Unsaved changes ")
        .borders(Borders::ALL)
        .border_type(Theme::parse_border_type(&config.colors.border_style))
        .border_style(Style::default().fg(border));

    let body = vec![
        Line::from(""),
        Line::from(format!(
            "  You have {} unsaved edit{} to mimeapps.list.",
            n,
            if n == 1 { "" } else { "s" }
        )),
        Line::from(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("y", key),
            Span::styled(" / Enter — discard & quit       ", dim),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("s", key),
            Span::styled(" — save & quit                  ", dim),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("n", key),
            Span::styled(" / Esc — cancel                 ", dim),
        ]),
    ];

    let para = Paragraph::new(body).block(block).wrap(Wrap { trim: false });
    f.render_widget(para, area);
}

fn centered_rect(area: Rect, percent_x: u16, fixed_height: u16) -> Rect {
    let total_h = area.height;
    let top = total_h.saturating_sub(fixed_height) / 2;
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(top),
            Constraint::Length(fixed_height),
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
