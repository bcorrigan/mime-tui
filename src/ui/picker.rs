use crate::app::{App, Focus, Mode, Relation};
use crate::config::{MimeTuiConfig, Theme};
use crate::ui::layout;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use tui_input::Input;

pub fn draw(f: &mut Frame, app: &mut App, config: &MimeTuiConfig) {
    let (title, items) = match app.mode.clone() {
        Mode::PickApp { for_mime } => {
            let visible: Vec<crate::model::DesktopApp> =
                app.picker_visible_apps().into_iter().cloned().collect();
            let items: Vec<String> = visible
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    let rel = app.relation_of(&for_mime, &a.id);
                    let body = format!("{}    {}", a.name, a.id);
                    decorate_row(rel, in_marked_range(app, i), &body)
                })
                .collect();
            (
                format!(" Toggle apps for {} ", for_mime),
                items,
            )
        }
        Mode::PickMime { for_app } => {
            let visible: Vec<crate::model::MimeType> =
                app.picker_visible_mimes().into_iter().cloned().collect();
            let items: Vec<String> = visible
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    let rel = app.relation_of(&m.id, &for_app);
                    let body = if m.description.is_empty() {
                        m.id.clone()
                    } else {
                        format!("{}    {}", m.id, m.description)
                    };
                    decorate_row(rel, in_marked_range(app, i), &body)
                })
                .collect();
            (
                format!(" Toggle mime types for {} ", for_app),
                items,
            )
        }
        _ => return,
    };

    let area = centered_rect(f.area(), 70, 70);
    f.render_widget(Clear, area);

    let border = Theme::parse_color(&config.colors.focus);
    let key = Style::default().add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Theme::parse_color(&config.colors.unfocused));

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(Theme::parse_border_type(&config.colors.border_style))
        .border_style(Style::default().fg(border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);

    render_picker_search(f, chunks[0], &app.pick_input, config);

    let clamped = app.pick_selected.min(items.len().saturating_sub(1));
    layout::render_list(
        f,
        chunks[1],
        " Candidates ",
        &items,
        Some(clamped),
        true,
        config,
        &mut app.pick_list_state,
    );

    let mark_active = app.pick_mark.is_some();
    let hint = if mark_active {
        Line::from(vec![
            Span::styled(" Space/Enter", key),
            Span::styled(": toggle range  ", dim),
            Span::styled("Ctrl-Space", key),
            Span::styled(": clear mark  ", dim),
            Span::styled("Esc", key),
            Span::styled(": close", dim),
        ])
    } else {
        Line::from(vec![
            Span::styled(" Space/Enter", key),
            Span::styled(": toggle  ", dim),
            Span::styled("Ctrl-Space", key),
            Span::styled(": set mark  ", dim),
            Span::styled("Esc", key),
            Span::styled(": close", dim),
        ])
    };
    f.render_widget(Paragraph::new(hint), chunks[2]);
}

/// Two single-char prefix slots: relation indicator + mark indicator. Padded
/// out so column alignment stays consistent regardless of which markers fire.
fn decorate_row(rel: Option<Relation>, marked: bool, body: &str) -> String {
    let rel_char = match rel {
        Some(Relation::Default) => '★',
        Some(Relation::Associated) => '✓',
        Some(Relation::DeclaredOnly) => '·',
        None => ' ',
    };
    let mark_char = if marked { '▌' } else { ' ' };
    format!("{}{} {}", rel_char, mark_char, body)
}

fn in_marked_range(app: &App, i: usize) -> bool {
    match app.pick_mark {
        None => false,
        Some(mark) => {
            let lo = mark.min(app.pick_selected);
            let hi = mark.max(app.pick_selected);
            i >= lo && i <= hi
        }
    }
}

fn render_picker_search(f: &mut Frame, area: Rect, input: &Input, config: &MimeTuiConfig) {
    layout::render_search_bar(f, area, input, Focus::Search, config);
}

fn centered_rect(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
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
