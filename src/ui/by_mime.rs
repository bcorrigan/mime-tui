use crate::app::{App, Focus};
use crate::config::{MimeTuiConfig, Theme};
use crate::icons;
use crate::storage;
use crate::ui::layout;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

/// If the default for `mime_id` was read from a file other than the user's own
/// `mimeapps.list`, return that file's basename for display. Otherwise None
/// (meaning the default lives where saves go, so no warning is needed).
fn source_label(app: &App, mime_id: &str) -> Option<String> {
    let src = app.assoc.default_sources.get(mime_id)?;
    let user = storage::mimeapps::user_mimeapps_path()?;
    if src == &user {
        return None;
    }
    Some(
        src.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| src.to_string_lossy().into_owned()),
    )
}

pub fn draw(f: &mut Frame, app: &mut App, area: Rect, config: &MimeTuiConfig) {
    let (left_area, right_area) = layout::horizontal_split(area);

    let visible: Vec<crate::model::MimeType> =
        app.visible_mimes().into_iter().cloned().collect();
    let display: Vec<String> = visible
        .iter()
        .map(|m| format!("{}  {}", icons::mime_icon(&m.id), m.id))
        .collect();
    let clamped_left = app.selected_left.min(display.len().saturating_sub(1));
    let focus_left = app.focus == Focus::Left;

    layout::render_list(
        f,
        left_area,
        " MIME types ",
        &display,
        Some(clamped_left),
        focus_left,
        config,
        &mut app.left_list_state,
    );
    app.left_rect = Some(left_area);

    let selected_mime = visible.get(clamped_left).cloned();
    draw_right(f, app, right_area, selected_mime.as_ref(), config);
    app.right_rect = Some(right_area);
}

fn draw_right(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    mime: Option<&crate::model::MimeType>,
    config: &MimeTuiConfig,
) {
    // Top: description + default (paragraph). Bottom: selectable associations.
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(3)])
        .split(area);

    render_summary(f, split[0], mime, app, config);
    render_associations(f, split[1], mime, app, config);
}

fn render_summary(
    f: &mut Frame,
    area: Rect,
    mime: Option<&crate::model::MimeType>,
    app: &App,
    config: &MimeTuiConfig,
) {
    let border_color = Theme::parse_color(&config.colors.border);
    let heading = Style::default()
        .fg(Theme::parse_color(&config.colors.focus))
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Theme::parse_color(&config.colors.unfocused));

    let title = mime
        .map(|m| format!(" {} ", m.id))
        .unwrap_or_else(|| " — ".into());

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(Theme::parse_border_type(&config.colors.border_style))
        .border_style(Style::default().fg(border_color));

    let mut lines: Vec<Line> = Vec::new();
    if let Some(m) = mime {
        let desc = if m.description.is_empty() {
            "(no description)".into()
        } else {
            m.description.clone()
        };
        lines.push(Line::from(desc));
        lines.push(Line::from(""));

        let pending_marker = if app.pending.set_default.contains_key(&m.id) {
            Span::styled(" *", heading)
        } else {
            Span::raw("")
        };
        let header = Line::from(vec![
            Span::styled("Default", heading),
            pending_marker,
        ]);
        lines.push(header);
        match app.effective_default_for(&m.id) {
            Some(d) => {
                lines.push(Line::from(format!("  {}", d.name)));
                lines.push(Line::from(Span::styled(format!("  {}", d.id), dim)));
                if let Some(src) = source_label(app, &m.id) {
                    lines.push(Line::from(Span::styled(
                        format!("  ⚠ from {} (saves will be shadowed)", src),
                        dim,
                    )));
                }
            }
            None => lines.push(Line::from(Span::styled("  (none set)", dim))),
        }
    }

    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: false }),
        area,
    );
}

fn render_associations(
    f: &mut Frame,
    area: Rect,
    mime: Option<&crate::model::MimeType>,
    app: &mut App,
    config: &MimeTuiConfig,
) {
    let Some(m) = mime else {
        layout::render_list(
            f,
            area,
            " Associations ",
            &[],
            None,
            false,
            config,
            &mut app.right_list_state,
        );
        return;
    };
    let mime_id = m.id.clone();
    let assoc = app.effective_associations_for(&mime_id);

    let default_id = app.effective_default_for(&mime_id).map(|d| d.id.clone());

    let items: Vec<String> = assoc
        .iter()
        .map(|e| {
            let marker = if Some(&e.id) == default_id.as_ref() {
                "  (default)"
            } else {
                ""
            };
            format!("{}{}", e.name, marker)
        })
        .collect();

    let focus_right = app.focus == Focus::Right;
    let clamped = app.selected_right.min(items.len().saturating_sub(1));
    // Only show the selection bar when the right pane is focused. With focus
    // on the left, the right pane is just informational — a "ghost" selection
    // here would imply navigability that isn't actually active.
    let selected = if focus_right { Some(clamped) } else { None };
    layout::render_list(
        f,
        area,
        " Associations ",
        &items,
        selected,
        focus_right,
        config,
        &mut app.right_list_state,
    );
}
