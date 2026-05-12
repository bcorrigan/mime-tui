use crate::app::{App, Focus, Relation};
use crate::config::{MimeTuiConfig, Theme};
use crate::icons;
use crate::ui::layout;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

pub fn draw(f: &mut Frame, app: &mut App, area: Rect, config: &MimeTuiConfig) {
    let (left_area, right_area) = layout::horizontal_split(area);

    let visible: Vec<crate::model::DesktopApp> =
        app.visible_apps().into_iter().cloned().collect();
    let display: Vec<String> = visible
        .iter()
        .map(|a| format!("{}  {}", icons::app_icon(&a.id), a.name))
        .collect();
    let clamped_left = app.selected_left.min(display.len().saturating_sub(1));
    let focus_left = app.focus == Focus::Left;

    layout::render_list(
        f,
        left_area,
        " Applications ",
        &display,
        Some(clamped_left),
        focus_left,
        config,
        &mut app.left_list_state,
    );
    app.left_rect = Some(left_area);

    let selected_app = visible.get(clamped_left).cloned();
    draw_right(f, app, right_area, selected_app.as_ref(), config);
    app.right_rect = Some(right_area);
}

fn draw_right(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    selected: Option<&crate::model::DesktopApp>,
    config: &MimeTuiConfig,
) {
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(6), Constraint::Min(3)])
        .split(area);

    render_summary(f, split[0], selected, config);
    render_mimes_list(f, split[1], selected, app, config);
}

fn render_summary(
    f: &mut Frame,
    area: Rect,
    selected: Option<&crate::model::DesktopApp>,
    config: &MimeTuiConfig,
) {
    let border_color = Theme::parse_color(&config.colors.border);
    let dim = Style::default().fg(Theme::parse_color(&config.colors.unfocused));

    let title = selected
        .map(|a| format!(" {} ", a.name))
        .unwrap_or_else(|| " — ".into());

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(Theme::parse_border_type(&config.colors.border_style))
        .border_style(Style::default().fg(border_color));

    let mut lines: Vec<Line> = Vec::new();
    if let Some(a) = selected {
        if !a.comment.is_empty() {
            lines.push(Line::from(a.comment.clone()));
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(format!("id: {}", a.id), dim)));
    }

    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: false }),
        area,
    );
}

fn render_mimes_list(
    f: &mut Frame,
    area: Rect,
    selected: Option<&crate::model::DesktopApp>,
    app: &mut App,
    config: &MimeTuiConfig,
) {
    let Some(a) = selected else {
        layout::render_list(
            f,
            area,
            " MIME types ",
            &[],
            None,
            false,
            config,
            &mut app.right_list_state,
        );
        return;
    };

    let app_id = a.id.clone();
    let list = app.mime_list_for_app(&app_id);
    let items: Vec<String> = list
        .iter()
        .map(|(m, rel)| {
            let marker = match rel {
                Relation::Default => "★ ",
                Relation::Associated => "+ ",
                Relation::DeclaredOnly => "· ",
            };
            let suffix = match rel {
                Relation::Default => "  (default)",
                Relation::Associated => "",
                Relation::DeclaredOnly => "  (declared, not associated)",
            };
            format!("{}{}{}", marker, m.id, suffix)
        })
        .collect();

    let focus_right = app.focus == Focus::Right;
    let clamped = app.selected_right.min(items.len().saturating_sub(1));
    let selected = if focus_right { Some(clamped) } else { None };
    layout::render_list(
        f,
        area,
        " MIME types ",
        &items,
        selected,
        focus_right,
        config,
        &mut app.right_list_state,
    );

    // Silence the spurious "unused" of Span in this module — we use it through
    // the Line constructor only via &str transparently. (No-op statement.)
    let _ = Span::raw("");
}
