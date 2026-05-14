use crate::app::App;
use crate::config::{MimeTuiConfig, Theme, PRESET_NAMES};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};

pub fn draw(f: &mut Frame, app: &App, selected: usize, config: &MimeTuiConfig) {
    // Compact modal — small enough that the rest of the app remains
    // visible underneath, so the user can see the current view repainted
    // with whichever theme is being previewed.
    let items_h = PRESET_NAMES.len() as u16;
    let hint_h = 1u16;
    let body_h = items_h + hint_h + 2; // +2 for top/bottom borders
    let area = centered_rect(f.area(), 36, body_h);
    f.render_widget(Clear, area);

    let border_color = Theme::parse_color(&config.colors.focus);
    let key = Style::default()
        .fg(Theme::parse_color(&config.colors.focus))
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Theme::parse_color(&config.colors.secondary));
    let highlight = Theme::parse_color(&config.colors.highlight);
    let selection_fg = Theme::parse_color(&config.colors.selection_fg);

    let text = crate::ui::layout::theme_text_style(config);
    let block = Block::default()
        .title(" Pick a theme ")
        .borders(Borders::ALL)
        .border_type(Theme::parse_border_type(&config.colors.border_style))
        .border_style(Style::default().fg(border_color))
        .style(text);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(items_h), Constraint::Length(hint_h)])
        .split(inner);

    // Items: preset name + tiny "current" marker on the active row so the
    // user knows where they started.
    let active = app.active_preset.as_deref();
    let items: Vec<ListItem> = PRESET_NAMES
        .iter()
        .map(|name| {
            let marker = if Some(*name) == active { " · current" } else { "" };
            let line = Line::from(vec![
                Span::raw(" "),
                Span::raw(name.to_string()),
                Span::styled(marker.to_string(), dim),
                Span::raw(" "),
            ]);
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .style(text)
        .highlight_style(Style::default().bg(highlight).fg(selection_fg));
    let mut state = ListState::default();
    state.select(Some(selected));
    f.render_stateful_widget(list, chunks[0], &mut state);

    let hint = Line::from(vec![
        Span::raw(" "),
        Span::styled("↑↓", key),
        Span::styled(":preview  ", dim),
        Span::styled("Enter", key),
        Span::styled(":keep  ", dim),
        Span::styled("Esc", key),
        Span::styled(":cancel", dim),
    ]);
    f.render_widget(Paragraph::new(hint).style(text), chunks[1]);
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
