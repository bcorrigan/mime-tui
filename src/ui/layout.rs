use ratatui::{
    Frame,
    layout::{Layout, Constraint, Direction, Rect},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    style::{Style, Color},
};
use tui_input::Input;
use crate::app::Focus;
use crate::config::{MimeTuiConfig, Theme, SearchPosition};

pub fn vertical_split(f: &Frame, search_height: u16, search_position: SearchPosition) -> (Rect, Rect) {
    let full_area = f.area();
    match search_position {
        SearchPosition::Top => {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(search_height), Constraint::Min(0)])
                .split(full_area);
            (chunks[0], chunks[1])
        }
        SearchPosition::Bottom => {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(0), Constraint::Length(search_height)])
                .split(full_area);
            (chunks[1], chunks[0])
        }
    }
}

/// Top region for the content panes, bottom 1-line strip for the status bar.
pub fn content_with_status_split(area: Rect) -> (Rect, Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(area);
    (chunks[0], chunks[1])
}

pub fn horizontal_split(area: Rect) -> (Rect, Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);
    (chunks[0], chunks[1])
}

#[allow(dead_code)] // Reserved for help/about overlay (Phase 4 polish).
pub fn render_description(
    f: &mut Frame,
    area: Rect,
    title: &str,
    text: &str,
    config: &MimeTuiConfig,
) {
    let border_color = Theme::parse_color(&config.colors.border);
    let block = Block::default()
        .title(format!(" {} ", title))
        .borders(Borders::ALL)
        .border_type(Theme::parse_border_type(&config.colors.border_style))
        .border_style(Style::default().fg(border_color));
    let paragraph = Paragraph::new(text)
        .block(block)
        .wrap(Wrap { trim: true });
    f.render_widget(paragraph, area);
}

pub fn render_search_bar(
    f: &mut Frame,
    area: Rect,
    input: &Input,
    focus: Focus,
    config: &MimeTuiConfig,
) {
    let border_color = if focus == Focus::Search {
        Theme::parse_color(&config.colors.focus)
    } else {
        Theme::parse_color(&config.colors.border)
    };

    let block = Block::default()
        .title(" Search ")
        .borders(Borders::ALL)
        .border_type(Theme::parse_border_type(&config.colors.border_style))
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);

    let query = input.value();
    let cursor_position = input.cursor();

    let query_chars: Vec<char> = query.chars().collect();
    let query_len = query_chars.len();

    let padding = 1;
    let available_width = (inner.width as usize).saturating_sub(padding * 2);

    let scroll_offset = if cursor_position >= available_width {
        cursor_position - available_width + 1
    } else {
        0
    };

    let visible_start = scroll_offset;
    let visible_end = (visible_start + available_width).min(query_len);
    let visible_text: String = query_chars[visible_start..visible_end].iter().collect();

    let padded_text = format!(" {} ", visible_text);

    let paragraph = Paragraph::new(padded_text)
        .block(block)
        .style(Style::default().fg(border_color));

    f.render_widget(paragraph, area);

    let cursor_x = inner.x + padding as u16 + (cursor_position - scroll_offset) as u16;
    let cursor_y = inner.y;
    f.set_cursor_position((cursor_x, cursor_y));
}

/// `selected: Some(i)` shows a highlight bar on row `i`; `None` renders the
/// list without any selection bar — used by the right pane when focus is on
/// the left, where a "ghost" selection isn't informative.
pub fn render_list(
    f: &mut Frame,
    area: Rect,
    title: &str,
    items: &[String],
    selected: Option<usize>,
    focus_on_title: bool,
    config: &MimeTuiConfig,
    state: &mut ListState,
) {
    match selected {
        Some(s) => {
            let sel = if s >= items.len() { 0 } else { s };
            state.select(Some(sel));
        }
        None => state.select(None),
    }

    let border_color = if focus_on_title {
        Theme::parse_color(&config.colors.focus)
    } else {
        Theme::parse_color(&config.colors.border)
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(Theme::parse_border_type(&config.colors.border_style))
        .border_style(Style::default().fg(border_color));

    let list_items: Vec<ListItem> = items.iter()
        .map(|a| ListItem::new(format!(" {} ", a)))
        .collect();

    let selection_color = if focus_on_title {
        Theme::parse_color(&config.colors.highlight)
    } else {
        Theme::parse_color(&config.colors.unfocused)
    };

    let highlight_style = match config.colors.highlight_type.to_lowercase().as_str() {
        "foreground" => Style::default().fg(selection_color),
        "background" | _ => Style::default().bg(selection_color).fg(Color::Black),
    };

    let list = List::new(list_items)
        .block(block)
        .highlight_style(highlight_style)
        .highlight_symbol("");

    f.render_stateful_widget(list, area, state);
}
