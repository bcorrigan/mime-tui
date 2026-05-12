use crate::config::{MimeTuiConfig, Theme};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

pub fn draw(f: &mut Frame, config: &MimeTuiConfig) {
    let area = centered_rect(f.area(), 70, 34);
    f.render_widget(Clear, area);

    let border = Theme::parse_color(&config.colors.focus);
    let key = Style::default()
        .fg(Theme::parse_color(&config.colors.focus))
        .add_modifier(Modifier::BOLD);
    let heading = Style::default()
        .fg(Theme::parse_color(&config.colors.highlight))
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Theme::parse_color(&config.colors.unfocused));

    let block = Block::default()
        .title(" Keybindings ")
        .borders(Borders::ALL)
        .border_type(Theme::parse_border_type(&config.colors.border_style))
        .border_style(Style::default().fg(border));

    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  Navigation", heading)));
    lines.extend([
        kv(key, dim, "  Tab        ", "switch view (by-mime ↔ by-app)"),
        kv(key, dim, "  ↑ / ↓      ", "navigate the focused list"),
        kv(key, dim, "  →          ", "enter edit mode (focus right pane)"),
        kv(key, dim, "  ←  / Esc   ", "leave edit mode (back to left)"),
        kv(key, dim, "  type       ", "fuzzy-search the left list"),
        kv(key, dim, "  Ctrl-A/E   ", "start / end of search (emacs)"),
        kv(key, dim, "  Ctrl-U/K/W ", "kill backward / forward / word"),
    ]);

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  Edits (Focus::Right)", heading)));
    lines.extend([
        kv(key, dim, "  d          ", "make selected the default"),
        kv(key, dim, "  r          ", "remove selected from associations"),
        kv(key, dim, "  c          ", "clear default for the current mime"),
        kv(key, dim, "  a          ", "open picker to add an association"),
    ]);

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  Picker (after 'a')", heading)));
    lines.extend([
        kv(key, dim, "  Space/Ent  ", "toggle association at cursor (stays open)"),
        kv(key, dim, "  Ctrl-Space ", "set mark / clear mark (for range toggle)"),
        kv(key, dim, "  ↑ / ↓      ", "move cursor (extends marked range)"),
        kv(key, dim, "  type       ", "fuzzy-filter candidates (clears mark)"),
        kv(key, dim, "  Esc        ", "close picker"),
    ]);
    lines.push(Line::from(vec![
        Span::raw("              "),
        Span::styled(
            "★",
            Style::default()
                .fg(Theme::parse_color(&config.colors.focus))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" default  ", dim),
        Span::styled(
            "✓",
            Style::default()
                .fg(Theme::parse_color(&config.colors.focus))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" associated  ", dim),
        Span::styled(
            "·",
            Style::default()
                .fg(Theme::parse_color(&config.colors.unfocused))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" declared-only", dim),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  File", heading)));
    lines.extend([
        kv(key, dim, "  Ctrl-S     ", "save pending edits to mimeapps.list"),
        kv(key, dim, "  Ctrl-Z     ", "discard all pending edits"),
    ]);

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  Other", heading)));
    lines.extend([
        kv(key, dim, "  ?          ", "this help (any key dismisses)"),
        kv(key, dim, "  Esc        ", "quit (confirms if unsaved)"),
        kv(key, dim, "  Ctrl-C/G   ", "same as Esc"),
    ]);

    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn kv<'a>(key: Style, dim: Style, k: &'a str, v: &'a str) -> Line<'a> {
    Line::from(vec![Span::styled(k, key), Span::styled(v, dim)])
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
